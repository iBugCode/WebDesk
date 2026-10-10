// WebDesk SDK 端到端验证:
//   1) sdk.js 加载为全局 WebDesk,版本号 = 项目根 package.json(构建时注入)
//   2) 双视图共享一路会话,渲染一致
//   3) 真实鼠标/键盘经 SDK 画布操作远端 Chrome(地址栏导航 + 颜色翻转)
//   4) SDK 能力:stats / setQuality / screenshot(原始分辨率) / set+getClipboard
//   5) destroy() 摘除视图、销毁后可重挂
import { chromium } from "playwright-core";
import fs from "node:fs";
import {
  TOKEN, WS_URL, SDK_JS, CHROME, ROOT_PACKAGE, SHOTS,
  startStatic, openHost, stopStatic,
  sleep, check, resetChrome, mountDesk, waitPainted, sample, clickFb,
  navigateRemote, TOGGLE_URL, FB_W, FB_H,
} from "./e2e-lib.mjs";

fs.mkdirSync(SHOTS, { recursive: true });
await resetChrome();

const browser = await chromium.launch({
  executablePath: CHROME, headless: true, args: ["--no-sandbox", "--disable-gpu"],
});
const page = await browser.newPage({ viewport: { width: 1200, height: 940 } });

// ---- 1) SDK 全局与版本号(构建时从 package.json 注入) ----
await openHost(page);
await page.addScriptTag({ path: SDK_JS });
const sdkVer = await page.evaluate(() => window.WebDesk && WebDesk.version);
const pkgVer = JSON.parse(fs.readFileSync(ROOT_PACKAGE, "utf8")).version;
check("SDK 全局加载 + 版本号来自 package.json", sdkVer === pkgVer, `sdk=${sdkVer} pkg=${pkgVer}`);

// 缺 token 必须同步报错(不会创建半初始化的连接)
const noTokenMsg = await page.evaluate((url) => {
  const m = document.createElement("div");
  m.id = "no-token";
  document.body.appendChild(m);
  try {
    WebDesk.window("#no-token", { url });
    return "";
  } catch (e) {
    return String(e.message || e);
  }
}, WS_URL);
check("缺 token 有明确报错", noTokenMsg.includes("缺少 token"), `msg=${noTokenMsg.slice(0, 40)}`);
await page.evaluate(() => document.getElementById("no-token").remove());

// ---- 2) 双视图共享会话,渲染一致 ----
await mountDesk(page); // 视图1
await page.evaluate(
  ({ url, token }) => {
    const m = document.createElement("div");
    m.id = "mount2";
    m.style.cssText = "width:400px;height:225px;background:#000;margin-top:12px;";
    document.body.appendChild(m);
    window.__h2 = WebDesk.window("#mount2", { url, token });
  },
  { url: WS_URL, token: TOKEN },
);
check("首帧渲染(视图1)", await waitPainted(page));
await sleep(1500);
const s1 = await sample(page, 0.5, 0.5);
check("视图2 出画且与视图1 一致", s1.n > 0, `视图1 中心=(${s1.r.toFixed(0)},${s1.g.toFixed(0)},${s1.b.toFixed(0)})`);
const stats0 = await page.evaluate(() => ({ v1: window.__h.stats(), v2: window.__h2.stats() }));
check("stats() 计数(共享连接同源)", stats0.v1.bytes === stats0.v2.bytes && stats0.v1.frames === stats0.v2.frames && stats0.v1.frames > 0,
  `bytes=${stats0.v1.bytes} frames=${stats0.v1.frames}`);

// ---- 3) 真实键鼠穿透:地址栏导航到翻转页并点击 ----
await navigateRemote(page, TOGGLE_URL);
const before = await sample(page, 480 / FB_W, 320 / FB_H);
await clickFb(page, 480, 320);
await sleep(800);
const after = await sample(page, 480 / FB_W, 320 / FB_H);
const flipped = (a, b) => a.r > a.b !== b.r > b.b;
check("SDK 画布真实点击穿透(颜色翻转)", flipped(before, after),
  `before=(${before.r.toFixed(0)},${before.g.toFixed(0)},${before.b.toFixed(0)}) after=(${after.r.toFixed(0)},${after.g.toFixed(0)},${after.b.toFixed(0)})`);

// ---- 4) SDK 能力 ----
// screenshot:原始分辨率 Blob
const shot = await page.evaluate(async () => {
  const blob = await window.__h.screenshot();
  const bmp = await createImageBitmap(blob);
  const size = window.__h.screenSize();
  return { w: bmp.width, h: bmp.height, bytes: blob.size, size };
});
check("screenshot() 原始分辨率", shot.w === FB_W && shot.h === FB_H && shot.bytes > 1000,
  `${shot.w}x${shot.h} ${shot.bytes}B screenSize=${shot.size ? shot.size.w + "x" + shot.size.h : "null"}`);

// setQuality:50% 画质后服务端推全屏,帧数增长
const f0 = (await page.evaluate(() => window.__h.stats())).frames;
await page.evaluate(() => window.__h.setQuality({ scale: 50 }));
await sleep(2000);
const f1 = (await page.evaluate(() => window.__h.stats())).frames;
check("setQuality() 下发生效(收到新帧)", f1 > f0, `frames ${f0}→${f1}`);
await page.evaluate(() => window.__h.setQuality({ scale: 100 }));
await sleep(1000);

// 剪贴板:set → get 往返
const CLIP_TEXT = "sdk-e2e-剪贴板-✓-0.3.0";
await page.evaluate((t) => window.__h.setClipboard(t), CLIP_TEXT);
await sleep(600);
const got = await page.evaluate(() => window.__h.getClipboard());
check("setClipboard → getClipboard 往返", got === CLIP_TEXT, `got="${got.slice(0, 24)}…"`);

// ---- 5) destroy / 重挂 ----
await page.evaluate(() => window.__h2.destroy());
const gone = await page.evaluate(() => document.querySelectorAll("#mount2 canvas").length);
check("destroy() 摘除视图", gone === 0, `剩余 canvas=${gone}`);
await page.evaluate(
  ({ url, token }) => {
    window.__h2 = WebDesk.window("#mount2", { url, token });
  },
  { url: WS_URL, token: TOKEN },
);
await sleep(400);
const back = await page.evaluate(() => document.querySelectorAll("#mount2 canvas").length);
check("销毁后可重新挂载(共享连接保持)", back === 1, `canvas=${back}`);
await page.evaluate(() => window.__h2.destroy());
await page.screenshot({ path: SHOTS + "/sdk-demo.png" });

// ---- 6) demo.html 冒烟:noVNC 风格侧栏默认收起 + 自动连接出画 ----
const port = await startStatic();
const demo = await browser.newPage({ viewport: { width: 1200, height: 800 } });
await demo.goto(
  `http://127.0.0.1:${port}/demo.html?url=${encodeURIComponent(WS_URL)}&token=${encodeURIComponent(TOKEN)}`,
);
await sleep(3500);
const demoState = await demo.evaluate(() => {
  const c = document.querySelector("#mount canvas");
  let painted = false;
  if (c && c.width > 300) {
    const d = c.getContext("2d").getImageData(c.width >> 1, c.height >> 1, 20, 20).data;
    for (let i = 0; i < d.length; i += 4) {
      if (d[i] + d[i + 1] + d[i + 2] > 30) { painted = true; break; }
    }
  }
  return {
    sidebarOpen: document.getElementById("sidebar").classList.contains("open"),
    toggleVisible: !!document.getElementById("side-toggle"),
    canvases: document.querySelectorAll("#mount canvas").length,
    painted,
  };
});
check("demo 页:左侧设置栏默认收起", !demoState.sidebarOpen && demoState.toggleVisible,
  `open=${demoState.sidebarOpen}`);
check("demo 页:自动连接并出画", demoState.canvases === 1 && demoState.painted,
  `canvas=${demoState.canvases} painted=${demoState.painted}`);
// 侧栏展开动作可用
await demo.click("#side-toggle");
await sleep(400);
const opened = await demo.evaluate(() => document.getElementById("sidebar").classList.contains("open"));
check("demo 页:点击左缘按钮可展开设置栏", opened);
await demo.screenshot({ path: SHOTS + "/demo-open.png" });

await browser.close();
stopStatic();
console.log(process.exitCode ? "SDK_FAIL" : "SDK_OK");
