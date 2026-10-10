// 指针/键盘精度 E2E:CSS 缩放画布(1100x619 显示 1280x720 fb)下
// 真实鼠标点击与键盘输入经 SDK 画布驱动远端 Chrome。
import { chromium } from "playwright-core";
import fs from "node:fs";
import {
  CHROME, SHOTS, FB_W, FB_H,
  sleep, check, resetChrome, mountDesk, waitPainted, sample, clickFb,
  navigateRemote, TOGGLE_URL,
} from "./e2e-lib.mjs";

fs.mkdirSync(SHOTS, { recursive: true });
await resetChrome();

const browser = await chromium.launch({
  executablePath: CHROME, headless: true, args: ["--no-sandbox", "--disable-gpu"],
});
const page = await browser.newPage({ viewport: { width: 1200, height: 940 } });

// ---- 1) 本地指针与 CSS 缩放前提 ----
await mountDesk(page, { width: 1100, height: 619 }); // CSS 缩放:1100/1280 = 0.859
check("首帧渲染", await waitPainted(page));
const scaling = await page.evaluate(() => {
  const c = document.querySelector("#mount canvas");
  const r = c.getBoundingClientRect();
  return {
    cssW: r.width, fbW: 1280, attrW: c.width,
    cursor: getComputedStyle(c).cursor,
    hasFakeCursor: !!document.getElementById("cursor"),
  };
});
check("画布 CSS 缩放生效(测试前提)", scaling.cssW === 1100 && scaling.attrW === 1100 && scaling.cssW < scaling.fbW,
  `canvas CSS ${scaling.cssW} attr ${scaling.attrW}`);
check("使用浏览器本地指针", !scaling.hasFakeCursor && scaling.cursor !== "none",
  `cursor=${scaling.cursor}, #cursor overlay=${scaling.hasFakeCursor}`);

// ---- 2) 导航到红蓝翻转页(真实点击地址栏 + 键盘输入) ----
await navigateRemote(page, TOGGLE_URL);

// ---- 3) CSS 缩放下真实点击精确:3 次色块翻转 ----
let hits = 0;
for (let i = 0; i < 3; i++) {
  const b = await sample(page, 480 / FB_W, 320 / FB_H);
  await clickFb(page, 480, 320);
  await sleep(800);
  const a = await sample(page, 480 / FB_W, 320 / FB_H);
  if (b.r > b.b !== a.r > a.b) hits++;
}
check("CSS 缩放下真实鼠标点击精确(3 次色块切换)", hits === 3, `命中 ${hits}/3`);

// ---- 4) 远端画面无 X 光标(XFixesHideCursor):指针停靠处纯色 ----
const cx = 1000, cy = 400; // Chrome 窗口内页面区域(窗口 80,60 → 页内均匀色块)
await clickFb(page, cx, cy);
await sleep(1200);
const uniform = await page.evaluate(([fx, fy]) => {
  const c = document.querySelector("#mount canvas");
  const x = Math.round(c.width * fx), y = Math.round(c.height * fy);
  const d = c.getContext("2d").getImageData(x, y, 24, 24).data;
  const r0 = d[0], g0 = d[1], b0 = d[2];
  let diff = 0;
  for (let i = 4; i < d.length; i += 4) {
    if (Math.abs(d[i] - r0) + Math.abs(d[i + 1] - g0) + Math.abs(d[i + 2] - b0) > 12) diff++;
  }
  return diff;
}, [cx / FB_W, cy / FB_H]);
check("远端画面无 X 光标(XFixesHideCursor)", uniform === 0, `24x24 采样差异像素 ${uniform}`);

// ---- 5) 50% 画质下指针精度 ----
await page.evaluate(() => window.__h.setQuality({ scale: 50 }));
await sleep(2500);
const b5 = await sample(page, 480 / FB_W, 320 / FB_H);
await clickFb(page, 480, 320);
await sleep(1500); // 翻转帧到达有时 >800ms(空闲唤醒抖动),放宽断言窗口
const a5 = await sample(page, 480 / FB_W, 320 / FB_H);
check("画面质量 50% 下指针精度", b5.r > b5.b !== a5.r > a5.b,
  `(${b5.r.toFixed(0)},${b5.g.toFixed(0)},${b5.b.toFixed(0)})→(${a5.r.toFixed(0)},${a5.g.toFixed(0)},${a5.b.toFixed(0)})`);

await page.screenshot({ path: SHOTS + "/pointer-local.png" });
await browser.close();
console.log(process.exitCode ? "POINTER_FAIL" : "POINTER_OK");
