//! Day/night cycle (spec §3.3 `daynight.rs`, art-direction §6):
//! `hour = (tick % TICKS_PER_DAY) / TICKS_PER_DAY * 24`. Day = white city
//! under warm sun; night = deep blue-black sky, buildings dim, but transit
//! (route stripes/vehicles/station rings) stays vivid/emissive — "transit
//! glows at night". Disabled on potato (fixed noon, per spec §4).
//!
//! Level 2 shadow work on top of the smoothed-hour fix:
//! - sun elevation published for atmosphere / other consumers
//! - cascade bounds adapt to camera height (tight near when zoomed in)
//! - shadow bias scales with sun elevation (grazing light needs more)
//! - High tier uses 4 cascades; Medium keeps 3
//! - Gaussian shadow filtering on the camera
//! - soft night shadow fade (hysteresis) instead of a hard cut

use std::f32::consts::PI;

use bevy::pbr::{
    CascadeShadowConfig, CascadeShadowConfigBuilder, DistanceFog, ShadowFilteringMethod,
};
use bevy::prelude::*;

use mf_state::{AttractLighting, LatestUi, QualityTier, Theme};

use crate::palette;

/// Attract-mode golden hour (local solar time). Low sun → long shadows on
/// Medium+; warm twilight tint on clear/ambient/sun color. Chosen so
/// `night_factor` sits in the dusk band without extinguishing shadows.
const ATTRACT_GOLDEN_HOUR: f32 = 19.0;

/// Mirrors `metroforge/src/core/constants.ts`'s `TICKS_PER_DAY` (spec: "each
/// tick is a 50ms host step; `TICKS_PER_DAY = 1200`").
const TICKS_PER_DAY: u64 = 1200;

/// Exponential chase rate for displayed hour → sim target. Settles ~95% in
/// ~0.35s, bridging the ~0.5s UiState gap without lagging dusk/dawn.
const HOUR_SMOOTH_RATE: f32 = 8.5;

/// Night-factor thresholds for enabling/disabling shadows with hysteresis
/// so the cascade maps don't thrash on/off at the boundary.
const SHADOW_OFF_NIGHT: f32 = 0.88;
const SHADOW_ON_NIGHT: f32 = 0.78;

/// Shared day/night state other layers (`terrain.rs`/`buildings.rs`, for
/// their unlit-tier material dimming) read without depending on the sun
/// entity itself.
#[derive(Resource, Clone, Copy)]
pub struct DayNightState {
    /// Smoothed hour shown to lights/materials (0..24).
    pub hour: f32,
    /// 0 = full day, 1 = full night; smoothly ramps across dusk/dawn.
    pub night_factor: f32,
    /// `sin(elevation_angle)` clamped to ≥0 — 0 at horizon, ~1 at zenith.
    /// Atmosphere uses this for god-ray strength and density shaping.
    pub sun_elevation: f32,
    /// Unit vector from origin toward the light (sun or moon).
    pub sun_direction: Vec3,
    /// Sim-tick targets; `apply_day_night_system` eases `hour`/`night_factor`
    /// toward these every frame.
    target_hour: f32,
    target_night_factor: f32,
    /// Last night_factor written to clear/ambient color, quantized to 1/256
    /// so steady-state frames skip dirtying those uniforms.
    applied_night_bucket: Option<u8>,
    /// Last hour written to the sun transform, quantized to 1/1024 of a day
    /// (~1.4 minutes) — fine enough that cascades don't pop, coarse enough
    /// to skip no-op transform writes once settled.
    applied_hour_bucket: Option<u16>,
    /// Hysteresis latch for night shadow disable.
    shadows_latched_on: bool,
    /// Last camera-height bucket used for cascade retune (meters / 64).
    applied_cam_height_bucket: Option<u16>,
}

impl Default for DayNightState {
    fn default() -> Self {
        // Noon until the first UiState arrives — matches the pre-game hold
        // in `compute_day_night_system` so Boot/MainMenu never flash midnight.
        DayNightState {
            hour: 12.0,
            night_factor: 0.0,
            sun_elevation: 1.0,
            sun_direction: Vec3::Y,
            target_hour: 12.0,
            target_night_factor: 0.0,
            applied_night_bucket: None,
            applied_hour_bucket: None,
            shadows_latched_on: true,
            applied_cam_height_bucket: None,
        }
    }
}

pub struct MfDayNightPlugin;

impl Plugin for MfDayNightPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DayNightState>()
            .add_systems(Startup, spawn_sun_system)
            .add_systems(
                Update,
                (
                    compute_day_night_system,
                    apply_day_night_system,
                    adapt_shadow_cascades_system,
                    ensure_shadow_filtering_system,
                )
                    .chain()
                    .in_set(crate::MfRenderSet::Dynamic),
            );
    }
}

/// Marker on the single directional sun/moon light. `pub(crate)` so
/// `atmosphere.rs` can attach `VolumetricLight` to the same entity.
#[derive(Component)]
pub(crate) struct Sun;

fn spawn_sun_system(mut commands: Commands) {
    commands.spawn((
        DirectionalLight {
            illuminance: 30_000.0,
            shadows_enabled: false,
            shadow_depth_bias: 0.04,
            shadow_normal_bias: 2.4,
            ..default()
        },
        Transform::from_xyz(0.0, 1000.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
        // Bevy's default cascade config (`maximum_distance: 150.0`) is tuned
        // for room/arena-scale scenes. Cities here run to tens of thousands
        // of meters across — widen cascades to cover a city. Extra overlap
        // softens cascade-boundary pop as the sun eases through the day.
        // Bounds are retuned every frame by `adapt_shadow_cascades_system`
        // from camera height; these are the Medium-tier startup defaults.
        CascadeShadowConfigBuilder {
            num_cascades: 3,
            minimum_distance: 0.5,
            first_cascade_far_bound: 400.0,
            maximum_distance: 20_000.0,
            overlap_proportion: 0.35,
        }
        .build(),
        Sun,
    ));
}

fn compute_day_night_system(
    ui: Res<LatestUi>,
    quality: Res<QualityTier>,
    theme: Res<Theme>,
    attract: Res<AttractLighting>,
    mut state: ResMut<DayNightState>,
) {
    // Dark/Purple ARE the night rig promoted to a standing theme (issue
    // #32): pin permanent midnight so the sun drops to the dim cool moon,
    // ambient falls to the night floor, and transit emissives read as a
    // glow — otherwise the noon sun floodlights the dark albedo back up to
    // mid-grey and the whole theme washes out. Checked before the
    // day-night-disabled tiers' fixed-noon path for the same reason.
    if *theme != Theme::Light {
        state.target_hour = 0.0;
        state.target_night_factor = 1.0;
        return;
    }
    // Title-screen attract diorama: lock golden hour regardless of sim
    // clock (and ahead of Potato's fixed-noon path) so the backdrop stays
    // warm/moody while the city sim races at 30× behind the menu.
    if attract.active {
        let (hour, night) = attract_golden_hour_targets();
        state.target_hour = hour;
        state.target_night_factor = night;
        return;
    }
    if !quality.knobs().day_night_enabled {
        state.target_hour = 12.0;
        state.target_night_factor = 0.0;
        return;
    }
    // Until the first UiState arrives (ConnectingSim/MainMenu/Loading), hold
    // noon rather than treating tick 0 as midnight — otherwise every pre-game
    // screen sits on the near-black night sky.
    // Gate target updates on `ui.is_changed()` for Light theme: tick only
    // advances with UiState (~2 Hz). Smoothing still runs every frame in
    // `apply_day_night_system`.
    if !ui.is_changed() && state.applied_hour_bucket.is_some() {
        return;
    }
    let Some(u) = &ui.0 else {
        state.target_hour = 12.0;
        state.target_night_factor = 0.0;
        return;
    };
    let hour = ((u.tick % TICKS_PER_DAY) as f32 / TICKS_PER_DAY as f32) * 24.0;
    let elevation = (((hour - 6.0) / 12.0) * PI).sin();
    state.target_hour = hour;
    state.target_night_factor = (-elevation * 1.2).clamp(0.0, 1.0);
}

/// Pure golden-hour targets for attract mode. Exposed to unit tests so the
/// dusk-band / shadow-on invariants don't drift silently.
fn attract_golden_hour_targets() -> (f32, f32) {
    let hour = ATTRACT_GOLDEN_HOUR;
    let elevation = (((hour - 6.0) / 12.0) * PI).sin();
    let night = (-elevation * 1.2).clamp(0.0, 1.0);
    (hour, night)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_day_night_system(
    time: Res<Time>,
    mut state: ResMut<DayNightState>,
    mut clear_color: ResMut<ClearColor>,
    mut ambient: ResMut<AmbientLight>,
    quality: Res<QualityTier>,
    theme: Res<Theme>,
    mut suns: Query<(&mut DirectionalLight, &mut Transform), With<Sun>>,
    mut fogs: Query<&mut DistanceFog, With<Camera3d>>,
) {
    let dt = time.delta_secs();
    // Ease displayed hour toward the sim target along the shortest wrap
    // around midnight so a 23.9 → 0.1 step doesn't spin the long way.
    let hour_delta = shortest_hour_delta(state.hour, state.target_hour);
    let hour_t = 1.0 - (-HOUR_SMOOTH_RATE * dt).exp();
    state.hour = (state.hour + hour_delta * hour_t).rem_euclid(24.0);
    state.night_factor += (state.target_night_factor - state.night_factor) * hour_t;

    let azimuth = (state.hour / 24.0) * std::f32::consts::TAU;
    let elevation_angle = ((state.hour - 6.0) / 12.0) * PI;
    let elev_sin = elevation_angle.sin();
    // Keep the light comfortably ABOVE the horizon at all hours: the old
    // .max(-0.2) parked the night sun below ground, raking every facade
    // with grazing up-light and speckling the dimmed city with lit faces.
    // At night this direction becomes a high, dim, cool moon instead.
    let sun_dir = Vec3::new(
        azimuth.cos() * elevation_angle.cos(),
        elev_sin.max(0.35),
        azimuth.sin() * elevation_angle.cos(),
    )
    .normalize_or_zero();
    state.sun_direction = sun_dir;
    state.sun_elevation = elev_sin.max(0.0).clamp(0.0, 1.0);

    let night_bucket = (state.night_factor * 255.0).round() as u8;
    let hour_bucket = (state.hour / 24.0 * 1024.0).round() as u16;
    let quality_or_theme_changed = quality.is_changed() || theme.is_changed();
    let night_dirty = state.applied_night_bucket != Some(night_bucket) || quality_or_theme_changed;
    let hour_dirty = state.applied_hour_bucket != Some(hour_bucket) || quality_or_theme_changed;
    if !night_dirty && !hour_dirty {
        return;
    }

    let n = state.night_factor;
    if night_dirty {
        state.applied_night_bucket = Some(night_bucket);
        let day_color = palette::sky_day();
        let night_color = palette::sky_night();
        // Warm the clear color slightly through twilight so dusk isn't a
        // straight white→navy lerp (pairs with atmosphere's golden fog).
        let twilight = {
            let t = 1.0 - ((n - 0.5).abs() * 2.0);
            t.clamp(0.0, 1.0).powf(1.2)
        };
        let dusk = Color::srgb(0.92, 0.72, 0.55);
        clear_color.0 = day_color.mix(&dusk, twilight * 0.35).mix(&night_color, n);
        // Distance fog (Potato/Low, quality sweep in lib.rs) must track the
        // clear color exactly - including this same twilight/night lerp and
        // the active theme - or the horizon shows a hard seam where fogged
        // geometry meets open sky. Correct with `day_night_enabled: false`
        // too: the fixed-noon branch still lands here with n=0.
        for mut fog in &mut fogs {
            fog.color = clear_color.0;
        }

        // Cool ambient at night, warm kiss at twilight.
        ambient.color = Color::WHITE
            .mix(&Color::srgb(1.0, 0.88, 0.75), twilight * 0.25)
            .mix(&Color::srgb(0.70, 0.78, 1.0), n * 0.45);
        ambient.brightness = 550.0 * (1.0 - n * 0.85);
    }

    state.applied_hour_bucket = Some(hour_bucket);

    let knobs = quality.knobs();
    // Hysteresis: turn shadows off deep into night, turn back on only after
    // climbing back toward dusk — avoids flicker at the threshold.
    if state.shadows_latched_on {
        if n >= SHADOW_OFF_NIGHT {
            state.shadows_latched_on = false;
        }
    } else if n <= SHADOW_ON_NIGHT {
        state.shadows_latched_on = true;
    }
    let shadows_ok = knobs.shadow_map_size.is_some() && state.shadows_latched_on;

    // Grazing light exaggerates shadow acne — push bias up as elevation drops.
    let elev = state.sun_elevation;
    let depth_bias = 0.028 + (1.0 - elev) * 0.055;
    let normal_bias = 1.9 + (1.0 - elev) * 1.6;

    let sun_pos = sun_dir * 1000.0;
    for (mut light, mut transform) in &mut suns {
        *transform = Transform::from_translation(sun_pos).looking_at(Vec3::ZERO, Vec3::Y);
        // Night floor is moonlight, not the old 8k-lux basement floodlight.
        // Soft high key: 30k keeps lit faces just under clip so white-on-white
        // geometry keeps separation and black roads stay legible.
        let day_lux = 2_000.0 + elev_sin.max(0.0) * 30_000.0;
        // Softly dim through the shadow-off band so the last frames before
        // disable aren't a hard contrast pop.
        let night_dim = if n > SHADOW_ON_NIGHT {
            1.0 - ((n - SHADOW_ON_NIGHT) / (1.0 - SHADOW_ON_NIGHT)).clamp(0.0, 1.0) * 0.55
        } else {
            1.0
        };
        light.illuminance = day_lux * night_dim;
        light.color = Color::srgb(1.0, 0.96, 0.88)
            .mix(&Color::srgb(1.0, 0.78, 0.55), {
                let t = 1.0 - ((n - 0.5).abs() * 2.0);
                t.clamp(0.0, 1.0).powf(1.2) * 0.45
            })
            .mix(&Color::srgb(0.55, 0.65, 0.9), n);
        light.shadows_enabled = shadows_ok;
        light.shadow_depth_bias = depth_bias;
        light.shadow_normal_bias = normal_bias;
    }
}

/// Retune cascade near/far bounds from camera height so a dolly-in gets
/// denser near-cascade texels and a city overview still covers the skyline.
fn adapt_shadow_cascades_system(
    quality: Res<QualityTier>,
    mut state: ResMut<DayNightState>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    mut suns: Query<&mut CascadeShadowConfig, With<Sun>>,
) {
    let knobs = quality.knobs();
    if knobs.shadow_map_size.is_none() {
        return;
    }
    let height = cameras
        .iter()
        .next()
        .map(|t| t.translation().y.max(120.0))
        .unwrap_or(2_000.0);
    let height_bucket = (height / 64.0).round() as u16;
    if state.applied_cam_height_bucket == Some(height_bucket) && !quality.is_changed() {
        return;
    }
    state.applied_cam_height_bucket = Some(height_bucket);

    let num_cascades = if matches!(*quality, QualityTier::High) {
        4
    } else {
        3
    };
    // Zoomed in (low camera): tight first cascade. Zoomed out: push it out
    // so the near map isn't wasted on a postage stamp under the lens.
    let first = (height * 0.22).clamp(180.0, 900.0);
    let maximum = (height * 7.5).clamp(10_000.0, 28_000.0).max(first * 4.0);

    let config = CascadeShadowConfigBuilder {
        num_cascades,
        minimum_distance: 0.5,
        first_cascade_far_bound: first,
        maximum_distance: maximum,
        overlap_proportion: 0.38,
    }
    .build();

    for mut cascade in &mut suns {
        *cascade = config.clone();
    }
}

/// Gaussian PCF on the view — Bevy's default is already Gaussian, but we
/// set it explicitly so a future camera spawn path can't silently fall back
/// to Hardware2x2 and reintroduce jagged cascade edges.
fn ensure_shadow_filtering_system(
    quality: Res<QualityTier>,
    mut commands: Commands,
    cameras: Query<(Entity, Option<&ShadowFilteringMethod>), With<Camera3d>>,
) {
    if quality.knobs().shadow_map_size.is_none() {
        return;
    }
    for (entity, method) in &cameras {
        match method {
            Some(ShadowFilteringMethod::Gaussian) => {}
            _ => {
                commands
                    .entity(entity)
                    .insert(ShadowFilteringMethod::Gaussian);
            }
        }
    }
}

fn shortest_hour_delta(from: f32, to: f32) -> f32 {
    let mut d = to - from;
    if d > 12.0 {
        d -= 24.0;
    } else if d < -12.0 {
        d += 24.0;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortest_hour_wraps_across_midnight() {
        let d = shortest_hour_delta(23.5, 0.5);
        assert!((d - 1.0).abs() < 1e-4);
        let d2 = shortest_hour_delta(0.5, 23.5);
        assert!((d2 + 1.0).abs() < 1e-4);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn shadow_hysteresis_band_is_ordered() {
        assert!(SHADOW_ON_NIGHT < SHADOW_OFF_NIGHT);
    }

    #[test]
    fn attract_golden_hour_is_in_dusk_band_with_shadows_still_on() {
        let (hour, night) = attract_golden_hour_targets();
        assert!((hour - ATTRACT_GOLDEN_HOUR).abs() < 1e-6);
        // Warm twilight kiss without extinguishing Medium+ shadows.
        assert!(
            (0.15..0.55).contains(&night),
            "expected dusk-band night_factor, got {night}"
        );
        assert!(
            night < SHADOW_ON_NIGHT,
            "golden hour must keep shadows latched on (night={night})"
        );
    }
}
