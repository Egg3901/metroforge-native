# Quality-tier truth pass

Owner feedback: the quality modes "look like shit". This is the tier-by-tier
visual audit and the rebalance that came out of it.

Screenshots are the standard verify-harness elevated in-city view over the NYC
dense core (`MF_QUALITY=<tier> MF_AUTOSTART=nyc MF_VERIFY_NETWORK=1`), captured
on this box's lavapipe software Vulkan at 1600x1000.

## What was wrong

| Tier | Before | Root cause |
|---|---|---|
| Potato | flat white mush, no edge definition | unlit near-white material (top vs side ~7% contrast) + no shadows + no outlines. Fog was already working (horizon fades to sky) — verified, not the problem. |
| Low | visually identical to Potato | same unlit white massing; day/night, trees and a longer draw distance don't help readability without shadows or outlines. "Potato with a longer draw distance." |
| Medium | good | lit `StandardMaterial` + real cascade shadows give depth. |
| High | best | shadows + lit + cel outlines on the dense core + unlimited draw distance. |

## The fix

The cel outline (inverted-hull, **one** dense-center chunk draw — bounded cost
regardless of tier, previously High-only) is the single biggest readability
lever for the unlit tiers. Turning it on for **every** tier turns "white mush"
into the intended "white cel blocks with crisp black edges" north star. Even
Potato (agent cap 0, no trees, no shadows) has the budget for one chunk.

Knob changes (`crates/mf-state/src/quality.rs`):

- **`outline_enabled`** new knob, `true` on all four tiers (was implicitly
  High-only). Headline fix for Potato + Low.
- Low **MSAA** left at 1 (off): 2x was tried as cheap edge-AA but WebGPU only
  guarantees sample counts `[1, 4]` for the depth target, and 2x panics on
  lavapipe / can be unsupported on the very integrated GPUs this tier targets.
  4x is too costly for the weak-GPU tier, so it stays off.

Also fixed the per-frame visibility-write churn in `buildings.rs` /
`trees.rs` draw-distance culling (unconditional `*vis =` every frame forced
Bevy to re-propagate/extract visibility even for a static camera; now writes
only on change).

## Before / after

Potato: `potato-city-before.png` → `potato-city-after.png`
Low: `low-city-before.png` → `low-city-after.png`
Medium: `medium-city-before.png` → `medium-city-after.png`
High: `high-city-before.png` → `high-city-after.png` (unchanged; already outlined)
