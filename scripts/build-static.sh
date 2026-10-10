#!/usr/bin/env bash
# 静态编译:产出零运行时依赖的 Linux musl 静态二进制。
#
# 依赖:x11rb 是纯 Rust X11 客户端(不链接 libX11);唯一的 C 代码是 zstd,
# 用 rust:alpine 容器里的 musl gcc 编译。为避免 root 产物污染本机 target/,
# 在 /tmp 的项目副本里构建,产物落到项目 dist-static/ 下。
#
# 用法: scripts/build-static.sh
# 产物: dist-static/webdesk-linux-static
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRATCH="${TMPDIR:-/tmp}/webdesk-static-build"

if ! rm -rf "$SCRATCH" 2>/dev/null; then
  # 上次容器内构建留下的 root 产物,本机无 sudo 时借容器清理
  docker run --rm -v "${TMPDIR:-/tmp}":/tmpdir rust:alpine rm -rf "/tmpdir/$(basename "$SCRATCH")"
fi
mkdir -p "$SCRATCH"
tar -C "$ROOT" --exclude=./target --exclude=./web/node_modules --exclude=./dist-static -cf - . | tar -C "$SCRATCH" -xf -

docker run --rm -v "$SCRATCH":/work -w /work rust:alpine sh -c '
  set -e
  RUSTFLAGS="-C target-feature=+crt-static" \
    cargo build --release --target x86_64-unknown-linux-musl
  cp target/x86_64-unknown-linux-musl/release/webdesk /work/webdesk-linux-static
  chmod 755 /work/webdesk-linux-static
'

mkdir -p "$ROOT/dist-static"
cp "$SCRATCH/webdesk-linux-static" "$ROOT/dist-static/webdesk-linux-static"
file "$ROOT/dist-static/webdesk-linux-static"
echo "产物: $ROOT/dist-static/webdesk-linux-static"
