# MetroForge Native

MetroForge Native is a Rust + Bevy desktop client for MetroForge that talks to the existing
TypeScript simulation over a local WebSocket sidecar process (spawned from `../metroforge/sidecar/`,
see `crates/mf-net`). The workspace splits the wire protocol (`mf-protocol`), the transport/process
management (`mf-net`), shared cross-crate render/game state (`mf-state`), the renderer
(`mf-render`), and the game shell (`mf-game`, binary `metroforge`) into separate crates so the
protocol and networking layers can later be reused by an in-process engine (e.g. on mobile, where
spawning a subprocess isn't allowed) without touching call sites.

## Building

```sh
# from /root/metroforge-native
cargo build --release -p mf-game

# run (requires a display; on a headless box the sidecar/protocol/state crates still
# build and test fine, but the bevy window will fail to open)
MF_SIDECAR_PATH=/path/to/metroforge-sidecar cargo run -p mf-game

# checks CI also runs
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

During development, if `$MF_SIDECAR_PATH` is unset, `mf-net` falls back to `bun run sidecar`
with cwd `../metroforge` (requires `bun` on `PATH` and the sidecar to exist there).
