// 点击→画面延迟 E2E(SDK 驱动)。
// 测量口径与 V1 基线一致:页面内派发指针事件(t0=performance.now()),
// 页面内 2ms 轮询像素直到变色 —— 只测 事件→WS→XTest→捕获→编码→WS→Canvas 管线,
// 不混入 CDP/Node 轮询往返开销(input.ts 对合成事件已做 setPointerCapture 防御)。
import { chromium } from "playwright-core";
import {
  CHROME, FB_W, FB_H,
  resetChrome, mountDesk, waitPainted, navigateRemote, TOGGLE_URL,
} from "./e2e-lib.mjs";

await resetChrome();

const browser = await chromium.launch({
  executablePath: CHROME, headless: true, args: ["--no-sandbox", "--disable-gpu"],
});
const page = await browser.newPage({ viewport: { width: 1200, height: 940 } });
await mountDesk(page);
if (!(await waitPainted(page))) {
  console.log("首帧失败");
  process.exit(1);
}
await navigateRemote(page, TOGGLE_URL);

const latencies = await page.evaluate(
  ({ fx, fy }) => {
    const c = document.querySelector("#mount canvas");
    const ctx = c.getContext("2d");
    const isRed = () => {
      const d = ctx.getImageData(
        Math.max(0, Math.round(c.width * fx) - 40),
        Math.max(0, Math.round(c.height * fy) - 25),
        80, 50,
      ).data;
      let r = 0, b = 0;
      for (let i = 0; i < d.length; i += 4) { r += d[i]; b += d[i + 2]; }
      return r > b;
    };
    const rect = c.getBoundingClientRect();
    const px = rect.left + c.width * fx;
    const py = rect.top + c.height * fy;
    return (async () => {
      const results = [];
      for (let n = 0; n < 10; n++) {
        const before = isRed();
        const t0 = performance.now();
        const ev = (buttons) =>
          new PointerEvent("pointerdown", {
            bubbles: true, clientX: px, clientY: py, button: 0, buttons, pointerId: 1,
          });
        c.dispatchEvent(ev(1)); // down
        c.dispatchEvent(new PointerEvent("pointerup", {
          bubbles: true, clientX: px, clientY: py, button: 0, buttons: 0, pointerId: 1,
        }));
        let dt = -1;
        for (let i = 0; i < 1000; i++) {
          await new Promise((r) => setTimeout(r, 2));
          if (isRed() !== before) { dt = performance.now() - t0; break; }
        }
        results.push(dt);
        await new Promise((r) => setTimeout(r, 400));
      }
      return results;
    })();
  },
  { fx: 480 / FB_W, fy: 320 / FB_H },
);

console.log("click→paint(ms):", latencies.map((v) => v.toFixed(0)).join(", "));
const ok = latencies.filter((v) => v >= 0);
if (ok.length) {
  const sorted = [...ok].sort((a, b) => a - b);
  const avg = ok.reduce((a, b) => a + b, 0) / ok.length;
  console.log(
    `命中 ${ok.length}/10 | 平均 ${avg.toFixed(1)}ms | p50 ${sorted[Math.floor(sorted.length / 2)].toFixed(0)}ms` +
      ` | min ${sorted[0].toFixed(0)}ms | max ${sorted[sorted.length - 1].toFixed(0)}ms`,
  );
  console.log(avg < 50 ? "LATENCY_OK(<50ms)" : "LATENCY_OVER");
  if (avg >= 50) process.exitCode = 1;
} else {
  console.log("无变化");
  process.exitCode = 1;
}
await browser.close();
