# MOONSHOT: MetroForge Native -> 1.0.0 (all-Rust sim, delete the sidecar)

You are a senior Rust/systems engineer. Drive MetroForge Native to a **1.0.0 release candidate** by finishing the all-Rust sim consolidation. This is a large, multi-part task; work methodically, gate every step, and VERIFY in the real client (never trust "it compiled").

## Repo / branch
- Repo: metroforge-native. Base branch: `feat/rust-sim-p0` (already contains P0-P4 + OSM real-city path; pushed to origin). You are in an isolated worktree based on it. Do your work on this branch (or a child branch you merge back into it); push when green. NEVER push to `master`.
- The native Rust sim lives in `crates/mf-sim` (pure, bevy-free). The client is `crates/{mf-net,mf-state,mf-render,mf-game}`. The TypeScript reference sim (what you are replacing) is under `sim/src/core`; the wire serializer + embedded transport are in `crates/mf-net` (`host.rs`, `embedded.rs`, `cities.rs`). Read `crates/mf-sim/PORT.md` — it is the running status/checklist. Read `MOONSHOT_1.0.md` (this file) and, if present, any `metroforge-native-sim-consolidation-decision` context.

## Determinism contract (NON-NEGOTIABLE)
NEW RUST BASELINE: idiomatic Rust, do NOT chase JS f64 bit-parity. RNG is already bit-exact (two streams `rng_state`/`ops_rng_state` — keep them separate and used by the right systems). Every new system must be: seeded RNG only, no wall-clock, `BTreeMap` (not `HashMap`) in any hashed/ordered path. Prove determinism with run-twice-identical-`state_hash` tests. Keep the hashed-vs-transient split in `types.rs`/`save.rs` intact.

## Remaining work to 1.0 (do ALL of it, in roughly this order)
1. **StaticBuildings real footprints**: emit the per-building footprint vectors (`sim/src/data/cities/*.buildings.json`, large) as the msgType=5 static payload from `mf-net/host.rs`, matching what `sim/src/host/*` sends, so real cities render true footprints (not just mask-derived massing). Handle data delivery like the existing OSM path (embed a couple, load rest from `$MF_CITY_DATA`/in-repo dir).
2. **Scenario catalog + `evaluateScenarioDay`**: port the data-driven scenario/progression content (`sim/src/core/scenario/*`, catalog + win/lose trees + mid-run events) so campaign/progression reaches parity. Wire `TickEvents.won`/fail conditions.
3. **replay.rs + reverse command bridge**: port `sim/src/core/replay.ts`; add the `SimCommand -> wire::Command` reverse bridge so `requestReplay` returns a real command log; ensure deterministic re-sim validates (this backs leaderboard anti-cheat).
4. **Saves**: enable the `mf-sim` `serde` feature path end-to-end so requestSave/loadSave work through `EmbeddedTransport` (round-trip preserves `state_hash`).
5. **Agents particle pool + cohortDemand HUD + traffic/demand/heatmap overlays**: port the remaining `host/` transient outputs so the embedded UiState/FrameSnapshot reach visual parity with the sidecar.
6. **CUTOVER (do last, only after 1-5 are green and you have verified an in-client embedded playthrough)**: flip the default `SimBackend` to `embedded`; delete the Bun sidecar (`sim/sidecar`), `dist-sidecar/`, and `WsTransport`/reconnect-to-sidecar code; re-point anything that spawned the sidecar. Keep the game fully working on the Rust sim alone. Update docs (`PORT.md`, `docs/`), and remove now-dead sidecar build steps from CI/workflows.
7. **Release**: bump versions to `1.0.0`, ensure the release workflow is intact, and prepare release notes (GitHub-generated style; owner rejects CHANGELOG.md; NO em/en dashes in any player-facing text). Push the branch. Do NOT push a `v1.0.0` tag yourself — leave that final action for owner sign-off; instead report that everything is green and tag-ready.

## Gates (run after every meaningful step; ALL must be green before you move on)
- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`  (includes determinism + parity tests; add tests for everything you port)
- **In-client verify** for anything visual: `mkdir -p verify && MF_SIM=embedded MF_AUTOSTART=nyc MF_VERIFY_DIR=verify MF_VERIFY_NETWORK=1 timeout 420 xvfb-run -a target/release/metroforge` (build `-p mf-game --release` first; lavapipe software Vulkan ~4min). LOOK at `verify/*.png` and confirm the feature actually renders. Do not claim visual success without inspecting the image.

## Guardrails
- Worktrees only; stage explicit files (never `git add .` blindly in a dirty tree).
- Keep `mf-sim` bevy-free and (except behind its `serde` feature) serde-free; wire/serde glue lives in `mf-net`.
- No em/en dashes in player-facing strings. Match existing code style.
- If you hit a genuinely ambiguous product decision, make the reasonable choice, document it in `PORT.md`, and keep going — do not stall.

## Report at the end
A concise summary: what you completed of 1-7, the final gate results, the in-client verify outcome (describe the screenshots), what (if anything) remains, and confirmation the tree is `v1.0.0` tag-ready on the pushed branch.
