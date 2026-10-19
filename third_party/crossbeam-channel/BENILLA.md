# benilla's `crossbeam-channel` fork

Upstream: [`crossbeam-rs/crossbeam`](https://github.com/crossbeam-rs/crossbeam), MIT OR Apache-2.0,
version `0.5.15` as published, wired in through `[patch.crates-io]` in the workspace root.

The one change: every `std::time::Instant` is `web_time::Instant`. On `wasm32-unknown-unknown`
`std::time::Instant::now()` panics, which killed the web build's network threads at their first
timed receive (`recv_timeout`, `after`, `select!` with a timeout). Natively `web_time::Instant` is a
re-export of `std::time::Instant`, so native builds are unchanged.

To check, `diff -r` against the registry's `crossbeam-channel-0.5.15/src` shows only those imports.
