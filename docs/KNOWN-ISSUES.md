# MetroForge — Known Issues

> The one file a new contributor reads first. State as of **v0.5.7-alpha** (2026-07-12).
>
> This is a living crack-map, not a changelog. **RULE: verify each entry against the GitHub
> issue state BEFORE starting work** — a stale version of this doc caused duplicate fixes of
> already-closed bugs. When you close an issue, update this file in the same PR.

Severity key: **P1** very visible · **P2** cosmetic/edge · **QA** needs real-hardware verification · **OWNER** owner action item.

## Open issues

| Sev | Issue | Where to start |
|-----|-------|----------------|
| **P1** | [native#40](https://github.com/Egg3901/metroforge-native/issues/40) — hard vertical lighting seam at noon (shadow cascade boundary) | `mf-render` daynight/cascade config |
| **P1** | [metroforge#39](https://github.com/Egg3901/metroforge/issues/39) — 96×96 field grid hard-caps shoreline/relief fidelity | pipeline `src/core/constants.ts FIELD_W/H` — v0.6 Solid Ground scope |
| **P1** | [metroforge#40](https://github.com/Egg3901/metroforge/issues/40) — relief is procedural fbm, no real DEM | pipeline `generator.ts` — v0.6 Solid Ground scope (SRTM ingest) |
| **P2** | [metroforge#1](https://github.com/Egg3901/metroforge/issues/1) / [#2](https://github.com/Egg3901/metroforge/issues/2) — generation clipping; main/local road intersections | pipeline geometry |
| **P1** | [metroforge#45](https://github.com/Egg3901/metroforge/issues/45) — warm-process determinism: process-global geometry caches leak across games | fix on `fix/23-determinism`, merged to local `integ/v1-roadmap`, pending push |
| **QA** | [native#108](https://github.com/Egg3901/metroforge-native/issues/108) — Win/mac builds never tested on real hardware | owner hardware session |
| **OWNER** | [native#109](https://github.com/Egg3901/metroforge-native/issues/109) — unsigned binaries (SmartScreen/Gatekeeper) | Authenticode + Apple notarization, start certs by v0.9 |

Roadmap context: active plan is ship plan v3 (ops-knowledge `metroforge-ship-plan-v1` v3); milestone tracking in [native#25](https://github.com/Egg3901/metroforge-native/issues/25) and [native#140](https://github.com/Egg3901/metroforge-native/issues/140) (v0.6).

## Resolved since the v0.5.1 crack-map (2026-07-11 sweep)

#102 Potato/Low black void · #103 outline scribble (#115) · #104/#105 l10n literals · #106 stale crash marker · #92 double crash reporters · #107 CI Windows check · #110 shared target dir · #48 camera/snapping · #27 traffic overlay · #112/#113 water threshold + building clipping (rendering side) · metroforge#23 (fresh-process determinism) · metroforge#41 height-join.

## Hazards for the next agent (workflow, not code)

- **Stale-doc hazard**: this file rots fast — trust GitHub issue state over this doc, and update this doc in the same PR that closes an issue.
- **Phantom-binary hazard**: after any build, check the exe mtime before trusting a run — a stale `target/release/metroforge` can make a "fix" look verified when the new code never compiled.
- **Shared-cache doctrine**: concurrent agents sharing a `CARGO_TARGET_DIR` cause ghost type errors. Use a private target dir per worktree.
- **Shared checkout**: do multi-file work in a git worktree; stage explicit files, never `git add .`.
- **Release gate**: verifier includes title + in-city autostart smokes; per-tier screenshot gates are v0.6 scope (#140).
