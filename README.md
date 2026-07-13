# MetroForge Desktop

MetroForge Desktop is the native 3D client for MetroForge: a single-player transit
network builder where you place stations, draw tracks, run routes, balance a budget,
and watch a city grow around the network you build.

Visual style: stark, high-contrast Mirror's Edge white-city minimalism. Buildings are
flat near-white blocks, streets are rich black, and the transit network you build is
the only thing in the world with color. See [`art-direction.md`](../art-direction.md)
for the full palette and rules (canonical constants live in
`crates/mf-render/src/palette.rs`).

The simulation itself is not reimplemented here. Desktop and the web prototype at
[transit.ahousedividedgame.com](https://transit.ahousedividedgame.com) run the exact
same deterministic TypeScript sim core, so a city plays out identically no matter
which client you use. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for why, and
for the determinism guarantee that keeps it true.

## Install

Prebuilt installers and archives for Windows, macOS (Apple Silicon), and Linux are
published on the [GitHub Releases page](https://github.com/Egg3901/metroforge-native/releases).
Each release includes the game executable, the sidecar executable, and the bundled
font license. No separate runtime to install.

### Windows

1. Download `metroforge-<version>-windows-x64-setup.exe` from
   [GitHub Releases](https://github.com/Egg3901/metroforge-native/releases).
2. Run the installer (Program Files, Start Menu shortcuts, Add/Remove Programs entry).
3. Releases are not Authenticode-signed, so Windows Defender SmartScreen will usually
   warn on first run: click **More info**, then **Run anyway**.
4. The game launches. A second launch focuses the existing window instead of starting
   another copy (and another sidecar).

Alternatively, download the `.zip` archive, extract it, and run `metroforge.exe`
(same SmartScreen prompt on first run).

Config and saves live under `%AppData%\Roaming\<org>\MetroForge\` (crash reports under
`%LocalAppData%\…\MetroForge\crash-reports\`). Explorer → Properties on `metroforge.exe`
shows the embedded version and icon.

### macOS

1. Download the `.dmg` file from
   [GitHub Releases](https://github.com/Egg3901/metroforge-native/releases).
2. Open the DMG and drag `MetroForge` to Applications.
3. Releases are ad-hoc signed (not Developer ID / notarized). On first launch,
   **right-click** the app and select **Open** (a plain double-click is blocked).
4. If Gatekeeper still blocks it: open System Settings → Privacy & Security, find the
   blocked-app message near the bottom, and click **Open Anyway**, then confirm.
5. The game launches.

### Linux

1. Download `metroforge-<version>-linux-x64.tar.gz` from
   [GitHub Releases](https://github.com/Egg3901/metroforge-native/releases).
2. Extract: `tar xzf metroforge-<version>-linux-x64.tar.gz`
3. Run: `./metroforge` from the extracted directory.

The `metroforge-sidecar` binary next to it is required and is used automatically.

**Optional desktop integration:** to add a menu entry, copy the bundled files:
```sh
mkdir -p ~/.local/share/applications ~/.local/share/icons
cp metroforge.desktop ~/.local/share/applications/
cp metroforge.png ~/.local/share/icons/
```

The game automatically detects your GPU and picks a graphics quality tier (see
below). If it runs slowly, lower the quality tier from the in-game HUD.

## Building from source

Prerequisites: Rust stable (see `rust-toolchain.toml`) and Bun 1.3. The sidecar's
TypeScript sim source now lives in-repo under [`sim/`](sim/) (monorepo
consolidation) — no sibling checkout needed. Full build docs — profiles, measured cold/warm
times, Bevy feature audit, `cargo-xwin`, sidecar Bun compile — are in
[`BUILDING.md`](BUILDING.md). Day-to-day setup: [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md).

```sh
# from the metroforge-native repo root
cargo build --release -p mf-game
```

To run the client against a sidecar, the client needs a `metroforge-sidecar`
executable, resolved in this order:

1. `$MF_SIDECAR_PATH`: an explicit path to a prebuilt sidecar binary.
2. A `metroforge-sidecar[.exe]` sitting next to the client executable (this is how a
   packaged release finds it).
3. Dev fallback: `bun run sidecar/index.ts` with the working directory set to the
   in-repo `sim/` package. This requires `bun` on `PATH` (or at
   `~/.bun/bin/bun`) and the `sim/` sidecar source present.

```sh
# option A: point at a prebuilt sidecar binary
MF_SIDECAR_PATH=/path/to/metroforge-sidecar cargo run -p mf-game

# option B: let mf-net fall back to `bun run sidecar/index.ts` in ./sim
cargo run -p mf-game
```

Set `MF_AUTOSTART=<presetKey>` (e.g. `MF_AUTOSTART=nyc`) to skip the `MainMenu` city
picker and jump straight to `Loading` with that city on Normal difficulty. Useful on
a box with no display to click through, and for scripted screenshots (see
[`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md) for the full headless verification
recipe).

Same checks CI runs:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Quality tiers

Auto-detected from the GPU adapter at boot: a discrete GPU picks High; an integrated
GPU picks Low, unless its name matches a known software/low-power renderer (Intel
UHD/HD Graphics, llvmpipe, lavapipe, SwiftShader), in which case it picks Potato;
anything unrecognized picks Medium. A `config.toml` override always wins over
auto-detection. Target: Potato holds 60 fps on an Intel UHD 620 at 12 km building
draw distance in NYC.

| knob | potato | low | medium | high |
|---|---|---|---|---|
| present mode | no vsync | vsync | vsync | vsync |
| MSAA | off | off | 4x | 4x |
| shadows | off | 1024 cascade | 2048 cascade | 4096 cascade |
| roads / vehicles material | unlit (flat black) | unlit | lit (`StandardMaterial`) | lit |
| buildings | cel-lit (`building_lit`) on every tier | cel-lit | cel-lit + shadows | cel-lit + shadows |
| building draw distance | 3 km | 6 km | 12 km | unlimited |
| agent cap | 0 | 100 | 250 | 400 |
| metro vehicle mesh | box brick | box brick | glTF consist (`.glb`) | glTF consist |
| terrain subdivision | coarsest (÷3) | coarse (÷2) | full | full |
| day/night cycle | off (fixed noon) | on | on | on |
| atmosphere (cloud cards + ground shadows) | off | off | on (Settings toggle) | on (denser steps) |
| precip particles (rain/snow) | off | up to 400 | up to 1 500 | up to 4 000 |
| wet-road / snow-slush grading | n/a (unlit roads) | n/a | on when weather active | on |
| main-menu diorama lighting | golden hour pinned (`AttractLighting`) on `MainMenu` at every tier | same | same | same |

Knob source: `QualityTier::knobs()` in `crates/mf-state/src/quality.rs`. Weather
rendering also requires sim weather in `UiState` (v0.7) **and** the Settings
`WeatherEffects` checkbox; precip counts above are caps from
`crates/mf-render/src/precip.rs` (`tier_count`: Potato = 0). Atmosphere needs
`atmosphere_enabled` on the tier plus `WeatherEffects::enabled`.

Unlit flat-black streets plus cel-lit near-white buildings are the art style, not
just the cheap fallback, so Potato still reads as MetroForge (dense-center cel
outline on every tier). Higher tiers add shadows, MSAA, bloom, stylized water,
glTF bridge/metro models, atmosphere, and scaled precip on top.

## Architecture

```
 +-------------------+       WebSocket (mf-wire v1)        +-----------------------+
 |  metroforge-native |  <------------------------------>  |  metroforge-sidecar   |
 |  (Rust / Bevy)     |   JSON control frames (handshake,   |  (Bun, compiled from  |
 |                    |   2 Hz UI, commands, toasts)        |  ./sim/sidecar/)      |
 |  mf-protocol       |   binary hot frames (50 ms ticks,   |                       |
 |  mf-net            |   fields, traffic, static masks)    |                       |
 |  mf-state          |                                     |  wraps the exact      |
 |  mf-render         |                                     |  deterministic TS     |
 |  mf-game (bin)     |                                     |  sim core             |
 +-------------------+                                     +-----------------------+
          ^                                                          ^
          |  spawned as a local child process                        |
          |  (or connects to one already running)                    |
          +----------------------------------------------------------+
```

`mf-net` is the only crate that knows the sim lives in a separate process. On mobile,
where subprocesses aren't allowed (notably iOS), a future in-process transport
satisfies the same `SimTransport` trait with no changes anywhere else. See
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Repo map

```
metroforge-native/
  Cargo.toml                 workspace manifest
  rust-toolchain.toml        pinned Rust channel
  crates/
    mf-protocol/             wire types + binary codec, no Bevy dependency
    mf-net/                  SimTransport, WebSocket client, sidecar process mgmt
    mf-state/                shared Bevy resources (city/fields/ui/frame/quality)
    mf-render/               the 3D renderer (terrain/roads/buildings/transit/...)
    mf-game/                 the game shell (bin `metroforge`): states/camera/HUD
  docs/
    ARCHITECTURE.md          crate responsibilities, determinism, design rationale
    PROTOCOL.md              mf-wire v1 full reference
    DEVELOPMENT.md           build/test/release workflow
  scripts/
    package.sh               stages a client + sidecar + font into a release archive
    tier-smoke.sh            per-tier in-city render smoke (unique-colour floors)
  sim/                       TypeScript sim (metroforge-sim): sim core, host loop,
                             content, city data, and the Bun sidecar (compiled into
                             the sidecar binary the client spawns)
  tools/blender/             scripted Blender 5 `.glb` asset pipeline (bridge/train/cloud)
  crates/mf-game/assets/models/  committed `.glb` outputs loaded by `mf-render`
  .github/workflows/         CI and release automation
```

The sidecar's TypeScript sim source lives in this repo under [`sim/`](sim/)
(the `metroforge-sim` package: sim core, host loop, content, city data, and the
Bun sidecar); see [`sim/README.md`](sim/README.md) and
[`sim/sidecar/README.md`](sim/sidecar/README.md).

## License

The bundled font (Inter) is licensed under the SIL Open Font License 1.1; its full
text and attribution ship as `OFL.txt` alongside every release
(`crates/mf-game/assets/fonts/OFL.txt`). No other licensing terms are declared for
this project at this time.
