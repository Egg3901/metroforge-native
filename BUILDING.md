# Building MetroForge Native

How to build the desktop client (`metroforge`) with the in-process Rust sim,
plus the measured compile-time / binary-size numbers from the build audit.

For day-to-day development (headless verify,
release tagging) see [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md).

## Prerequisites

| Tool | Notes |
|------|--------|
| **Rust** | Pinned in `rust-toolchain.toml` (currently `1.96`). `rustup` installs it automatically; needs `rustfmt` + `clippy`. |
| **Linux system libs** | `libasound2-dev`, `libudev-dev`, `pkg-config` (Bevy audio + udev). |
| **In-repo `sim/`** | TypeScript reference sim/content tooling lives at `sim/` in this repo. |
| **Windows cross-compile (optional)** | `clang`, `llvm`, `lld`, `cargo-xwin`, and `rustup target add x86_64-pc-windows-msvc`. |

```sh
# Debian/Ubuntu Bevy deps
sudo apt-get install -y libasound2-dev libudev-dev pkg-config

# Windows cross-compile extras (same as release.yml)
sudo apt-get install -y clang llvm lld
rustup target add x86_64-pc-windows-msvc
cargo install cargo-xwin
```

## Quick build

```sh
# from the metroforge-native repo root
cargo build --release -p mf-game
# -> target/release/metroforge
```

Dev iteration (our crates at `opt-level = 1`, dependencies at `opt-level = 2`
so Bevy stays usable without a full release rebuild):

```sh
cargo run -p mf-game
```

## Cargo profiles

Defined in the workspace `Cargo.toml`:

```toml
[profile.dev]
opt-level = 1

[profile.dev.package."*"]
opt-level = 2

[profile.release]
lto = "thin"
codegen-units = 1
strip = true
```

- **dev**: keep our crates fast to recompile; bump dependency optimization so
  a `cargo run` frame rate is playable.
- **release**: thin LTO + single codegen unit + symbol strip for the shipped
  binary. These knobs do **not** change game behavior; they only affect the
  artifact size and link time.

## Measured compile times and binary size

Numbers from a 4-vCPU Linux box (`rustc 1.96`, clean `target/`,
`cargo build --release -p mf-game --timings`). Re-run locally with
`--timings` and open `target/cargo-timings/cargo-timing.html`.

| Configuration | Cold wall-clock | `metroforge` size |
|---------------|-----------------|-------------------|
| **Before** (Bevy default features, stock release profile) | **5m 09s** (309 s) | **115.5 MiB** (121,147,680 B) |
| Bevy feature trim only | **4m 28s** (268 s) | **101.3 MiB** (106,206,600 B) |
| **After** (feature trim + `lto=thin` / `codegen-units=1` / `strip=true`) | **4m 53s** (294 s) | **54.9 MiB** (57,610,544 B) |

Deltas vs before:

| Change | Compile time | Binary size |
|--------|--------------|-------------|
| Bevy feature trim | **−41 s** (−13%) | **−14.2 MiB** (−12%) |
| Release profile (on trimmed graph) | +26 s vs trim-only (LTO cost) | **−46.3 MiB** |
| **Combined** | **−15 s** (−5%) | **−60.6 MiB** (−52%) |

Warm / incremental (after a successful release build, no source changes):
**~0.2 s** (`Finished ... in 0.20s`). A one-crate touch of `mf-game` under
the release profile still pays thin LTO on the final binary (~1–2 min on this
box).

### Top compile-time offenders (before → after)

From `cargo build --release --timings` unit durations (parallelism means
these sum to more than wall-clock):

| Crate | Before | After | Notes |
|-------|--------|-------|-------|
| `bevy_pbr` | 116.8 s | 90.9 s | Still the critical path |
| `bevy_render` | 70.0 s | 47.6 s | |
| `bevy_core_pipeline` | 54.2 s | 40.2 s | |
| `bevy_ui` / `bevy_sprite` | 46.4 / 39.8 s | 28.0 / 23.8 s | Kept (subway vignette uses `Node`/`ImageNode`) |
| `bevy_picking` | 23.9 s | **removed** | Unused; also dropped from `bevy_egui` |
| `bevy_animation` | 23.6 s | **removed** | No skeletal / clip animation |
| `bevy_gltf` / `gltf-json` | 21.6 / 18.6 s | **re-enabled** | Scripted Blender asset pipeline (tools/blender/) loads .glb bridge/train/cloud models |
| `mf-game` (final bin) | (smaller) | 95.0 s | Higher under thin LTO + `codegen-units=1` |

## Bevy feature audit

Workspace `bevy` dependency uses `default-features = false` with an explicit
allowlist. Disabled vs Bevy 0.16 defaults (and why):

| Feature | Status | Reason |
|---------|--------|--------|
| `animation` | off | No skeletal animation in-tree |
| `bevy_gltf` / `bevy_scene` | **on** | Scripted Blender `.glb` asset pipeline (tools/blender/); `bevy_scene` is pulled in transitively by `bevy_gltf` |
| `bevy_gilrs` | off | No gamepad input |
| `bevy_picking` (+ mesh/sprite/ui backends) | off | Input is egui + custom raycasts; `bevy_egui` picking feature also off |
| `vorbis` | off | SFX are custom `Decodable` chip samples, not Ogg files |
| `webgl2` / Android defaults | off | Desktop-only client |
| `sysinfo_plugin` / `custom_cursor` / `bevy_input_focus` / `smaa_luts` / `default_font` | off | Unused |
| `bevy_gizmos` | **on** | Tool ghosts + demand overlays |
| `bevy_audio` | **on** | `MfAudioPlugin` custom sources |
| `bevy_ui` / `bevy_text` / `bevy_sprite` | **on** | Subway vignette `ImageNode` |
| `bevy_state` | **on** | `AppState` machine |
| `png` / `hdr` / `tonemapping_luts` / `x11` / `multi_threaded` | **on** | Screenshots, bloom/tonemap, Linux windowing |

Verify after changing features:

```sh
cargo check -p mf-protocol
cargo check -p mf-net
cargo check -p mf-state
cargo check -p mf-render
cargo check -p mf-game
```

## Dependency duplicates

`cargo tree -d` / `cargo deny check bans` (warn-only in CI). After the audit:

**Removed by feature trim / pin:**

- `base64` 0.21 + 0.22 → single (dropped `bevy_gltf`'s 0.22 consumer)
- `bevy_animation`, `bevy_gltf`, `bevy_gilrs`, `bevy_picking`, `bevy_scene`, `sysinfo`, `lewton`/vorbis stack gone from the graph

**Unified where we own the pin:**

- `tungstenite` `0.24` → `0.26` so it uses `thiserror` 2 (same major as `mf-protocol` / `mf-net`)

**Still duplicated (transitive major splits; not safely unifiable here):**

`thiserror` 1+2 (calloop / encase / naga_oil still on 1), `ttf-parser` 0.20/0.21/0.25,
`rustix` 0.38+1.x, `bitflags` 1+2 (ktx2 via tonemap LUTs), `hashbrown`, `getrandom`,
`rand` 0.8+0.9, etc. Tracked as `cargo deny` warnings.

## cargo-deny

Config: [`deny.toml`](deny.toml). CI runs it as a **non-blocking** step in
[`.github/workflows/ci.yml`](.github/workflows/ci.yml) (`continue-on-error: true`)
so duplicate/yanked/license signal never fails the smoke or clippy gate.

```sh
cargo install cargo-deny   # or use the CI action locally via act
cargo deny check advisories bans licenses sources
```

## Windows cross-compile (`cargo-xwin`)

Same path `release.yml` uses (Linux runner → Windows MSVC target):

```sh
sudo apt-get install -y clang llvm lld
rustup target add x86_64-pc-windows-msvc
cargo install cargo-xwin

cargo xwin build --release -p mf-game --target x86_64-pc-windows-msvc
# -> target/x86_64-pc-windows-msvc/release/metroforge.exe
```

`cargo-xwin` downloads the MSVC CRT / Windows SDK on first use (~hundreds of MB)
and caches under `~/.cache/cargo-xwin`. Do not change the release profile or
Bevy feature set in a way that breaks this target — CI packages the PE next to
the Windows desktop build.

## Embedded sim runtime

MetroForge ships with the Rust sim embedded in-process. No Bun sidecar build is
required for runtime.

```sh
# from repo root
cargo run -p mf-game
```

Package the desktop client with `./scripts/package.sh <linux|windows|macos> <version>`.

## CI / release invariants

Do not break these when touching the build:

- [`.github/workflows/ci.yml`](.github/workflows/ci.yml) — `fmt`, `clippy -D warnings`, `test`, boot smoke, cargo-deny (warn-only)
- [`.github/workflows/release.yml`](.github/workflows/release.yml) — gate clippy, Linux+Windows (`cargo-xwin`) + macOS matrix, packaged Linux smoke gate
- Cross-compile: `cargo xwin build --release -p mf-game --target x86_64-pc-windows-msvc` must keep working
