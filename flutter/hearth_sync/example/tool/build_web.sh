#!/usr/bin/env bash
# Build the demo PWA in the main-thread configuration that needs no COOP/COEP
# headers (the spike's recipe; ADR 0011, ADR 0012).
#
# Why not `flutter_rust_bridge_codegen build-web`: in 2.13.0 it always uses the
# nightly toolchain with `-Z build-std` and threaded-WASM rustflags
# (+atomics, --shared-memory, --import-memory). A shared WebAssembly.Memory needs
# SharedArrayBuffer, which browsers only give cross-origin-isolated pages, and
# GitHub Pages cannot send those headers. Every bridge function is #[frb(sync)],
# so frb's worker pool (a lazy thread_local) is never built and plain stable
# wasm32 with non-shared memory is enough.
#
# wasm-pack is the pinned one in the repo's git-ignored .tools/ (0.15.0), and it
# fetches the wasm-bindgen CLI matching rust/Cargo.lock (0.2.92) into .tools/.
set -euo pipefail
cd "$(dirname "$0")/.."
repo="$(cd ../../.. && pwd)"
export WASM_PACK_CACHE="$repo/.tools/wasm-pack-cache"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/../rust/target}"
"$repo/.tools/bin/wasm-pack" build -t no-modules -d "$PWD/web/pkg" --no-typescript \
  --out-name hearth_sync_bridge --release ../rust
flutter build web --release
