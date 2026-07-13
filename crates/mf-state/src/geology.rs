//! Client-side geology context (v0.8 Underground, diorama cutaway).
//!
//! The subsurface strata model lives in the sim (`sim/src/core/geology.ts`) as
//! a PURE, deterministic O(1) function of `(seed, city geology profile,
//! surface elevation)` — there is no stored 3D grid on the wire. Rather than
//! round-trip a `strataProbe` query per perimeter sample (64/side × 4 sides =
//! 256 requests just to draw the diorama edge, redrawn on every city load),
//! `mf-render`'s `diorama.rs` MIRRORS that small pure function client-side and
//! reconstructs the bands directly from data the client already holds:
//!
//! - the **seed** and **city key** the client itself chose at `init`
//!   (`mf-game`'s `PendingInit`), published here, and
//! - the real-elevation channel (`CurrentCity::elevation`), already present.
//!
//! This resource is that published `(seed, city_key)`. `strataProbe` stays on
//! the wire for one-off point inspection UIs; the ambient diorama edge and the
//! cutaway cut-face are computed locally so they cost zero network traffic and
//! rebuild only when the city (hence this context) changes.
//!
//! The mirrored numeric profile tables live in `mf-render` next to the mesh
//! builder that consumes them; only the identifying `(seed, city_key)` needs
//! to cross the crate boundary, which is all this resource carries.

use bevy_ecs::prelude::*;

/// Seed + city key for the currently-loaded world, published by `mf-game`
/// when it stages an `init` / `loadSave`. `mf-render` reads this (plus
/// `CurrentCity::elevation`) to reconstruct the subsurface strata for the
/// diorama slab edge and the cutaway cut-face without any `strataProbe`
/// round-trips.
///
/// `city_key` is the `preset_key` (e.g. `"nyc"`, `"boston"`); `None` /
/// unrecognized falls back to the generic temperate geology profile, exactly
/// like the sim's `geologyProfile`. `seed` mirrors `InitPayload.seed`.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct GeologyContext {
    /// World RNG seed (mirrors `InitPayload.seed`); drives the deterministic
    /// band-thickness / water-table value noise.
    pub seed: u64,
    /// City geology-profile key (`preset_key`); `None` = generic profile.
    pub city_key: Option<String>,
}

impl GeologyContext {
    /// Update in place, returning `true` if anything actually changed (so
    /// callers can avoid spuriously tripping Bevy change-detection every
    /// frame — the diorama rebuild is keyed partly on this).
    pub fn set(&mut self, seed: u64, city_key: Option<String>) -> bool {
        if self.seed == seed && self.city_key == city_key {
            return false;
        }
        self.seed = seed;
        self.city_key = city_key;
        true
    }
}

/// Cutaway clip-plane state (v0.8 Underground diorama cutaway). When
/// `active`, the renderer slices the diorama slab along a vertical plane
/// parallel to the camera and passing through the map centre, revealing the
/// banded strata cut-face inward (the same treatment as the permanent slab
/// edge). Toggled from the HUD geology button or a deep-zoom trigger; owned
/// here in `mf-state` (like [`crate::SubwayView`]) so both `mf-game` input and
/// `mf-render` can read/write it, with the per-frame animation driven by
/// whichever system has frame-delta access.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct ClipPlane {
    /// Player's target (true = cutaway showing).
    pub active: bool,
    /// Eased 0..1 progress toward `active`.
    pub t: f32,
    /// Unit plane normal in the world XZ plane (`.x`, `.z`); the plane passes
    /// through the map origin. Geometry on the `+normal` side is clipped away
    /// so the cut-face is revealed. Refreshed from the camera each frame so
    /// the cut stays parallel to the view.
    pub normal_x: f32,
    /// Z component of the world-XZ plane normal (see [`Self::normal_x`]).
    pub normal_z: f32,
}

impl Default for ClipPlane {
    fn default() -> Self {
        ClipPlane {
            active: false,
            t: 0.0,
            // Default cut faces along +X (camera looking down -X reveals it);
            // refreshed live once the cutaway is engaged.
            normal_x: 1.0,
            normal_z: 0.0,
        }
    }
}

/// Full cutaway transition duration (matches the subway view's ~400ms feel).
pub const CLIP_TRANSITION_SECS: f32 = 0.4;

impl ClipPlane {
    /// Flip the cutaway target; progress still eases via [`Self::step`].
    pub fn toggle(&mut self) {
        self.active = !self.active;
    }

    /// Advance `t` toward `active as 0/1` over [`CLIP_TRANSITION_SECS`].
    pub fn step(&mut self, dt_secs: f32) {
        let target = if self.active { 1.0 } else { 0.0 };
        let rate = 1.0 / CLIP_TRANSITION_SECS;
        let max_delta = rate * dt_secs;
        if self.t < target {
            self.t = (self.t + max_delta).min(target);
        } else if self.t > target {
            self.t = (self.t - max_delta).max(target);
        }
    }

    /// True when the cutaway is doing anything (visible or mid-transition),
    /// i.e. the clip should be applied this frame.
    pub fn engaged(&self) -> bool {
        self.active || self.t > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geology_context_set_reports_change() {
        let mut g = GeologyContext::default();
        assert!(g.set(42, Some("nyc".into())));
        assert!(!g.set(42, Some("nyc".into())));
        assert!(g.set(43, Some("nyc".into())));
        assert!(g.set(43, None));
    }

    #[test]
    fn clip_plane_eases_and_reports_engaged() {
        let mut c = ClipPlane::default();
        assert!(!c.engaged());
        c.toggle();
        assert!(c.engaged());
        for _ in 0..40 {
            c.step(CLIP_TRANSITION_SECS / 40.0);
        }
        assert!((c.t - 1.0).abs() < 1e-4);
        c.toggle();
        for _ in 0..40 {
            c.step(CLIP_TRANSITION_SECS / 40.0);
        }
        assert!(c.t.abs() < 1e-4);
        assert!(!c.engaged());
    }
}
