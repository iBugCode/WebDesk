// WebDesk SDK —— 一行代码把远程桌面嵌入任意网页:
//
//   <script src="https://你的静态服务器/dist/sdk.js"></script>
//   <script>
//     const h = WebDesk.window("#mount", { url: "ws://agent主机:9527", token: "..." });
//   </script>
//
// 打包:esbuild --global-name=WebDesk(见 web/package.json build:sdk)。
// WEBDESK_VERSION 在构建时从项目根 package.json 注入(--define),
// 与 agent 启动横幅同源,代码中不手写版本常量。
//
// 设计要点:
// - Agent 只提供 WebSocket;SDK 独立分发,复用主渲染/输入模块
//   (本地指针、按需帧、设置消息)。
// - FPS 完全由前端控制(0.8.0):SDK 检测输入(画布内移动/点击/滚轮/按键)
//   与按住状态,交互时报 maxFps、空闲倒计时(默认 3s,opts.idleTimeout)
//   到点报 idleFps;服务端纯执行 SETTINGS 的 fps 字节,不做任何输入检测。
// - 同一页面可多次 WebDesk.window(...) 挂多个视图:所有视图共享
//   一路 WebSocket 会话(服务端多会话并存),帧解码一次,各视图
//   drawImage 本地缩放。
// - 键盘作用域为视图画布(获得焦点才接收),不劫持宿主页面按键。
// - 能力:画质设置(setQuality)、剪贴板(set/get/onClipboard)、
//   原屏幕截图(screenshot,原始分辨率)、流量统计(stats)。
// - 连接断开自动 3s 重连;destroy() 摘除视图,最后一个视图销毁时关闭连接。

import {
  decodeServer,
  encodeSettings,
  encodeClipboardSet,
  CLIPBOARD_GET,
  KEEPALIVE,
  HelloMsg,
  ServerMsg,
} from "./protocol";
import { Renderer } from "./renderer";
import { InputSender } from "./input";

/** 构建时由 esbuild --define 注入(源:项目根 package.json) */
declare const WEBDESK_VERSION: string;
export const version = WEBDESK_VERSION;

export interface DeskStatus {
  connected: boolean;
  text: string;
}

export interface QualityInput {
  /** 画面质量:传输分辨率百分比 10..100 */
  scale?: number;
  /** 画面压缩:0 = 无损(zstd),否则 WebP lossy quality(1..100)(V2 §5/§6) */
  compression?: number;
  /** @deprecated 0.6.0 前的旧字段名,等同 compression */
  jpeg?: number;
  /** 最低 FPS(画面静止时不发送) */
  idleFps?: number;
  /** 最高 FPS(用户操作中) */
  maxFps?: number;
}

export interface WindowOptions extends QualityInput {
  /** Agent WS 服务地址,如 "ws://192.168.1.10:9527"(默认:页面同主机 :9527) */
  url?: string;
  /** 访问 token(-k 启动参数指定的值;必填) */
  token?: string;
  /** 连接状态回调 */
  onStatus?: (s: DeskStatus) => void;
  /** 远端剪贴板推送(属主变化 / getClipboard 应答) */
  onClipboard?: (text: string) => void;
  /**
   * 空闲倒计时(毫秒,默认 3000):SDK 检测到输入(画布内移动/点击/滚轮/按键)
   * 或按键按住期间按 maxFps 上报;停止输入且无按住超过此时长后回落 idleFps。
   * 0.8.0 起 FPS 由前端控制,服务端只是执行器。
   */
  idleTimeout?: number;
}

export interface ScreenshotOptions {
  /** 默认 "png";可选 "jpeg" */
  format?: "png" | "jpeg";
  /** jpeg 质量 0..100 */
  quality?: number;
}

export interface DeskHandle {
  /** 挂载目标元素 */
  readonly element: HTMLElement;
  /** 远端画面画布(CSS 尺寸自适应容器) */
  readonly canvas: HTMLCanvasElement;
  /** 更新画质设置,立即生效 */
  setQuality(q: QualityInput): void;
  /** 写入远端剪贴板(X11 CLIPBOARD) */
  setClipboard(text: string): void;
  /** 读取远端剪贴板;超时/为空返回 "" */
  getClipboard(timeoutMs?: number): Promise<string>;
  /** 原屏幕截图(原始分辨率,非视图缩放尺寸) */
  screenshot(opts?: ScreenshotOptions): Promise<Blob>;
  /** 远端屏幕尺寸(hello 之后可用) */
  screenSize(): { w: number; h: number } | null;
  /** 累计流量/帧数统计 */
  stats(): { bytes: number; frames: number };
  /** 摘除视图;最后一个视图销毁时关闭共享连接 */
  destroy(): void;
}

/** 内部画质状态(jpeg 为旧字段名,读取时并入 compression) */
type Quality = { scale: number; compression: number; idleFps: number; maxFps: number };
const DEFAULTS = { scale: 100, compression: 70, idleFps: 1, maxFps: 20 };

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}

/** 服务器只提供 WS:默认连页面同主机的默认端口;file:// 等无主机场景必须显式传 url */
function wsUrlOf(opts: WindowOptions): string {
  const base = (opts.url || defaultWsOrigin()).replace(/\/+$/, "");
  return (base.startsWith("ws") ? base : base.replace(/^http/, "ws")) + "/ws";
}

function defaultWsOrigin(): string {
  if (location.hostname) return `ws://${location.hostname}:9527`;
  throw new Error(
    "WebDesk.window: 无法推断服务地址(页面无主机名),请传 opts.url,如 WebDesk.window('#id', { url: 'ws://192.168.1.10:9527', token: '...' })",
  );
}

function resolveToken(explicit?: string): string {
  if (explicit) return explicit;
  const q = new URLSearchParams(location.search).get("token");
  if (q) return q;
  try {
    return localStorage.getItem("webdesk.token") || "";
  } catch {
    return "";
  }
}

/** 单个嵌入视图:容器内的画布 + 状态角标 + FPS 角标 + 输入 */
class RemoteDesk implements DeskHandle {
  readonly element: HTMLElement;
  readonly canvas: HTMLCanvasElement;
  private wrap: HTMLDivElement;
  private statusEl: HTMLDivElement;
  private fpsEl: HTMLDivElement;
  private ctx: CanvasRenderingContext2D;
  private input: InputSender;
  private ro: ResizeObserver;
  private rafId = 0;
  private destroyed = false;
  private onStatus?: (s: DeskStatus) => void;
  private onClipboard?: (text: string) => void;

  constructor(
    private shared: SharedClient,
    container: HTMLElement,
    opts: WindowOptions,
  ) {
    this.onStatus = opts.onStatus;
    this.onClipboard = opts.onClipboard;
    this.element = container;

    this.wrap = document.createElement("div");
    // flex 居中:画布按原屏幕宽高比等比缩放(contain),容器多余部分留黑边
    this.wrap.style.cssText =
      "position:relative;width:100%;height:100%;min-height:80px;background:#000;overflow:hidden;" +
      "display:flex;align-items:center;justify-content:center";
    this.canvas = document.createElement("canvas");
    this.canvas.style.cssText = "display:block;outline:none;cursor:default";
    this.canvas.tabIndex = 0; // 点击聚焦后接收键盘
    this.statusEl = document.createElement("div");
    this.statusEl.style.cssText =
      "position:absolute;left:8px;bottom:8px;padding:2px 8px;border-radius:4px;" +
      "background:rgba(0,0,0,.65);color:#eee;font:12px/1.4 system-ui,sans-serif;display:none";
    // FPS 角标(屏幕右上角):纯展示,不可点选、不挡画布输入
    this.fpsEl = document.createElement("div");
    this.fpsEl.style.cssText =
      "position:absolute;top:6px;right:6px;z-index:3;padding:2px 8px;border-radius:6px;" +
      "background:rgba(0,0,0,.5);color:#7ec8ff;font:11px/1.5 ui-monospace,Menlo,Consolas,monospace;" +
      "pointer-events:none;user-select:none;-webkit-user-select:none";
    this.fpsEl.textContent = "0 FPS";
    this.wrap.append(this.canvas, this.statusEl, this.fpsEl);
    container.appendChild(this.wrap);

    const ctx = this.canvas.getContext("2d", { alpha: false });
    if (!ctx) throw new Error("WebDesk.window: 无法创建 2D 上下文");
    this.ctx = ctx;

    this.input = new InputSender((buf) => shared.send(buf), this.canvas);
    this.input.attach("element");
    // 活跃度检测(0.8.0):画布内任何输入都通知共享客户端切换 FPS 上报
    this.input.onUserInput = () => shared.viewActivity(this);
    this.canvas.addEventListener("pointerdown", () => {
      window.getSelection()?.removeAllRanges();
      this.canvas.focus();
    });
    this.canvas.addEventListener("dragstart", (e) => e.preventDefault());

    this.ro = new ResizeObserver(() => this.resize());
    this.ro.observe(container);
    this.resize();
    this.setStatus({ connected: false, text: "WebDesk 连接中…" });
  }

  /**
   * 重算画布尺寸:宽高比恒等于原屏幕(contain 等比),容器多余部分留黑边,
   * 绝不拉伸画面。hello 未知时先铺满容器,hello 到达后由 SharedClient 再触发。
   */
  resize(): void {
    if (this.destroyed) return;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const boxW = this.wrap.clientWidth;
    const boxH = this.wrap.clientHeight;
    if (boxW <= 0 || boxH <= 0) return;
    const sw = this.shared.hello?.width || boxW;
    const sh = this.shared.hello?.height || boxH;
    const fit = Math.min(boxW / sw, boxH / sh);
    const cssW = Math.max(1, Math.floor(sw * fit));
    const cssH = Math.max(1, Math.floor(sh * fit));
    this.canvas.style.width = `${cssW}px`;
    this.canvas.style.height = `${cssH}px`;
    const w = Math.round(cssW * dpr);
    const h = Math.round(cssH * dpr);
    if (w > 0 && h > 0 && (this.canvas.width !== w || this.canvas.height !== h)) {
      this.canvas.width = w;
      this.canvas.height = h;
      this.schedule();
    }
  }

  /** 请求一次重绘(rAF 合并;仅在共享连接每帧 onPaint 与尺寸变化时调用) */
  schedule(): void {
    if (this.destroyed || this.rafId) return;
    this.rafId = requestAnimationFrame(() => {
      this.rafId = 0;
      this.paint();
    });
  }

  private paint(): void {
    const master = this.shared.master;
    if (!master.width || !master.height) return;
    this.ctx.imageSmoothingEnabled = true;
    this.ctx.drawImage(master, 0, 0, this.canvas.width, this.canvas.height);
  }

  setFramebufferSize(w: number, h: number): void {
    this.input.setFramebufferSize(w, h);
  }

  /** 该视图是否仍在交互:idleTimeout 内有输入,或有键按住(供共享客户端聚合) */
  inputHot(idleTimeoutMs: number): boolean {
    return this.input.inputHot(idleTimeoutMs);
  }

  setStatus(s: DeskStatus): void {
    if (this.destroyed) return;
    this.statusEl.textContent = s.text;
    this.statusEl.style.display = s.text ? "block" : "none";
    this.statusEl.style.color = s.connected ? "#9fd49f" : "#ff6b6b";
    this.onStatus?.(s);
  }

  deliverClipboard(text: string): void {
    if (!this.destroyed) this.onClipboard?.(text);
  }

  /** 共享客户端每秒推一次实测帧率 */
  setFps(n: number): void {
    if (!this.destroyed) this.fpsEl.textContent = `${n} FPS`;
  }

  setQuality(q: QualityInput): void {
    this.shared.updateQuality(q);
  }

  setClipboard(text: string): void {
    this.shared.send(encodeClipboardSet(text));
  }

  getClipboard(timeoutMs = 2000): Promise<string> {
    return this.shared.getClipboard(timeoutMs);
  }

  screenshot(opts: ScreenshotOptions = {}): Promise<Blob> {
    return new Promise((resolve, reject) => {
      this.shared.master.toBlob(
        (b) => (b ? resolve(b) : reject(new Error("WebDesk: 截图失败"))),
        opts.format === "jpeg" ? "image/jpeg" : "image/png",
        opts.quality,
      );
    });
  }

  screenSize(): { w: number; h: number } | null {
    const h = this.shared.hello;
    return h ? { w: h.width, h: h.height } : null;
  }

  stats(): { bytes: number; frames: number } {
    return { bytes: this.shared.bytesTotal, frames: this.shared.framesTotal };
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.ro.disconnect();
    this.input.detach();
    if (this.rafId) cancelAnimationFrame(this.rafId);
    this.wrap.remove();
    this.shared.viewGone(this); // 先退出活跃聚合,再走关闭流程
    this.shared.remove(this);
  }
}

/** 页面级共享客户端:一路 WS + 一次解码,喂给所有视图 */
class SharedClient {
  readonly master: HTMLCanvasElement;
  hello: HelloMsg | null = null;
  bytesTotal = 0;
  framesTotal = 0;
  /** 实测帧率(每秒采样 framesTotal 差值,推给所有视图角标) */
  private fps = 0;
  private lastFpsFrames = 0;
  private fpsTimer = 0;
  private keepaliveTimer = 0;
  private renderer: Renderer | null = null;
  private views = new Set<RemoteDesk>();
  private quality: Quality;
  private ws: WebSocket | null = null;
  private retryTimer = 0;
  private closed = false;
  private paintedOnce = false;
  private clipWaiters: Array<(text: string) => void> = [];
  /** 空闲倒计时:停止输入且无按住超过此时长回落 idleFps(0.8.0 前端控档) */
  private idleTimeoutMs: number;
  /** 当前处于交互态的视图集合 */
  private activeViews = new Set<RemoteDesk>();
  /** 是否已按交互档(maxFps)上报;false = 空闲档(idleFps) */
  private fpsHigh = false;
  private idleTimer = 0;

  constructor(
    private wsUrl: string,
    private token: string,
    opts: WindowOptions,
  ) {
    this.master = document.createElement("canvas");
    this.quality = {
      scale: clamp(opts.scale ?? DEFAULTS.scale, 10, 100),
      compression: clamp(opts.compression ?? opts.jpeg ?? DEFAULTS.compression, 0, 100),
      idleFps: Math.max(1, opts.idleFps ?? DEFAULTS.idleFps),
      maxFps: Math.max(opts.idleFps ?? DEFAULTS.idleFps, opts.maxFps ?? DEFAULTS.maxFps),
    };
    this.idleTimeoutMs = Math.max(500, opts.idleTimeout ?? 3000);
    this.fpsTimer = window.setInterval(() => this.tickFps(), 1000);
    // 应用层保活:画面静止时客户端原本零发送,部分反代(如 CF 边缘)只看
    // 客户端方向的空闲就会掐链路(服务端的协议 Ping 可能被中间层代答/忽略)。
    // 每 25s 发一帧 1 字节 KEEPALIVE,重置沿途所有空闲计时;服务端将其忽略。
    this.keepaliveTimer = window.setInterval(() => this.send(KEEPALIVE), 25000);
    this.connect();
  }

  private tickFps(): void {
    this.fps = this.framesTotal - this.lastFpsFrames;
    this.lastFpsFrames = this.framesTotal;
    for (const v of this.views) v.setFps(this.fps);
  }

  /** setQuality / 后续 window() 调用更新画质,立即生效 */
  updateQuality(opts: QualityInput): void {
    const next: Quality = {
      scale: clamp(opts.scale ?? this.quality.scale, 10, 100),
      compression: clamp(
        opts.compression ?? opts.jpeg ?? this.quality.compression,
        0,
        100,
      ),
      idleFps: Math.max(1, opts.idleFps ?? this.quality.idleFps),
      maxFps: Math.max(opts.idleFps ?? this.quality.idleFps, opts.maxFps ?? this.quality.maxFps),
    };
    if (
      next.scale !== this.quality.scale ||
      next.compression !== this.quality.compression ||
      next.idleFps !== this.quality.idleFps ||
      next.maxFps !== this.quality.maxFps
    ) {
      this.quality = next;
      this.sendSettings();
    }
  }

  add(view: RemoteDesk): void {
    this.views.add(view);
    view.setFps(this.fps);
    if (this.hello) {
      view.setFramebufferSize(this.hello.width, this.hello.height);
      view.resize(); // 屏幕尺寸已知:按真实宽高比重算画布
    }
    view.schedule();
  }

  remove(view: RemoteDesk): void {
    this.views.delete(view);
    if (this.views.size === 0) this.shutdown();
  }

  send(buf: Uint8Array): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(buf);
    }
  }

  getClipboard(timeoutMs = 2000): Promise<string> {
    return new Promise<string>((resolve) => {
      let waiter: (text: string) => void;
      const timer = window.setTimeout(() => {
        this.clipWaiters = this.clipWaiters.filter((w) => w !== waiter);
        resolve("");
      }, timeoutMs);
      waiter = (text) => {
        window.clearTimeout(timer);
        this.clipWaiters = this.clipWaiters.filter((w) => w !== waiter);
        resolve(text);
      };
      this.clipWaiters.push(waiter);
      this.send(CLIPBOARD_GET);
    });
  }

  private sendSettings(): void {
    // 0.8.0:FPS 上限由前端决定 —— 交互档报 maxFps,空闲档报 idleFps
    const fps = this.fpsHigh ? this.quality.maxFps : this.quality.idleFps;
    this.send(
      encodeSettings(
        this.quality.scale,
        this.quality.compression,
        this.quality.idleFps,
        this.quality.maxFps,
        fps,
      ),
    );
  }

  // ---- 活跃度控制(0.8.0,服务端零输入检测)----
  // 任何视图有输入 → 上报 maxFps;之后 idleTimeout 内无输入且无按住 → 上报
  // idleFps。按住不松开(handled 由 InputSender 跟踪)不算空闲,Up 后才开始
  // 倒计时 —— 即「Down 进高帧率,Up 后 3s 降档」。

  /** 任意视图有用户输入(InputSender.onUserInput) */
  viewActivity(v: RemoteDesk): void {
    this.activeViews.add(v);
    this.setFpsHigh(true);
    this.armIdle();
  }

  /** 视图销毁:退出活跃聚合,可能是最后一个活跃视图 */
  viewGone(v: RemoteDesk): void {
    this.activeViews.delete(v);
    if (this.activeViews.size === 0) this.setFpsHigh(false);
  }

  private setFpsHigh(on: boolean): void {
    if (this.fpsHigh === on) return;
    this.fpsHigh = on;
    this.sendSettings();
  }

  private armIdle(): void {
    window.clearTimeout(this.idleTimer);
    this.idleTimer = window.setTimeout(() => {
      this.idleTimer = 0;
      for (const v of [...this.activeViews]) {
        if (!v.inputHot(this.idleTimeoutMs)) this.activeViews.delete(v);
      }
      if (this.activeViews.size === 0) {
        this.setFpsHigh(false);
      } else {
        // 仍有视图在交互(如按住不松开):继续等下一轮
        this.armIdle();
      }
    }, this.idleTimeoutMs);
  }

  private connect(): void {
    if (this.closed) return;
    const ws = new WebSocket(`${this.wsUrl}?token=${encodeURIComponent(this.token)}`);
    ws.binaryType = "arraybuffer";
    this.ws = ws;

    ws.onopen = () => {
      this.statusAll({ connected: true, text: "已连接,等待首帧…" });
      this.sendSettings();
    };
    ws.onmessage = (ev) => {
      if (typeof ev.data === "string") return;
      const data = new Uint8Array(ev.data);
      this.bytesTotal += data.byteLength;
      let msg: ServerMsg;
      try {
        msg = decodeServer(data);
      } catch (e) {
        console.error("WebDesk SDK: decode error", e);
        return;
      }
      this.onServerMsg(msg);
    };
    ws.onclose = (ev) => {
      this.ws = null;
      if (this.closed) return;
      this.statusAll({
        connected: false,
        text: `连接断开(${ev.reason || ev.code}),3s 后重连…`,
      });
      this.retryTimer = window.setTimeout(() => this.connect(), 3000);
    };
    ws.onerror = () => {
      // onclose 会跟随
    };
  }

  private onServerMsg(msg: ServerMsg): void {
    switch (msg.type) {
      case "hello": {
        this.hello = msg.hello;
        this.renderer = new Renderer(this.master);
        this.renderer.onPaint = () => {
          if (!this.paintedOnce) {
            // 首帧已上画,撤掉"等待首帧"角标
            this.paintedOnce = true;
            this.statusAll({ connected: true, text: "" });
          }
          for (const v of this.views) v.schedule();
        };
        this.renderer.init(this.hello.width, this.hello.height);
        for (const v of this.views) {
          v.setFramebufferSize(this.hello.width, this.hello.height);
          v.resize(); // 首次得知屏幕尺寸:从"铺满"切换为等比 letterbox
        }
        break;
      }
      case "frame": {
        this.framesTotal++;
        this.renderer?.applyFrame(msg.frame);
        break;
      }
      case "clipboard": {
        const waiters = this.clipWaiters;
        this.clipWaiters = [];
        for (const w of waiters) w(msg.text);
        for (const v of this.views) v.deliverClipboard(msg.text);
        break;
      }
    }
  }

  private statusAll(s: DeskStatus): void {
    for (const v of this.views) v.setStatus(s);
  }

  private shutdown(): void {
    this.closed = true;
    window.clearInterval(this.fpsTimer);
    window.clearInterval(this.keepaliveTimer);
    window.clearTimeout(this.retryTimer);
    window.clearTimeout(this.idleTimer);
    this.ws?.close();
    this.ws = null;
    if (shared === this) shared = null;
  }
}

let shared: SharedClient | null = null;
let sharedToken = "";
let sharedUrl = "";

function mount(selector: string, opts: WindowOptions = {}): DeskHandle {
  const container = document.querySelector(selector);
  if (!(container instanceof HTMLElement)) {
    throw new Error(`WebDesk.window: 找不到元素 ${selector}`);
  }
  const token = resolveToken(opts.token);
  if (!token) {
    throw new Error(
      "WebDesk.window: 缺少 token —— 请传入 opts.token(agent 以 -k <token> 启动),或在页面 URL 加 ?token=",
    );
  }
  const wsUrl = wsUrlOf(opts);
  if (!shared) {
    shared = new SharedClient(wsUrl, token, opts);
  } else {
    if (sharedToken !== token || sharedUrl !== wsUrl) {
      throw new Error("WebDesk.window: 同一页面仅支持一个 WebDesk 服务(url/token 需一致)");
    }
    shared.updateQuality(opts);
  }
  sharedToken = token;
  sharedUrl = wsUrl;
  const view = new RemoteDesk(shared, container, opts);
  shared.add(view);
  return view;
}

export { mount as window };
