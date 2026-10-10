// 浏览器事件 → WebDesk Input Message(设计文档 §12)。

import { encodeKey, encodeClipboardSet, encodeMouse, MouseAction } from "./protocol";

// event.code → X keysym
const CODE_KEYSYM: Record<string, number> = {
  Escape: 0xff1b,
  Tab: 0xff09,
  Backspace: 0xff08,
  Enter: 0xff0d,
  NumpadEnter: 0xff0d,
  Space: 0x0020,
  CapsLock: 0xffe5,
  F1: 0xffbe,
  F2: 0xffbf,
  F3: 0xffc0,
  F4: 0xffc1,
  F5: 0xffc2,
  F6: 0xffc3,
  F7: 0xffc4,
  F8: 0xffc5,
  F9: 0xffc6,
  F10: 0xffc7,
  F11: 0xffc8,
  F12: 0xffc9,
  PrintScreen: 0xff61,
  ScrollLock: 0xff14,
  Pause: 0xff13,
  Insert: 0xff63,
  Home: 0xff50,
  PageUp: 0xff55,
  Delete: 0xffff,
  End: 0xff57,
  PageDown: 0xff56,
  ArrowRight: 0xff53,
  ArrowLeft: 0xff51,
  ArrowDown: 0xff54,
  ArrowUp: 0xff52,
  NumLock: 0xff7f,
  ContextMenu: 0xff67,
  ShiftLeft: 0xffe1,
  ShiftRight: 0xffe2,
  ControlLeft: 0xffe3,
  ControlRight: 0xffe4,
  AltLeft: 0xffe9,
  AltRight: 0xffea,
  MetaLeft: 0xffeb,
  MetaRight: 0xffec,
  // 小键盘
  NumpadDivide: 0xffaf,
  NumpadMultiply: 0xffaa,
  NumpadSubtract: 0xffad,
  NumpadAdd: 0xffab,
  NumpadDecimal: 0xffae,
  Numpad0: 0xffb0,
  Numpad1: 0xffb1,
  Numpad2: 0xffb2,
  Numpad3: 0xffb3,
  Numpad4: 0xffb4,
  Numpad5: 0xffb5,
  Numpad6: 0xffb6,
  Numpad7: 0xffb7,
  Numpad8: 0xffb8,
  Numpad9: 0xffb9,
};

/** event.key(单个字符)→ keysym:ASCII 直接取码位。 */
function keysymFromKey(key: string): number {
  if (key.length === 1) {
    const cp = key.codePointAt(0)!;
    if (cp >= 0x20 && cp < 0x100) return cp;
    if (cp >= 0x100) return 0x01000000 + cp; // ISO 10646 keysym
  }
  return 0;
}

export class InputSender {
  private send: (buf: Uint8Array) => void;
  private canvas: HTMLCanvasElement;
  private fbW = 0;
  private fbH = 0;
  /** 滚轮小数累积 */
  private accX = 0;
  private accY = 0;
  /** pointermove 合并:每帧最多发一条 */
  private pendingMove: { x: number; y: number } | null = null;
  private moveRaf = 0;
  /** 粘贴拦截时挂起的远端 Ctrl 掩码 */
  private pasteGuard = false;
  /** —— 活跃度检测(0.8.0,SDK 端 FPS 档位控制的数据源)—— */
  private lastInputAt = 0;
  private heldButtons = 0;
  private heldKeys = new Set<number>();
  /** 任何用户输入发出后回调(SharedClient 据此上报 ACTIVITY 并重排空闲倒计时) */
  onUserInput: (() => void) | null = null;

  constructor(send: (buf: Uint8Array) => void, canvas: HTMLCanvasElement) {
    this.send = send;
    this.canvas = canvas;
  }

  setFramebufferSize(w: number, h: number): void {
    this.fbW = w;
    this.fbH = h;
  }

  /** 是否仍在交互:idleTimeoutMs 内有输入,或鼠标键/键盘键仍被按住 */
  inputHot(idleTimeoutMs: number): boolean {
    return (
      performance.now() - this.lastInputAt < idleTimeoutMs ||
      this.heldButtons !== 0 ||
      this.heldKeys.size > 0
    );
  }

  /** 输入打点:记录时间 + 通知共享客户端 */
  private touch(): void {
    this.lastInputAt = performance.now();
    this.onUserInput?.();
  }

  /**
   * 绑定事件。
   * keyTarget="window"(默认,主应用):页面任意处的按键都发往远端;
   * keyTarget="element"(SDK 嵌入):只响应画布获得焦点后的按键,
   * 不劫持宿主页面;画布失焦时释放修饰键防卡死。
   */
  attach(keyTarget: "window" | "element" = "window"): void {
    const c = this.canvas;

    c.addEventListener("pointermove", (e) => this.onPointerMove(e));
    c.addEventListener("pointerdown", (e) => this.onPointerButton(e, true));
    c.addEventListener("pointerup", (e) => this.onPointerButton(e, false));
    c.addEventListener("pointerleave", () => {
      // 光标离开画布不再移动
      if (this.pendingMove) {
        this.flushMove();
      }
    });
    c.addEventListener("wheel", (e) => this.onWheel(e), { passive: false });
    c.addEventListener("contextmenu", (e) => e.preventDefault());

    const keyHost: EventTarget = keyTarget === "element" ? c : window;
    keyHost.addEventListener("keydown", (e) => this.onKey(e as KeyboardEvent, true));
    keyHost.addEventListener("keyup", (e) => this.onKey(e as KeyboardEvent, false));
    // 失焦时释放按键,避免远端按键卡死
    (keyTarget === "element" ? c : window).addEventListener("blur", () => this.releaseAll());
  }

  detach(): void {
    if (this.moveRaf) cancelAnimationFrame(this.moveRaf);
  }

  /** 画布坐标 → framebuffer 坐标。 */
  private toFb(clientX: number, clientY: number): { x: number; y: number } {
    const rect = this.canvas.getBoundingClientRect();
    const x = Math.round(((clientX - rect.left) / rect.width) * this.fbW);
    const y = Math.round(((clientY - rect.top) / rect.height) * this.fbH);
    return {
      x: Math.max(0, Math.min(this.fbW - 1, x)),
      y: Math.max(0, Math.min(this.fbH - 1, y)),
    };
  }

  private onPointerMove(e: PointerEvent): void {
    const { x, y } = this.toFb(e.clientX, e.clientY);
    this.pendingMove = { x, y };
    if (!this.moveRaf) {
      this.moveRaf = requestAnimationFrame(() => this.flushMove());
    }
    if (e.buttons !== 0 && this.canvas.hasPointerCapture(e.pointerId)) {
      // 拖动中由 capture 保证继续收到事件,无需额外处理
    }
  }

  private flushMove(): void {
    this.moveRaf = 0;
    if (!this.pendingMove) return;
    const { x, y } = this.pendingMove;
    this.pendingMove = null;
    this.send(encodeMouse(MouseAction.Move, 0, x, y));
    this.touch();
  }

  private onPointerButton(e: PointerEvent, down: boolean): void {
    e.preventDefault();
    // 确保位置先于按下事件到达
    const { x, y } = this.toFb(e.clientX, e.clientY);
    this.send(encodeMouse(MouseAction.Move, 0, x, y));
    // browser: 0 left 1 middle 2 right → X: 1 2 3
    const button = e.button + 1;
    this.send(encodeMouse(down ? MouseAction.Down : MouseAction.Up, button, x, y));
    // 按住跟踪:Down 无条件保持活跃,Up 后才开始空闲倒计时
    if (down) {
      this.heldButtons |= button;
      try {
        if (!this.canvas.hasPointerCapture(e.pointerId)) {
          this.canvas.setPointerCapture(e.pointerId);
        }
      } catch {
        // 合成事件(测试/自动化)没有活动指针,捕获失败可忽略
      }
    } else {
      this.heldButtons &= ~button;
    }
    this.touch();
  }

  private onWheel(e: WheelEvent): void {
    e.preventDefault();
    const { x, y } = this.toFb(e.clientX, e.clientY);
    let dx: number;
    let dy: number;
    if (e.deltaMode === 1) {
      dx = e.deltaX;
      dy = e.deltaY;
    } else if (e.deltaMode === 2) {
      dx = e.deltaX * 16;
      dy = e.deltaY * 16;
    } else {
      // pixel 模式:Chrome 一格 ≈ 100px,Firefox ≈ 53px
      dx = e.deltaX / 53;
      dy = e.deltaY / 53;
    }
    this.accX += dx;
    this.accY += dy;
    const clicksX = Math.trunc(this.accX);
    const clicksY = Math.trunc(this.accY);
    if (clicksX !== 0) this.accX -= clicksX;
    if (clicksY !== 0) this.accY -= clicksY;
    if (clicksX !== 0 || clicksY !== 0) {
      this.send(encodeMouse(MouseAction.Wheel, 0, x, y, clicksX, clicksY));
      this.touch();
    }
  }

  private keysymOf(e: KeyboardEvent): number {
    if (CODE_KEYSYM[e.code] !== undefined) return CODE_KEYSYM[e.code];
    return keysymFromKey(e.key);
  }

  private onKey(e: KeyboardEvent, down: boolean): void {
    // 本地组合键放行:devtools / 刷新
    if (e.ctrlKey && e.shiftKey) return;
    if (e.key === "F5" || e.key === "F12") return;

    // Ctrl/Cmd+V 拦截:先把本地文本写入远端剪贴板,再注入 v
    if (down && (e.ctrlKey || e.metaKey) && (e.code === "KeyV" || e.key === "v")) {
      e.preventDefault();
      this.doPaste(e.ctrlKey ? 0xffe3 : 0xffeb, e);
      return;
    }

    // Ctrl/Cmd+C:拦截并从远端拉取剪贴板到本地
    if (down && (e.ctrlKey || e.metaKey) && (e.code === "KeyC" || e.key === "c")) {
      // 不 preventDefault:远端执行复制,server 检测到属主变化会自动推送
    }

    const keysym = this.keysymOf(e);
    if (!keysym) return;
    e.preventDefault();
    if (down && e.repeat) return; // X server 自带自动重复
    this.send(encodeKey(down, keysym));
    // 按住跟踪:按住只有首条 down(repeat 已过滤),松开的 up 才移除
    if (down) this.heldKeys.add(keysym);
    else this.heldKeys.delete(keysym);
    this.touch();
  }

  /** 读取本地剪贴板 → CLIPBOARD_SET → 注入 v(见设计文档 §13 流程)。 */
  private async doPaste(ctrlKeysym: number, _e: KeyboardEvent): Promise<void> {
    if (this.pasteGuard) return;
    this.touch(); // Ctrl+V 是用户输入:走剪贴板注入分支也要计为活跃
    this.pasteGuard = true;
    try {
      const text = await navigator.clipboard.readText();
      if (text) {
        this.send(encodeClipboardSet(text));
      }
      this.send(encodeKey(true, ctrlKeysym));
      this.send(encodeKey(true, 0x76)); // v
      this.send(encodeKey(false, 0x76));
      this.send(encodeKey(false, ctrlKeysym));
    } catch {
      // 读剪贴板失败(权限):仍注入 Ctrl+V,粘贴远端旧内容
      this.send(encodeKey(true, ctrlKeysym));
      this.send(encodeKey(true, 0x76));
      this.send(encodeKey(false, 0x76));
      this.send(encodeKey(false, ctrlKeysym));
    } finally {
      this.pasteGuard = false;
    }
  }

  private releaseAll(): void {
    // 失焦后按住状态全部作废(收不到对应 up),清空避免活跃判定卡在"按住"
    this.heldButtons = 0;
    this.heldKeys.clear();
    // 释放常见修饰键,防止远端按键卡死
    for (const ks of [0xffe1, 0xffe2, 0xffe3, 0xffe4, 0xffe9, 0xffea, 0xffeb, 0xffec]) {
      this.send(encodeKey(false, ks));
    }
    this.touch(); // 通知共享客户端重排空闲倒计时
  }
}
