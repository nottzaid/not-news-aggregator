#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
portfolio_root="${PORTFOLIO_ROOT:-/home/muradkant/Projects/portfolio}"
output_dir="$portfolio_root/public/projects/not-news-web"
emsdk_root="${EMSDK_ROOT:-/home/muradkant/.local/share/emsdk}"
skia_source_dir="${SKIA_SOURCE_DIR:-/home/muradkant/Projects/rust-skia-not-news/skia-bindings/skia}"

# shellcheck source=/dev/null
source "$emsdk_root/emsdk_env.sh" >/dev/null

export EMCC_CFLAGS="-fwasm-exceptions -s WASM_LEGACY_EXCEPTIONS=1 -s SUPPORT_LONGJMP=wasm -s ERROR_ON_UNDEFINED_SYMBOLS=0 -s DEFAULT_TO_CXX=1 -s MIN_WEBGL_VERSION=2 -s MAX_WEBGL_VERSION=2 -s MODULARIZE=1 -s EXPORT_NAME=createNotNewsModule -s EXPORTED_RUNTIME_METHODS=GL,UTF8ToString -s ALLOW_MEMORY_GROWTH=1 -s FILESYSTEM=0 -s ENVIRONMENT=web"
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-Clink-arg=-fwasm-exceptions -Clink-arg=-sSUPPORT_LONGJMP=wasm -Clink-arg=-sWASM_LEGACY_EXCEPTIONS=1"
export FORCE_SKIA_BUILD=1
export SKIA_SOURCE_DIR="$skia_source_dir"

cargo build \
  --manifest-path "$repo_root/Cargo.toml" \
  --package not-news-web \
  --target wasm32-unknown-emscripten \
  --release

mkdir -p "$output_dir"
cp "$repo_root/target/wasm32-unknown-emscripten/release/not_news_web.js" "$output_dir/not_news_web.js"
cp "$repo_root/target/wasm32-unknown-emscripten/release/not_news_web.wasm" "$output_dir/not_news_web.wasm"

echo "Not News WebGL runtime written to $output_dir"
