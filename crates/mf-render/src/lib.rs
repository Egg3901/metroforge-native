//! `mf-render` — the 3D renderer (spec §3.3), composed as `MfRenderPlugin`.
//! Mirror's-Edge art direction (`art-direction.md`, BINDING): stark white
//! city, black streets, vivid color reserved exclusively for the transit
//! network the player builds. See `crates/mf-render/src/palette.rs` for the
//! single source of truth on colors.
//!
//! Layers, in bake/update order (see [`MfRenderSet`]):
//! terrain -> roads/buildings/transit (static, cached by version/structural
//! signature) -> vehicles/agents/daynight/atmosphere/subway (dynamic, every
//! frame).

mod agents;
mod atmosphere;
mod buildings;
mod daynight;
mod mesh_utils;
mod outline;
/// Public so `mf-game` ghost previews can share the same vivid route table
/// (and theme) as finished transit — see `tools.rs` route_ghost_color.
pub mod palette;
mod reveal;
mod roads;
mod sky;
mod stats;
mod street_lamps;
mod subway;
mod terrain;
mod transit;
mod trees;
mod vehicles;
mod water;

pub use stats::RenderCacheStats;

use bevy::core_pipeline::bloom::{Bloom, BloomCompositeMode, BloomPrefilter};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::pbr::{DirectionalLightShadowMap, DistanceFog, FogFalloff};
use bevy::prelude::*;

use mf_state::{QualityTier, Theme};

use crate::daynight::DayNightState;

pub use buildings::BuildingsDenseCenter;

/// Peak bloom intensity at full night (Medium/High). Ramps linearly with
/// `DayNightState.night_factor`; 0 during day so the bloom node early-outs.
const BLOOM_INTENSITY_NIGHT: f32 = 0.18;
/// Prefilter keeps the white-city albedo out of the bloom extract so only
/// emissive transit / lamps / headlights contribute the night glow.
const BLOOM_PREFILTER_THRESHOLD: f32 = 0.55;
const BLOOM_PREFILTER_SOFTNESS: f32 = 0.3;

/// Ordering backbone for the whole crate. `Terrain` must run (and, on a
/// rebuild, replace `mf_state::HeightAt`) before anything that samples
/// ground height; `Statics` (roads/buildings/transit) cache-check every
/// frame but only rebuild on version/structural change; `Dynamic`
/// (vehicles/agents/day-night/subway) runs every frame unconditionally.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MfRenderSet {
    Terrain,
    Statics,
    Dynamic,
}

pub struct MfRenderPlugin;

impl Plugin for MfRenderPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            Update,
            (
                MfRenderSet::Terrain,
                MfRenderSet::Statics,
                MfRenderSet::Dynamic,
            )
                .chain(),
        )
        .insert_resource(DirectionalLightShadowMap { size: 2048 })
        .init_resource::<RenderCacheStats>()
        .add_plugins((
            reveal::MfRevealPlugin,
            sky::MfSkyPlugin,
            terrain::MfTerrainPlugin,
            water::MfWaterPlugin,
            roads::MfRoadsPlugin,
            buildings::MfBuildingsPlugin,
            transit::MfTransitPlugin,
            trees::MfTreesPlugin,
            street_lamps::MfStreetLampsPlugin,
            vehicles::MfVehiclesPlugin,
            agents::MfAgentsPlugin,
            daynight::MfDayNightPlugin,
            atmosphere::MfAtmospherePlugin,
            subway::MfSubwayPlugin,
            outline::MfOutlinePlugin,
        ))
        .add_systems(
            Update,
            (
                sync_theme_system.before(MfRenderSet::Terrain),
                // Runs before daynight's apply system (same `Dynamic` set)
                // so a freshly-inserted `DistanceFog` gets its real
                // day/night-matched color the same frame it's spawned,
                // rather than showing one frame of the `Color::WHITE`
                // placeholder default.
                apply_quality_render_settings_system
                    .in_set(MfRenderSet::Dynamic)
                    .before(daynight::apply_day_night_system),
                // Bloom intensity tracks the smoothed night_factor; runs
                // after daynight so the same-frame dusk ramp is visible.
                sync_bloom_system
                    .in_set(MfRenderSet::Dynamic)
                    .after(daynight::apply_day_night_system),
            ),
        );
    }
}

/// Publishes `Res<Theme>` into `palette.rs`'s process-global (see that
/// module's doc comment for why) — runs before `MfRenderSet::Terrain` so any
/// theme change is visible to every material-build system in this same
/// frame, not the next one. Cheap even every frame (a single atomic store,
/// gated on `is_changed`), and covers the one-time "just inserted" tick at
/// startup the same way every other `Res<T>::is_changed()` check in this
/// crate does.
fn sync_theme_system(theme: Res<Theme>) {
    if !theme.is_changed() {
        return;
    }
    palette::set_theme(*theme);
}

/// Spec §4 knob table, the render-global settings that don't belong to any
/// one layer: MSAA sample count (a per-camera `Component` in Bevy 0.16, not
/// a resource — applied to every `Camera3d` found, regardless of which
/// plugin spawned it) and the shadow-cascade map resolution. Everything
/// else in the table (materials, draw distances, agent caps, terrain
/// subdivision, day/night on/off) is consumed directly by the relevant
/// layer module from `QualityTier::knobs()`.
/// True if `falloff` is already the linear `start..end` we'd set — lets the
/// per-frame fog reconcile skip a redundant write (and its change-detection
/// trigger) when the camera's fog is already correct for the tier.
fn fog_falloff_matches(falloff: &FogFalloff, start: f32, end: f32) -> bool {
    matches!(
        falloff,
        FogFalloff::Linear { start: s, end: e } if *s == start && *e == end
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_quality_render_settings_system(
    quality: Res<QualityTier>,
    mut shadow_map: ResMut<DirectionalLightShadowMap>,
    mut commands: Commands,
    cameras: Query<Entity, With<Camera3d>>,
    cameras_missing_msaa: Query<Entity, (With<Camera3d>, Without<Msaa>)>,
    cameras_missing_fog: Query<Entity, (With<Camera3d>, Without<DistanceFog>)>,
    // Cameras that ALREADY have a `DistanceFog` (the in-game camera spawns
    // with one at `Startup`, see `mf-game`'s `camera.rs`). Needed so the
    // per-tier fog knob can *override* that spawn-time falloff — see the fog
    // reconcile block below.
    mut cameras_with_fog: Query<(Entity, &mut DistanceFog, Option<&Tonemapping>), With<Camera3d>>,
    cameras_missing_bloom: Query<Entity, (With<Camera3d>, Without<Bloom>)>,
    mut camera_hdr: Query<&mut Camera, With<Camera3d>>,
) {
    let knobs = quality.knobs();
    let msaa = match knobs.msaa_samples {
        1 => Msaa::Off,
        2 => Msaa::Sample2,
        8 => Msaa::Sample8,
        _ => Msaa::Sample4,
    };
    // `mf-game`'s camera.rs spawns `Camera3d` only on `OnEnter(InGame)`,
    // which happens well after `QualityTier`'s one-time "just inserted"
    // change-detection tick has already passed — so a plain
    // `quality.is_changed()` gate here would leave a freshly spawned camera
    // with NO `Msaa` component at all (no MSAA, i.e. visibly aliased/dashed
    // thin geometry like roads/route stripes) until the player happened to
    // touch the quality selector. Backfill any camera missing it every
    // frame (cheap: at most one camera), and only redo the full sweep when
    // the tier actually changes.
    for camera in &cameras_missing_msaa {
        commands.entity(camera).insert(msaa);
    }
    // Same backfill problem for `DistanceFog` on tiers that want it
    // (Potato/Low): a freshly spawned camera otherwise renders with no fog
    // component at all until the tier changes. Color starts at the
    // `Color::WHITE` default; `daynight::apply_day_night_system` (ordered
    // right after this system, same frame) immediately overwrites it with
    // the real sky-matched color, so there's no visible flash.
    // Fog tiers also switch the camera to `Tonemapping::None`. Fog blends
    // toward `fog.color` BEFORE the in-shader tonemapper runs, while the
    // `ClearColor` sky behind it is written raw — so with the default
    // TonyMcMapface curve a fully fogged fragment lands visibly darker/
    // cooler than the sky it must merge into, drawing a hard seam along
    // the horizon (measured: sky #DFE6EA vs fully-fogged terrain #B5BABC).
    // The fog tiers are exactly the `unlit_material` tiers (Potato/Low),
    // where every material is a flat artist-picked sRGB color and there is
    // no HDR lighting to compress, so bypassing the tonemapper both fixes
    // the seam exactly (fully-fogged pixel == sky pixel) and renders the
    // palette faithfully. Lit tiers (Medium/High) keep Bevy's default.
    //
    // IMPORTANT (horizon "paper map" fix): `mf-game`'s `camera.rs` spawns the
    // in-game camera at `Startup` *already carrying* a long-range
    // `DistanceFog` (start 8km / end 55km) tuned for the Medium/High framing.
    // On the fog tiers (Potato/Low) the draw distance is only 3-6km, so that
    // 8km fog never engages and the horizon renders raw un-fogged terrain and
    // aliased road scribbles — exactly the reported bug. The old backfill
    // only touched cameras `Without<DistanceFog>`, so a camera that already
    // had the spawn-time fog was never corrected to the per-tier knob values,
    // and `daynight` only syncs fog *color*, never the falloff. Reconcile the
    // falloff (and `Tonemapping::None`, see note above) on EVERY camera every
    // frame when the tier wants fog — cheap, one camera — so the knob is
    // authoritative regardless of any spawn-time or Medium-default fog.
    if let Some((start, end)) = knobs.fog {
        for (camera, mut fog, tonemapping) in &mut cameras_with_fog {
            if !fog_falloff_matches(&fog.falloff, start, end) {
                fog.falloff = FogFalloff::Linear { start, end };
            }
            // Guarded so we only re-insert (and respecialize the pipeline)
            // when it isn't already `None`, not every frame.
            if tonemapping != Some(&Tonemapping::None) {
                commands.entity(camera).insert(Tonemapping::None);
            }
        }
        for camera in &cameras_missing_fog {
            commands.entity(camera).insert((
                DistanceFog {
                    falloff: FogFalloff::Linear { start, end },
                    ..default()
                },
                Tonemapping::None,
            ));
        }
    }
    // Bloom backfill for Medium/High: camera may spawn after the tier's
    // first change tick. Intensity starts at 0 (day); `sync_bloom_system`
    // ramps it with night_factor. Mutate `Camera.hdr` in place — never
    // replace the whole `Camera` component (would wipe viewport/order).
    if knobs.bloom_enabled {
        for camera in &cameras_missing_bloom {
            commands
                .entity(camera)
                .insert(bloom_settings(0.0, *quality));
            if let Ok(mut cam) = camera_hdr.get_mut(camera) {
                cam.hdr = true;
            }
        }
    }
    if !quality.is_changed() {
        return;
    }
    for camera in &cameras {
        commands.entity(camera).insert(msaa);
        match knobs.fog {
            Some((start, end)) => {
                commands.entity(camera).insert((
                    DistanceFog {
                        falloff: FogFalloff::Linear { start, end },
                        ..default()
                    },
                    Tonemapping::None,
                ));
            }
            None => {
                commands
                    .entity(camera)
                    .remove::<DistanceFog>()
                    .insert(Tonemapping::default());
            }
        }
        if knobs.bloom_enabled {
            commands
                .entity(camera)
                .insert(bloom_settings(0.0, *quality));
            if let Ok(mut cam) = camera_hdr.get_mut(camera) {
                cam.hdr = true;
            }
        } else {
            commands.entity(camera).remove::<Bloom>();
            if let Ok(mut cam) = camera_hdr.get_mut(camera) {
                cam.hdr = false;
            }
        }
    }
    shadow_map.size = knobs.shadow_map_size.unwrap_or(2048) as usize;
}

fn bloom_settings(intensity: f32, tier: QualityTier) -> Bloom {
    // Medium uses a smaller mip chain to keep the night bloom pass inside
    // the ~1.5ms CI-smoke budget; High gets the default NATURAL resolution.
    let max_mip = match tier {
        QualityTier::High => 512,
        _ => 256,
    };
    Bloom {
        intensity,
        low_frequency_boost: 0.55,
        low_frequency_boost_curvature: 0.9,
        high_pass_frequency: 1.0,
        prefilter: BloomPrefilter {
            threshold: BLOOM_PREFILTER_THRESHOLD,
            threshold_softness: BLOOM_PREFILTER_SOFTNESS,
        },
        // Non-default prefilter requires Additive (Bevy bloom docs).
        composite_mode: BloomCompositeMode::Additive,
        max_mip_dimension: max_mip,
        scale: Vec2::ONE,
    }
}

/// Ramps `Bloom.intensity` with `night_factor` on bloom-enabled tiers.
/// Intensity 0 skips the bloom node entirely (day / dusk start).
/// Runs every frame (cheap f32 compare) so a late-spawned camera that just
/// received its Bloom backfill picks up the current night intensity without
/// waiting for the next day/night change tick.
fn sync_bloom_system(
    quality: Res<QualityTier>,
    day_night: Res<DayNightState>,
    mut blooms: Query<&mut Bloom, With<Camera3d>>,
) {
    if !quality.knobs().bloom_enabled {
        return;
    }
    let intensity = BLOOM_INTENSITY_NIGHT * day_night.night_factor.clamp(0.0, 1.0);
    for mut bloom in &mut blooms {
        if (bloom.intensity - intensity).abs() > 1e-4 {
            bloom.intensity = intensity;
        }
    }
}
