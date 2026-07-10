use bevy_ecs::prelude::*;

/// Subway-view toggle state (Tab / HUD button, art-direction §7). `active`
/// is the target the player last chose; `t` is the eased 0..1 progress
/// toward it (0 = normal view, 1 = fully in subway view). `mf-state` only
/// owns the *state*; whichever system has frame-delta-time available
/// (typically `mf-render`'s `subway.rs`, which is what actually animates
/// building-chunk squash / stripe alpha / the vignette) calls [`SubwayView::step`]
/// each frame to advance `t` — `mf-game`'s `input.rs` only flips `active`.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct SubwayView {
    pub active: bool,
    pub t: f32,
}

impl Default for SubwayView {
    fn default() -> Self {
        SubwayView {
            active: false,
            t: 0.0,
        }
    }
}

/// Full transition duration (art-direction §7: "~400ms ease").
pub const SUBWAY_TRANSITION_SECS: f32 = 0.4;

impl SubwayView {
    pub fn toggle(&mut self) {
        self.active = !self.active;
    }

    /// Advance `t` toward `active as 0/1` at a constant rate over
    /// [`SUBWAY_TRANSITION_SECS`]. Idempotent to call from multiple systems
    /// in the same frame (clamped), though in practice only one system
    /// should own the call per app.
    pub fn step(&mut self, dt_secs: f32) {
        let target = if self.active { 1.0 } else { 0.0 };
        let rate = 1.0 / SUBWAY_TRANSITION_SECS;
        let max_delta = rate * dt_secs;
        if self.t < target {
            self.t = (self.t + max_delta).min(target);
        } else if self.t > target {
            self.t = (self.t - max_delta).max(target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_and_step_reach_target() {
        let mut sv = SubwayView::default();
        sv.toggle();
        assert!(sv.active);
        // Stepping for the full transition duration should land exactly at 1.0.
        for _ in 0..40 {
            sv.step(SUBWAY_TRANSITION_SECS / 40.0);
        }
        assert!((sv.t - 1.0).abs() < 1e-4);

        sv.toggle();
        assert!(!sv.active);
        for _ in 0..40 {
            sv.step(SUBWAY_TRANSITION_SECS / 40.0);
        }
        assert!(sv.t.abs() < 1e-4);
    }
}
