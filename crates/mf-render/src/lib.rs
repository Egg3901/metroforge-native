//! `mf-render` — the 3D renderer (spec §3.3), composed as `MfRenderPlugin`.
//! Mirror's-Edge art direction (`art-direction.md`, BINDING): stark white
//! city, black streets, vivid color reserved exclusively for the transit
//! network the player builds. See `crates/mf-render/src/palette.rs` for the
//! single source of truth on colors.
//!
//! Layers, in bake/update order (see [`MfRenderSet`]):
//! terrain -> roads/buildings/transit (static, cached by version/structural
//! signature) -> vehicles/agents/daynight/subway (dynamic, every frame).

mod agents;
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
mod subway;
mod terrain;
mod transit;
mod trees;
mod vehicles;

use bevy::pbr::DirectionalLightShadowMap;
use bevy::prelude::*;

use mf_state::{QualityTier, Theme};

pub use buildings::BuildingsDenseCenter;

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
            subway::MfSubwayPlugin,
            outline::MfOutlinePlugin,
        ))
        .add_systems(
            Update,
            (
                sync_theme_system.before(MfRenderSet::Terrain),
                apply_quality_render_settings_system.in_set(MfRenderSet::Dynamic),
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
    if !quality.is_changed() {
        return;
    }
    for camera in &cameras {
        commands.entity(camera).insert(msaa);
    }
    shadow_map.size = knobs.shadow_map_size.unwrap_or(2048) as usize;
}
