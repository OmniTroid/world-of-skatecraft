# Web build

Skatecraft in Chrome: the full client compiled to `wasm32-unknown-unknown`, drawing through WebGPU,
talking to the local server through a WebSocket bridge.

## Run it

Needs nightly Rust (`rustup toolchain install nightly --component rust-src`),
[wasi-sdk](https://github.com/WebAssembly/wasi-sdk) for the Lua VM's C, `wasm-bindgen-cli` at the
lockfile's version, and the local server up.

```sh
WASI_SDK=/path/to/wasi-sdk scripts/web-build.sh
WOW_DIR=$(readlink WoW) docker compose -f web/compose.yaml up -d
```

Then open <http://localhost:8090> and pick where the game data comes from: served by the local
server, or your own WoW folder (Chromium only). `?data=server` or `?data=local` skips the choice.

## How it works

- **Threads**: an atomics build (nightly `-Z build-std`) over shared memory, so the page needs
  COOP/COEP (`web/nginx.conf`). Bevy stays single-threaded; benilla's own threads (network, audio
  render, stream decode) are web workers via `wasm_thread`.
- **Data**: `benilla-mpq`'s file layer (`src/fs/web.rs`) reads with synchronous Range XHRs and a
  block cache: from nginx in server mode, from blob URLs of the picked files in local mode.
- **Network**: `benilla-protocol::net` gives the protocol a `TcpStream` that is a WebSocket on
  `/tcp/<port>`, which websockify bridges to realmd and the world server; the main thread pumps it.
- **Lua**: `third_party/lua-src` builds for the web against wasi-sdk with wasm exceptions for
  `setjmp`; `web/wasi.js` answers its libc's WASI imports.
- **Audio**: an AudioWorklet (`web/audio-worklet.js`) plays from a ring in the shared memory that a
  worker fills (`sound/output/web.rs`), so a busy main thread cannot starve it; resumed on the
  first click. The page's `__benillaAudio` counts underrun quanta.
- **Forks**: `third_party/crossbeam-channel` uses `web_time::Instant` (std's panics on the web);
  `third_party/kira` decodes streams on a worker. Each has a `BENILLA.md`.
- **Shaders**: WebGPU rejects implicit-LOD sampling in non-uniform control flow, which naga allows,
  so `terrain.wgsl` and `static_gx.wgsl` sample before branching.

## Not yet

- The skate mode and the clipboard.
- Settings and saved variables: `benilla-config/` writes fail, so nothing persists.
- Pipelines compile on first use, so new scenes hitch; asset parsing runs on the main thread.
- A main-thread lock contended by a worker traps (`Atomics.wait` is refused there); none is seen yet.
