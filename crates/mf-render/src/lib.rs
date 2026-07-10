//! `mf-render` — STUB. The real 3D renderer (spec §3.3: terrain/roads/
//! buildings/transit/vehicles/agents/day-night/subway-view/palette) is owned
//! by a separate agent working in parallel. This crate exists only so the
//! workspace compiles and `mf-game` has a concrete `MfRenderPlugin` to add
//! to its `App` today.
//!
//! ## For the mf-render implementer
//!
//! Read `crates/mf-state/src/lib.rs` first — that crate (already
//! implemented, not a stub) holds every cross-crate Resource you need:
//!
//! - `mf_state::CurrentCity` — `StaticCityJson` + the 0-3 mask byte arrays,
//!   with `.masks_complete()` to know when it's safe to bake.
//! - `mf_state::LatestFields` — latest `Fields` binary frame (terrain/pop/
//!   jobs/landValue/water/parks), versioned via `Fields.version`.
//! - `mf_state::LatestUi` — latest `UiState` (stations/tracks/routes/etc.),
//!   2 Hz.
//! - `mf_state::LatestFrame` — latest `FrameSnapshot` (vehicles/agents/color
//!   table), 20 Hz.
//! - `mf_state::QualityTier` (+ `.knobs()`) — the spec §4 knob table as
//!   plain data (render scale, MSAA, shadow map size, material style,
//!   building draw distance, agent cap, vehicle mesh tier, terrain subdiv
//!   divisor, day/night on/off). Map these onto real `bevy_render`/`bevy_pbr`
//!   types in your plugins.
//! - `mf_state::SubwayView { active, t }` — toggle state + eased 0..1
//!   progress; `mf-game`'s `input.rs` flips `active` on Tab, nobody else
//!   advances `t` yet — your `subway.rs` system should call `.step(dt)` on
//!   it (see the doc comment on `SubwayView::step`) since you're the one
//!   with per-frame `Time` access and the actual animation to drive.
//! - `mf_state::HeightAt(Box<dyn Fn(f32, f32) -> f32 + Send + Sync>)` —
//!   currently a flat-ground placeholder (`|_, _| 0.0`); your `terrain.rs`
//!   should replace `app.world_mut().resource_mut::<HeightAt>().0` with a
//!   real bilinear sampler once fields are loaded, so roads/buildings/
//!   transit/vehicles/agents can all call `HeightAt::sample`.
//!
//! All of the above are populated by `mf_state::MfStatePlugin`'s internal
//! system, which drains `mf-net`'s `Events<FromSimMsg>` — you don't need to
//! touch `mf-net` or `mf-state`'s internals, just add `MfStatePlugin` (or
//! rely on `mf-game` having already added it) and read the resources.
//!
//! Palette constants (art-direction.md, BINDING) belong in
//! `crates/mf-render/src/palette.rs` per spec §3.3 — not yet created here;
//! add it alongside your other layer modules (`terrain.rs`, `roads.rs`,
//! `buildings.rs`, `transit.rs`, `vehicles.rs`, `agents.rs`, `daynight.rs`,
//! `subway.rs`).

use bevy_app::{App, Plugin};

/// Composes the per-layer sub-plugins described in spec §3.3. Currently a
/// no-op so the workspace/binary compile and run headlessly; see the module
/// doc comment above for what to build here.
pub struct MfRenderPlugin;

impl Plugin for MfRenderPlugin {
    fn build(&self, _app: &mut App) {
        // Intentionally empty. mf-game depends on mf-state directly for any
        // v1 HUD readouts it needs, so leaving this empty doesn't block the
        // game shell from booting and running headlessly.
    }
}
