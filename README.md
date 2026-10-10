<div align="center">
  <img src="assets/logo.png" alt="WebDesk logo" width="180" height="180" />
  <h1>WebDesk</h1>
  <p><strong>A web-first remote desktop: X11 damage-driven streaming to a browser tab over WebSocket — the browser <em>is</em> the client.</strong></p>
  <p>
    <a href="#quick-start"><img alt="Quick Start" src="https://img.shields.io/badge/install-Quick%20Start-3b82f6" /></a>
    <a href="#license"><img alt="License MIT" src="https://img.shields.io/badge/license-MIT-green" /></a>
    <img alt="Language" src="https://img.shields.io/badge/Rust-1.75%2B-orange?logo=rust" />
    <img alt="Runtime" src="https://img.shields.io/badge/deps-zero%20runtime-blue" />
    <a href="README.zh-CN.md"><img alt="中文文档" src="https://img.shields.io/badge/docs-%E7%AE%80%E4%BD%93%E4%B8%AD%E6%96%87-red" /></a>
  </p>
</div>

# WebDesk

Web-first remote desktop: run a desktop on Linux + Xvfb, and operate it directly from any browser tab — no client installation, no port forwarding, nothing to download. Frames are captured with X11 damage events, encoded as dirty rectangles (WebP / zstd), and composited on a Canvas by a single-file JavaScript SDK: idle bandwidth is ~0, interaction latency is <= 50 ms, and the whole agent is a ~6 MB static Rust binary.

English | [简体中文](README.zh-CN.md) | [Quick Guide / 使用说明](USAGE.md)

## What it's for

WebDesk solves the "give someone a browser link, they get a desktop" problem. It is not a general-purpose remote-control tool — it is the fastest path from *a Linux box with an X display* to *an interactive window inside another web page*.

Typical scenarios:

- **Browser-as-a-remote-browser / cloud desktop for web work**: host Chrome (or Chrome-based browsers) inside Xvfb on your server, open it through a tunnel URL, and use it from any device with a browser — including machines you can't install anything on.
- **Embedding a remote browser in your own product**: one line, `WebDesk.window("#mount", {url, token})`, mounts a live remote desktop into any page — ideal for SaaS console previews, QA sandboxes, automation dashboards, or "watch the automation run" panels.
- **Fingerprint / anti-detect browser farms**: pair with [veilbrowser](https://github.com/iBugCode/veilbrowser) — run veil-chromium profiles headlessly on farm servers and inspect/drive them remotely through WebDesk instead of VNC.
- **Low-bandwidth remote access**: over hotel Wi-Fi, 4G/LTE, or intercontinental links where VNC/RDP feeds stall, WebDesk keeps interactive because it *never sends what didn't change*.
- **Automation observability**: attach a browser tab to an agent-driven desktop (e.g. Playwright / CDP automation on Xvfb) and watch the pixels with near-zero impact on the session itself.

## Why not just use VNC or RDP?

VNC and RDP are battle-tested full-desktop protocols. They are also 20-plus-year-old designs that continuously push pixels and assume a native client. WebDesk rethinks both assumptions for the browser era:

| Dimension | VNC | RDP | WebDesk |
|---|---|---|---|
| Client | Native client per platform (or heavyweight Java/HTML re-implementations) | Native / thin client, gateway stack | **Any browser tab** — a ~50 KB single-file SDK |
| Idle bandwidth | Constant full-screen (or tiled) refresh polling | Low but requires RDP gateway/channel infra | **~0 B — literally zero frames while static** (damage-driven) |
| Content model | Sends everything, letting the server decide | Heavily tuned for Windows + Office workloads | Sends *only dirty rectangles*; scrolling is sent as a block-copy pointer |
| FPS model | Fixed refresh loop (typically 10–60 fps regardless of change) | Adaptive per channel | **Two-state** (interactive cap / idle cap), decided by the client SDK per connected viewer |
| Resolution handling | Captures at full res everywhere | Compressed at full res | Server-side downscale (10–100%) + lossless zstd / lossy WebP per rect |
| Deployment | Server daemon + per-OS clients + port rules | Gateway/licensing complexity | Static ~6 MB binary + any reverse proxy; the browser does the rest |
| Auth | VNC password (weak by default), or SSH tunnel | Kerberos/NLA ecosystems | Per-connection token **or** Ed25519-JWT, validated at WS upgrade |
| Session ownership | Shared/mirrored | Multi-session | Single owner with takeover (deterministic) |

**Where WebDesk wins**

- **Low-bandwidth interactive smoothness.** Three compounding policies: (1) idle cost is exactly zero — a static desktop sends nothing, not "low bitrate frames"; (2) small edits send small rects, so a caret blink costs a few hundred bytes while a VNC server still ships full-frame inventory; (3) scroll is a `COPY` pointer, not pixels. On thin links the user still gets full-frame-rate *during* interaction, because the frame budget is spent only where something actually changed.
- **Zero client install.** If it opens a webpage, it can drive the desktop. That collapses the last step of any remote workflow (onboarding a client machine) to zero.
- **Embeddable.** The desktop is a DOM widget with a capability API (screenshots, clipboard, per-view quality), not a fullscreen app — you can compose it inside dashboards, wizards, and multi-pane consoles.
- **Tunnel-friendly.** Random idle-WS heartbeat, WSS behind Cloudflare/nginx, auth at upgrade time — designed from day one to live behind a reverse proxy on a public URL.

**Honest trade-offs / where VNC-RDP still wins**

- **No video/video-conferencing yet.** Rect-based WebP encoding is wrong for 30 fps H.264 content; RDP with AVC / H.264 hardware pipelines still streams video better.
- **X11 only (Windows/macOS agents don't exist yet)**, and RDP owns the polished Windows session story.
- **No file transfer / drive mapping / printer passthrough** — clipboard is UTF-8 text only in V1.
- **No built-in audio.**
- **Non-Latin IME input isn't covered yet** (core keysyms only).
- **Single session**: a second connection takes over rather than collaborating; there's no viewer-multiplex crowd-control mode.
- Single-administration design: DISPLAY comes from argv/env; WebDesk deliberately doesn't ship an account/database or multi-user stacking later.

## Architecture

```
                    WebDesk Agent (Rust, WebSocket only)
                          |
                    X11 Connection
          +---------------+---------------+
          |                               |
       XDamage                         XTest input injection
          |                               |
          v                               v
    Dirty Regions (merge)            Mouse/Keyboard
          |
          v
    Region Capture (XShm / GetImage)
          |
          v
    Update Scheduler (event-driven + dynamic FPS)
          |
       +--+---+
       |      |
     COPY   RECT (scale → raw/zstd/webp)
       +--+---+
          |
       WebSocket (Axum, /ws only)
          |
          v
       Browser (Canvas 2D compositing; page served by any static host)
```

Core principles:

- **Never send when nothing changed** — idle bandwidth ≈ 0
- **Small changes send small regions** (XDamage dirty rects + area-bounded merging)
- **COPY instead of re-sending pixels** when content scrolls
- **Input takes priority over frames**
- **The browser does the final rendering** (canvas is the framebuffer)

Damage robustness: XDamage can under-report region edges by 1–2 px (window hide,
anti-aliased edges, sub-pixel layout). Every dirty rect is inflated by 2 px before
merge/capture, so stale 1-px borders are prevented without ever breaking the
zero-idle-bandwidth principle. The COPY scroll optimization is deliberately
conservative: it fires only after dense verification confirms the rect really is a pure translation and the exposed strip really is new content;
anything else — a progress-bar fill, a caret blink — falls back to re-encoding
the whole rect, so a false COPY can never shift moving content on screen.

Encoding: dirty regions are WebP-lossy encoded (the "compression" setting, 1–100) or zstd-raw lossless (0) after optional
server-side quality scaling ("quality" setting, 10–100%); the browser decodes WebP
natively on Canvas.

## Quick Start

Requirements: Rust 1.75+, Node 18+, Xvfb, python3 (serves the demo page). A window
manager and Chrome can come from the system, or be unpacked under `.testenv/` (below).

```bash
# 1. Build agent + SDK
cargo build --release
(cd web && npm install && npm run build)   # produces web/dist/sdk.js

# 2. One-shot start (Xvfb :100 + openbox + Chrome + agent + demo static server)
scripts/dev.sh

# Or manually:
Xvfb :100 -screen 0 1280x720x24 &
openbox &                                    # any lightweight WM
(cd web && python3 -m http.server 8090 &)    # serves demo.html + sdk.js
DISPLAY=:100 ./target/release/webdesk --listen 127.0.0.1:9527 -k <token>
```

> The agent serves WebSocket only (`/ws`), never HTML/JS. The demo page lives at
> `http://127.0.0.1:8090/demo.html?url=ws://127.0.0.1:9527&token=<token>`.

### Static build (zero runtime deps)

x11rb is a pure-Rust X11 client (no libX11 linkage); combined with musl you get a
single binary that runs on any x86_64 Linux (the only C code is zstd and libwebp,
compiled with musl gcc inside a container):

```bash
scripts/build-static.sh        # output: dist-static/webdesk-linux-static (ldd: statically linked, ~6MB)
```

Upload to any Linux server with an X server (e.g. Xvfb) and run:

```bash
scp dist-static/webdesk-linux-static srv:/root/webdesk/
DISPLAY=:99 ./webdesk-linux-static --listen 0.0.0.0:9527 -k <token>
# Version is read from package.json next to the binary (or set WEBDESK_PACKAGE_JSON)
```

For public deployments put the WS endpoint behind a reverse proxy:

```bash
# e.g. Cloudflare Tunnel
cloudflared tunnel route ...   # route -> wss://your-host
# e.g. nginx: proxy_pass http://127.0.0.1:9527 with Upgrade headers
```

### Rootless test env (.testenv)

On machines without sudo, unpack Chrome/openbox into `.testenv/`; `scripts/dev.sh`
picks it up automatically:

```bash
mkdir -p .testenv && cd .testenv
curl -sL -o chrome.deb https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb
dpkg-deb -x chrome.deb chrome/
mkdir debs && (cd debs && apt-get download openbox libobt2v5 libobrender32v5 libimlib2t64 \
  && for d in *.deb; do dpkg-deb -x "$d" ../openbox/; done)
```

## Authentication

Every WebSocket connection must present a credential on `/ws?token=<credential>`.
Two independent doors; presenting **either** one authorizes the connection:

| Door | How to enable | Who issues credentials |
|---|---|---|
| Static token | `-k <token>` (a random one is generated and printed if omitted) | You, at agent start |
| JWT (EdDSA / Ed25519) | `--jwt-public-key <key>` | Your backend, signing JWTs with the matching private key |

JWT details:

- Only `alg: "EdDSA"` is accepted — `none` and every other algorithm are rejected
  even if the signature is valid. The signature is verified *before* claims parsing.
- `exp` / `nbf` are enforced when present (30 s clock-skew leeway). Tokens without
  them are treated as long-lived.
- The payload is not interpreted — any validly-signed token grants access. Give
  tokens a short `exp` (e.g. ≤ 10 min) and have clients refresh; note the SDK keeps
  reconnecting with the same token, so an expired token means the page needs a new one.
- Generate a keypair with the agent itself:

```bash
webdesk --generate-jwt-keypair
# -----BEGIN PRIVATE KEY----- ... (PKCS#8, keep safe)
# -----BEGIN PUBLIC KEY------ ... (SPKI)
# public key hex for --jwt-public-key: e3a9...
```

The PEM output is fully interoperable with `openssl genpkey -algorithm ed25519` /
`openssl pkey -pubout`. `--jwt-public-key` accepts: 64-char hex, 32-byte base64,
a PEM SPKI public key, or a path to a file containing any of these.

Sign a JWT from your backend (Node.js example):

```js
import crypto from "node:crypto";
const { privateKey } = crypto.generateKeyPairSync("ed25519"); // keep the matching pub key on the agent
const b64u = (s) => Buffer.from(s).toString("base64url");
const hb = b64u(JSON.stringify({ alg: "EdDSA", typ: "JWT" }));
const pb = b64u(JSON.stringify({ sub: "user1", exp: Math.floor(Date.now() / 1000) + 600 }));
const sig = crypto.sign(null, Buffer.from(`${hb}.${pb}`), privateKey);
const jwt = `${hb}.${pb}.${Buffer.from(sig).toString("base64url")}`;
```

The SDK and demo page need no changes — pass the JWT wherever you passed the token.

## CLI Parameters

| Flag | Default | Description |
|---|---|---|
| `--listen` | `127.0.0.1:9527` | WS listen address (localhost-only by default, security boundary) |
| `-k, --token` | random | Static session token; every WS connection must carry a valid credential |
| `--jwt-public-key` | – | Ed25519 public key for JWT auth (see Authentication) |
| `--generate-jwt-keypair` | – | Generate an Ed25519 keypair and exit |
| `--display` | `$DISPLAY` | X11 DISPLAY (never client-controlled) |
| `--max-fps` | `20` | Initial FPS cap; from 0.8.0 the client's SETTINGS fps byte takes over at runtime |
| `--idle-fps` | `1` | [legacy] no-viewer fallback cap only (last client left); the idle/interactive modes moved to the client |
| `--idle-timeout-ms` | `3000` | [legacy] unused since 0.8.0 — the idle countdown lives in the SDK (`idleTimeout` option, 3 s) |
| `--batch-ms` | `12` | Damage batch/merge window |
| `--scale-pct` | `100` | Initial quality (transmitted resolution percentage) |
| `--compression` | `70` | Initial compression: 0 = zstd lossless, 1–100 = WebP lossy quality |

## SDK (embed in any page with one line)

The agent serves WS only; the SDK is a standalone single file (`web/dist/sdk.js`,
IIFE global `WebDesk`) you host on any static server:

```html
<div id="mount" style="width:880px;height:495px"></div>
<script src="https://your-static-host/dist/sdk.js"></script>
<script>
  const h = WebDesk.window("#mount", {
    url: "ws://127.0.0.1:9527",   // defaults to page host :9527
    token: "<credential>",        // static token or JWT; defaults to page ?token=
  });
</script>
```

Capability API:

| API | Description |
|---|---|
| `WebDesk.window(selector, opts)` | Mount a remote desktop; opts: `url/token/scale/compression/idleFps/maxFps/idleTimeout/onStatus/onClipboard` (`jpeg` accepted as the pre-0.6.0 alias of `compression`) |
| `h.setQuality({scale,compression,idleFps,maxFps})` | Quality settings, effective immediately |
| `h.setClipboard(text)` / `h.getClipboard()` | Write/read the remote clipboard (X11 CLIPBOARD) |
| `h.onClipboard` (opts) | Push callback for remote clipboard changes |
| `h.screenshot({format,quality})` | Full-resolution screenshot (PNG/JPEG) |
| `h.stats()` / `h.screenSize()` | Cumulative traffic/frames; remote screen size |
| `h.destroy()` | Remove the view; closing the last view closes the connection |

- Mount multiple views on one page — they share a single session; frames decode
  once, each view scales locally
- Each view has a live FPS badge in its top-right corner (measured per second;
  display-only HTML, never blocks canvas input)
- Canvas aspect ratio always matches the remote screen (contain letterbox — no
  stretching at any container size)
- Keyboard forwards only while the view has focus (never hijacks the host page);
  modifiers auto-release on blur; auto-reconnect after 3 s
- `WebDesk.version` is injected by esbuild from the root package.json at build time
- Note: newer Chrome's Local Network Access blocks WS to localhost from opaque
  origins (`about:blank`, `file://`); host the page over http(s)

## Demo Page (noVNC style)

`web/demo.html`: settings panel on the **left edge, collapsed by default** (click
the ☰ button to expand), containing:

> Release note: demo.html references `dist/sdk.js?v=<version>` (the HTML isn't CDN
> cached but the .js is; the version query busts the cache). Update it in sync with
> package.json on every release.

- Connection: url / token inputs
- Picture: quality / compression (zstd lossless or WebP lossy) / min & max FPS (instant)
- Clipboard: read/write remote
- Screenshot: one-click full-resolution PNG
- Status: connection state + cumulative traffic/frames

```bash
cd web && python3 -m http.server 8090
# open http://127.0.0.1:8090/demo.html?url=ws://127.0.0.1:9527&token=<token>
```

## Dynamic FPS

Frame rate never depends on how much of the screen changed — only on whether anyone is interacting:

| State | Frame rate |
|---|---|
| Screen static | 0 (nothing sent) |
| Client reports idle cap | ≤ idle FPS (default 1) |
| Client reports interactive cap | ≤ max FPS (default 20) |

The **FPS policy lives entirely in the SDK** (since 0.8.0): the agent has no
"idle/interactive" modes and does zero input detection — it executes one
number, the current FPS cap carried by `SETTINGS`. The SDK owns the policy:
any input over the canvas (move / click / wheel / keys, plus a
held button or key) switches the reported cap to `maxFps`; after the idle countdown (default 3 s, `idleTimeout`
option) with no input and nothing held, it reports `idleFps`. In other words:
**Down goes high immediately, Up starts the 3 s countdown.** XDamage itself is
event-driven and has no FPS — it only reports *what* changed; the
scheduler rate-limits capture/encode/send to the cap, so the browser receives
at most that many frames per second, and a static screen still sends nothing.

## Protocol (WebSocket Binary, little-endian)

Server → client:

- `HELLO (0x01)`: proto version, screen size, pixel format RGBA8888, supported codecs
- `FRAME (0x02)`: seq, timestamp, op list
  - `RECT`: x/y/w/h (screen coords) + pw/ph (payload size after quality scaling) + codec (raw/zstd/webp) + payload
  - `COPY`: pixel block copy (client-local drawImage)
  - `CURSOR`: kept for compatibility (since V1.0.1 the client renders a local pointer; the agent doesn't send it)
- `CLIPBOARD (0x15)`: UTF-8 text push

Client → server:

- `MOUSE (0x10)`: move/down/up/wheel + coords + wheel steps
- `KEY (0x11)`: press/release + keysym
- `SETTINGS (0x12)`: quality / compression / idle FPS / max FPS / **current FPS cap**
  (0.8.0 added the 6th byte: the frontend decides the number — 20 while
  interacting, 1 when idle — and the agent executes it as-is; legacy 5-byte
  messages fall back to max FPS)
- `CLIPBOARD_SET (0x13)` / `CLIPBOARD_GET (0x14)`
- `KEEPALIVE (0x16)`: link keepalive (SDK sends every 25 s; the agent ignores it)

WS keepalive: when the screen is static the connection carries no data frames at
all (core principle), and reverse proxies like Cloudflare kill idle WS after ~100 s.
Two heartbeats run in parallel: the agent sends a WS protocol-level Ping every 25 s
(the browser stack answers Pong without page JS), and the SDK sends a 1-byte
KEEPALIVE every 25 s so the client→origin direction also stays active (some edges
judge idleness by client-direction traffic only). Neither touches page JS nor the
dynamic-FPS state machine.

## Security Boundary

- The agent listens on WebSocket only and defaults to `127.0.0.1`; put it behind a
  reverse proxy (WSS) for public deployments
- Every WS connection must present a valid static token or a valid JWT
- DISPLAY is decided by startup args/environment only; clients cannot set it
- The agent provides no shell/command execution and no static file serving

## Performance (local Xvfb measurements)

| Metric | Target | Measured |
|---|---|---|
| Click → canvas update | < 50 ms (< 30 ms LAN) | p50 ≈ 47 ms (in-page event→pixel) |
| Idle traffic | ≈ 0 | 0 B / 0 frames over 2.5 s static desktop |
| Whole interaction session | far below continuous full-screen | ~2.2 MB / 81 frames (incl. full-screen navigation) |

> Verify with (needs `cd scripts && npm install` for playwright-core and the
> .testenv Chrome; the host page is embedded — no extra server needed):
>
> ```bash
> export WEBDESK_TOKEN=<agent -k token>
> node scripts/e2e-sdk.mjs         # SDK: version injection/dual view/input/screenshot/clipboard/destroy/demo smoke
> node scripts/e2e-pointer.mjs      # pointer: CSS scaling accuracy/local pointer/no remote cursor/50% quality
> node scripts/e2e-canvas.mjs       # rendering/idle traffic/keyboard/wheel/quality settings
> node scripts/e2e-holdfps.mjs      # held mouse button/key keeps max FPS; release → 3 s countdown to idle
> node scripts/e2e-accept.mjs       # acceptance: navigation/window drag/WebP/clipboard
> node scripts/e2e-latency.mjs      # click→frame latency
> node scripts/e2e-jwt.mjs          # JWT auth: accept/expiry/wrong key/alg-none/static token coexistence
> node scripts/e2e-remote.mjs       # remote deployment (tunnel demo page: render/input passthrough/sidebar)
> ```

## Related Projects

- **[veilbrowser](https://github.com/iBugCode/veilbrowser)** — open-source
  fingerprint (anti-detect) browser SDK (CDP JS engine + self-compiled
  veil-chromium kernel). WebDesk is a natural companion to it: run
  veil-chromium profiles inside Xvfb on your server and attach WebDesk to the
  same DISPLAY — you get a zero-install, tunneled, token-protected remote view
  of each fingerprint profile instead of standing up a VNC stack per instance.

## Directory Layout

```
WebDesk/
├── Cargo.toml / package.json      # version = root package.json, read at startup; injected into SDK at build
├── assets/                        # logo (SVG + PNG) used by this README
├── src/
│   ├── main.rs / config.rs        # entry + CLI (-k token, --jwt-public-key)
│   ├── auth.rs                    # static token + JWT(EdDSA) verification
│   ├── server.rs                  # Axum, /ws only (auth at upgrade), no static files
│   ├── session.rs                 # session state + event-driven scheduler
│   ├── clipboard.rs               # X11 CLIPBOARD ownership + XFixes watch
│   ├── protocol/{frame,input}.rs  # binary protocol codecs
│   ├── capture/{x11,damage,region}.rs  # XShm/GetImage, XDamage, rect merge/pad
│   ├── codec/{resample,webp,zstd}.rs  # scaling, WebP lossy, zstd, scroll detection
│   └── input/{mouse,keyboard}.rs  # XTest injection + keysym→keycode map
├── web/
│   ├── src/{protocol,renderer,input,sdk}.ts  # full SDK source (bundled → dist/sdk.js)
│   ├── dist/sdk.js                # IIFE global WebDesk (single-file distribution)
│   └── demo.html                  # noVNC-style demo (left panel, collapsed by default)
├── scripts/
│   ├── dev.sh                     # one-shot start (incl. demo static server)
│   ├── e2e-lib.mjs                # E2E shared: inject sdk.js / coordinate mapping / navigation
│   └── e2e-*.mjs                  # end-to-end suites (playwright-core, uses .testenv Chrome)
└── Docs/ (not published in this repo)
```

## Known Limits

- Video playback not supported (no H264/WebRTC yet)
- UTF-8 text clipboard only; no file transfer or audio
- Keyboard maps core keysyms; non-Latin IME input is not covered
- Single browser session: a second browser takes over and the old one disconnects
  (with auto-reconnect on both ends they can take over each other — don't use from multiple ends simultaneously)
- Newer Chrome's Local Network Access: opaque-origin pages (file:// etc.) can't
  reach localhost WS; host the page over http(s)

## License

MIT — see [LICENSE](LICENSE).
