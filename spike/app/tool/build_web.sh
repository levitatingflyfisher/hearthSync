#!/usr/bin/env bash
# Build the PWA in the main-thread configuration that needs no COOP/COEP headers.
#
# Why not `flutter_rust_bridge_codegen build-web`: in 2.13.0 it always uses the
# nightly toolchain with `-Z build-std` and threaded-WASM rustflags
# (+atomics, --shared-memory, --import-memory). A shared WebAssembly.Memory needs
# SharedArrayBuffer, which browsers only give cross-origin-isolated pages, and
# GitHub Pages cannot send those headers. Every bridge function here is
# #[frb(sync)], so the worker pool (a lazy thread_local) is never built and
# plain stable wasm32 with non-shared memory is enough.
set -euo pipefail
cd "$(dirname "$0")/.."
wasm-pack build -t no-modules -d "$PWD/web/pkg" --no-typescript \
  --out-name rust_lib_app --release rust
flutter build web --release
