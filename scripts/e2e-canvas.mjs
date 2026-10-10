// 渲染/空闲流量/输入/滚轮/画质设置 E2E(SDK 驱动)。
import { chromium } from "playwright-core";
import fs from "node:fs";
import {
  CHROME, SHOTS,
  sleep, check, resetChrome, mountDesk, waitPainted, pageHash,
  navigateRemote, SCROLL_URL,
} from "./e2e-lib.mjs";

fs.mkdirSync(SHOTS, { recursive: true });
await resetChrome();

const browser = await chromium.launch({
  executablePath: CHROME, headless: true, args: ["--no-sandbox", "--disable-gpu"],
});
const page = await browser.newPage({ viewport: { width: 1200, height: 940 } });

// ---- 1) 首帧渲染(整屏) ----
await mountDesk(page);
check("首帧渲染(整屏)", await waitPainted(page));

// ---- 2) 空闲流量 ≈ 0(静态桌面无任何帧) ----
await sleep(1500); // 等待拖尾帧结束
const s0 = await page.evaluate(() => window.__h.stats());
await sleep(2500);
const s1 = await page.evaluate(() => window.__h.stats());
const idleBytes = s1.bytes - s0.bytes;
const idleFrames = s1.frames - s0.frames;
check("空闲流量 ≈ 0(不变化就不发送)", idleBytes < 2048 && idleFrames <= 3,
  `2.5s 内 ${idleBytes}B / ${idleFrames} 帧`);

// ---- 3) 键盘注入 → 远端画面变化 ----
const hash0 = await pageHash(page);
await navigateRemote(page, SCROLL_URL); // 地址栏输入本身就是键盘注入
const hash1 = await pageHash(page);
check("键盘注入 → 远端画面变化", hash0 !== hash1, `${hash0.slice(0, 8)} → ${hash1.slice(0, 8)}`);

// ---- 4) 滚轮注入 → 页面滚动 ----
const h2 = await pageHash(page);
await page.evaluate(() => document.querySelector("#mount canvas").focus());
// 真实滚轮:指针放到画布中上部,向下滚 5 格
const rect = await page.evaluate(() => {
  const r = document.querySelector("#mount canvas").getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 3 };
});
await page.mouse.move(rect.x, rect.y);
for (let i = 0; i < 5; i++) {
  await page.mouse.wheel(0, 120);
  await sleep(150);
}
await sleep(1500);
const h3 = await pageHash(page);
check("滚轮注入 → 页面滚动", h2 !== h3, `${h2.slice(0, 8)} → ${h3.slice(0, 8)}`);

// ---- 5) 会话累计流量显著低于持续整屏(增量更新生效) ----
const total = await page.evaluate(() => window.__h.stats());
check("增量更新:累计流量远低于持续整屏", total.bytes < 6 * 1024 * 1024,
  `累计 ${(total.bytes / 1024).toFixed(0)}KB / ${total.frames} 帧`);

// ---- 6) 画质设置下发(50%)→ 服务端推全屏帧 ----
const f0 = (await page.evaluate(() => window.__h.stats())).frames;
await page.evaluate(() => window.__h.setQuality({ scale: 50 }));
await sleep(2500);
const f1 = (await page.evaluate(() => window.__h.stats())).frames;
check("画面质量 50% 设置下发成功", f1 > f0, `frames ${f0}→${f1}`);

await page.screenshot({ path: SHOTS + "/canvas.png" });
await browser.close();
console.log(process.exitCode ? "CANVAS_FAIL" : "CANVAS_ALL_PASS");
