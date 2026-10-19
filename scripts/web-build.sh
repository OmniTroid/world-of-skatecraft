#!/usr/bin/env bash
# Builds the web client into web/dist (docs/WEB.md). Needs wasi-sdk for the Lua VM's C:
#   WASI_SDK=/path/to/wasi-sdk scripts/web-build.sh
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
: "${WASI_SDK:?set WASI_SDK to a wasi-sdk directory}"
export CC_wasm32_unknown_unknown="$WASI_SDK/bin/clang"
export AR_wasm32_unknown_unknown="$WASI_SDK/bin/llvm-ar"
export WASI_SYSROOT="$WASI_SDK/share/wasi-sysroot"
profile="${WEB_PROFILE:-play}"
# Threads are web workers over shared memory: std rebuilt with atomics, so nightly.
export RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals \
  -C link-arg=--shared-memory -C link-arg=--max-memory=4294967296 -C link-arg=--import-memory \
  -C link-arg=--export=__wasm_init_tls -C link-arg=--export=__tls_size \
  -C link-arg=--export=__tls_align -C link-arg=--export=__tls_base \
  -C link-arg=--export=__heap_base -C link-arg=--export=__data_end"
cargo +nightly build -Z build-std=std,panic_abort -p benilla --no-default-features \
    --profile "$profile" --target wasm32-unknown-unknown
# Emptied, not removed: the web container bind-mounts the folder.
mkdir -p web/dist
find web/dist -mindepth 1 -delete
wasm-bindgen --target web --no-typescript --out-dir web/dist \
    "target/wasm32-unknown-unknown/$profile/benilla.wasm"
# The Lua VM's libc imports a bare `wasi_snapshot_preview1`, which workers cannot map; point it at
# the shim, and publish the shared memory to it on every thread.
python3 - web/dist/benilla.js <<'PY'
import re, sys
p = sys.argv[1]
s = open(p).read()
s = s.replace('from "wasi_snapshot_preview1"', 'from "./wasi.js"')
s, n = re.subn(r"memory: memory \|\| (new WebAssembly\.Memory\([^)]*\))",
               r"memory: (globalThis.__benillaMemory = memory || \1)", s)
assert n == 1, "the shared memory's creation moved"
# Workers get 32 MiB stacks, the skate engine's; wasm-bindgen's default is 2 MiB.
s, n = re.subn(r"wasm\.__wbindgen_start\(thread_stack_size\);",
               "if (thread_stack_size === undefined && typeof WorkerGlobalScope !== 'undefined') "
               "thread_stack_size = 32 << 20;\n    wasm.__wbindgen_start(thread_stack_size);", s)
assert n == 1, "the thread start moved"
open(p, "w").write(s)
PY
cp web/index.html web/wasi.js web/audio-worklet.js web/dist/
echo "web/dist ready: docker compose -f web/compose.yaml up -d, then http://localhost:8090"
