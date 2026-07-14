# MetroForge Desktop

MetroForge Desktop is the native 3D client for MetroForge: a single-player transit
network builder where you place stations, draw tracks, run routes, balance a budget,
and watch a city grow around the network you build.

Visual style: stark, high-contrast Mirror's Edge white-city minimalism. Buildings are
flat near-white blocks, streets are rich black, and the transit network you build is
the only thing in the world with color. See [`art-direction.md`](../art-direction.md)
for the full palette and rules (canonical constants live in
`crates/mf-render/src/palette.rs`).

The simulation is now fully in-process Rust in this desktop client (no external
sidecar process). See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for crate
boundaries and determinism guarantees.

## Install

Prebuilt installers and archives for Windows, macOS (Apple Silicon), and Linux are
published on the [GitHub Releases page](https://github.com/Egg3901/metroforge-native/releases).
Each release includes the game executable and the bundled font license. No
separate runtime to install.

### Windows

1. Download `metroforge-<version>-windows-x64-setup.exe` from
   [GitHub Releases](https://github.com/Egg3901/metroforge-native/releases).
2. Run the installer (Program Files, Start Menu shortcuts, Add/Remove Programs entry).
3. Releases are not Authenticode-signed, so Windows Defender SmartScreen will usually
   warn on first run: click **More info**, then **Run anyway**.
4. The game launches. A second launch focuses the existing window instead of
   starting another copy.

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

**Optional desktop integration:** to add a menu entry, copy the bundled files:
```sh
mkdir -p ~/.local/share/applications ~/.local/share/icons
cp metroforge.desktop ~/.local/share/applications/
cp metroforge.png ~/.local/share/icons/
```

The game automatically detects your GPU and picks a graphics quality tier (see
below). If it runs slowly, lower the quality tier from the in-game HUD.

## Building from source

Prerequisites: Rust stable (see `rust-toolchain.toml`). The TypeScript reference
sim/content tooling lives in-repo under [`sim/`](sim/) (no sibling checkout
needed). Full build docs - profiles, measured cold/warm times, Bevy feature
audit, and `cargo-xwin` - are in
[`BUILDING.md`](BUILDING.md). Day-to-day setup: [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md).

```sh
# from the metroforge-native repo root
cargo build --release -p mf-game
```

Run locally:

```sh
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
| render scale | 0.75 | 1.0 | 1.0 | 1.0 |
| MSAA | off | off | 4x | 4x |
| shadows | off | off | 2048 cascade | 4096 cascade |
| material | unlit vertex color | unlit | lit (StandardMaterial) | lit (StandardMaterial) |
| building draw distance | 3 km | 6 km | 12 km | unlimited |
| agent cap | 0 | 100 | 250 | 400 |
| vehicle mesh | quad billboard | low-poly box | box | chamfered box |
| terrain subdivision | coarsest | coarse | full | full |
| day/night cycle | off (fixed noon) | on | on | on |
| weather (fog/clouds) | off | off | dual-layer volumetric (toggleable) | dual-layer volumetric (toggleable) |

Unlit rendering plus flat vertex colors and zero textures is the whole art style, not
just the cheap fallback, so Potato still looks like MetroForge. Higher tiers only add
shadows, MSAA, emissive glow, dual-layer scrolling volumetric fog/clouds (Medium+,
toggleable in Settings; ground mist + high cloud deck with a shared wind field),
and chamfered vehicle meshes on top.

## Architecture

```
 +-------------------+
 |  metroforge-native|
 |  (Rust / Bevy)    |
 |                   |
 |  mf-protocol      |
 |  mf-net           |
 |  mf-state         |
 |  mf-render        |
 |  mf-game (bin)    |
 +-------------------+
```

`mf-net` is the transport/wire seam that owns the in-process sim host thread and
message flow into ECS. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Repo map

```
metroforge-native/
  Cargo.toml                 workspace manifest
  rust-toolchain.toml        pinned Rust channel
  crates/
    mf-protocol/             wire types + binary codec, no Bevy dependency
    mf-net/                  SimTransport + embedded host thread + wire bridge
    mf-state/                shared Bevy resources (city/fields/ui/frame/quality)
    mf-render/               the 3D renderer (terrain/roads/buildings/transit/...)
    mf-game/                 the game shell (bin `metroforge`): states/camera/HUD
  docs/
    ARCHITECTURE.md          crate responsibilities, determinism, design rationale
    PROTOCOL.md              mf-wire v1 full reference
    DEVELOPMENT.md           build/test/release workflow
  scripts/
    package.sh               stages the client + assets + font into release archives
  sim/                       TypeScript reference sim/content tooling
  .github/workflows/         CI and release automation
```

The TypeScript reference sim/content package lives in this repo under
[`sim/`](sim/); see [`sim/README.md`](sim/README.md).

## License

The bundled font (Inter) is licensed under the SIL Open Font License 1.1; its full
text and attribution ship as `OFL.txt` alongside every release
(`crates/mf-game/assets/fonts/OFL.txt`). No other licensing terms are declared for
this project at this time.
