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
mod photomode;
mod reveal;
mod roads;
mod sky;
mod subway;
mod terrain;
mod transit;
mod trees;
mod vehicles;

use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::pbr::{DirectionalLightShadowMap, DistanceFog, FogFalloff};
use bevy::prelude::*;

use mf_state::{QualityTier, Theme};

pub use buildings::BuildingsDenseCenter;
pub use photomode::PhotoModeRender;

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
        .add_plugins((
            reveal::MfRevealPlugin,
            sky::MfSkyPlugin,
            terrain::MfTerrainPlugin,
            roads::MfRoadsPlugin,
            buildings::MfBuildingsPlugin,
            transit::MfTransitPlugin,
            trees::MfTreesPlugin,
            vehicles::MfVehiclesPlugin,
            agents::MfAgentsPlugin,
            daynight::MfDayNightPlugin,
            atmosphere::MfAtmospherePlugin,
            subway::MfSubwayPlugin,
            outline::MfOutlinePlugin,
            photomode::MfPhotoModeRenderPlugin,
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
fn apply_quality_render_settings_system(
    quality: Res<QualityTier>,
    mut shadow_map: ResMut<DirectionalLightShadowMap>,
    mut commands: Commands,
    cameras: Query<Entity, With<Camera3d>>,
    cameras_missing_msaa: Query<Entity, (With<Camera3d>, Without<Msaa>)>,
    cameras_missing_fog: Query<Entity, (With<Camera3d>, Without<DistanceFog>)>,
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
    if let Some((start, end)) = knobs.fog {
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
    }
    shadow_map.size = knobs.shadow_map_size.unwrap_or(2048) as usize;
}
