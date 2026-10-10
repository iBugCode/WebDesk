#!/bin/bash
# WebDesk 开发/演示环境一键启动:Xvfb + 窗口管理器 + Chrome + Agent
#
# 用法:
#   scripts/dev.sh                 # 启动全部(默认 1280x720)
#   scripts/dev.sh 1920x1080       # 指定分辨率
#   scripts/dev.sh stop            # 停止全部
#
# 无 root 环境自动使用 .testenv 下解压的 openbox/chrome(见 README)。
set -e
cd "$(dirname "$0")/.."
ROOT=$(pwd)
SIZE="${1:-1280x720}"
DISPLAY_NUM=100
export DISPLAY=":${DISPLAY_NUM}"

if [ "$1" = "stop" ]; then
  pkill -f 'webdesk --listen' 2>/dev/null || true
  pkill -f "Xvfb :${DISPLAY_NUM}" 2>/dev/null || true
  pkill -f 'http.server 8090' 2>/dev/null || true
  echo "已停止"
  exit 0
fi

TESTENV="$ROOT/.testenv"

# 1) Xvfb
if ! xdpyinfo >/dev/null 2>&1; then
  rm -f "/tmp/.X${DISPLAY_NUM}-lock" 2>/dev/null || true
  Xvfb ":${DISPLAY_NUM}" -screen 0 "${SIZE}x24" -nolisten tcp &
  sleep 1
  echo "Xvfb :${DISPLAY_NUM} ${SIZE} 已启动"
fi

# 2) 窗口管理器(优先系统 openbox,否则 .testenv)
if ! xprop -root _NET_SUPPORTING_WM_CHECK >/dev/null 2>&1; then
  if command -v openbox >/dev/null; then
    openbox &
  else
    export LD_LIBRARY_PATH="$TESTENV/openbox/usr/lib/x86_64-linux-gnu:$LD_LIBRARY_PATH"
    export XDG_DATA_DIRS="$TESTENV/openbox/usr/share:$XDG_DATA_DIRS"
    "$TESTENV/openbox/usr/bin/openbox" &
  fi
  sleep 0.6
  echo "窗口管理器已启动"
fi

# 3) Chrome(可选,存在才启动;--no-sandbox 仅用于本地解压运行)
if [ -x "$TESTENV/chrome/opt/google/chrome/chrome" ] && ! pgrep -f "$TESTENV/chrome" >/dev/null; then
  mkdir -p /tmp/webdesk-chrome-profile
  nohup "$TESTENV/chrome/opt/google/chrome/chrome" \
    --no-sandbox --disable-gpu --disable-dev-shm-usage \
    --user-data-dir=/tmp/webdesk-chrome-profile --no-first-run \
    --window-position=80,60 --window-size=1100,600 \
    "https://www.wikipedia.org" >/dev/null 2>&1 &
  sleep 2
  echo "Chrome 已启动"
fi

# 4) WebDesk Agent(只提供 WebSocket;页面由静态服务器托管)
cargo build --release
TOKEN=$(mktemp -u XXXXXXXXXXXXXXXXXXXXXXXX 2>/dev/null || head -c 24 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | head -c 24)
echo
echo "启动 WebDesk Agent…"
DISPLAY=":${DISPLAY_NUM}" ./target/release/webdesk --listen 127.0.0.1:9527 -k "$TOKEN" &
AGENT_PID=$!

# 5) 静态托管 demo 页(sdk.js + demo.html;agent 本体不发 HTML)
DEMO_PORT=8090
DEMO_URL=""
if command -v python3 >/dev/null && ! ss -ltn 2>/dev/null | grep -q ":${DEMO_PORT} "; then
  (cd "$ROOT/web" && nohup python3 -m http.server "$DEMO_PORT" >/dev/null 2>&1 &)
  sleep 0.5
  DEMO_URL="http://127.0.0.1:${DEMO_PORT}/demo.html?url=ws://127.0.0.1:9527&token=${TOKEN}"
fi
sleep 1.5
echo
echo "=============================================="
echo "  演示页:   ${DEMO_URL:-<手动托管 web/ 目录后打开 demo.html>}"
echo "  WebSocket: ws://127.0.0.1:9527/ws?token=$TOKEN"
echo "=============================================="
wait $AGENT_PID
