#!/usr/bin/env bash
# 探测某个 rustc 工具链为 wasm32-wasip2 发出的 WASI 导入版本
# 用法: ./scripts/probe_wasi_version.sh <toolchain>  例如 nightly / stable / 1.97.0
set -e
TC="${1:?usage: probe_wasi_version.sh <toolchain>}"
PROBE=/tmp/wasiprobe
rustup toolchain install "$TC" --profile minimal >/dev/null 2>&1 || true
rustup target add wasm32-wasip2 --toolchain "$TC" >/dev/null 2>&1
if [ ! -d "$PROBE" ]; then
  cargo new "$PROBE" --bin --offline >/dev/null 2>&1
fi
rustup run "$TC" cargo build --manifest-path "$PROBE/Cargo.toml" --target wasm32-wasip2 --offline -q 2>/dev/null \
  || rustup run "$TC" cargo build --manifest-path "$PROBE/Cargo.toml" --target wasm32-wasip2 -q
echo -n "$TC -> "
wasm-tools component wit "$PROBE/target/wasm32-wasip2/debug/wasiprobe.wasm" 2>/dev/null \
  | grep -oE "wasi:[a-z]+/[a-z-]+@[0-9.]+" | sort -u | tail -1
