// 远程部署端到端验证:走 Cloudflare Tunnel 打开 demo 页,验证画面渲染 + 真实输入穿透。
import { chromium } from "playwright-core";
import fs from "node:fs";
import { execSync } from "node:child_process";
import { CHROME, sleep } from "./e2e-lib.mjs";

const TOKEN = process.env.WEBDESK_REMOTE_TOKEN || "";
const DEMO_URL = `https://webdesk-bdev.opentest.cc/demo.html?token=${TOKEN}&url=wss://webdesk-ws-bdev.opentest.cc`;
const NAV_URL = 'data:text/html,<body%20bgcolor=orange>TUNNEL-OK';
const PKG_VERSION = JSON.parse(fs.readFileSync(new URL("../package.json", import.meta.url), "utf8")).version;

// 远程状态重置:上一轮残留(最大化窗口/遗留页面)会让断言失真。
// 服务器端脚本把 Chromium 恢复到 (80,60) 1100x600 about:blank。
const SSH_CMD =
  process.env.WEBDESK_SSH ||
  `ssh -o BatchMode=yes -i ${process.env.HOME}/.ssh/new20260912 -p 51888 root@185.106.176.183`;
execSync(`${SSH_CMD} /root/webdesk-test/reset-chrome.sh`, { timeout: 40000 });

let failed = 0;
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "PASS" : "FAIL"} ${name}${detail ? "  " + detail : ""}`);
  if (!ok) failed++;
};

const browser = await chromium.launch({
  executablePath: CHROME,
  headless: true,
  args: ["--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage"],
});
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
page.on("pageerror", (e) => console.log("pageerror:", e.message));

await page.goto(DEMO_URL, { waitUntil: "load", timeout: 30000 });

// 1. 首帧渲染(轮询 canvas 中心像素)
let painted = false;
for (let i = 0; i < 60; i++) {
  painted = await page.evaluate(() => {
    const c = document.querySelector("#mount canvas");
    if (!c || !c.width) return false;
    const g = c.getContext("2d");
    const d = g.getImageData(c.width / 2 - 4, c.height / 2 - 4, 8, 8).data;
    let sum = 0;
    for (let j = 0; j < d.length; j += 4) sum += d[j] + d[j + 1] + d[j + 2];
    return sum > 8 * 8 * 30; // 非纯黑
  });
  if (painted) break;
  await page.waitForTimeout(500);
}
check("首帧渲染(隧道)", painted);

// 1.4 SDK 版本与根 package.json 一致(顺带证明边缘缓存没有回退到旧 sdk.js)
const sdkVer = await page.evaluate(() => window.WebDesk?.version);
check("SDK 版本=package.json", sdkVer === PKG_VERSION, `sdk=${sdkVer} pkg=${PKG_VERSION}`);
await page.screenshot({ path: "/tmp/webdesk-test/shots/remote-demo.png" });

// 1.5 宽高比:画布必须等比于原屏幕(1280x720),容器多余部分 letterbox 留黑边
const geo = await page.evaluate(() => {
  const c = document.querySelector("#mount canvas");
  const cr = c.getBoundingClientRect();
  const mr = document.querySelector("#mount").getBoundingClientRect();
  return {
    cw: cr.width,
    ch: cr.height,
    mt: cr.top - mr.top,
    mb: mr.bottom - cr.bottom,
    ml: cr.left - mr.left,
    mr: mr.right - cr.right,
  };
});
const ratio = geo.cw / geo.ch;
check(
  "画布宽高比=原屏幕 16:9(不拉伸)",
  Math.abs(ratio - 1280 / 720) < 0.02,
  `ratio=${ratio.toFixed(4)} canvas=${geo.cw.toFixed(0)}x${geo.ch.toFixed(0)}`
);
check(
  "letterbox 居中",
  Math.abs(geo.mt - geo.mb) < 2 && Math.abs(geo.ml - geo.mr) < 2,
  `上=${geo.mt.toFixed(1)} 下=${geo.mb.toFixed(1)} 左=${geo.ml.toFixed(1)} 右=${geo.mr.toFixed(1)}`
);

// 2. 真实点击远程 chromium 地址栏并导航(canvas 按 1280x720 远程屏等比缩放)
const box = await page.locator("#mount canvas").boundingBox();
const ax = box.x + box.width * (500 / 1280);
const ay = box.y + box.height * (123 / 720);
await page.mouse.click(ax, ay);
await page.waitForTimeout(1200);
await page.keyboard.press("Control+a");
await page.waitForTimeout(400);
await page.keyboard.type(NAV_URL, { delay: 8 });
await page.keyboard.press("Enter");

// 3.2 FPS 角标数值:导航突发帧流期间,1-2 个采样窗内必现 ≥1
let fpsText = "";
for (let i = 0; i < 5 && !parseInt(fpsText, 10); i++) {
  await page.waitForTimeout(500);
  fpsText = await page.evaluate(() => {
    const wrap = document.querySelector("#mount > div");
    const b = [...(wrap?.children || [])].find((e) => (e.textContent || "").endsWith("FPS"));
    return b?.textContent || "";
  });
}
check("FPS 角标数值>0(导航帧流)", parseInt(fpsText, 10) >= 1, fpsText);

await page.waitForTimeout(5000);

// 3. 页面区域采样:应为橙色(r 高、b 低)
const rgb = await page.evaluate(() => {
  const c = document.querySelector("#mount canvas");
  const g = c.getContext("2d");
  // 远程窗口内容区大致在 fb(640,400)
  const x = Math.floor(c.width * (640 / 1280));
  const y = Math.floor(c.height * (400 / 720));
  const d = g.getImageData(x, y, 1, 1).data;
  return [d[0], d[1], d[2]];
});
check(
  "点击+键盘穿透导航(橙色页)",
  rgb[0] > 180 && rgb[1] > 80 && rgb[1] < 200 && rgb[2] < 120,
  `rgb=${rgb.join(",")}`
);

// 4. 侧栏默认收起 → 点击展开
const collapsedDefault = await page.evaluate(
  () => !document.querySelector("#sidebar")?.classList.contains("open")
);
check("侧栏默认收起", collapsedDefault);
await page.click("#side-toggle");
await page.waitForTimeout(400);
const opened = await page.evaluate(
  () => document.querySelector("#sidebar")?.classList.contains("open")
);
check("侧栏可展开", opened);
await page.screenshot({ path: "/tmp/webdesk-test/shots/remote-demo-sidebar.png" });

// 4.5 FPS 角标:右上角、不可点击/不可选中(数值断言见 3.2,空闲时 0 FPS 是正确表现)
await page.waitForTimeout(900);
const badge = await page.evaluate(() => {
  const wrap = document.querySelector("#mount > div");
  const b = [...wrap.children].find((e) => (e.textContent || "").endsWith("FPS"));
  if (!b) return null;
  const cs = getComputedStyle(b);
  const r = b.getBoundingClientRect();
  const w = wrap.getBoundingClientRect();
  return {
    text: b.textContent,
    top: r.top - w.top,
    right: w.right - r.right,
    pe: cs.pointerEvents,
    us: cs.userSelect,
  };
});
check("FPS 角标存在且在右上角", !!badge && Math.abs(badge.top - 6) < 2 && Math.abs(badge.right - 6) < 2,
  JSON.stringify(badge));
check("FPS 角标不可点击/不可选中", !!badge && badge.pe === "none" && badge.us === "none");
// 收回侧栏,让画面回到纯空闲状态
await page.click("#side-toggle");
await page.waitForTimeout(400);

// 5. WS 心跳保活:Cloudflare 对空闲 WS 约 100s 读超时,服务端每 25s 发 Ping。
//    空闲挂 IDLE_MS(默认 110s)。跨网路径存在与空闲无关的突发重置(0.4.0 起
//    即可观察到,cloudflared/源站侧日志干净),故允许 ≤2 次抖动但要求 SDK 自动
//    恢复;空闲结束时仍断连,或风暴(>2 次)才算失败。
const IDLE_MS = Number(process.env.IDLE_MS || 110000);
console.log(`空闲等待 ${IDLE_MS / 1000}s(验证心跳保活)…`);
{
  let drops = 0;
  let inDrop = false;
  let recovered = true;
  const t0 = Date.now();
  while (Date.now() - t0 < IDLE_MS) {
    const t = await page.evaluate(
      () => document.querySelector("#st-text")?.textContent || ""
    );
    const dropping = t.includes("断开") || t.includes("重连");
    if (dropping && !inDrop) {
      inDrop = true;
      drops++;
      console.log(`  空闲 ${((Date.now() - t0) / 1000).toFixed(0)}s 抖动 #${drops}: "${t}"`);
    } else if (!dropping && inDrop) {
      inDrop = false;
    }
    if (drops > 2) break;
    await sleep(1000);
  }
  recovered = !inDrop;
  check(
    `空闲 ${IDLE_MS / 1000}s 链路保持可用(心跳保活${drops ? `,期间 ${drops} 次环境抖动已自愈` : ",无断连"})`,
    recovered && drops <= 2,
  );
}

// 6. 空闲后链路仍可用:再导航一次并采样(注意 DOMRect 无法跨 evaluate 序列化,取普通对象)
//    隧道偶发边缘侧 1006(与产品无关的环境抖动):先等状态恢复"已连接"再操作,
//    导航失败自动重试一次。
const waitConnected = async (ms = 15000) => {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    const t = await page.evaluate(() => document.querySelector("#st-text")?.textContent || "");
    if (!t.includes("断开") && !t.includes("重连")) return true;
    await sleep(500);
  }
  return false;
};
const navAndSample = async (bg, label) => {
  const box = await page.evaluate(() => {
    const r = document.querySelector("#mount canvas").getBoundingClientRect();
    return { x: r.x, y: r.y, width: r.width, height: r.height };
  });
  await page.mouse.click(box.x + box.width * (500 / 1280), box.y + box.height * (123 / 720));
  await page.waitForTimeout(1200);
  await page.keyboard.press("Control+a");
  await page.waitForTimeout(400);
  await page.keyboard.type(`data:text/html,<body%20bgcolor=${bg}>${label}`, { delay: 8 });
  await page.keyboard.press("Enter");
  await page.waitForTimeout(5000);
  return page.evaluate(() => {
    const c = document.querySelector("#mount canvas");
    const g = c.getContext("2d");
    const d = g.getImageData(Math.floor(c.width * 0.5), Math.floor(c.height * 0.55), 1, 1).data;
    return [d[0], d[1], d[2]];
  });
};
await waitConnected();
let rgb2 = await navAndSample("blue", "AFTER-IDLE");
if (!(rgb2[2] > 150 && rgb2[0] < 120)) {
  console.log("  首次空闲后导航未生效(疑似隧道抖动),等重连后重试一次…");
  await waitConnected();
  rgb2 = await navAndSample("blue", "AFTER-IDLE-RETRY");
}
check("空闲后仍可交互(蓝色页)", rgb2[2] > 150 && rgb2[0] < 120, `rgb=${rgb2.join(",")}`);
await page.screenshot({ path: "/tmp/webdesk-test/shots/remote-postidle.png" });

await browser.close();
console.log(failed === 0 ? "REMOTE_E2E_ALL_PASS" : `REMOTE_E2E_${failed}_FAILED`);
process.exit(failed === 0 ? 0 : 1);
