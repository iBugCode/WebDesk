<div align="center">
  <img src="assets/logo.png" alt="WebDesk Logo" width="180" height="180" />
  <h1>WebDesk</h1>
  <p><strong>Web-first 的远程桌面:X11 脏区驱动的画面串流,经 WebSocket 直达浏览器标签页 —— 浏览器本身就是客户端。</strong></p>
  <p>
    <a href="#快速开始"><img alt="快速开始" src="https://img.shields.io/badge/install-快速开始-3b82f6" /></a>
    <a href="#license"><img alt="License MIT" src="https://img.shields.io/badge/license-MIT-green" /></a>
    <img alt="Language" src="https://img.shields.io/badge/Rust-1.75%2B-orange?logo=rust" />
    <img alt="Runtime" src="https://img.shields.io/badge/deps-零运行时依赖-blue" />
    <a href="README.md"><img alt="English docs" src="https://img.shields.io/badge/docs-English-red" /></a>
  </p>
</div>

# WebDesk

Web-first 的远程桌面系统:在 Linux + Xvfb 上运行桌面,用任意浏览器标签页直接操作 —— 无需安装客户端、无需端口映射、无需下载任何东西。画面由 X11 damage 事件捕获,按脏矩形编码(WebP / zstd)推送,由一个单文件 JS SDK 在 Canvas 上合成:**空闲带宽 ≈ 0**,交互延迟 ≤ 50 ms,整个 Agent 是一个约 6 MB 的静态 Rust 二进制。

简体中文 | [English](README.md) | [简要使用说明](USAGE.md)

## 它是干什么的

WebDesk 解决的是"给对方一个链接,对方就得到一个桌面"的问题。它不是大而全的远程控制工具,而是**从"一台有 X display 的 Linux 机器"到"另一个网页里的一个可交互窗口"**之间的最短路径。

典型场景:

- **云端浏览器 / 云桌面**:把 Chrome(或任何基于 Chrome 的浏览器)跑在服务器的 Xvfb 里,通过隧道 URL 打开,在任何设备上使用 —— 包括那些你装不了任何软件的机器。
- **把远程桌面嵌进你自己的产品**:一行 `WebDesk.window("#mount", {url, token})` 即可在任意页面挂载实时远端桌面 —— 适合 SaaS 控制台预览、QA 沙箱、自动化看板、"围观自动化执行"面板。
- **指纹/反检测浏览器集群**:配合 [veilbrowser](https://github.com/iBugCode/veilbrowser) —— 在农场服务器上无头运行 veil-chromium 各指纹环境,用 WebDesk 远程查看和驾驭它们,而不是每个实例都搭一套 VNC。
- **弱网远程访问**:酒店 Wi-Fi、4G/LTE、跨洲链路上 VNC/RDP 卡成幻灯片的场景,WebDesk 依然流畅 —— 因为它**从不发送没变化的东西**。
- **自动化可观测性**:给跑在 Xvfb 上的 agent 桌面(Playwright / CDP 自动化)挂一个观察窗,近乎零开销地看像素。

## 为什么不直接用 VNC / RDP?

VNC 和 RDP 久经考验,但都是二十多年前的设计:持续推像素、假定有原生客户端。WebDesk 把这两个假设都为浏览器时代重做了:

| 维度 | VNC | RDP | WebDesk |
|---|---|---|---|
| 客户端 | 各平台原生客户端(或笨重的 Java/HTML 重实现) | 原生/瘦客户端 + 网关栈 | **任意浏览器标签页**——约 50 KB 单文件 SDK |
| 空闲带宽 | 固定整屏(或分块)轮询刷新 | 较低,但需要 RDP 网关/通道设施 | **≈ 0 B —— 画面静止就是零帧**(damage 驱动) |
| 内容模型 | 什么都发,靠服务端拍脑袋 | 深度绑定 Windows + Office 负载 | **只发脏矩形**;滚动只发一个块复制指针 |
| FPS 模型 | 固定刷新环(典型 10~60 fps,与变化无关) | 按通道自适应 | **两态**(交互档/空闲档),由客户端 SDK 决定 |
| 分辨率处理 | 全程原始分辨率 | 全程压缩 | 服务端缩放(10~100%)+ 每矩形无失 zstd / 有损 WebP |
| 部署 | 服务端守护进程 + 各 OS 客户端 + 端口规则 | 网关/授权体系复杂 | ~6 MB 静态二进制 + 任意反代;其余交给浏览器 |
| 认证 | VNC 密码(默认很弱),或再套 SSH 隧道 | Kerberos/NLA 生态 | 每连接 token **或** Ed25519-JWT,WS 升级时验证 |
| 会话归属 | 共享/镜像 | 多会话 | 单属主 + 接管(行为确定) |

**WebDesk 赢在哪**

- **弱网下的交互流畅度。** 三条策略叠加:(1) 空闲成本精确为零 —— 静止桌面什么都不发,不是"低保真帧";(2) 小改动只发小矩形 —— 光标闪烁只花几百字节,而 VNC 服务端仍在按整屏清单发货;(3) 滚动是 `COPY` 指针而非像素。链路再差,交互*期间*照样满帧率,因为帧预算只花在真正变化的地方。
- **客户端零安装。** 能开网页就能操作桌面,把任何远程流程的最后一步(准备客户端机器)压缩到零。
- **可嵌入。** 桌面是一个带能力 API(截图、剪贴板、逐视图画质)的 DOM 组件,不是全屏应用 —— 可以拼进仪表盘、向导和多窗格控制台。
- **天生亲和隧道。** 空闲 WS 心跳、WSS 反代(Cloudflare/nginx)、升级时认证 —— 从第一行代码就按"公网 URL + 反代"设计。

**诚实的取舍 / VNC·RDP 仍然更强的地方**

- **暂不支持视频/视频会议。** 矩形 WebP 编码对 30fps H.264 内容是错的工具;RDP 的 AVC / H.264 硬件管线仍更适合流视频。
- **仅有 X11**(Windows/macOS Agent 尚不存在);Windows 会话的成熟方案仍是 RDP。
- **无文件传输/驱动器映射/打印机重定向** —— V1 剪贴板仅 UTF-8 文本。
- **无内置音频。**
- **非 Latin 输入法(IME)未覆盖**(仅 core keysym)。
- **单会话**:第二个连接是接管而非协作;没有多人围观模式。

## 架构

```
                    WebDesk Agent (Rust, 只提供 WebSocket)
                          |
                    X11 Connection
          +---------------+---------------+
          |                               |
       XDamage                         XTest 输入注入
          |                               |
          v                               v
    Dirty Region(合并)              Mouse/Keyboard
          |
          v
    Region Capture(XShm / GetImage)
          |
          v
    Update Scheduler(事件驱动 + 动态 FPS)
          |
       +--+---+
       |      |
     COPY   RECT(缩放 → raw/zstd/webp)
       +--+---+
          |
       WebSocket(Axum,仅 /ws)
          |
          v
       Browser(Canvas 2D 本地合成,页面由静态服务器托管)
```

核心原则:

- **不变化就不发送** —— 空闲带宽 ≈ 0
- **小变化只发送小区域**(XDamage 脏矩形 + 面积膨胀合并)
- **可以 COPY 就不重传像素**(滚动检测)
- **输入优先于画面**
- **浏览器负责最终渲染**(Canvas framebuffer)

脏区鲁棒性:XDamage 对部分绘制(窗口隐藏、抗锯齿边缘、亚像素布局)可能按边界少报 1~2px。对策:每个脏区在合并/捕获前四周外扩 2px —— 预防残影的同时保持「空闲零带宽」原则。COPY 滚动优化同样从严:只有密集校验证实「矩形内容确实整体平移、暴露条带确实是新内容」才走 COPY;进度条填充、光标闪烁这类「大面积静止 + 小块动态」一律退回整矩形重传 —— 误判的 COPY 会把动态内容整体平移且无自愈帧兜底,绝不允许发生。

编码:脏区先按「画面质量」设置做服务端缩放(10~100%),再按「画面压缩」设置编码 —— WebP 有损(1~100)或 zstd/raw 无损(0),浏览器用原生 WebP 解码上 Canvas。

## 快速开始

依赖:Rust 1.75+、Node 18+、Xvfb、python3(托管 demo 静态页)。窗口管理器与 Chrome 可用系统安装,或免 root 放入 `.testenv/`(见下)。

```bash
# 1. 构建 Agent + SDK
cargo build --release
(cd web && npm install && npm run build)   # 产出 web/dist/sdk.js

# 2. 一键启动(Xvfb :100 + openbox + Chrome + Agent + demo 静态服务)
scripts/dev.sh

# 或手动启动:
Xvfb :100 -screen 0 1280x720x24 &
openbox &                                    # 任意轻量 WM
(cd web && python3 -m http.server 8090 &)    # 静态托管 demo.html + sdk.js
DISPLAY=:100 ./target/release/webdesk --listen 127.0.0.1:9527 -k <token>
```

> Agent 只提供 WebSocket(`/ws`),不提供任何 HTML/JS。demo 页面在
> `http://127.0.0.1:8090/demo.html?url=ws://127.0.0.1:9527&token=<token>`。

### 静态编译(零运行时依赖)

x11rb 是纯 Rust X11 客户端(不链接 libX11),配合 musl 可产出任意 x86_64 Linux 直接运行的单一二进制(C 代码仅 zstd 与 libwebp,用容器内 musl gcc 编译):

```bash
scripts/build-static.sh        # 产物: dist-static/webdesk-linux-static(ldd 验证 statically linked,约 6MB)
```

上传到任意 Linux 服务器即可运行(仅需目标机有 X server,如 Xvfb):

```bash
scp dist-static/webdesk-linux-static srv:/root/webdesk/
DISPLAY=:99 ./webdesk-linux-static --listen 0.0.0.0:9527 -k <token>
# 版本号从二进制旁的 package.json 读取(或 WEBDESK_PACKAGE_JSON 环境变量指定)
```

公网部署请把 WS 端点置于反向代理之后:

```bash
# 如 Cloudflare Tunnel
cloudflared tunnel route ...   # 路由 -> wss://your-host
# 如 nginx: proxy_pass http://127.0.0.1:9527,带 Upgrade 头
```

### 免 root 测试环境(.testenv)

无 sudo 的机器可把 Chrome/openbox 解压到 `.testenv/`,`scripts/dev.sh` 会自动使用:

```bash
mkdir -p .testenv && cd .testenv
curl -sL -o chrome.deb https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb
dpkg-deb -x chrome.deb chrome/
mkdir debs && (cd debs && apt-get download openbox libobt2v5 libobrender32v5 libimlib2t64 \
  && for d in *.deb; do dpkg-deb -x "$d" ../openbox/; done)
```

## 认证

所有 WS 连接必须在 `/ws?token=<凭据>` 上出示凭据。两扇门彼此独立、可并存,**任一通过即放行**:

| 门 | 如何启用 | 凭据由谁签发 |
|---|---|---|
| 静态 token | `-k <token>`(省略则随机生成并打印) | 部署者,启动 Agent 时 |
| JWT(EdDSA / Ed25519) | `--jwt-public-key <公钥>` | 你的后端,用配对私钥签发 JWT |

JWT 规则:

- 只接受 `alg: "EdDSA"`;`none` 与其他算法即使签名有效也一律拒绝。先验签,再解析 claims,防算法混淆。
- `exp` / `nbf` 存在即强制(30s 时钟偏移容忍);不带视为长期有效。
- payload 不做任何语义解释——有效签名即视为可接入。建议给 token 设较短 `exp`(如 ≤ 10 分钟)由接入方刷新;注意 SDK 断线重沿用原 token 重连,过期后需页面重新下发新 token。
- Agent 自带密钥对生成:

```bash
webdesk --generate-jwt-keypair
# -----BEGIN PRIVATE KEY----- ...(PKCS#8,妥善保管)
# -----BEGIN PUBLIC KEY------ ...(SPKI)
# 公钥 hex(--jwt-public-key 参数用): e3a9...
```

PEM 与 `openssl genpkey -algorithm ed25519` / `openssl pkey -pubout` 完全互通。`--jwt-public-key` 支持:64 位 hex、32 字节 base64、PEM SPKI 公钥,或包含以上任一内容的文件路径。

后端签发示例(Node.js):

```js
import crypto from "node:crypto";
const { privateKey } = crypto.generateKeyPairSync("ed25519"); // 配对公钥配到 Agent
const b64u = (s) => Buffer.from(s).toString("base64url");
const hb = b64u(JSON.stringify({ alg: "EdDSA", typ: "JWT" }));
const pb = b64u(JSON.stringify({ sub: "user1", exp: Math.floor(Date.now() / 1000) + 600 }));
const sig = crypto.sign(null, Buffer.from(`${hb}.${pb}`), privateKey);
const jwt = `${hb}.${pb}.${Buffer.from(sig).toString("base64url")}`;
```

SDK 与 demo 页无需任何改动——把原来放 token 的位置换成 JWT 即可。

## CLI 参数

| 参数 | 默认 | 说明 |
|---|---|---|
| `--listen` | `127.0.0.1:9527` | WS 监听地址(默认仅 localhost,安全边界) |
| `-k, --token` | 随机 | 静态会话 token,所有 WS 连接必须携带有效凭据 |
| `--jwt-public-key` | – | JWT(EdDSA/Ed25519)验签公钥,见「认证」 |
| `--generate-jwt-keypair` | – | 生成 Ed25519 密钥对后退出 |
| `--display` | `$DISPLAY` | X11 DISPLAY(不接受客户端指定) |
| `--max-fps` | `20` | 初始 FPS 上限;0.8.0 起运行中由前端 SETTINGS 的 fps 字节接管 |
| `--idle-fps` | `1` | [遗留] 仅作无观众回落兜底(最后一个客户端离开后);空闲/交互模式已移到前端 |
| `--idle-timeout-ms` | `3000` | [遗留] 0.8.0 起不生效 —— 空闲倒计时移到前端 SDK(`idleTimeout` 选项,默认 3s) |
| `--batch-ms` | `12` | Damage 批量合并窗口 |
| `--scale-pct` | `100` | 初始画面质量(传输分辨率百分比) |
| `--compression` | `70` | 初始画面压缩:0 = zstd 无损,1~100 = WebP 有损压缩质量 |

## SDK(一行嵌入任意网页)

Agent 只提供 WS;SDK 是独立分发的单文件(`web/dist/sdk.js`,IIFE 全局 `WebDesk`),放到任意静态服务器即可:

```html
<div id="mount" style="width:880px;height:495px"></div>
<script src="https://你的静态服务器/dist/sdk.js"></script>
<script>
  const h = WebDesk.window("#mount", {
    url: "ws://127.0.0.1:9527",   // 缺省连页面同主机 :9527
    token: "<凭据>",              // 静态 token 或 JWT;缺省取页面 ?token=
  });
</script>
```

能力 API:

| API | 说明 |
|---|---|
| `WebDesk.window(selector, opts)` | 挂载远程桌面;opts:`url/token/scale/compression/idleFps/maxFps/idleTimeout/onStatus/onClipboard`(旧字段名 `jpeg` 仍接受,等同 `compression`) |
| `h.setQuality({scale,compression,idleFps,maxFps})` | 画质设置,立即生效 |
| `h.setClipboard(text)` / `h.getClipboard()` | 写入/读取远端剪贴板(X11 CLIPBOARD) |
| `h.onClipboard` (opts) | 远端剪贴板推送回调 |
| `h.screenshot({format,quality})` | 原屏幕截图(PNG/JPEG,原始分辨率) |
| `h.stats()` / `h.screenSize()` | 累计流量/帧数;远端屏幕尺寸 |
| `h.destroy()` | 摘除视图;最后一个视图销毁时关闭连接 |

- 同页可多次 `WebDesk.window(...)` 挂多个视图,共享同一路会话;帧只解码一次,各视图本地缩放
- 视图右上角内置实时 FPS 角标(每秒采样实测帧率;纯展示 HTML,不挡画布输入)
- 画布宽高比恒等于原屏幕(contain 等比缩放,容器多余部分留黑边),任何容器尺寸下画面都不会拉伸
- 键盘在视图获得焦点后才转发(不劫持宿主页面),失焦自动释放修饰键;断线 3s 自动重连
- `WebDesk.version` 构建时由 esbuild 从项目根 package.json 注入(不手写常量)
- 注意:新版 Chrome 的 Local Network Access 会拦截 opaque origin 页面(如 `about:blank`、`file://`)到本机的 WS;宿主页请用 http(s) 托管

## Demo 页面(noVNC 风格)

`web/demo.html`:设置面板在**左侧,默认收起**(点左缘按钮展开),包含:

> 发布注意:demo.html 通过 `dist/sdk.js?v=<版本>` 引用 SDK(HTML 不被 CDN 缓存而 .js 会被缓存,带版本参数即天然失效);发版时需与 package.json 同步更新该参数。

- 连接:url / token 输入
- 画面:画质 / 压缩(zstd 无损或 WebP 有损)/ 最低与最高 FPS(即时生效)
- 剪贴板:读取/写入远端
- 截图:一键截取原屏幕原始分辨率 PNG
- 状态:连接状态 + 累计流量/帧数

```bash
cd web && python3 -m http.server 8090
# 打开 http://127.0.0.1:8090/demo.html?url=ws://127.0.0.1:9527&token=<token>
```

## 动态 FPS

帧率从不取决于「屏幕变了多少」,只取决于「有没有人在操作」:

| 状态 | 帧率 |
|---|---|
| 画面静止 | 0(不发送) |
| 客户端上报空闲档 | ≤ 最低 FPS(默认 1) |
| 客户端上报交互档 | ≤ 最高 FPS(默认 20) |

**FPS 策略完全在前端 SDK**(0.8.0 起):服务端没有「空闲/交互」模式、不做任何输入检测,只执行 SETTINGS 下发的一个数字(当前 FPS 上限)。策略全在 SDK:检测到画布内输入(移动/点击/滚轮/按键,以及按住不松开的键)即按 `maxFps` 上报;停止输入超过空闲倒计时(默认 3 秒,`idleTimeout` 选项可调)且无按住,回落 `idleFps`。也就是「**Down 立即高帧率,Up 后 3 秒倒计时**」。XDamage 本身是事件驱动的,没有 FPS 概念——它只报告「哪里变了」;调度器按该上限对捕获/编码/发送限速,画面静止时依旧零发送。

## 协议(WebSocket Binary,小端)

服务端 → 客户端:

- `HELLO (0x01)`:proto 版本、屏幕宽高、像素格式 RGBA8888、支持的编码
- `FRAME (0x02)`:seq、时间戳、op 列表
  - `RECT`:x/y/w/h(原始坐标)+ pw/ph(缩放后 payload 尺寸)+ codec(raw/zstd/webp)+ payload
  - `COPY`:src→dst 像素块复制(客户端本地 drawImage)
  - `CURSOR`:保留兼容(V1.0.1 起客户端用本地指针,服务端默认不发送)
- `CLIPBOARD (0x15)`:UTF-8 文本推送

客户端 → 服务端:

- `MOUSE (0x10)`:move/down/up/wheel + 坐标 + 滚轮格数
- `KEY (0x11)`:按下/释放 + keysym
- `SETTINGS (0x12)`:画面质量 / 画面压缩 / 最低 FPS / 最高 FPS / **当前 FPS 上限**(0.8.0 追加第 6 字节:要 20 发 20、要 1 发 1,服务端原样执行;旧 5 字节格式服务端回退按最高 FPS 处理)
- `CLIPBOARD_SET (0x13)` / `CLIPBOARD_GET (0x14)`
- `KEEPALIVE (0x16)`:链路保活帧(SDK 每 25s 一帧;服务端忽略,不计入输入活动)

WS 保活:画面静止时连接上没有任何数据帧(核心原则),而 Cloudflare 等反代对空闲 WS 约 100s 读超时会掐连接。双向各挂一条心跳:Agent 每 25s 发一次 WS 协议层 Ping(浏览器协议栈自动回 Pong);SDK 每 25s 发一帧 1 字节 KEEPALIVE,保证"客户端→源站"方向在中间层眼里也不空闲(部分边缘只看客户端方向的流量判空闲)。两条心跳都不经过、也不扰动页面 JS 与动态 FPS 状态机。

## 安全边界

- Agent 只监听 WebSocket 且默认仅 `127.0.0.1`;公网部署请置于反向代理(WSS)之后
- 所有 WS 连接必须出示有效静态 token 或有效 JWT
- DISPLAY 只能由启动参数/环境决定;客户端不可指定
- Agent 不提供任何 shell/命令执行,不提供静态文件服务

## 性能(本机 Xvfb 实测)

| 指标 | 目标 | 实测 |
|---|---|---|
| 点击 → Canvas 更新 | < 50ms(局域网争取 <30ms) | p50 ≈ 47ms(页面内事件→像素口径) |
| 空闲流量 | ≈ 0 | 静态桌面 2.5s 实测 0B / 0 帧 |
| 整轮交互会话流量 | 显著低于持续整屏 | ~2.2MB / 81 帧(含整屏导航) |

> 验证脚本(需 `cd scripts && npm install` 安装 playwright-core,并使用 .testenv Chrome;内嵌静态宿主页,无需额外服务):
>
> ```bash
> export WEBDESK_TOKEN=<agent -k 的 token>
> node scripts/e2e-sdk.mjs         # SDK:版本注入/双视图/键鼠穿透/截图/剪贴板/destroy/demo 冒烟
> node scripts/e2e-pointer.mjs      # 指针:CSS 缩放精度/本地指针/无远端光标/50% 画质
> node scripts/e2e-canvas.mjs       # 渲染/空闲流量/键盘/滚轮/画质设置
> node scripts/e2e-holdfps.mjs      # 按住鼠标/键盘保持最高 FPS,松开 3s 后降档
> node scripts/e2e-accept.mjs       # 验收:导航/拖动窗口/WebP 压缩/剪贴板
> node scripts/e2e-latency.mjs      # 点击→画面延迟
> node scripts/e2e-jwt.mjs          # JWT 认证:放行/过期/错钥/alg-none/静态 token 并存
> node scripts/e2e-remote.mjs       # 远程部署验证(隧道 demo 页:渲染/输入穿透/侧栏)
> ```

## 相关项目

- **[veilbrowser](https://github.com/iBugCode/veilbrowser)** —— 开源指纹(反检测)浏览器 SDK(CDP JS 引擎 + 自编译 veil-chromium 内核)。WebDesk 是它的天然搭档:把 veil-chromium 各指纹环境跑在服务器的 Xvfb 里,把 WebDesk 挂到同一 DISPLAY —— 每个指纹环境得到一个零安装、走隧道、带 token 鉴权的远程查看窗口,而不是每实例都搭一套 VNC。

## 目录结构

```
WebDesk/
├── Cargo.toml / package.json      # 版本号以项目根 package.json 为准,启动时读取;SDK 构建时注入
├── assets/                        # 本 README 使用的 LOGO(SVG + PNG)
├── src/
│   ├── main.rs / config.rs        # 入口与 CLI(-k token、--jwt-public-key)
│   ├── auth.rs                    # 静态 token + JWT(EdDSA)验证
│   ├── server.rs                  # Axum,仅 /ws(升级时认证),无静态资源
│   ├── session.rs                 # 会话状态 + 事件驱动调度器
│   ├── clipboard.rs               # X11 CLIPBOARD 属主/读取 + XFixes 监听
│   ├── protocol/{frame,input}.rs  # 二进制协议编解码
│   ├── capture/{x11,damage,region}.rs  # XShm/GetImage、XDamage、矩形合并/外扩
│   ├── codec/{resample,webp,zstd}.rs  # 缩放、WebP 有损、zstd、滚动检测
│   └── input/{mouse,keyboard}.rs  # XTest 注入 + keysym→keycode 映射
├── web/
│   ├── src/{protocol,renderer,input,sdk}.ts  # SDK 全部源码(打包 → dist/sdk.js)
│   ├── dist/sdk.js                # IIFE 全局 WebDesk(单文件分发)
│   └── demo.html                  # noVNC 风格演示页(左侧设置栏,默认收起)
├── scripts/
│   ├── dev.sh                     # 一键启动(含 demo 静态服务)
│   ├── e2e-lib.mjs                # E2E 共用:注入 sdk.js / 坐标映射 / 导航
│   └── e2e-*.mjs                  # 端到端验证(playwright-core,需 .testenv Chrome)
└── Docs/(不随本仓库发布)
```

## 已知边界

- 不支持视频播放场景(无 H264/WebRTC)
- 仅支持 UTF-8 文本剪贴板;无文件传输、无音频
- 键盘按 core keysym→keycode 映射,非 Latin 输入法(IME)未覆盖
- 单浏览器会话:第二个浏览器连接会接管,旧会话断开(两端都开自动重连时会互相接管,请勿多端同用)
- 新版 Chrome 的 Local Network Access:opaque origin 页面(file:// 等)无法直连本机 WS,宿主页需 http(s) 托管

## License

MIT —— 见 [LICENSE](LICENSE)。
