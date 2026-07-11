# MetroForge — Known Issues

> The one file a new contributor reads first. State as of **v0.5.1-alpha** (2026-07-11),
> after ~35 PRs merged across `metroforge-native` and `metroforge` in ~24 hours by parallel agents.
>
> This is a living crack-map, not a changelog. Each entry links its tracking issue, gives a
> severity, and names a suspected root cause and the file(s) to start in. **Fix nothing based on
> this doc alone — read the issue, then verify against current code** (parallel agents move fast).

Severity key: **P0** unplayable/data-loss · **P1** very visible / ships broken · **P2** cosmetic/edge · **DEBT** no runtime break but pays down later · **QA** needs real-hardware verification · **OWNER** owner action item.

---

## Rendering / visual

| # | Sev | Issue | Where to start |
|---|-----|-------|----------------|
| Potato/Low in-city renders as a black void | **P0** | [native#102](https://github.com/Egg3901/metroforge-native/issues/102) | `mf-render` atmosphere/fog from #83; `terrain_material.rs`; `quality_boot.rs` |
| Distant cel outlines collapse into a black scribble clump | **P1** | [native#103](https://github.com/Egg3901/metroforge-native/issues/103) | `crates/mf-render/src/outline.rs` (no distance fade) |

- **#102** is the headline regression. Suspect: #83's atmosphere rework ported a shader uniform wrong on the unlit low-tier path, and/or the #83 custom `TerrainMaterial` fails to specialize on the software `llvmpipe` low path (Potato/Low leave `cloud.z==0`). The world (buildings + terrain) draws as void on exactly Potato/Low; High/Ultra are fine. #95's in-city smoke may still miss it on low tiers. **No open striker `fix/tier-visual-regression` PR existed at audit time** (open PRs were only #66, #73) — if one appears, link it on #102 instead of a duplicate fix.
- **#103**: `outline.rs` inverted-hull uses a fixed `OUTLINE_PUSH_M = 0.9` with no far-distance alpha/width falloff, so distant edges overlap into a smear. Fade in the outline shader.

## Terrain generation (end-to-end)

Terrain flows: **metroforge sidecar pipeline** (Overpass geometry → water/park mask bake → 96×96 field with procedural relief) **→ native rendering** (`terrain.rs` bilinear sample + water threshold + road grading + #83 `TerrainMaterial`). Root causes filed in the repo where they live; cross-repo pairs link each other.

| # | Sev | Issue | Repo / root cause |
|---|-----|-------|-------------------|
| 96×96 field grid (~125 m/cell) hard-caps shoreline + relief fidelity | **P1** | [metroforge#39](https://github.com/Egg3901/metroforge/issues/39) | **data/pipeline** — `src/core/constants.ts FIELD_W/H=96` |
| Real-city relief is procedural fbm, no real DEM/elevation | **P1** | [metroforge#40](https://github.com/Egg3901/metroforge/issues/40) | **data/pipeline** — `generator.ts` fbm relief |
| Building heights single-source: height-join only run for Cleveland+NYC | **DEBT** | [metroforge#41](https://github.com/Egg3901/metroforge/issues/41) | **data** — only `cleveland/nyc.buildings.json` exist |
| Water is a hard 0.5 threshold on the coarse field (shoreline stair-step / inland smear) | **P1** | [native#112](https://github.com/Egg3901/metroforge-native/issues/112) ← blocked on metroforge#39 | **rendering** — `terrain.rs sample()` binary cliff, no beach band |
| Building bases clip/float on relief; `MAX_RELIEF` capped at 90 m as a band-aid | **P1** | [native#113](https://github.com/Egg3901/metroforge-native/issues/113) | **rendering** — `terrain.rs` (flat prisms on graded field) |

Notes:
- The importer already area-samples the ~19 m OSM water mask (7×7 per field cell, majority vote) to fit the coast to the 125 m grid "as closely as a 125 m grid allows" — a mitigation, not a fix. True shoreline fidelity needs a higher-res field or a decoupled hi-res land/water field (the `water_mask` StaticMask is ~640 res and already ships separately from the sim field).
- No elevation raster is fetched anywhere; hilly cities (Seattle/SF) render essentially flat. Native then caps `MAX_RELIEF` to 90 m (down from spec 200–400 m) because more relief buried buildings on slopes — so the two issues compound.
- Only Cleveland and NYC have height-joined `*.buildings.json`; the other 10 cities (Boston, Seattle, Atlanta, Chicago, DC, LA, Philly, SF) rely on sparse OSM height tags. Boston/Seattle/Atlanta are the flagged "blocked on MS height-join" set — run `height-join.ts --source=ms` per city.
- The Potato/Low void (#102) may be `TerrainMaterial`-related — see the terrain note on that issue.

## Localization / UI design system

| # | Sev | Issue | Where |
|---|-----|-------|-------|
| `city_select.rs` + `routes_panel.rs`: unkeyed UI literals + stale design tokens | **DEBT** | [native#104](https://github.com/Egg3901/metroforge-native/issues/104) | mixed raw `RichText`/`Color32` vs `ds::` helpers |
| Reconnect/sim-error (#82) + multi-select hint (#87) strings are unkeyed literals | **DEBT** | [native#105](https://github.com/Egg3901/metroforge-native/issues/105) | `mf-net/reconnect.rs`, `sidecar.rs`, `crash.rs`, `routes_panel.rs` |

Both continue the l10n gap prepped in #97: hardcoded English literals never reach the string table. #104 also flags design-token drift from #79's custom-painted widgets (`design_system.rs`).

## Crash reporting / boot

| # | Sev | Issue | Where |
|---|-----|-------|-------|
| Stale `SHOW_NOTICE` marker makes next boot / CI harness show the crash dialog | **P1** | [native#106](https://github.com/Egg3901/metroforge-native/issues/106) | `crash.rs` (`NOTICE_MARKER`, written ~L170, cleared ~L313) |
| Double crash reporters: crash (#80) should supersede crash_report (#72) | **DEBT** | [native#92](https://github.com/Egg3901/metroforge-native/issues/92) | `main.rs` installs both hooks; `crash.rs` + `crash_report.rs` both write |

- **#106**: the marker file persists in the OS data dir; any later process against the same dir (CI smoke #95, MF_SOAK harness, local dev) inherits it and boots into the crash notice instead of the game. Session-scope or CI-clear the marker.
- **#92**: confirmed both `crash_report::install_panic_hook` (#72) and `crash.rs::write_crash_report` (#80) fire on every panic, writing overlapping files. #80 should win.

## Build / CI / platform

| # | Sev | Issue | Where |
|---|-----|-------|-------|
| CI never cross-compiles Windows (add `cargo xwin check`) | **P1 / DEBT** | [native#107](https://github.com/Egg3901/metroforge-native/issues/107) | `.github/workflows/ci.yml` is Linux-only |
| Windows citizenship (#72) + macOS dmg fix (#91) never tested on real hardware | **QA** | [native#108](https://github.com/Egg3901/metroforge-native/issues/108) | cross-compiled only; needs real Win/mac smoke |
| Shipped binaries are unsigned (SmartScreen/Gatekeeper friction) | **OWNER** | [native#109](https://github.com/Egg3901/metroforge-native/issues/109) | Authenticode (Win) + Developer ID + notarize (mac) |
| Shared `CARGO_TARGET_DIR` fingerprint corruption under concurrent agents | **DEBT** | [native#110](https://github.com/Egg3901/metroforge-native/issues/110) | per-agent target dirs or `sccache`; auto-deploy timer clobbers too |

- **#107**: `ci.yml` only builds/clippies the host Linux target; the Windows `#[cfg(windows)]` paths compile only in `release.yml` on tag. That is how the `JobHandle` Send/Sync break reached master (fixed in #100). Add a fast `cargo xwin check --target x86_64-pc-windows-msvc` on PRs — the xwin cache already exists in `release.yml`.
- **#110**: parallel agent worktrees (`.claude/worktrees/agent-*`) plus the `auto-deploy.timer` (2-min) can build into a shared target dir → stale-type "ghost" compile errors that vanish on clean rebuild. Give each agent its own `CARGO_TARGET_DIR` or adopt sccache.

## Older / pre-existing (context, not from this sprint)

- [native#48](https://github.com/Egg3901/metroforge-native/issues/48) — camera controls inverted + placement/road-snapping broken.
- [native#40](https://github.com/Egg3901/metroforge-native/issues/40) — hard vertical lighting seam at noon (shadow cascade boundary).
- [native#27](https://github.com/Egg3901/metroforge-native/issues/27) — traffic overlay should paint congestion on streets, not a heatmap.
- [metroforge#23](https://github.com/Egg3901/metroforge/issues/23) — determinism test failing (same seed ≠ identical state hash).
- [metroforge#2](https://github.com/Egg3901/metroforge/issues/2) / [#1](https://github.com/Egg3901/metroforge/issues/1) — generation clipping (streets/buildings overrun blocks); main roads don't intersect local streets.

---

## Hazards for the next agent (workflow, not code)

- **Phantom-binary hazard**: after any build, check the exe mtime before trusting a run — a stale `target/release/metroforge` can make a "fix" look verified when the new code never compiled.
- **Shared-cache doctrine**: concurrent agents share a target dir → ghost type errors (see #110). Rebuild clean if an error doesn't match the source on disk.
- **Shared checkout**: do multi-file work in a git worktree; stage explicit files, never `git add .` (the tree carries stray untracked dirs like `.claude/`, `.worktree-storefront/`).
- **Release gate**: the verifier now includes an in-city autostart smoke (#95) on top of the title-only gate — but it may not cover low tiers (see #102).
