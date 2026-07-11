# Changelog

All notable changes to MetroForge Desktop (metroforge-native). Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions are git
tags of the form `vX.Y.Z-alpha`, each published with smoke-gated artifacts
for Linux, Windows, and macOS (arm64). Sidecar/data changes land in the
sibling `metroforge` repo and reach releases via the pinned sidecar SHA in
`release.yml`.

## [Unreleased]

### Added
- Windows desktop polish: DPI-aware windowing, F11/Alt+Enter borderless fullscreen
  (persisted), remember size/position, pause rendering when minimized/alt-tabbed,
  single-instance focus-existing-window guard, embedded exe icon + VERSIONINFO,
  crash reports under the OS local data dir
- Main menu overhaul and app icon (#47)
- Release installers: Windows NSIS setup.exe, macOS .dmg, Linux tarball with
  desktop integration (#52)

### Fixed
- Sidecar: NYC terrain water-bleed and missing NJ/Brooklyn road coverage
  (metroforge#29)
- README SmartScreen / Gatekeeper notes now match unsigned Windows and ad-hoc
  signed macOS release reality

## [0.4.2-alpha] - 2026-07-10

The "streets actually render" release. v0.4.1 shipped with no visible roads
in real cities (three stacked rendering bugs); this supersedes it.

### Fixed
- Roads rebuilt when the sim's fields version changes; previously they baked
  once against first-version terrain heights and ended up buried under the
  relief that arrived a version later (#38).
- Road surface lift raised 0.5m → 2m: at overview zoom on near-flat terrain
  the ribbons lost the depth fight against the terrain mesh and the whole
  street grid vanished from skyline framings (#41).
- Water-crossing road segments ride an 8m bridge deck instead of hugging
  water level, where only a black sliver of the ribbon surfaced (#41).
- Terrain graded flat in corridors under roads and around stations, ending
  the stripe-through-building artifact on slopes (#37, closes #33).
- CI workflow had been failing at 0s since the v0.4 integration commit — a
  duplicate `with:` key made `ci.yml` unparseable (#36).
- Sidecar (metroforge #24): procedural relief for real cities damped ~70%
  and faded to sea level near shorelines — flat urban islands (Roosevelt
  Island) no longer render as sand dunes.
- Sidecar (metroforge #25): city imports expand their bbox to a true square
  in meters, so the world square fills edge to edge (nyc road coverage
  359→519 of 576 grid cells — Brooklyn and New Jersey now have streets);
  road polylines are clipped at the map edge instead of running off-world.

### Added
- Theme system (#39, closes #32): Light (unchanged default), Dark
  (standing-night city with glowing routes and dark HUD), and Purple
  (vaporwave). Selectable in menu/pause, persisted in config.toml,
  `MF_THEME` env override.
- Cleveland as the second real-buildings city (metroforge #26): 33,553
  OSM vector footprints, regenerated square-fill bundle.

## [0.4.1-alpha] - 2026-07-10

### Fixed
- World polish wave (#31): roads-vs-terrain load race, flowing route lines,
  park trees, blue water, Penn Station footprint cap.

## [0.4.0-alpha] - 2026-07-10

### Added
- "It's a campaign" (#30): campaign structure, save/load with sidecar
  hydration, diorama attract mode, promo screenshot harness
  (`MF_PROMO_DIR`, `MF_HIDE_HUD`).

### Fixed
- Buried-streets geometry: densified road polylines (24m), 90m terrain
  relief cap, footprint-min building bases (#30).

## [0.3.0-alpha] - 2026-07-10

### Added
- "It's a puzzle" (#28): gravity demand arcs, station and finance panels,
  map mode, load-fat route stripes, overlay network-dimming doctrine.

## [0.2.0-alpha] - 2026-07-10

### Added
- "It's a game" (#26): build tools, command bus with undo, build toolbar,
  route panel, egui design system.

## [0.1.5-alpha] - 2026-07-10

### Added
- Transit color pop (#22): touching route bundles, wide bands, network demo
  harness.

### Fixed
- Night lighting on routes (#22); soft high-key lighting and map-style
  road widths (#23).

## [0.1.4-alpha] - 2026-07-10

### Added
- `building:part` rendering via wire v2 (#21): tiered and elevated building
  masses (77k+ stacked masses in NYC).

### Fixed
- Verify harness determinism: real-road street framing, deterministic
  reveal, midday capture window (#20).

## [0.1.3-alpha] - 2026-07-10

### Added
- Cursor + zoom reveal (#19): dithered building dissolve around the pointer
  and at close camera range.

## [0.1.2-alpha] - 2026-07-10

### Added
- Real building footprint prisms with cel shading (#12, #15): NYC renders
  67k+ real OSM footprints.
- Pause screen, menu redesign, procedural chiptune SFX, camera smoothing,
  stable monospace top bar (#11).

### Fixed
- Perf wave 2 (#14): backface culling, winding fixes, matte materials —
  cumulative −37.6% median frame time on the software rasterizer.
- Release workflow: sidecar checkout pinned to a full 40-char SHA (#13, #17).

## [0.1.1-alpha] - 2026-07-10

### Fixed
- v0.1.0 blue screen: pre-game UI never rendered because no camera existed
  before the in-game state (#1).
- Release smoke gate installs winit X11 runtime libs on the runner (#10).

### Added
- Artifact cold-run smoke gate in the release pipeline — every published
  artifact boots headless and screenshots before upload (#8).
- Perf wave 1 (#9): quality auto-detect, steady-state churn elimination,
  4ms command latency (−13% median frame).

## [0.1.0-alpha] - 2026-07-10

**Known issue: does not start (blue-black screen at launch) — use
v0.1.1-alpha.** Kept for history.

### Added
- First public build: Bevy 0.16 native client driving the TypeScript sim
  sidecar over the binary wire protocol; Mirror's Edge white art direction;
  procedural + real-city (NYC) maps; transit building, routes, subway view.
