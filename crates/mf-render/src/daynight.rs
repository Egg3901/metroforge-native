//! Day/night cycle (spec §3.3 `daynight.rs`, art-direction §6):
//! `hour = (tick % TICKS_PER_DAY) / TICKS_PER_DAY * 24`. Day = white city
//! under warm sun; night = deep blue-black sky, buildings dim, but transit
//! (route stripes/vehicles/station rings) stays vivid/emissive — "transit
//! glows at night". Disabled on potato (fixed noon, per spec §4).

use std::f32::consts::PI;

use bevy::pbr::CascadeShadowConfigBuilder;
use bevy::prelude::*;

use mf_state::{LatestUi, QualityTier};

use crate::palette;

/// Mirrors `metroforge/src/core/constants.ts`'s `TICKS_PER_DAY` (spec: "each
/// tick is a 50ms host step; `TICKS_PER_DAY = 1200`").
const TICKS_PER_DAY: u64 = 1200;

/// Shared day/night state other layers (`terrain.rs`/`buildings.rs`, for
/// their unlit-tier material dimming) read without depending on the sun
/// entity itself.
#[derive(Resource, Default, Clone, Copy)]
pub struct DayNightState {
    pub hour: f32,
    /// 0 = full day, 1 = full night; smoothly ramps across dusk/dawn.
    pub night_factor: f32,
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

#[derive(Component)]
struct Sun;

fn spawn_sun_system(mut commands: Commands) {
    commands.spawn((
        DirectionalLight {
            illuminance: 30_000.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(0.0, 1000.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
        // Bevy's default cascade config (`maximum_distance: 150.0`) is tuned
        // for room/arena-scale scenes. Cities here run to tens of thousands
        // of meters across, so with the default config literally the entire
        // visible ground/skyline sits beyond the last cascade — every
        // fragment then samples as shadowed, crushing the whole "white
        // city" to a flat ambient-only grey regardless of tier. Widen the
        // cascades to actually cover a city.
        CascadeShadowConfigBuilder {
            num_cascades: 3,
            minimum_distance: 0.5,
            first_cascade_far_bound: 300.0,
            maximum_distance: 20_000.0,
            overlap_proportion: 0.2,
        }
        .build(),
        Sun,
    ));
}

fn compute_day_night_system(
    ui: Res<LatestUi>,
    quality: Res<QualityTier>,
    mut state: ResMut<DayNightState>,
) {
    if !quality.knobs().day_night_enabled {
        state.hour = 12.0;
        state.night_factor = 0.0;
        return;
    }
    // Until the first UiState arrives (ConnectingSim/MainMenu/Loading), hold
    // noon rather than treating tick 0 as midnight — otherwise every pre-game
    // screen sits on the near-black night sky.
    let Some(u) = &ui.0 else {
        state.hour = 12.0;
        state.night_factor = 0.0;
        return;
    };
    let hour = ((u.tick % TICKS_PER_DAY) as f32 / TICKS_PER_DAY as f32) * 24.0;
    let elevation = (((hour - 6.0) / 12.0) * PI).sin();
    state.hour = hour;
    state.night_factor = (-elevation * 1.2).clamp(0.0, 1.0);
}

fn apply_day_night_system(
    state: Res<DayNightState>,
    mut clear_color: ResMut<ClearColor>,
    mut ambient: ResMut<AmbientLight>,
    quality: Res<QualityTier>,
    mut suns: Query<(&mut DirectionalLight, &mut Transform), With<Sun>>,
) {
    let n = state.night_factor;
    let day_color = palette::sky_day();
    let night_color = palette::sky_night();
    clear_color.0 = day_color.mix(&night_color, n);

    ambient.color = Color::WHITE;
    // 950/116k lux were tuned while broken normals ate most direct light
    // (pre PR #14); with lighting actually working they overexpose the
    // scene and the tonemapper compresses everything toward white, washing
    // the transit colors pastel. High-key, with headroom.
    ambient.brightness = 550.0 * (1.0 - n * 0.85);

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
    for (mut light, mut transform) in &mut suns {
        *transform = Transform::from_translation(sun_pos).looking_at(Vec3::ZERO, Vec3::Y);
        // Night floor is moonlight, not the old 8k-lux basement floodlight.
        light.illuminance = 2_000.0 + elevation_angle.sin().max(0.0) * 55_000.0;
        light.color = Color::srgb(1.0, 0.96, 0.88).mix(&Color::srgb(0.55, 0.65, 0.9), n);
        light.shadows_enabled = knobs.shadow_map_size.is_some();
    }
}
