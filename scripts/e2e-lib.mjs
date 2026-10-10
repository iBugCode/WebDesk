// E2E 共用:注入 dist/sdk.js,以 SDK API 驱动(agent 只提供 WS,无页面可加载)。
import fs from "node:fs";
import http from "node:http";
import { execSync } from "node:child_process";

export const TOKEN =
  process.env.WEBDESK_TOKEN || fs.readFileSync("/tmp/webdesk-token", "utf8").trim();
export const WS_URL = "ws://127.0.0.1:9527";
export const SDK_JS = new URL("../web/dist/sdk.js", import.meta.url).pathname;
export const DEMO_HTML = new URL("../web/demo.html", import.meta.url).pathname;
export const CHROME =
  "/home/bigcode/data/dev/Rust/WebDesk/.testenv/chrome/opt/google/chrome/chrome";
export const ROOT_PACKAGE = new URL("../package.json", import.meta.url).pathname;
export const SHOTS = "/tmp/webdesk-test/shots";

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export const check = (name, ok, extra = "") => {
  console.log(`${ok ? "PASS" : "FAIL"} ${name}${extra ? " | " + extra : ""}`);
  if (!ok) process.exitCode = 1;
};

// 远端屏幕 fb 尺寸(HELLO 上报,Xvfb 1280x720)
export const FB_W = 1280;
export const FB_H = 720;

/** 重置远端 Chrome 到已知几何位置(窗口 80,60 1100x600,about:blank) */
export function resetChrome() {
  execSync('pkill -f "testenv/chrome/opt/google/chrome/[c]hrome" || true');
  execSync("rm -f /tmp/webdesk-chrome-profile/Singleton*");
  execSync(
    `DISPLAY=:100 nohup ${CHROME} --no-sandbox --disable-gpu --disable-dev-shm-usage` +
      " --user-data-dir=/tmp/webdesk-chrome-profile --no-first-run" +
      ' --window-position=80,60 --window-size=1100,600 about:blank >/dev/null 2>&1 &',
  );
  return sleep(4500);
}

// 内嵌静态服务器:为 E2E 提供 localhost 宿主页(about:blank 等 opaque origin
// 会被新版 Chrome 的 Local Network Access 拦掉到 127.0.0.1 的 WebSocket)。
let staticSrv = null;
let staticPort = 0;

export async function startStatic() {
  if (staticSrv) return staticPort;
  staticSrv = http.createServer((req, res) => {
    // 只看 path,忽略 ?v= 等查询串(demo.html 用版本参数引用 sdk.js)
    const path = new URL(req.url || "/", "http://x").pathname;
    if (path.endsWith("/sdk.js")) {
      try {
        res.writeHead(200, { "content-type": "application/javascript" }).end(fs.readFileSync(SDK_JS));
      } catch {
        res.writeHead(404).end("404");
      }
    } else if (path.startsWith("/demo.html")) {
      try {
        res.writeHead(200, { "content-type": "text/html; charset=utf-8" }).end(fs.readFileSync(DEMO_HTML));
      } catch {
        res.writeHead(404).end("404");
      }
    } else {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" }).end("<!doctype html><title>e2e-host</title><body></body>");
    }
  });
  await new Promise((r) => staticSrv.listen(0, "127.0.0.1", r));
  staticPort = staticSrv.address().port;
  staticSrv.unref();
  return staticPort;
}

export function stopStatic() {
  if (staticSrv) {
    staticSrv.close();
    staticSrv = null;
  }
}

/** 打开 E2E 宿主页(静态服务器上的空白页,同源连 agent 不触发 LNA) */
export async function openHost(page) {
  const port = await startStatic();
  await page.goto(`http://127.0.0.1:${port}/e2e-host`);
}

/** agent 404 页面上注入 SDK 并挂载视图(真实浏览器事件从该画布驱动远端) */
export async function mountDesk(page, { width = 1100, height = 619, quality = {} } = {}) {
  await openHost(page);
  await page.addScriptTag({ path: SDK_JS });
  await page.evaluate(
    ({ width, height, quality, url, token }) => {
      document.body.style.margin = "0";
      const m = document.createElement("div");
      m.id = "mount";
      m.style.cssText = `width:${width}px;height:${height}px;background:#000;`;
      document.body.appendChild(m);
      window.__h = WebDesk.window("#mount", { url, token, ...quality });
    },
    { width, height, quality, url: WS_URL, token: TOKEN },
  );
}

/** 等待首帧上画(中心非黑) */
export async function waitPainted(page, timeoutMs = 12000) {
  return page
    .waitForFunction(
      () => {
        const c = document.querySelector("#mount canvas");
        if (!c || !c.width) return false;
        const d = c.getContext("2d").getImageData(c.width >> 1, c.height >> 1, 40, 40).data;
        for (let i = 0; i < d.length; i += 4) {
          if (d[i] + d[i + 1] + d[i + 2] > 30) return true;
        }
        return false;
      },
      { timeout: timeoutMs, polling: 250 },
    )
    .then(() => true)
    .catch(() => false);
}

/** 采样视图画布上 (fx,fy)(画面比例坐标)处 size×size 的平均颜色 */
export async function sample(page, fx, fy, size = 30) {
  return page.evaluate(
    ([fx, fy, size]) => {
      const c = document.querySelector("#mount canvas");
      if (!c) return { r: -1, g: -1, b: -1, n: 0 };
      const x = Math.max(0, Math.min(c.width - size, Math.round(c.width * fx) - (size >> 1)));
      const y = Math.max(0, Math.min(c.height - size, Math.round(c.height * fy) - (size >> 1)));
      const d = c.getContext("2d").getImageData(x, y, size, size).data;
      let r = 0, g = 0, b = 0, n = 0;
      for (let i = 0; i < d.length; i += 4) {
        r += d[i]; g += d[i + 1]; b += d[i + 2]; n++;
      }
      return { r: r / n, g: g / n, b: b / n, n };
    },
    [fx, fy, size],
  );
}

/** 16x12 网格亮度的简易哈希(检测画面变化) */
export async function pageHash(page) {
  return page.evaluate(() => {
    const c = document.querySelector("#mount canvas");
    let h = "";
    if (!c) return "nocanvas";
    const ctx = c.getContext("2d");
    for (let gy = 0; gy < 12; gy++) {
      for (let gx = 0; gx < 16; gx++) {
        const d = ctx.getImageData(
          Math.round(((gx + 0.5) / 16) * c.width),
          Math.round(((gy + 0.5) / 12) * c.height),
          1, 1,
        ).data;
        h += Math.floor((d[0] + d[1] + d[2] / 2) / 32).toString(16);
      }
    }
    return h;
  });
}

/** 在视图画布上按 fb 坐标真实点击(自动映射 CSS 比例;点击使画布获得键盘焦点) */
export async function clickFb(page, x, y, button = "left") {
  const b = await page.evaluate(() => {
    const r = document.querySelector("#mount canvas").getBoundingClientRect();
    return { left: r.left, top: r.top, w: r.width, h: r.height };
  });
  await page.mouse.click(b.left + (x / FB_W) * b.w, b.top + (y / FB_H) * b.h, { button });
}

/** 地址栏导航:真实点击地址栏 → Ctrl+A 全选 → 键盘输入 URL → 回车 */
export async function navigateRemote(page, urlB64) {
  await clickFb(page, 500, 123); // Chrome 窗口 (80,60):地址栏中心
  await sleep(800);
  await page.keyboard.down("Control");
  await page.keyboard.press("a");
  await page.keyboard.up("Control");
  await sleep(300);
  await page.keyboard.type(urlB64, { delay: 6 });
  await page.keyboard.press("Enter");
  await sleep(4000);
}

/** 全屏红蓝翻转页(点击任意处切换) */
export const TOGGLE_URL =
  "data:text/html;base64," +
  Buffer.from(
    `<html><body style="margin:0"><div id="b" style="position:fixed;inset:0;background:#c81e1e" onclick="b._t=!b._t;b.style.background=b._t?'#1e1ec8':'#c81e1e'"></div></body></html>`,
  ).toString("base64");

/** 可滚动长页(滚轮测试) */
export const SCROLL_URL =
  "data:text/html;base64," +
  Buffer.from(
    `<body style="margin:0;background:#00aa33"><h1 style="font-size:80px" id=t>SCROLLTEST</h1><div style="height:2500px;background:linear-gradient(#f0f,#ff0)"></div><h1 id=b style="position:absolute;top:2600px;font-size:80px">BOTTOM</h1></body>`,
  ).toString("base64");
