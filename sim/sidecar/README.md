# MetroForge sim sidecar

A Bun WebSocket server that wraps the existing deterministic sim core
(`../src/core/`) and the existing worker host loop (`../src/host/sim.worker.ts`)
behind a small wire protocol, so a native client can drive the same simulation the
web build uses without a browser. It exists for exactly one reason: **one sim
implementation** serves the web game at transit.ahousedividedgame.com and the
desktop client (`metroforge-native`), rather than two implementations that could
silently drift apart.

Nothing about game logic lives in this directory. `simHost.ts` is a near-verbatim
port of `sim.worker.ts` (same accumulator loop, same `applyCommand`/`simTick`, same
`AgentPool`, same event/insight/UI-building logic) with the worker-specific plumbing
(`self`, `postMessage`, dynamic `loadOsmCity`) swapped for a class + an injected
`send()` callback and a synchronous city resolver.

## Wire protocol

Full reference, including every JSON message and the exact byte layout of all four
binary frame types: [`metroforge-native/docs/PROTOCOL.md`](https://github.com/Egg3901/metroforge-native/blob/master/docs/PROTOCOL.md)
in the sibling `metroforge-native` repo. `wire.ts` in this directory is one of the
two implementations that document was checked against (the other is
`metroforge-native/crates/mf-protocol/src/binary.rs`); if you change a frame layout
here, that document needs updating too, and the two encoders must still agree
byte-for-byte.

## Run

```sh
# from metroforge-native/sim
bun install
bun run sidecar/index.ts --port 0
```

`--port 0` (the default) asks the OS for a free port. On startup the process prints
exactly one JSON line to stdout:

```json
{"mf":"sidecar","protocolVersion":1,"port":54231,"pid":12345}
```

That line is the handshake a parent process (the native client's `mf-net`, or the
smoke test below) reads to learn which port to connect to. `--headless-speed <n>`
sets the sim speed immediately on connect and removes the per-step tick cap, for
non-interactive runs that need to fast-forward.

## Compile matrix

`bun build --compile` produces a single-file executable per target, with no Bun
runtime dependency at the destination. All three cross-compile from Linux:

```sh
# from sidecar/, or via the package.json scripts below
bun run compile:linux           # -> ../dist-sidecar/metroforge-sidecar
bun run compile:windows         # -> ../dist-sidecar/metroforge-sidecar.exe
bun run compile:darwin-arm64    # -> ../dist-sidecar/metroforge-sidecar-darwin-arm64
```

Equivalently, spelled out:

```sh
bun build --compile --target=bun-linux-x64    ./index.ts --outfile metroforge-sidecar
bun build --compile --target=bun-windows-x64  ./index.ts --outfile metroforge-sidecar.exe
bun build --compile --target=bun-darwin-arm64 ./index.ts --outfile metroforge-sidecar-darwin-arm64
```

Output sizes are roughly 97 MB (Linux), 101 MB (Windows), 67 MB (macOS ARM64): all
ten city JSON bundles are embedded in the binary (see below), which accounts for
most of that.

## Smoke test

```sh
# against the interpreted source
bun run sidecar/smoke-test.ts

# against a compiled binary instead (proves the embedded city data survives
# `bun build --compile`, not just the interpreted path)
MF_SIDECAR_BIN=/path/to/metroforge-sidecar bun run sidecar/smoke-test.ts
```

This is the determinism regression gate, and it is the thing that actually backs
the "one sim implementation" claim above rather than just asserting it. It runs two
independent sessions against the same seed (NYC, seed 12345), builds an identical
small bus network on real street geometry pulled from the `ready` payload, drives
several hundred ticks, and asserts:

1. Vehicles actually moved between two consecutive `frame` binaries in both runs
   (the sim isn't stalled or frozen).
2. Both runs, having applied the same command log at the same tick numbers, agree
   on `requestReplay`'s `stateHash` at the same final tick.

It exits non-zero on any failure. CI runs it once interpreted and, before a release,
once against each compiled target: a divergence introduced specifically by
`bun build --compile` (rather than by a logic bug reachable either way) would show
up as the interpreted run passing while the compiled run fails, or vice versa.

Speed is deliberately frozen at 0 during network setup and then run at 240 (an exact
multiple of 20, so `speed/20` stays an integer and the tick accumulator never
carries a fraction), specifically so tick count is a pure function of elapsed 50 ms
firings for both runs. Building the network while ticks were live would let
wall-clock jitter in how long the setup round-trip takes apply those commands at
different tick numbers between the two runs, which would make them diverge for
reasons that have nothing to do with sim determinism.

## Why `cities.ts` statically imports every city

`bun build --compile` cannot resolve a dynamic `import()` at binary boot: the exact
async `loadOsmCity(key)` path the web build uses for on-demand city loading simply
does not survive compilation into a single-file executable. `cities.ts` works around
this by statically importing all ten city JSON bundles up front
(`../src/data/cities/*.json`) and exposing a synchronous `resolveCity(key)` in their
place. The cost is roughly 6.4 MB of embedded JSON per platform binary; that's
judged acceptable against the alternative of the sidecar silently failing to load
any city once compiled. This is the one piece of `sim.worker.ts`'s host-layer code
that could not be reused verbatim; everything else in `simHost.ts` is a near-direct
port.
