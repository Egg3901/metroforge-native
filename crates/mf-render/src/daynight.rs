//! Day/night cycle (spec §3.3 `daynight.rs`, art-direction §6):
//! `hour = (tick % TICKS_PER_DAY) / TICKS_PER_DAY * 24`. Day = white city
//! under warm sun; night = deep blue-black sky, buildings dim, but transit
//! (route stripes/vehicles/station rings) stays vivid/emissive — "transit
//! glows at night". Disabled on potato (fixed noon, per spec §4).
//!
//! Sun direction is smoothed every render frame toward the sim-tick target
//! so cascade shadows glide instead of snapping on the ~2 Hz UiState
//! cadence (and so the sun keeps moving through midday — the old
//! night-factor bucket gate froze the transform whenever `night_factor`
//! stayed at 0).

use std::f32::consts::PI;

use bevy::pbr::CascadeShadowConfigBuilder;
use bevy::prelude::*;

use mf_state::{LatestUi, QualityTier, Theme};

use crate::palette;

/// Mirrors `metroforge/src/core/constants.ts`'s `TICKS_PER_DAY` (spec: "each
/// tick is a 50ms host step; `TICKS_PER_DAY = 1200`").
const TICKS_PER_DAY: u64 = 1200;

/// Exponential chase rate for displayed hour → sim target. Settles ~95% in
/// ~0.35s, bridging the ~0.5s UiState gap without lagging dusk/dawn.
const HOUR_SMOOTH_RATE: f32 = 8.5;

/// Shared day/night state other layers (`terrain.rs`/`buildings.rs`, for
/// their unlit-tier material dimming) read without depending on the sun
/// entity itself.
#[derive(Resource, Clone, Copy)]
pub struct DayNightState {
    /// Smoothed hour shown to lights/materials (0..24).
    pub hour: f32,
    /// 0 = full day, 1 = full night; smoothly ramps across dusk/dawn.
    pub night_factor: f32,
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
}

impl Default for DayNightState {
    fn default() -> Self {
        // Noon until the first UiState arrives — matches the pre-game hold
        // in `compute_day_night_system` so Boot/MainMenu never flash midnight.
        DayNightState {
            hour: 12.0,
            night_factor: 0.0,
            target_hour: 12.0,
            target_night_factor: 0.0,
            applied_night_bucket: None,
            applied_hour_bucket: None,
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
                (compute_day_night_system, apply_day_night_system)
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
            // City-scale cascades have large texels at distance; a slightly
            // higher normal bias cuts self-shadow acne without obvious
            // peter-panning on building footprints.
            shadow_depth_bias: 0.04,
            shadow_normal_bias: 2.4,
            ..default()
        },
        Transform::from_xyz(0.0, 1000.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
        // Bevy's default cascade config (`maximum_distance: 150.0`) is tuned
        // for room/arena-scale scenes. Cities here run to tens of thousands
        // of meters across, so with the default config literally the entire
        // visible ground/skyline sits beyond the last cascade — every
        // fragment then samples as shadowed, crushing the whole "white
        // city" to a flat ambient-only grey regardless of tier. Widen the
        // cascades to actually cover a city. Extra overlap softens the
        // cascade-boundary pop as the sun eases through the day.
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

pub(crate) fn apply_day_night_system(
    time: Res<Time>,
    mut state: ResMut<DayNightState>,
    mut clear_color: ResMut<ClearColor>,
    mut ambient: ResMut<AmbientLight>,
    quality: Res<QualityTier>,
    theme: Res<Theme>,
    mut suns: Query<(&mut DirectionalLight, &mut Transform), With<Sun>>,
) {
    let dt = time.delta_secs();
    // Ease displayed hour toward the sim target along the shortest wrap
    // around midnight so a 23.9 → 0.1 step doesn't spin the long way.
    let hour_delta = shortest_hour_delta(state.hour, state.target_hour);
    let hour_t = 1.0 - (-HOUR_SMOOTH_RATE * dt).exp();
    state.hour = (state.hour + hour_delta * hour_t).rem_euclid(24.0);
    let night_t = hour_t;
    state.night_factor += (state.target_night_factor - state.night_factor) * night_t;

    let night_bucket = (state.night_factor * 255.0).round() as u8;
    let hour_bucket = (state.hour / 24.0 * 1024.0).round() as u16;
    let quality_or_theme_changed = quality.is_changed() || theme.is_changed();
    let night_dirty =
        state.applied_night_bucket != Some(night_bucket) || quality_or_theme_changed;
    let hour_dirty = state.applied_hour_bucket != Some(hour_bucket) || quality_or_theme_changed;
    if !night_dirty && !hour_dirty {
        return;
    }

    let n = state.night_factor;
    if night_dirty {
        state.applied_night_bucket = Some(night_bucket);
        let day_color = palette::sky_day();
        let night_color = palette::sky_night();
        clear_color.0 = day_color.mix(&night_color, n);

        ambient.color = Color::WHITE;
        // 950/116k lux were tuned while broken normals ate most direct light
        // (pre PR #14); with lighting actually working they overexpose the
        // scene and the tonemapper compresses everything toward white, washing
        // the transit colors pastel. High-key, with headroom.
        ambient.brightness = 550.0 * (1.0 - n * 0.85);
    }

    if !hour_dirty && !night_dirty {
        return;
    }
    state.applied_hour_bucket = Some(hour_bucket);

    let azimuth = (state.hour / 24.0) * std::f32::consts::TAU;
    let elevation_angle = ((state.hour - 6.0) / 12.0) * PI;
    // Keep the light comfortably ABOVE the horizon at all hours: the old
    // .max(-0.2) parked the night sun below ground, raking every facade
    // with grazing up-light and speckling the dimmed city with lit faces.
    // At night this direction becomes a high, dim, cool moon instead.
    let sun_pos = Vec3::new(
        azimuth.cos() * elevation_angle.cos(),
        elevation_angle.sin().max(0.35),
        azimuth.sin() * elevation_angle.cos(),
    ) * 1000.0;

    let knobs = quality.knobs();
    // Hard moonlight shadows at city scale look like a stuck noon cascade
    // with the wrong tint — fade them out through deep night.
    let shadows_ok = knobs.shadow_map_size.is_some() && n < 0.92;
    for (mut light, mut transform) in &mut suns {
        *transform = Transform::from_translation(sun_pos).looking_at(Vec3::ZERO, Vec3::Y);
        // Night floor is moonlight, not the old 8k-lux basement floodlight.
        // Soft high key: at 55k the sun-facing half of the city still
        // clipped to paper white (owner: left side washed out, streets
        // invisible). 30k keeps lit faces just under clip so white-on-white
        // geometry keeps separation and black roads stay legible.
        light.illuminance = 2_000.0 + elevation_angle.sin().max(0.0) * 30_000.0;
        light.color = Color::srgb(1.0, 0.96, 0.88).mix(&Color::srgb(0.55, 0.65, 0.9), n);
        light.shadows_enabled = shadows_ok;
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
}
