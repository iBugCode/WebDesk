// 设计 §18 V1 验收(SDK 驱动):整屏渲染 / 地址栏导航 / 点击 / 拖动窗口 / WebP 压缩 / 剪贴板。
import { chromium } from "playwright-core";
import fs from "node:fs";
import {
  CHROME, SHOTS, FB_W,
  sleep, check, resetChrome, mountDesk, waitPainted, pageHash, clickFb,
  navigateRemote, TOGGLE_URL,
} from "./e2e-lib.mjs";

fs.mkdirSync(SHOTS, { recursive: true });
await resetChrome();

const browser = await chromium.launch({
  executablePath: CHROME, headless: true, args: ["--no-sandbox", "--disable-gpu"],
});
const page = await browser.newPage({ viewport: { width: 1200, height: 940 } });

// ---- 1) 浏览器看到 Xvfb 完整桌面 ----
await mountDesk(page);
check("打开 SDK 视图看到 Xvfb 完整桌面", await waitPainted(page));

// ---- 2) 地址栏输入 URL + 回车 → 页面加载 ----
const h0 = await pageHash(page);
await navigateRemote(page, TOGGLE_URL);
const h1 = await pageHash(page);
check("地址栏输入 URL + 回车 → 页面加载", h0 !== h1, `${h0.slice(0, 8)}→${h1.slice(0, 8)}`);

// ---- 3) 点击页面(红蓝翻转) ----
const before = await sampleColor(page);
await clickFb(page, 480, 320);
await sleep(800);
const after = await sampleColor(page);
check("点击页面(颜色翻转)", before.r > before.b !== after.r > after.b,
  `(${before.r.toFixed(0)},${before.b.toFixed(0)})→(${after.r.toFixed(0)},${after.b.toFixed(0)})`);

// ---- 4) 拖动窗口标题栏 → 窗口移动 ----
const hd0 = await pageHash(page);
// Chrome 窗口 (80,60) 1100x600:标题栏在 fb (630, 75) 附近(翻转页仍显示标题栏)
await page.evaluate(() => document.querySelector("#mount canvas").focus());
const dragAt = async (x, y) => {
  const b = await page.evaluate(() => {
    const r = document.querySelector("#mount canvas").getBoundingClientRect();
    return { left: r.left, top: r.top, w: r.width, h: r.height };
  });
  const px = b.left + (x / FB_W) * b.w;
  const py = b.top + (y / 720) * b.h;
  await page.mouse.move(px, py);
  await page.mouse.down();
  for (let i = 1; i <= 12; i++) {
    await page.mouse.move(px + (i * 150) / 12, py + (i * 100) / 12);
    await sleep(30);
  }
  await page.mouse.up();
};
await dragAt(630, 75);
await sleep(1500);
const hd1 = await pageHash(page);
check("拖动窗口标题栏 → 窗口移动", hd0 !== hd1, `${hd0.slice(0, 8)}→${hd1.slice(0, 8)}`);

// ---- 5) WebP 压缩模式正常渲染(V2):切 80(服务端整屏重编码)后点击仍正常上画 ----
await page.evaluate(() => window.__h.setQuality({ compression: 80 }));
await sleep(1000);
const wb = await sampleColor(page);
await clickFb(page, 480, 320);
await sleep(800);
const wa = await sampleColor(page);
check("画面压缩 WebP-80 模式正常渲染", wb.r > wb.b !== wa.r > wa.b,
  `(${wb.r.toFixed(0)},${wb.b.toFixed(0)})→(${wa.r.toFixed(0)},${wa.b.toFixed(0)})`);

// ---- 6) 剪贴板 Set→Get 往返(UTF-8) ----
const TEXT = "验收-clipboard-✓";
await page.evaluate((t) => window.__h.setClipboard(t), TEXT);
await sleep(600);
const got = await page.evaluate(() => window.__h.getClipboard());
check("剪贴板 Set→Get 往返(UTF-8)", got === TEXT, `"${got}"`);

await page.screenshot({ path: SHOTS + "/accept.png" });
await browser.close();
console.log(process.exitCode ? "ACCEPT_FAIL" : "ACCEPT_ALL_PASS");

async function sampleColor(page) {
  return page.evaluate(() => {
    const c = document.querySelector("#mount canvas");
    const x = Math.round(c.width * (480 / 1280)), y = Math.round(c.height * (320 / 720));
    const d = c.getContext("2d").getImageData(x, y, 30, 30).data;
    let r = 0, b = 0, n = 0;
    for (let i = 0; i < d.length; i += 4) { r += d[i]; b += d[i + 2]; n++; }
    return { r: r / n, b: b / n };
  });
}
