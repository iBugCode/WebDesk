// JWT(EdDSA/Ed25519)认证 E2E:
//   独立 agent 实例(127.0.0.1:9533,同 DISPLAY=:100),-k 与 --jwt-public-key 并存。
//   1) 合法 JWT(有 exp / 无 exp)可连接          2) 过期 JWT 拒绝
//   3) 错误公钥签发的 JWT 拒绝                     4) alg=none(即使真签名)拒绝
//   5) 静态 token 仍可用(双门并存)               6) 乱字符串拒绝
//   7) SDK 用 JWT 走完整连接并收到首帧
// 运行:cd scripts && node e2e-jwt.mjs
import { chromium } from "playwright-core";
import crypto from "node:crypto";
import { spawn } from "node:child_process";
import net from "node:net";
import {
  CHROME, ROOT_PACKAGE, SDK_JS,
  startStatic, openHost, stopStatic,
  sleep, check, resetChrome, waitPainted,
} from "./e2e-lib.mjs";

const PORT = 9533;
const STATIC_TOKEN = "static-tok-e2e";

// ---- 签发工具(node crypto 原生 Ed25519,与 agent 的 ed25519-dalek 同算法) ----
const { publicKey, privateKey } = crypto.generateKeyPairSync("ed25519");
// SPKI DER 末 32 字节即裸公钥
const pubHex = publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("hex");
const wrongKey = crypto.generateKeyPairSync("ed25519").privateKey;

const b64u = (s) => Buffer.from(s).toString("base64url");
function jwt(payload, key = privateKey, header = { alg: "EdDSA", typ: "JWT" }) {
  const hb = b64u(JSON.stringify(header));
  const pb = b64u(JSON.stringify(payload));
  const sig = crypto.sign(null, Buffer.from(`${hb}.${pb}`), key);
  return `${hb}.${pb}.${Buffer.from(sig).toString("base64url")}`;
}

const now = () => Math.floor(Date.now() / 1000);

// ---- 启动独立 agent(静态 token + JWT 双门) ----
const bin = new URL("../target/release/webdesk", import.meta.url).pathname;
const agent = spawn(bin, [
  "--listen", `127.0.0.1:${PORT}`,
  "-k", STATIC_TOKEN,
  "--jwt-public-key", pubHex,
], {
  env: { ...process.env, DISPLAY: ":100", WEBDESK_PACKAGE_JSON: ROOT_PACKAGE },
  stdio: ["ignore", "inherit", "inherit"],
});

async function waitPort() {
  for (let i = 0; i < 50; i++) {
    const ok = await new Promise((res) => {
      const c = net.connect(PORT, "127.0.0.1", () => { c.destroy(); res(true); });
      c.on("error", () => res(false));
    });
    if (ok) return true;
    await sleep(100);
  }
  return false;
}
if (!(await waitPort())) {
  console.log("FAIL agent 未在 9533 端口监听");
  process.exit(1);
}

try {
  await resetChrome();
  const browser = await chromium.launch({
    executablePath: CHROME, headless: true, args: ["--no-sandbox", "--disable-gpu"],
  });
  const page = await browser.newPage({ viewport: { width: 1200, height: 940 } });
  await openHost(page);

  // 浏览器内 WS 探测:open = 放行,error/close = 拒绝
  const probe = (token) => page.evaluate(({ url, token }) => new Promise((res) => {
    const ws = new WebSocket(`${url}/ws?token=${encodeURIComponent(token)}`);
    const done = (v) => { try { ws.close(); } catch {} res(v); };
    ws.onopen = () => done("open");
    ws.onerror = () => done("rejected");
    ws.onclose = () => done("rejected");
    setTimeout(() => done("timeout"), 4000);
  }), { url: `ws://127.0.0.1:${PORT}`, token });

  check("合法 JWT(带 exp)可连接", (await probe(jwt({ sub: "u1", exp: now() + 600 }))) === "open");
  check("合法 JWT(无 exp)可连接", (await probe(jwt({ sub: "u2" }))) === "open");
  check("过期 JWT 拒绝", (await probe(jwt({ sub: "u3", exp: now() - 3600 }))) === "rejected");
  check("错误公钥签发的 JWT 拒绝", (await probe(jwt({ sub: "u4", exp: now() + 600 }, wrongKey))) === "rejected");
  check(
    "alg=none 拒绝(即使签名有效)",
    (await probe(jwt({ sub: "u5", exp: now() + 600 }, privateKey, { alg: "none" }))) === "rejected",
  );
  check("静态 token 仍可用(双门并存)", (await probe(STATIC_TOKEN)) === "rejected" ? false : true,
    await probe(STATIC_TOKEN));
  check("乱字符串拒绝", (await probe("garbage-token")) === "rejected");

  // ---- SDK 用 JWT 走完整连接 ----
  await page.addScriptTag({ path: SDK_JS });
  const goodJwt = jwt({ sub: "sdk", exp: now() + 600 });
  await page.evaluate(({ url, token }) => {
    const m = document.createElement("div");
    m.id = "mount";
    m.style.cssText = "width:1100px;height:619px;background:#000;";
    document.body.appendChild(m);
    window.__h = WebDesk.window("#mount", { url, token });
  }, { url: `ws://127.0.0.1:${PORT}`, token: goodJwt });
  const painted = await waitPainted(page);
  check("SDK + JWT 完整连接并收到首帧", painted);

  await browser.close();
} finally {
  agent.kill("SIGTERM");
  stopStatic();
}
console.log(process.exitCode ? "JWT_E2E_FAIL" : "JWT_E2E_ALL_PASS");
