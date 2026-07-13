# MetroForge — Known Issues

> The one file a new contributor reads first. State as of **v0.7.0-alpha** /
> **v0.8 geology in progress** on `integ/v1-roadmap` (2026-07-13).
>
> This is a living crack-map, not a changelog. **RULE: verify each entry against the GitHub
> issue state BEFORE starting work** — a stale version of this doc caused duplicate fixes of
> already-closed bugs. When you close an issue, update this file in the same PR.

Severity key: **P1** very visible · **P2** cosmetic/edge · **QA** needs real-hardware verification · **OWNER** owner action item.

## Open issues

| Sev | Issue | Where to start |
|-----|-------|----------------|
| **P1** | [metroforge#45](https://github.com/Egg3901/metroforge/issues/45) — warm-process determinism: process-global geometry caches leak across games | `sim/` long-lived sidecar / geometry caches |
| **P2** | [native#143](https://github.com/Egg3901/metroforge-native/issues/143) — grade-separation legibility polish (deck/skirt/shadow tones at distance) | `mf-render` roads / grade decks |
| **P2** | [native#144](https://github.com/Egg3901/metroforge-native/issues/144) — bridge model polish (tower-style consistency, puff scene split) | `tools/blender/`, `mf-render/bridges.rs` |
| **P2** | [native#16](https://github.com/Egg3901/metroforge-native/issues/16) — buildings: `building:part` ingestion, LOD/async bake, street-camera collision | `mf-render/buildings.rs`, city pipeline |
| **P2** | [metroforge#1](https://github.com/Egg3901/metroforge/issues/1) / [#2](https://github.com/Egg3901/metroforge/issues/2) — generation clipping; main/local road intersections | pipeline geometry (post-1.0) |
| **QA** | [native#108](https://github.com/Egg3901/metroforge-native/issues/108) — Win/mac builds never tested on real hardware | owner hardware session |
| **OWNER** | [native#109](https://github.com/Egg3901/metroforge-native/issues/109) — unsigned binaries (SmartScreen/Gatekeeper) | Authenticode + Apple notarization, start certs by v0.9 |

Roadmap context: ship plan v3 — [native#25](https://github.com/Egg3901/metroforge-native/issues/25). v0.6 Solid Ground ([native#140](https://github.com/Egg3901/metroforge-native/issues/140)) shipped (monorepo `sim/`, real DEM msgType=7, determinism harness). v0.7 weather is on the wire + render lane. v0.8 geology (`trackCost.breakdown`, `strataProbe`) is landing on `integ/v1-roadmap`.

## Resolved since the v0.5.7 crack-map (2026-07-13 sweep)

#40 noon shadow cascade seam · #141 black tower at all hours · #102 Potato/Low in-city void (+ `scripts/tier-smoke.sh` CI gate) · #103 outline scribble (#115) · #104/#105 l10n literals · #106 stale crash marker · #92 double crash reporters · #107 CI Windows check · #110 shared target dir · #48 camera/snapping · #27 traffic overlay · #112/#113 water threshold + building clipping (rendering side) · #140 v0.6 Solid Ground · #142 sim vitest alias · metroforge#23 fresh-process determinism · metroforge#39 96×96 field cap · metroforge#40 procedural-only relief (real DEM + msgType=7) · metroforge#41 height-join.

## Hazards for the next agent (workflow, not code)

- **Stale-doc hazard**: this file rots fast — trust GitHub issue state over this doc, and update this doc in the same PR that closes an issue.
- **Phantom-binary hazard**: after any build, check the exe mtime before trusting a run — a stale `target/release/metroforge` can make a "fix" look verified when the new code never compiled.
- **Shared-cache doctrine**: concurrent agents sharing a `CARGO_TARGET_DIR` cause ghost type errors. Use a private target dir per worktree.
- **Shared checkout**: do multi-file work in a git worktree; stage explicit files, never `git add .`.
- **Release gate**: verifier + `tier-smoke.sh` per-tier in-city colour floors; title-only smokes are insufficient (#102).
