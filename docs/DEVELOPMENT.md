# Development

## Prerequisites

- **Rust stable**: pinned in `rust-toolchain.toml` (`rustup` will pick it up
  automatically), with the `rustfmt` and `clippy` components.
- **Bun 1.3**: for building/running the TypeScript sidecar. Installed at
  `~/.bun/bin/bun` on the shared dev box; anywhere else, `bun` just needs to be on
  `PATH`.
- A sibling checkout of the [`metroforge`](https://github.com/Egg3901/metroforge)
  repo at `../metroforge` (relative to this repo): the sidecar's TypeScript sim
  source lives there under `sidecar/`, currently on the `feat/sim-sidecar` branch
  pending merge to `master`. See
  [`/root/metroforge/sidecar/README.md`](../../metroforge/sidecar/README.md) for
  sidecar-specific setup.

## Workspace layout

A single Cargo workspace, resolver 2, five member crates:

```
mf-protocol   pure wire types + binary codec, no Bevy dependency
mf-net        SimTransport trait, WebSocket client, sidecar process management
mf-state      shared Bevy Resources, fed from mf-net's event stream
mf-render     the 3D renderer, one sub-plugin per visual layer
mf-game       the game shell, binary `metroforge`
```

Full crate-by-crate responsibilities: [`ARCHITECTURE.md`](ARCHITECTURE.md).

## Cargo gates

Run from the repo root; these are the exact checks CI runs on every push and PR
(`ci.yml`, Linux-only: private-repo runner minutes make Windows/macOS too
expensive to run on every push; the 3-OS matrix lives in `release.yml` instead):

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI also runs `cargo deny` (advisories / duplicate versions / licenses) as a
**non-blocking** warn step — see [`BUILDING.md`](../BUILDING.md) and
[`deny.toml`](../deny.toml).

Build profiles, measured compile times / binary sizes, Bevy feature trimming,
and cross-compile notes live in [`BUILDING.md`](../BUILDING.md).

`cargo test --workspace` includes `mf-protocol`'s fixture round-trip tests (binary
decode -> encode -> byte equality; JSON literal decode -> encode -> value equality)
and `mf-state`'s quality-tier detection unit tests. It does **not** include
`mf-net`'s live-sidecar integration tests by default: those are `#[ignore]`d
because the sidecar may not be built in every environment `cargo test` runs in. Run
them explicitly once a sidecar binary is available:

```sh
cargo test -p mf-net --test live_sidecar -- --ignored
```

Set `MF_REQUIRE_SIDECAR=1` alongside that to turn "sidecar unavailable" from a
silent skip into a hard failure, for a CI job that's supposed to have one built.

## Running against a dev sidecar

The client needs a `metroforge-sidecar` executable at runtime. In development,
either point it at a prebuilt one or let it fall back to running the TypeScript
source directly under `bun`:

```sh
# against a prebuilt sidecar binary
MF_SIDECAR_PATH=/path/to/metroforge-sidecar cargo run -p mf-game

# against the interpreted TS source in ../metroforge/sidecar/index.ts
cargo run -p mf-game
```

`MF_AUTOSTART=<presetKey>` (e.g. `nyc`) skips the `MainMenu` city picker and jumps
straight to `Loading` with that city on Normal difficulty: this box has no display
to click an egui menu through, and it doubles as a fast-boot path for screenshots
and scripted smoke tests.

### Sidecar crash-recovery harness

`MF_TEST_KILL_SIDECAR=<seconds>` (e.g. `30`) kills the owned sidecar that many
wall-clock seconds after `InGame`, then asserts the client recovers in place
(re-handshake + autosave/city restore, no MainMenu bounce). Writes
`sidecar-recovery-result.txt` (or `$MF_TEST_KILL_SIDECAR_RESULT`) with `ok=1` on
success. Optional CI job: `sidecar-recovery` in `.github/workflows/ci.yml`
(`continue-on-error: true` until the sidecar binary path is a hard gate).

```sh
MF_AUTOSTART=nyc MF_TEST_KILL_SIDECAR=30 MF_SIDECAR_PATH=/path/to/metroforge-sidecar \
  cargo run -p mf-game --release
```

## Performance harness (`MF_PERF`)

`MF_PERF=1` enables Bevy's frame-time / entity-count diagnostics and a sample-
then-exit harness in `crates/mf-game/src/perf.rs`. After `InGame` settles it
records frame-time percentiles, visible-mesh draw-call proxies, entity/mesh/
material counts, and instrumented mf-render system CPU for `MF_PERF_SECONDS`
(default 60), prints an `MF_PERF REPORT`, then quits. Set `MF_PERF_ASSERT=1` to
also enforce `MF_PERF_BUDGET_FRAME_MS_P95` / `MF_PERF_BUDGET_DRAW_CALLS_P95`
(loose defaults aimed at lavapipe smoke, not a GPU FPS target).

```sh
export MF_AUTOSTART=nyc
export MF_PERF=1
export MF_PERF_SECONDS=60
# optional CI gate:
# export MF_PERF_ASSERT=1
cargo run --release -p mf-game
```

`MF_PERF_LOG=1` alone still enables the lighter once-per-second diagnostic log
without the sample-then-exit harness.

## Headless verification recipe

This box has no GPU, so rendering is validated with Mesa's software Vulkan
implementation (lavapipe) under a virtual X display, using `mf-game`'s built-in
verification harness (`crates/mf-game/src/verify.rs`) rather than a human clicking
through the game.

The harness is entirely inert unless `MF_VERIFY_DIR` is set. When set (alongside
`MF_AUTOSTART` so it can reach `InGame` without a menu), it drives a fixed sequence
once in-game: let the static layers settle, screenshot `default.png`; dolly to
street level, screenshot `street.png`; toggle subway view, screenshot `subway.png`;
drop to Potato quality, screenshot `potato.png`; quit. Frame budgets between stages
are generous since software rasterization on this box is slow.

```sh
export MF_AUTOSTART=nyc
export MF_VERIFY_DIR=/root/metroforge-native/verify
mkdir -p "$MF_VERIFY_DIR"
xvfb-run -a cargo run --release -p mf-game
```

`xvfb-run -a` allocates a free virtual display automatically (no `DISPLAY` needs to
be set by hand). The four PNGs land in `$MF_VERIFY_DIR`; a passing run produces
pixel-varied (non-uniform) images at all four stages, which is the automatable
proxy for "the renderer actually drew something" on a box with no eyes on it.

Required packages on a fresh box: `xvfb` and Mesa's Vulkan driver package
(lavapipe), plus whatever X11 utility packages your distro splits `xvfb-run` into.

## Soak harness (unbounded-growth check)

`MF_SOAK=<seconds>` arms a long-session growth check (`crates/mf-game/src/soak.rs`).
Pair it with `MF_AUTOSTART` so the run reaches `InGame` without a menu. While
armed the harness:

1. Sets sim speed to **20x** so dusk/dawn churn the night-paint path many times.
2. Orbits the camera around the dense city center.
3. Logs entity / `Assets<Mesh>` / `Assets<StandardMaterial>` / per-layer cache
   counts **every minute**.
4. After a 90s warmup, fails (exit code 1) if any tracked counter grows
   **superlinearly** across the sample series. Plateau or mild linear growth
   (e.g. the grow-only vehicle entity pool hitting a new high-water mark) passes.

```sh
export MF_AUTOSTART=nyc
export MF_SOAK=7200   # two wall-clock hours; use a smaller value for a smoke check
xvfb-run -a cargo run --release -p mf-game
```

A short smoke check (`MF_SOAK=300`) is enough to confirm the harness arms and
samples; the full 7200s run is what catches dusk/dawn material churn and
transit rebuild leaks over a long session. Toggle the in-game **F11** overlay
during a normal play session to watch the same counters live.

## Windows cross-compile

Windows release builds are cross-compiled from Linux with `cargo-xwin` rather than
run on a `windows-latest` GitHub runner: this is the setup `release.yml` actually
uses, proven working on this box:

```sh
sudo apt-get install -y clang llvm lld
rustup target add x86_64-pc-windows-msvc
cargo install cargo-xwin

cargo xwin build --release -p mf-game --target x86_64-pc-windows-msvc
```

`cargo-xwin` downloads the MSVC CRT and Windows SDK on first use (a few hundred MB)
and takes a couple of minutes to build itself from source the first time; both are
cached in CI. A clean build from here takes about 2 minutes and produces a
~60 MB PE32+ executable. The dependency graph has no TLS crate that would otherwise
drag in OpenSSL/rustls platform pain, which is most of why this cross-compile stays
clean.

## Release process

A release is: compile the sidecar for each target OS, build the client for each
target OS, stage them together with the font license, package, and publish to
GitHub Releases with auto-generated notes.

### 1. Package a build locally

`scripts/package.sh <os> <version>` (owned by CI, but usable locally to test
packaging) stages `target/release/metroforge[.exe]`, the matching
`dist-sidecar/metroforge-sidecar[.exe|-darwin-arm64]`, and
`crates/mf-game/assets/fonts/OFL.txt` into `release-artifacts/`:

```sh
cargo build --release -p mf-game
# build or place a matching sidecar binary under dist-sidecar/ first, see the
# metroforge/sidecar README for compile:linux/compile:windows/compile:darwin-arm64
./scripts/package.sh linux 0.1.0-alpha
```

`os` is one of `linux`, `windows`, `macos`. The script fails loudly with a
build-command hint if either binary is missing.

### 2. Tag and let CI build+publish

`release.yml` triggers on tags matching `v*` and builds all three platforms
(Linux and Windows cross-compiled on `ubuntu-latest`; macOS on `macos-latest`,
which is Apple Silicon), packages each with `package.sh`, and publishes via
`softprops/action-gh-release` with `generate_release_notes: true`.

```sh
git tag v0.1.0-alpha
git push origin v0.1.0-alpha
```

Equivalently, to build+publish from a local checkout instead of relying purely on
the tag push trigger:

```sh
gh release create v0.1.0-alpha --generate-notes
```

Either path uses GitHub's automatic release-notes generation, which is configured
by [`.github/release.yml`](../.github/release.yml) in this repo: PRs merged since
the previous tag are grouped into sections by label (Features, Rendering,
Simulation & Protocol, Fixes, Performance, Other). Label a PR correctly before it
merges: that's what determines which section its entry lands in, not anything
about the release process itself.

### 3. Sidecar source pin

Both `ci.yml` and `release.yml` check out the sidecar source from the sibling
`metroforge` repo. As of this writing that checkout still points at the
`feat/sim-sidecar` branch (not yet merged to `master`): both workflow files carry
a `TODO(sidecar-merge)` marking where to flip the `ref` to `master` (and ideally to
a pinned commit SHA or tag rather than a floating branch name) once that merge
lands. This repo's docs/scripts do not own that flip; it is tracked in
`.github/workflows/`, owned separately from these docs.
