//! Weather render driver + material coupling (v0.7).
//!
//! Owns the frame-driven half of the weather system whose *state* lives in
//! `mf_state::WeatherRender`:
//! - [`drive_weather_system`] eases the sim-authored weather every frame
//!   (reads `LatestUi` + `Time`, honours the `MF_FORCE_WEATHER` dev override),
//!   and only marks the resource changed while a weight is actually moving so
//!   downstream change-detection (daynight/atmosphere) idles at steady state.
//! - [`wet_roads_system`] raises specular/reflectance on the black road
//!   materials while it is raining (the wet-sheen "money shot") and lerps them
//!   toward white as snow accumulates.
//! - [`snow_ground_system`] pushes the snow-depth accumulator into the terrain
//!   material's shader uniform so ground/parks whiten.
//! - [`rain_stripe_glow_system`] gives route stripes a subtle emissive lift at
//!   night in the rain (bloom then blooms them into the wet street).
//!
//! Art direction (BINDING): the city stays white, transit stays the only
//! colour; weather is fog + light grade + precipitation, never a coloured wash.
//! Precip *particles* live in [`crate::precip`]; this module is the lighting /
//! material side. All effects here are inert on the fog tiers (Potato/Low)
//! because their materials are `unlit` (reflectance/specular do nothing) and
//! the terrain snow lerp is gated to the lit terrain material.

use std::sync::OnceLock;

use bevy::pbr::StandardMaterial;
use bevy::prelude::*;

use mf_protocol::WeatherState;
use mf_state::{parse_forced_weather, LatestUi, QualityTier, WeatherEffects, WeatherRender};

use crate::daynight::DayNightState;
use crate::roads::RoadSurface;
use crate::terrain_material::TerrainMaterial;
use crate::transit::RouteStripe;

pub struct MfWeatherRenderPlugin;

impl Plugin for MfWeatherRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                // Runs first in Dynamic so daynight/atmosphere (same set) see
                // this frame's eased weights. Ordered before the daynight apply
                // so the grade/flash lands the same frame.
                drive_weather_system.before(crate::daynight::apply_day_night_system),
                wet_roads_system,
                snow_ground_system,
                rain_stripe_glow_system,
            )
                .in_set(crate::MfRenderSet::Dynamic),
        );
    }
}

/// Parsed `MF_FORCE_WEATHER` dev override, resolved once. `docs/BUILDING.md`
/// documents the values (`clear|overcast|rain|fog|snow|storm[:intensity]`).
fn forced_weather() -> Option<(WeatherState, Option<f32>)> {
    static FORCED: OnceLock<Option<(WeatherState, Option<f32>)>> = OnceLock::new();
    *FORCED.get_or_init(|| {
        std::env::var("MF_FORCE_WEATHER")
            .ok()
            .and_then(|raw| parse_forced_weather(&raw))
    })
}

/// True when two eased weather snapshots differ enough to be worth a
/// change-detection tick (so daynight/atmosphere recompute during transitions
/// but idle once settled).
fn weather_moved(a: &WeatherRender, b: &WeatherRender) -> bool {
    const EPS: f32 = 5e-4;
    a.state != b.state
        || (a.rain - b.rain).abs() > EPS
        || (a.snow - b.snow).abs() > EPS
        || (a.overcast - b.overcast).abs() > EPS
        || (a.fog - b.fog).abs() > EPS
        || (a.storm - b.storm).abs() > EPS
        || (a.snow_depth - b.snow_depth).abs() > EPS
        || (a.lightning - b.lightning).abs() > EPS
}

fn drive_weather_system(time: Res<Time>, ui: Res<LatestUi>, mut weather: ResMut<WeatherRender>) {
    let ui_state = ui.0.as_ref();
    let tick = ui_state.map(|u| u.tick).unwrap_or(0);

    // Discrete inputs: the dev override wins, otherwise the sim's UiState.
    let (state, intensity, season, event) = match forced_weather() {
        Some((forced_state, forced_intensity)) => (
            Some(forced_state),
            forced_intensity.or(ui_state.and_then(|u| u.weather_intensity).map(|i| i as f32)),
            ui_state.and_then(|u| u.weather_season),
            ui_state.and_then(|u| u.weather_event),
        ),
        None => (
            ui_state.and_then(|u| u.weather_state),
            ui_state.and_then(|u| u.weather_intensity).map(|i| i as f32),
            ui_state.and_then(|u| u.weather_season),
            ui_state.and_then(|u| u.weather_event),
        ),
    };

    // Integrate on a scratch copy so we only trip change-detection when a
    // weight actually moved (idle steady state = no downstream recompute).
    let mut next = weather.clone();
    next.set_inputs(state, intensity, season, event);
    next.step(time.delta_secs(), tick);

    if weather_moved(&weather, &next) {
        *weather = next;
    } else {
        *weather.bypass_change_detection() = next;
    }
}

/// Per-road-material baseline captured lazily so the wet/snow grade always
/// lerps from a stable original (roads.rs re-bakes `base_color` on a rare
/// structural rebuild; we detect that by watching for our own last write to
/// disappear and re-capture).
#[derive(Default)]
struct RoadBaseline {
    base: LinearRgba,
    roughness: f32,
    reflectance: f32,
    last_written: LinearRgba,
}

#[allow(clippy::type_complexity)]
fn wet_roads_system(
    weather: Res<WeatherRender>,
    effects: Res<WeatherEffects>,
    quality: Res<QualityTier>,
    day_night: Res<DayNightState>,
    roads: Query<&MeshMaterial3d<StandardMaterial>, With<RoadSurface>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut baselines: Local<std::collections::HashMap<AssetId<StandardMaterial>, RoadBaseline>>,
) {
    // Wet sheen / snow whitening only read on the lit tiers (Medium/High):
    // the fog tiers keep roads `unlit`, where reflectance/specular are inert.
    if quality.knobs().unlit_material {
        return;
    }
    let active = effects.enabled && weather.is_active();
    // When fully clear we still want to run one restoring pass, so don't early
    // out purely on `!active` — but skip once everything is already restored.
    let wet = if active {
        weather.rain.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let snow = if active {
        weather.snow_depth.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if !active && baselines.is_empty() {
        return;
    }

    for handle in &roads {
        let id = handle.0.id();
        let Some(mat) = materials.get_mut(&handle.0) else {
            continue;
        };
        let entry = baselines.entry(id).or_insert_with(|| RoadBaseline {
            base: mat.base_color.to_linear(),
            roughness: mat.perceptual_roughness,
            reflectance: mat.reflectance,
            last_written: mat.base_color.to_linear(),
        });
        // Structural rebuild reset the material out from under us: re-capture.
        let cur = mat.base_color.to_linear();
        if (cur.red - entry.last_written.red).abs() > 1e-3
            || (cur.green - entry.last_written.green).abs() > 1e-3
            || (cur.blue - entry.last_written.blue).abs() > 1e-3
        {
            entry.base = cur;
            entry.roughness = mat.perceptual_roughness;
            entry.reflectance = mat.reflectance;
        }

        // Wet asphalt reads darker + far shinier; snow turns it to SLUSH — a
        // grey-white, NOT the near-white the ground/parks whiten to. Roads are
        // the dark mass that gives the city its street grid, so at full
        // accumulation they must stay a clearly darker slush line (owner: the
        // previous ~#eaeaf2 target collapsed roads into the ~#e9eae5 ground and
        // the whole grid vanished under snow). Target #b8bcc0 (~0.72 luma) keeps
        // roads ~40/255 luma below the ground even at max snow (see the
        // road-region delta gate in the capture harness).
        let base = Vec3::new(entry.base.red, entry.base.green, entry.base.blue);
        let wet_col = base * (1.0 - 0.28 * wet);
        // #b8bcc0 in linear-ish working space (these road materials are authored
        // in srgb component values, matching `entry.base`).
        let slush_col = Vec3::new(0.722, 0.737, 0.753);
        // Cap the lerp so even snow_depth==1 never fully reaches the slush tone,
        // holding a little of the road's darkness in reserve.
        let rgb = wet_col.lerp(slush_col, (snow * 0.92).clamp(0.0, 0.92));
        let out = LinearRgba::new(rgb.x, rgb.y, rgb.z, entry.base.alpha);
        mat.base_color = Color::from(out);
        entry.last_written = out;
        // Lower roughness + raise reflectance while wet = crisp street specular.
        // Pushed harder than before (owner: daytime wet streets read matte) so
        // the sheen actually reads glossy in daylight, not just under night bloom.
        mat.perceptual_roughness = entry.roughness * (1.0 - 0.85 * wet);
        mat.reflectance = entry.reflectance + (0.72 - entry.reflectance).max(0.0) * wet;
    }

    // Fully restored and clear: drop the baselines so the map doesn't grow.
    if !active {
        baselines.clear();
    }
    let _ = day_night; // reserved: night-specific road tint hook.
}

fn snow_ground_system(
    weather: Res<WeatherRender>,
    effects: Res<WeatherEffects>,
    quality: Res<QualityTier>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut last: Local<f32>,
) {
    // Snow whitening rides the lit terrain material's shader uniform; the fog
    // tiers use a flat vertex-coloured ground with no such uniform.
    if quality.knobs().unlit_material {
        return;
    }
    let target = if effects.enabled {
        weather.snow_depth.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if (target - *last).abs() < 1e-3 {
        return;
    }
    *last = target;
    for (_, mat) in materials.iter_mut() {
        mat.extension.weather.x = target;
    }
}

/// Base emissive strength route stripes are painted with (see `transit.rs`).
const STRIPE_EMISSIVE_BASE: f32 = 0.45;

#[allow(clippy::too_many_arguments)]
fn rain_stripe_glow_system(
    weather: Res<WeatherRender>,
    effects: Res<WeatherEffects>,
    quality: Res<QualityTier>,
    overlay: Res<mf_state::OverlayState>,
    focus: Res<mf_state::RouteFocus>,
    day_night: Res<DayNightState>,
    stripes: Query<(&RouteStripe, &MeshMaterial3d<StandardMaterial>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut lifted: Local<bool>,
) {
    // Only the bloom tiers, and only while the overlay/route-focus wash (which
    // owns stripe emissive) is NOT active — otherwise we'd fight it.
    let owns_stage = overlay.mode != mf_state::OverlayMode::Off || focus.route_id.is_some();
    let lift = if effects.enabled && !quality.knobs().unlit_material && !owns_stage {
        (weather.rain * day_night.night_factor).clamp(0.0, 1.0)
    } else {
        0.0
    };

    if lift > 0.02 {
        for (stripe, handle) in &stripes {
            if let Some(mat) = materials.get_mut(&handle.0) {
                mat.emissive =
                    crate::palette::emissive(stripe.color, STRIPE_EMISSIVE_BASE + lift * 0.55);
            }
        }
        *lifted = true;
    } else if *lifted {
        // Restore the painted base emissive exactly once when the lift ends.
        for (stripe, handle) in &stripes {
            if let Some(mat) = materials.get_mut(&handle.0) {
                mat.emissive = crate::palette::emissive(stripe.color, STRIPE_EMISSIVE_BASE);
            }
        }
        *lifted = false;
    }
}
