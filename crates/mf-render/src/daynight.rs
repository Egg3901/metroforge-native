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

use mf_state::{AttractLighting, EffectiveKnobs, LatestUi, Theme, WeatherRender};

use crate::palette;
use crate::photomode::PhotoModeRender;

/// Attract-mode golden hour (local solar time). Low sun → long shadows on
/// Medium+; warm twilight tint on clear/ambient/sun color. Chosen so
/// `night_factor` sits in the dusk band without extinguishing shadows.
const ATTRACT_GOLDEN_HOUR: f32 = 19.0;

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
    effective: Res<EffectiveKnobs>,
    theme: Res<Theme>,
    photo: Res<PhotoModeRender>,
    attract: Res<AttractLighting>,
    day_night_pref: Res<mf_state::DayNightEnabled>,
    mut state: ResMut<DayNightState>,
) {
    // Photo-mode scrubber: local hour override, does not touch the sim clock.
    // Checked first so a scrub still works under Dark/Purple/potato (the
    // player asked to frame a shot, not to fight the theme's fixed hour).
    if let Some(hour) = photo.override_hour {
        let hour = hour.rem_euclid(24.0);
        let elevation = (((hour - 6.0) / 12.0) * PI).sin();
        state.target_hour = hour;
        state.target_night_factor = (-elevation * 1.2).clamp(0.0, 1.0);
        return;
    }
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
    // Tier pins noon (Potato), OR the player turned the cycle off in Settings.
    if !effective.0.day_night_enabled || !day_night_pref.enabled {
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
    // Prefer the sidecar's sim `hourOfDay` (sim-depth, PR #31) so the sky
    // rig and the HUD clock (which reads the same field via
    // `UiState::display_hour`) stay in lockstep; fall back to the
    // tick-derived clock for old sidecars that omit it.
    let hour = u.display_hour() as f32;
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
    effective: Res<EffectiveKnobs>,
    theme: Res<Theme>,
    weather: Res<WeatherRender>,
    mut suns: Query<(&mut DirectionalLight, &mut Transform), With<Sun>>,
    mut fogs: Query<&mut DistanceFog, With<Camera3d>>,
) {
    let dt = time.delta_secs();
    // Ease displayed hour toward the sim target along the shortest wrap
    // around midnight so a 23.9 → 0.1 step doesn't spin the long way.
    let hour_t = 1.0 - (-HOUR_SMOOTH_RATE * dt).exp();
    state.hour = advance_hour(state.hour, state.target_hour, hour_t);
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

    let night_bucket = quantize_night_bucket(state.night_factor);
    let hour_bucket = quantize_hour_bucket(state.hour);
    let quality_or_theme_changed = effective.is_changed() || theme.is_changed();
    let night_dirty = state.applied_night_bucket != Some(night_bucket) || quality_or_theme_changed;
    let hour_dirty = state.applied_hour_bucket != Some(hour_bucket) || quality_or_theme_changed;
    // Weather grades the light every frame it is transitioning / flashing:
    // `WeatherRender` only marks itself changed while a weight is actually
    // moving (its driver bypasses change-detection at steady state), so this
    // recomputes the sun/ambient during weather but idles when settled.
    let weather_dirty = weather.is_changed();
    if !night_dirty && !hour_dirty && !weather_dirty {
        return;
    }
    // Overcast/storm close the sky: dim the key sun, lift ambient fill so the
    // white city flattens instead of going muddy (the issue #40 ambient-fill
    // pattern). A lightning flash is a brief additive luminance pulse on both
    // ambient and the key light (no geometry) — the cloud cards + this pulse
    // are the whole flash.
    let overcast = (weather.overcast + weather.storm * 0.6).clamp(0.0, 1.0);
    let sun_dim = 1.0 - overcast * 0.55;
    let ambient_lift = 1.0 + overcast * 0.55;
    let flash = weather.lightning;

    let n = state.night_factor;
    if night_dirty {
        state.applied_night_bucket = Some(night_bucket);
        let day_color = palette::sky_day();
        let night_color = palette::sky_night();
        // Warm the clear color through twilight AND low-sun golden hour so
        // dusk/dawn aren't a straight white→navy lerp (pairs with atmosphere).
        let twilight = {
            let t = 1.0 - ((n - 0.5).abs() * 2.0);
            t.clamp(0.0, 1.0).powf(1.2)
        };
        let golden = {
            let low_sun = (1.0 - (state.sun_elevation / 0.35).clamp(0.0, 1.0)).clamp(0.0, 1.0);
            let not_deep_night = (1.0 - ((n - 0.55).max(0.0) / 0.45)).clamp(0.0, 1.0);
            low_sun * not_deep_night
        };
        let warm = twilight.max(golden);
        let dusk = Color::srgb(1.0, 0.72, 0.48);
        clear_color.0 = day_color.mix(&dusk, warm * 0.55).mix(&night_color, n);
        // Distance fog (Potato/Low, quality sweep in lib.rs) must track the
        // clear color exactly - including this same twilight/night lerp and
        // the active theme - or the horizon shows a hard seam where fogged
        // geometry meets open sky. Correct with `day_night_enabled: false`
        // too: the fixed-noon branch still lands here with n=0.
        for mut fog in &mut fogs {
            fog.color = clear_color.0;
        }

        // Cool ambient at night, warm kiss at golden hour / twilight.
        ambient.color = Color::WHITE
            .mix(&Color::srgb(1.0, 0.82, 0.62), warm * 0.40)
            .mix(&Color::srgb(0.70, 0.78, 1.0), n * 0.45);
    }

    // Ambient skylight tracks the sun's height, not just the night factor
    // (issue #40). With a flat 550-lux ambient, a midday cast shadow sits
    // ~60x below sunlit faces; in districts where tall tight blocks put
    // nearly every facade below the roofline in true cast shadow, that whole
    // half of the skyline collapses into one flat grey mass — and where the
    // building-height regime steps down across an avenue, the lit/shaded
    // boundary reads as a hard vertical renderer "seam" (the shadowing is
    // geometrically correct; the rendering of it is too harsh). Real skies
    // fill shadows hardest at noon, so scale ambient with elevation:
    // shadowed facades keep separation at high sun while `sun_elevation`
    // is 0 through dusk/night, leaving golden-hour and night looks alone.
    // Written on hour OR night dirt so the fill follows the climbing sun.
    ambient.brightness =
        550.0 * (1.0 + state.sun_elevation * 1.25) * (1.0 - n * 0.85) * ambient_lift
            + flash * 2600.0;

    state.applied_hour_bucket = Some(hour_bucket);

    let knobs = effective.0;
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
        let elev_pos = elev_sin.max(0.0);
        // Shave ~10% off the zenith peak: paired with the elevation-scaled
        // ambient above, this narrows the lit-vs-shaded gap at high sun
        // without dimming golden-hour light (the trim vanishes as elev -> 0).
        let day_lux = 2_000.0 + elev_pos * 30_000.0 * (1.0 - 0.10 * elev_pos);
        // Softly dim through the shadow-off band so the last frames before
        // disable aren't a hard contrast pop.
        let night_dim = if n > SHADOW_ON_NIGHT {
            1.0 - ((n - SHADOW_ON_NIGHT) / (1.0 - SHADOW_ON_NIGHT)).clamp(0.0, 1.0) * 0.55
        } else {
            1.0
        };
        light.illuminance = day_lux * night_dim * sun_dim + flash * 45_000.0;
        let twilight = {
            let t = 1.0 - ((n - 0.5).abs() * 2.0);
            t.clamp(0.0, 1.0).powf(1.2)
        };
        let golden = {
            let low_sun = (1.0 - (elev / 0.35).clamp(0.0, 1.0)).clamp(0.0, 1.0);
            let not_deep_night = (1.0 - ((n - 0.55).max(0.0) / 0.45)).clamp(0.0, 1.0);
            low_sun * not_deep_night
        };
        let warm = twilight.max(golden);
        light.color = Color::srgb(1.0, 0.96, 0.88)
            .mix(&Color::srgb(1.0, 0.70, 0.42), warm * 0.70)
            .mix(&Color::srgb(0.55, 0.65, 0.9), n);
        light.shadows_enabled = shadows_ok;
        light.shadow_depth_bias = depth_bias;
        light.shadow_normal_bias = normal_bias;
    }
}

/// Retune cascade near/far bounds from camera height so a dolly-in gets
/// denser near-cascade texels and a city overview still covers the skyline.
fn adapt_shadow_cascades_system(
    effective: Res<EffectiveKnobs>,
    mut state: ResMut<DayNightState>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    mut suns: Query<&mut CascadeShadowConfig, With<Sun>>,
) {
    let knobs = effective.0;
    if knobs.shadow_map_size.is_none() {
        return;
    }
    let height = cameras
        .iter()
        .next()
        .map(|t| t.translation().y.max(120.0))
        .unwrap_or(2_000.0);
    let height_bucket = (height / 64.0).round() as u16;
    if state.applied_cam_height_bucket == Some(height_bucket) && !effective.is_changed() {
        return;
    }
    state.applied_cam_height_bucket = Some(height_bucket);

    let num_cascades = if knobs.shadow_map_size == Some(4096) {
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
    effective: Res<EffectiveKnobs>,
    mut commands: Commands,
    cameras: Query<(Entity, Option<&ShadowFilteringMethod>), With<Camera3d>>,
) {
    if effective.0.shadow_map_size.is_none() {
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

/// One smoothing step of displayed hour toward `target`, wrapping into
/// `[0, 24)`. `t` is the exponential blend factor in `[0, 1]`.
fn advance_hour(hour: f32, target: f32, t: f32) -> f32 {
    let hour_delta = shortest_hour_delta(hour, target);
    (hour + hour_delta * t).rem_euclid(24.0)
}

/// Night-factor dirty bucket: 256 levels across `[0, 1]`.
fn quantize_night_bucket(night_factor: f32) -> u8 {
    (night_factor.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Hour dirty bucket: 1024 levels across a day (~1.4 simulated minutes).
fn quantize_hour_bucket(hour: f32) -> u16 {
    let wrapped = hour.rem_euclid(24.0);
    (wrapped / 24.0 * 1024.0).round() as u16 % 1024
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
    fn shortest_hour_delta_is_antisymmetric() {
        // For every pair, delta(a,b) == -delta(b,a) — the property that
        // keeps dusk→dawn and dawn→dusk smoothing from picking opposite
        // directions inconsistently.
        for &(a, b) in &[
            (0.0, 0.0),
            (23.5, 0.5),
            (0.5, 23.5),
            (6.0, 18.0),
            (18.0, 6.0),
            (1.0, 13.0),
            (12.0, 0.0),
            (0.0, 12.0),
            (23.9, 0.1),
        ] {
            let ab = shortest_hour_delta(a, b);
            let ba = shortest_hour_delta(b, a);
            assert!(
                (ab + ba).abs() < 1e-4,
                "delta({a},{b})={ab} is not antisymmetric of {ba}"
            );
            assert!(ab.abs() <= 12.0 + 1e-4, "delta longer than half-day: {ab}");
        }
    }

    #[test]
    fn advance_hour_wraps_into_day_and_takes_short_path() {
        // 23.9 → 0.1 must step forward (~+0.2), never the long way back.
        let stepped = advance_hour(23.9, 0.1, 1.0);
        assert!((stepped - 0.1).abs() < 1e-4, "got {stepped}");
        // Partial step stays in [0, 24).
        let partial = advance_hour(23.9, 0.1, 0.5);
        assert!((0.0..24.0).contains(&partial), "got {partial}");
        assert!(
            !(0.1..=23.9).contains(&partial),
            "should move toward midnight wrap, got {partial}"
        );
    }

    #[test]
    fn advance_hour_identity_when_already_at_target() {
        for h in [0.0, 6.0, 12.0, 18.0, 23.999] {
            let out = advance_hour(h, h, 0.5);
            assert!((out - h.rem_euclid(24.0)).abs() < 1e-4, "h={h} out={out}");
        }
    }

    #[test]
    fn night_bucket_quantization_is_stable_near_boundaries() {
        assert_eq!(quantize_night_bucket(0.0), 0);
        assert_eq!(quantize_night_bucket(1.0), 255);
        assert_eq!(quantize_night_bucket(0.5), 128);
        // Probe the interior of bucket 128: values whose `*255` lands in
        // (127.5, 128.5) must all quantize identically. 0.5 itself sits
        // exactly on the 127.5 half-up boundary, so nudge from the center.
        let center = 128.0 / 255.0;
        let base = quantize_night_bucket(center);
        assert_eq!(base, 128);
        let half_bucket = 0.4 / 255.0;
        assert_eq!(quantize_night_bucket(center + half_bucket), base);
        assert_eq!(quantize_night_bucket(center - half_bucket), base);
    }

    #[test]
    fn hour_bucket_quantization_wraps_and_is_stable() {
        assert_eq!(quantize_hour_bucket(0.0), 0);
        assert_eq!(quantize_hour_bucket(12.0), 512);
        // Exactly 24.0 wraps to the same bucket as 0.0.
        assert_eq!(quantize_hour_bucket(24.0), quantize_hour_bucket(0.0));
        assert_eq!(quantize_hour_bucket(-0.001), quantize_hour_bucket(23.999));
        // Stability: a sub-bucket nudge must not change the bucket.
        let noon = quantize_hour_bucket(12.0);
        let step = 24.0 / 1024.0;
        assert_eq!(quantize_hour_bucket(12.0 + step * 0.2), noon);
        assert_eq!(quantize_hour_bucket(12.0 - step * 0.2), noon);
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
