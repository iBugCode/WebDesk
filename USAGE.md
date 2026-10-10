# WebDesk 简要使用说明

> 详细背景与原理见 [README.zh-CN.md](README.zh-CN.md)(中文)/ [README.md](README.md)(English)。本页只讲"怎么用"。

## 1. 本机 30 秒跑起来

```bash
cargo build --release && (cd web && npm install && npm run build)
scripts/dev.sh          # 一键: Xvfb :100 + openbox + Chrome + agent + demo 页
```

浏览器打开 `http://127.0.0.1:8090/demo.html?url=ws://127.0.0.1:9527&token=<agent 启动时打印的 token>`。

## 2. 部署到 Linux 服务器

```bash
scripts/build-static.sh                          # 产出 dist-static/webdesk-linux-static(静态链接,~5MB)
scp dist-static/webdesk-linux-static srv:/root/webdesk/
# 服务器上(需要 Xvfb 或任意 X server):
DISPLAY=:99 Xvfb :99 -screen 0 1280x720x24 &
cd /root/webdesk && echo '{"version":"..."}' > package.json   # 可选,仅供版本号展示
DISPLAY=:99 ./webdesk-linux-static --listen 127.0.0.1:9527 -k <随机长随机串>
```

Agent 只监听 WebSocket;公网请放在 WSS 反代之后(nginx / cloudflared 均可),
页面(sdk.js + demo.html 或你自己的宿主页)由任意静态服务器托管。

## 3. 认证:两种方式,可并存

所有连接都必须携带凭据:`/ws?token=<凭据>`。

**方式 A:静态 token(最简单)**

```bash
webdesk -k "一串随机字符串"
```

**方式 B:JWT(Ed25519 签名,适合多用户/程序接入)**

```bash
# ① 生成密钥对(私钥自己保管,公钥给 agent)
webdesk --generate-jwt-keypair
# ② 用公钥(hex)启动
webdesk -k "静态串" --jwt-public-key e3a9...     # 不想要静态串可不写 -k(会随机生成一个,打印在日志里)
```

后端签发(拿私钥签,Node.js 3 行):

```js
const hb = Buffer.from('{"alg":"EdDSA","typ":"JWT"}').toString("base64url");
const pb = Buffer.from(JSON.stringify({ sub: "u1", exp: Math.floor(Date.now()/1000) + 600 })).toString("base64url");
const jwt = `${hb}.${pb}.${Buffer.from(crypto.sign(null, Buffer.from(`${hb}.${pb}`), privateKey)).toString("base64url")}`;
```

规则速记:只认 `alg=EdDSA`;`exp`/`nbf` 写了就生效(30s 容忍);payload 内容
不解释,签名对上就放行。openssl 生成的 Ed25519 密钥(PEM)可直接用:
`--jwt-public-key /path/pub.pem`。

**页面/SDK 端**:两种方式完全一样——把 JWT 字符串当成 token 传即可,
SDK、demo 页都不需要改代码。

## 4. 常用参数速查

| 参数 | 默认 | 什么时候调 |
|---|---|---|
| `--listen` | `127.0.0.1:9527` | 反代同机保持默认;仅本机用 |
| `-k` | 随机 | 总是显式指定一个强随机串 |
| `--jwt-public-key` | – | 要 JWT 认证时 |
| `--max-fps` | `20` | 初始 FPS 上限;0.8.0 起运行中由前端接管(交互 20 / 空闲 1) |
| `--idle-fps` | `1` | [遗留] 仅作无观众回落兜底,空闲/交互模式已移到前端 SDK |
| `--idle-timeout-ms` | `3000` | [遗留] 0.8.0 起不生效;空闲倒计时在 SDK(`idleTimeout` 选项,默认 3s:Down 进高帧率,Up 后 3s 降档,按住不松开保持高帧率) |
| `--compression` | `70`(WebP 有损) | 远程看图片/网页可设 70~85,体积大幅下降;0 = zstd 无损 |
| `--scale-pct` | `100` | 高分屏省带宽可设 75 |

运行中也可在 demo 页设置面板(左侧 ☰)即时改画质/FPS,不用重启。

## 5. 页面嵌入最小示例

```html
<div id="mount" style="width:880px;height:495px"></div>
<script src="https://你的静态服务器/dist/sdk.js"></script>
<script>
  WebDesk.window("#mount", { url: "wss://你的ws域名", token: "<凭据>" });
</script>
```

同页可挂多个视图(共享一路会话);右上角 FPS 角标实时显示实测帧率。

## 6. 常见问题

- **空闲约 40~100s 后断开重连**:确认两端都用了 0.4.0+ 版本(双向心跳:
  Agent 25s WS Ping + SDK 25s KEEPALIVE)。老版本 SDK 配新 agent 仍会被
  Cloudflare 这类只看客户端方向流量的边缘掐断。
- **升级后还是旧画面/旧行为**:sdk.js 会被 CDN/浏览器缓存。宿主页引用时带
  `?v=<版本>` 参数(demo.html 已内置),发版必须同步更新。
- **file:// 或 about:blank 页面连不上本机 WS**:新版 Chrome 的 Local Network
  Access 拦截 opaque origin。用 http(s) 托管宿主页。
- **画面残留 1px 边框**(隐藏窗口/黑色块后):0.5.0 起脏区四周外扩 2px 预防;
  0.6.0 起不再在交互结束后额外推送整屏自愈帧(保持空闲零带宽)。若仍有请反馈
  具体应用场景。
- **画面糊了/想要无损**:默认画面压缩为 WebP 有损 70;demo 页左侧 ☰ 面板可
  即时切「无损 zstd」或调 WebP 质量,SDK 用 `setQuality({ compression: 0 })`。
- **第二个浏览器一连,第一个就断**:V1 单会话设计,后连者接管。多端同用时
  两端自动重连会互相踢,请勿这样用。
- **键盘打不了中文**:V1 按 core keysym 映射,IME 未覆盖。
