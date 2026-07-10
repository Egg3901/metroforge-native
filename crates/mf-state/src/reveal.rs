use bevy_ecs::prelude::*;
use bevy_math::Vec2;

/// Cursor/close-camera "reveal" state (issue #18): the world-space hole the
/// renderer's building material dissolves toward, so players can see
/// streets under dense building fabric near the mouse (and, when the camera
/// has dollied in close, near the camera target too — see `mf-game`'s
/// `reveal_input.rs` for how the two triggers are folded into one hole).
///
/// Mirrors the `HeightAt`/`SubwayView` split already used by this crate:
/// `mf-state` only owns the *value*, `mf-game`'s `reveal_input.rs` computes
/// it every frame (it has the cursor ray, `CameraRig`, and `SubwayView`
/// gating available), and `mf-render`'s `reveal.rs` copies it into the
/// shared building material's shader uniform.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct RevealState {
    /// World-space (X, Z) hole center.
    pub center: Vec2,
    /// Fully-dissolved radius (meters) — inside this, buildings are 100%
    /// discarded regardless of `strength` reaching 1.0.
    pub inner: f32,
    /// Fully-solid radius (meters) — outside this, buildings are never
    /// discarded.
    pub outer: f32,
    /// 0..1 eased master gain on the whole effect: 0 = it never fires (flat
    /// solid city, as if the feature were off), 1 = the full `inner`/`outer`
    /// falloff applies. Separate from `inner`/`outer` so the effect can
    /// fade in/out smoothly without animating the radii themselves (see
    /// `reveal_input_system`'s easing).
    pub strength: f32,
}

impl Default for RevealState {
    /// Matches the "no effect" resting state: a hole with `strength == 0`
    /// discards nothing (see `mf-render/src/reveal.rs`'s shader doc), so the
    /// exact `center`/`inner`/`outer` values here are inert until the game
    /// systems start driving `strength` upward.
    fn default() -> Self {
        RevealState {
            center: Vec2::ZERO,
            inner: 60.0,
            outer: 180.0,
            strength: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_zero_strength() {
        // The resting state must be inert: no reveal system has run yet
        // (e.g. before the first InGame frame), and the shader must treat
        // that as "no effect" — strength 0 is what guarantees that (see
        // `mf-render`'s reveal.wgsl: `mix(1.0, t_geom, strength)`).
        assert_eq!(RevealState::default().strength, 0.0);
    }
}
