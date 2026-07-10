//! Road ribbons (spec §3.3 `roads.rs`): one merged mesh per class
//! (arterial/collector/local — "≤3 meshes"), rebuilt once per city load.
//! Local-road visibility is LOD-toggled by camera height. All classes are
//! the same rich-black `ROAD` color per art-direction §2 ("differentiate by
//! width only"); arterials additionally get a 1m `ROAD_EDGE` hairline on
//! medium/high tier.

use bevy::prelude::*;

use mf_state::{CurrentCity, HeightAt, QualityTier};

use crate::mesh_utils::{append_ribbon, MeshBuffers};
use crate::palette;

/// Road surface sits just above bare ground (spec: "heightAt + 0.5").
const ROAD_Y_OFFSET: f32 = 0.5;
/// Widths per spec §3.3 (already includes `roadScale` multiplication).
const ARTERIAL_WIDTH: f64 = 40.0;
const COLLECTOR_WIDTH: f64 = 24.0;
const LOCAL_WIDTH: f64 = 13.0;
/// Camera height above which local-road detail is hidden (LOD).
const LOCAL_ROAD_LOD_HEIGHT: f32 = 4_000.0;

pub struct MfRoadsPlugin;

impl Plugin for MfRoadsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoadsState>().add_systems(
            Update,
            (
                build_roads_system.in_set(crate::MfRenderSet::Statics),
                local_road_lod_system.in_set(crate::MfRenderSet::Dynamic),
            ),
        );
    }
}

#[derive(Resource, Default)]
struct RoadsState {
    /// Cheap structural signature: `(roads.len(), total point count)`. Roads
    /// never change after `ready` in v1, but this keys the rebuild the same
    /// way the other layers key off `fieldsVersion`/UI structural hashes.
    signature: Option<(usize, usize)>,
    entities: Vec<Entity>,
    local_entity: Option<Entity>,
}

#[derive(Component)]
struct LocalRoadMarker;

/// Marker on every road-surface mesh entity (all classes + the arterial
/// hairline edge) so `subway.rs` can fade their alpha toward 0.3 in subway
/// view without reaching into this module's internals.
#[derive(Component)]
pub struct RoadSurface;

fn build_roads_system(
    mut commands: Commands,
    city: Res<CurrentCity>,
    height_at: Res<HeightAt>,
    mut state: ResMut<RoadsState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    quality: Res<QualityTier>,
) {
    let Some(city_json) = &city.static_city else {
        return;
    };
    let total_points: usize = city_json.roads.iter().map(|r| r.points.len()).sum();
    let signature = (city_json.roads.len(), total_points);
    if state.signature == Some(signature) {
        return;
    }
    state.signature = Some(signature);

    for e in state.entities.drain(..) {
        commands.entity(e).despawn();
    }
    state.local_entity = None;

    let road_scale = city_json.road_scale as f32;
    let road_color = palette::road();
    let unlit = quality.knobs().unlit_material;

    let mut by_class: [MeshBuffers; 3] =
        [MeshBuffers::new(), MeshBuffers::new(), MeshBuffers::new()];
    // index 0 = arterial, 1 = collector, 2 = local
    let mut edge_buf = MeshBuffers::new();

    for road in &city_json.roads {
        let pts: Vec<Vec2> = road
            .points
            .chunks_exact(2)
            .map(|c| Vec2::new(c[0] as f32, c[1] as f32))
            .collect();
        if pts.len() < 2 {
            continue;
        }
        let (idx, width) = match road.cls.as_str() {
            "arterial" => (0usize, ARTERIAL_WIDTH as f32 * road_scale),
            "collector" => (1usize, COLLECTOR_WIDTH as f32 * road_scale),
            _ => (2usize, LOCAL_WIDTH as f32 * road_scale),
        };
        append_ribbon(
            &mut by_class[idx],
            &pts,
            ROAD_Y_OFFSET,
            width,
            road_color,
            |x, z| height_at.sample(x, z),
        );
        if idx == 0 {
            append_ribbon(
                &mut edge_buf,
                &pts,
                ROAD_Y_OFFSET + 0.05,
                width + 2.0,
                palette::road_edge(),
                |x, z| height_at.sample(x, z),
            );
        }
    }

    let names = ["arterial", "collector", "local"];
    for (i, buf) in by_class.into_iter().enumerate() {
        if buf.is_empty() {
            continue;
        }
        let mesh = meshes.add(buf.build());
        let material = materials.add(StandardMaterial {
            double_sided: true,
            cull_mode: None,
            base_color: Color::WHITE,
            unlit,
            alpha_mode: AlphaMode::Blend,
            ..default()
        });
        let mut entity_commands = commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            Visibility::default(),
            RoadSurface,
            Name::new(format!("roads-{}", names[i])),
        ));
        if names[i] == "local" {
            entity_commands.insert(LocalRoadMarker);
            state.local_entity = Some(entity_commands.id());
        }
        state.entities.push(entity_commands.id());
    }

    // Arterial hairline edge, medium/high tier only (art-direction §1).
    if !unlit && !edge_buf.is_empty() {
        let mesh = meshes.add(edge_buf.build());
        let material = materials.add(StandardMaterial {
            double_sided: true,
            cull_mode: None,
            base_color: Color::WHITE,
            unlit: false,
            alpha_mode: AlphaMode::Blend,
            ..default()
        });
        let e = commands
            .spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material),
                Transform::IDENTITY,
                Visibility::default(),
                RoadSurface,
                Name::new("roads-arterial-edge"),
            ))
            .id();
        state.entities.push(e);
    }
}

/// Hide the local-roads mesh once the camera climbs above
/// [`LOCAL_ROAD_LOD_HEIGHT`] (spec: "Local-roads Visibility toggled by
/// camera height"). Reads Bevy's own `Camera3d`/`Transform` rather than
/// `mf-game`'s `CameraRig` component, since `mf-render` must not depend on
/// `mf-game` (the dependency runs the other way).
fn local_road_lod_system(
    state: Res<RoadsState>,
    cameras: Query<&Transform, With<Camera3d>>,
    mut visibility: Query<&mut Visibility>,
) {
    let Some(entity) = state.local_entity else {
        return;
    };
    let Ok(cam_transform) = cameras.single() else {
        return;
    };
    let Ok(mut vis) = visibility.get_mut(entity) else {
        return;
    };
    *vis = if cam_transform.translation.y > LOCAL_ROAD_LOD_HEIGHT {
        Visibility::Hidden
    } else {
        Visibility::Visible
    };
}
