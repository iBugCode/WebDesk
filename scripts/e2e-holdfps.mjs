// 按住不松开 = 持续交互(0.8.0 FPS 由前端控制):
// - SDK 检测输入/按住:按下(Down)立即按 maxFps 上报 SETTINGS.fps,按住不松开
//   保持高帧率,松开(Up)后 3s 空闲倒计时到点回落 idleFps
// - 服务端纯执行 SETTINGS 的 fps 字节,不做任何输入检测:
//   裸 WS 直接下发 fps=20/fps=1 应同样直接控制帧率
// 方法:远端打开跑马灯动画页(持续产生脏区),用 stats().frames 差值测各窗口帧数。
import { chromium } from "playwright-core";
import {
  CHROME, TOKEN, WS_URL,
  sleep, check, resetChrome, mountDesk, waitPainted, navigateRemote, clickFb,
} from "./e2e-lib.mjs";

// 跑马灯:div 持续右移,持续产生 damage(帧率只受调度器 FPS 档位限制)
const HOLD_URL =
  "data:text/html;base64," +
  Buffer.from(
    `<body style="margin:0;background:#fff"><div id=m style="position:fixed;top:300px;left:0;width:120px;height:60px;background:#1a73e8"></div>
<script>let x=0;(function tick(){x=(x+3)%1160;m.style.left=x+'px';requestAnimationFrame(tick)})();</script>`,
  ).toString("base64");

await resetChrome();
const browser = await chromium.launch({ executablePath: CHROME, headless: true, args: ["--no-sandbox", "--disable-gpu"] });
const page = await browser.newPage({ viewport: { width: 1200, height: 940 } });

await mountDesk(page);
check("首帧渲染", await waitPainted(page));
await navigateRemote(page, HOLD_URL);

async function framesIn(ms) {
  const a = await page.evaluate(() => window.__h.stats().frames);
  await sleep(ms);
  const b = await page.evaluate(() => window.__h.stats().frames);
  return b - a;
}

// ---- 1) 基线:无输入 >3s 后应已降档(≤ idle 1 FPS) ----
await sleep(4500);
const idle1 = await framesIn(2500);
check("空闲(无输入 3s+)降档 ≤ idle FPS", idle1 <= 5, `${idle1} 帧 / 2.5s`);

// ---- 2) 按住鼠标左键不松开:4s(> idle timeout)后仍应保持 max FPS ----
const b = await page.evaluate(() => {
  const r = document.querySelector("#mount canvas").getBoundingClientRect();
  return { left: r.left, top: r.top, w: r.width, h: r.height };
});
await page.mouse.move(b.left + b.w / 2, b.top + b.h / 2);
await page.mouse.down(); // 按下不松开
await sleep(4000); // 若无按住跟踪,此处交互态早已超时
const holdBtn = await framesIn(2500);
check("按住鼠标不松开保持 max FPS(超时后仍交互)", holdBtn >= 15, `${holdBtn} 帧 / 2.5s`);

// ---- 3) 松开鼠标:3s 倒计时后降档 ----
await page.mouse.up();
await sleep(4500);
const idle2 = await framesIn(2500);
check("松开鼠标 3s 后降档 ≤ idle FPS", idle2 <= 5, `${idle2} 帧 / 2.5s`);

// ---- 4) 按住键盘键不松开:同样保持 max FPS ----
await clickFb(page, 640, 400); // 点击使画布获得键盘焦点(本身是一次输入)
await sleep(200);
await page.keyboard.down("a"); // SDK 过滤 repeat:按住期间只有这一条 down
await sleep(4000); // 点击输入的时间戳已超时,高 FPS 只能来自按住状态
const holdKey = await framesIn(2500);
check("按住键盘不松开保持 max FPS(超时后仍交互)", holdKey >= 15, `${holdKey} 帧 / 2.5s`);

// ---- 5) 松开键盘:降档 ----
await page.keyboard.up("a");
await sleep(4500);
const idle3 = await framesIn(2500);
check("松开键盘 3s 后降档 ≤ idle FPS", idle3 <= 5, `${idle3} 帧 / 2.5s`);

// ---- 6) 服务端纯执行:裸 WS 直发 SETTINGS fps,不经任何 SDK/输入 ----
// SETTINGS: [0x12][scale=100][compression=70][idle=1][max=20][fps]
const raw = await page.evaluate(async (wsUrl) => {
  const ws = new WebSocket(wsUrl);
  ws.binaryType = "arraybuffer";
  await new Promise((res, rej) => { ws.onopen = res; ws.onerror = () => rej(new Error("ws fail")); });
  window.__rawWs = ws;
  window.__setFps = (fps) => ws.send(new Uint8Array([0x12, 100, 70, 1, 20, fps]));
  window.__setFps(20); // 前端要 20 就发 20
  return true;
}, `${WS_URL}/ws?token=${encodeURIComponent(TOKEN)}`);
check("裸 WS 连接成功", raw === true);
await sleep(1000);
const rawHigh = await framesIn(2500);
check("裸 WS 下发 fps=20 → 帧率立即抬升", rawHigh >= 15, `${rawHigh} 帧 / 2.5s`);
await page.evaluate(() => window.__setFps(1)); // 要 1 就发 1
await sleep(3500);
const rawLow = await framesIn(2500);
check("裸 WS 下发 fps=1 → 帧率回落", rawLow <= 5, `${rawLow} 帧 / 2.5s`);
await page.evaluate(() => window.__rawWs.close());

await browser.close();
console.log(process.exitCode ? "HOLDFPS_FAIL" : "HOLDFPS_ALL_PASS");
process.exit(process.exitCode || 0);
