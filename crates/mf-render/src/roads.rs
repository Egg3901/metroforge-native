//! Road ribbons (spec §3.3 `roads.rs`): one merged mesh per class
//! (arterial/collector/local — "≤3 meshes"), rebuilt once per city load.
//! Local-road visibility is LOD-toggled by camera height. All classes are
//! the same rich-black `ROAD` color per art-direction §2 ("differentiate by
//! width only"); arterials additionally get a 1m `ROAD_EDGE` hairline on
//! medium/high tier.

use bevy::prelude::*;
use bevy::render::mesh::MeshAabb;

use mf_state::{CurrentCity, EffectiveKnobs, HeightAt, Theme};

use crate::mesh_utils::{append_ribbon, MeshBuffers};
use crate::palette;
use crate::RenderCacheStats;

/// Road surface lift above ground. The spec said 0.5, but at overview zoom
/// on near-flat terrain a 0.5m offset loses the depth fight against the
/// terrain mesh at grazing angles (roads visibly vanish from skyline
/// framings; found on the flattened real-city relief). 2m is still
/// imperceptible as elevation at street zoom and keeps the ribbons winning
/// depth at distance.
pub(crate) const ROAD_Y_OFFSET: f32 = 2.0;
/// Water-crossing segments ride a fixed deck height instead of hugging
/// `WATER_LEVEL_Y` — a road at water level renders as a barely-visible black
/// sliver mid-river (owner-flagged on the East River bridges). A flat
/// causeway a few meters up reads as a bridge at city zoom.
pub(crate) const BRIDGE_DECK_Y: f32 = 8.0;
/// Widths per spec §3.3 (already includes `roadScale` multiplication).
// Widened ~1.5x from real-world-ish 40/24/13: at overview zoom the true
// widths are a few pixels and vanish into the bright ground (the oldest
// render-backlog item, owner-flagged twice). Slight exaggeration is the
// standard map-style tradeoff.
// `pub(crate)`: `terrain.rs` reuses these as the terrain-grading corridor
// half-width source (see `terrain::grade_terrain`) so the graded corridor
// stays in lockstep with the ribbon width instead of drifting via a
// duplicated constant.
pub(crate) const ARTERIAL_WIDTH: f64 = 60.0;
pub(crate) const COLLECTOR_WIDTH: f64 = 36.0;
pub(crate) const LOCAL_WIDTH: f64 = 20.0;
/// Camera height above which local-road detail is hidden (LOD).
const LOCAL_ROAD_LOD_HEIGHT: f32 = 4_000.0;
/// Collectors hide above this height (arterials stay for skyline structure).
const COLLECTOR_ROAD_LOD_HEIGHT: f32 = 8_000.0;

pub struct MfRoadsPlugin;

impl Plugin for MfRoadsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoadsState>().add_systems(
            Update,
            (
                build_roads_system.in_set(crate::MfRenderSet::Statics),
                road_lod_system.in_set(crate::MfRenderSet::Dynamic),
                apply_quality_to_roads_material_system.in_set(crate::MfRenderSet::Dynamic),
            ),
        );
    }
}

#[derive(Resource, Default)]
struct RoadsState {
    /// Cheap structural signature: `(fields version, roads.len(), total
    /// point count, theme, densify step bits)`. Road geometry never changes
    /// after `ready` in v1, but the terrain the ribbons drape over rebuilds
    /// on every fields version — baking only once left roads buried under
    /// relief that arrived in a later version (the residual half of the
    /// roads race). `Theme` rides along so a theme switch forces a full
    /// rebuild (road color is baked into mesh vertex color at build time).
    /// Densify step bits so a quality-tier change rebuilds at the new
    /// ribbon resolution.
    signature: Option<(u32, usize, usize, Theme, u32)>,
    /// Class entity ids (arterial/collector/local) — reused across rebuilds.
    class_entities: [Option<Entity>; 3],
    edge_entity: Option<Entity>,
    local_entity: Option<Entity>,
    collector_entity: Option<Entity>,
    /// Long-lived mesh assets reused via [`MeshBuffers::apply_to_mesh`].
    class_meshes: [Option<Handle<Mesh>>; 3],
    edge_mesh: Option<Handle<Mesh>>,
    class_materials: [Option<Handle<StandardMaterial>>; 3],
    edge_material: Option<Handle<StandardMaterial>>,
    /// Scratch buffers kept across rebuilds so vertex Vecs retain capacity.
    scratch_class: [MeshBuffers; 3],
    scratch_edge: MeshBuffers,
    /// Tracked so `road_lod_system` can hide the arterial mesh once the
    /// camera climbs above the active tier's fog `end` — above that height the
    /// whole network is fully fogged to the sky color anyway, so hiding it is
    /// free and removes the aliased sub-pixel scribbles it would otherwise
    /// draw at the horizon on the no-MSAA fog tiers (Potato/Low).
    arterial_entity: Option<Entity>,
}

#[derive(Component)]
struct LocalRoadMarker;

#[derive(Component)]
struct CollectorRoadMarker;

/// Marker on every road-surface mesh entity (all classes + the arterial
/// hairline edge) so `subway.rs` can fade their alpha toward 0.3 in subway
/// view without reaching into this module's internals.
#[derive(Component)]
pub struct RoadSurface;

/// Marker on just the 3 road-class entities (arterial/collector/local) —
/// NOT the arterial edge, which is intentionally always lit regardless of
/// tier. Lets `apply_quality_to_roads_material_system` retarget only the
/// materials whose `unlit` should track the tier.
#[derive(Component)]
struct RoadClassSurface;

#[allow(clippy::too_many_arguments)]
fn build_roads_system(
    mut commands: Commands,
    city: Res<CurrentCity>,
    fields: Res<mf_state::LatestFields>,
    height_at: Res<HeightAt>,
    mut state: ResMut<RoadsState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    effective: Res<EffectiveKnobs>,
    theme: Res<Theme>,
    mut stats: ResMut<RenderCacheStats>,
    counters: Res<crate::perf::PerfCounters>,
) {
    let Some(city_json) = &city.static_city else {
        return;
    };
    // RACE FIX: `ready` (roads) arrives before `fields` (terrain), and this
    // system builds exactly once per signature - building against the
    // placeholder flat HeightAt buried every road under the real relief
    // (intermittently, per frame timing: the recurring "why are the roads
    // never showing"). Wait for real terrain before baking.
    let Some(f) = &fields.0 else {
        return;
    };
    let total_points: usize = city_json.roads.iter().map(|r| r.points.len()).sum();
    let densify_step = effective.0.ribbon_densify_step_m;
    let signature = (
        f.version,
        city_json.roads.len(),
        total_points,
        *theme,
        densify_step.to_bits(),
    );
    if state.signature == Some(signature) {
        return;
    }
    let _span = tracing::info_span!("roads_rebuild").entered();
    let _timer = crate::perf::PerfSpan::start(&counters.roads_rebuild_us);
    state.signature = Some(signature);

    let road_scale = city_json.road_scale as f32;
    let road_color = palette::road();
    let unlit = effective.0.unlit_material;

    for buf in &mut state.scratch_class {
        buf.clear();
    }
    state.scratch_edge.clear();

    for road in &city_json.roads {
        let pts: Vec<Vec2> = road
            .points
            .chunks_exact(2)
            .map(|c| Vec2::new(c[0] as f32, c[1] as f32))
            .collect();
        if pts.len() < 2 {
            continue;
        }
        // Follow the terrain, not just the sparse simplified vertices.
        // Step is tiered: Potato/Low use coarser densify to cut rebuild
        // vertices and draw cost.
        let pts = crate::mesh_utils::densify_polyline(&pts, densify_step);
        let (idx, width) = match road.cls.as_str() {
            "arterial" => (0usize, ARTERIAL_WIDTH as f32 * road_scale),
            "collector" => (1usize, COLLECTOR_WIDTH as f32 * road_scale),
            _ => (2usize, LOCAL_WIDTH as f32 * road_scale),
        };
        let deck_height = |x: f32, z: f32| {
            let h = height_at.sample(x, z);
            if h <= crate::terrain::WATER_LEVEL_Y + 0.01 {
                BRIDGE_DECK_Y
            } else {
                h
            }
        };
        append_ribbon(
            &mut state.scratch_class[idx],
            &pts,
            ROAD_Y_OFFSET,
            width,
            road_color,
            deck_height,
        );
        if idx == 0 {
            append_ribbon(
                &mut state.scratch_edge,
                &pts,
                ROAD_Y_OFFSET + 0.05,
                width + 2.0,
                palette::road_edge(),
                deck_height,
            );
        }
    }

    let names = ["arterial", "collector", "local"];
    state.local_entity = None;
    state.collector_entity = None;

    #[allow(clippy::needless_range_loop)]
    for i in 0..3 {
        if state.scratch_class[i].is_empty() {
            if let Some(e) = state.class_entities[i].take() {
                commands.entity(e).despawn();
            }
            state.class_meshes[i] = None;
            state.class_materials[i] = None;
            continue;
        }
        let mesh_handle = state.class_meshes[i]
            .get_or_insert_with(|| {
                meshes.add(Mesh::new(
                    bevy::render::mesh::PrimitiveTopology::TriangleList,
                    bevy::render::render_asset::RenderAssetUsages::default(),
                ))
            })
            .clone();
        let aabb = {
            let mesh = meshes.get_mut(&mesh_handle).expect("road class mesh");
            state.scratch_class[i].apply_to_mesh(mesh);
            mesh.compute_aabb().unwrap_or_default()
        };
        let material_handle = state.class_materials[i]
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: road_color,
                    unlit,
                    alpha_mode: AlphaMode::Blend,
                    perceptual_roughness: 1.0,
                    reflectance: 0.0,
                    ..default()
                })
            })
            .clone();
        if let Some(mat) = materials.get_mut(&material_handle) {
            mat.base_color = road_color;
            mat.unlit = unlit;
        }
        let entity = if let Some(e) = state.class_entities[i] {
            if let Ok(mut commands_e) = commands.get_entity(e) {
                commands_e.insert((
                    Mesh3d(mesh_handle.clone()),
                    MeshMaterial3d(material_handle.clone()),
                    aabb,
                    Visibility::Visible,
                ));
                e
            } else {
                let mut entity_commands = commands.spawn((
                    Mesh3d(mesh_handle.clone()),
                    MeshMaterial3d(material_handle.clone()),
                    Transform::IDENTITY,
                    Visibility::default(),
                    aabb,
                    RoadSurface,
                    RoadClassSurface,
                    Name::new(format!("roads-{}", names[i])),
                ));
                if names[i] == "local" {
                    entity_commands.insert(LocalRoadMarker);
                } else if names[i] == "collector" {
                    entity_commands.insert(CollectorRoadMarker);
                }
                let id = entity_commands.id();
                state.class_entities[i] = Some(id);
                id
            }
        } else {
            let mut entity_commands = commands.spawn((
                Mesh3d(mesh_handle),
                MeshMaterial3d(material_handle),
                Transform::IDENTITY,
                Visibility::default(),
                aabb,
                RoadSurface,
                RoadClassSurface,
                Name::new(format!("roads-{}", names[i])),
            ));
            if names[i] == "local" {
                entity_commands.insert(LocalRoadMarker);
            } else if names[i] == "collector" {
                entity_commands.insert(CollectorRoadMarker);
            }
            let id = entity_commands.id();
            state.class_entities[i] = Some(id);
            id
        };
        if names[i] == "local" {
            state.local_entity = Some(entity);
        } else if names[i] == "collector" {
            state.collector_entity = Some(entity);
        } else if names[i] == "arterial" {
            state.arterial_entity = Some(entity);
        }
    }

    // Arterial hairline edge, medium/high tier only (art-direction §1).
    if !unlit && !state.scratch_edge.is_empty() {
        let mesh_handle = state
            .edge_mesh
            .get_or_insert_with(|| {
                meshes.add(Mesh::new(
                    bevy::render::mesh::PrimitiveTopology::TriangleList,
                    bevy::render::render_asset::RenderAssetUsages::default(),
                ))
            })
            .clone();
        let aabb = {
            let mesh = meshes.get_mut(&mesh_handle).expect("road edge mesh");
            state.scratch_edge.apply_to_mesh(mesh);
            mesh.compute_aabb().unwrap_or_default()
        };
        let material_handle = state
            .edge_material
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: palette::road_edge(),
                    unlit: false,
                    alpha_mode: AlphaMode::Blend,
                    perceptual_roughness: 1.0,
                    reflectance: 0.0,
                    ..default()
                })
            })
            .clone();
        if let Some(mat) = materials.get_mut(&material_handle) {
            mat.base_color = palette::road_edge();
        }
        if let Some(e) = state.edge_entity {
            if let Ok(mut commands_e) = commands.get_entity(e) {
                commands_e.insert((
                    Mesh3d(mesh_handle),
                    MeshMaterial3d(material_handle),
                    aabb,
                    Visibility::Visible,
                ));
            } else {
                state.edge_entity = Some(
                    commands
                        .spawn((
                            Mesh3d(mesh_handle),
                            MeshMaterial3d(material_handle),
                            Transform::IDENTITY,
                            Visibility::default(),
                            aabb,
                            RoadSurface,
                            Name::new("roads-arterial-edge"),
                        ))
                        .id(),
                );
            }
        } else {
            state.edge_entity = Some(
                commands
                    .spawn((
                        Mesh3d(mesh_handle),
                        MeshMaterial3d(material_handle),
                        Transform::IDENTITY,
                        Visibility::default(),
                        aabb,
                        RoadSurface,
                        Name::new("roads-arterial-edge"),
                    ))
                    .id(),
            );
        }
    } else if let Some(e) = state.edge_entity.take() {
        commands.entity(e).despawn();
        state.edge_mesh = None;
        state.edge_material = None;
    }
    stats.road_entities = state.class_entities.iter().filter(|e| e.is_some()).count()
        + usize::from(state.edge_entity.is_some());
}

/// Hide local/collector road meshes once the camera climbs above their LOD
/// heights (spec: "Local-roads Visibility toggled by camera height";
/// collectors follow at a higher threshold so arterials alone remain for
/// skyline structure). Reads Bevy's own `Camera3d`/`Transform` rather than
/// `mf-game`'s `CameraRig` component, since `mf-render` must not depend on
/// `mf-game` (the dependency runs the other way).
fn road_lod_system(
    state: Res<RoadsState>,
    effective: Res<EffectiveKnobs>,
    cameras: Query<&Transform, With<Camera3d>>,
    mut visibility: Query<&mut Visibility>,
    counters: Res<crate::perf::PerfCounters>,
) {
    let _span = tracing::info_span!("road_lod").entered();
    let _timer = crate::perf::PerfSpan::start(&counters.road_lod_us);
    let Ok(cam_transform) = cameras.single() else {
        return;
    };
    let y = cam_transform.translation.y;

    // On the fog tiers (Potato/Low) everything past the fog `end` distance is
    // fully blended to the sky color, so any road mesh whose nearest point is
    // beyond that is invisible regardless — but with no MSAA those distant
    // sub-pixel ribbons still alias into the black "scribbles" the horizon
    // showed. Clamp each class's hide-height to the fog `end` so it drops out
    // as soon as it's fully fogged, and give arterials (which otherwise never
    // hide, to hold skyline structure on the un-fogged tiers) a hide-height at
    // all. Off the fog tiers the original height-only LOD is unchanged and
    // arterials never hide.
    let fog_end = effective.0.fog.map(|(_, end)| end);
    let local_hide = fog_end.map_or(LOCAL_ROAD_LOD_HEIGHT, |e| e.min(LOCAL_ROAD_LOD_HEIGHT));
    let collector_hide = fog_end.map_or(COLLECTOR_ROAD_LOD_HEIGHT, |e| {
        e.min(COLLECTOR_ROAD_LOD_HEIGHT)
    });

    // Gate the write through `set_visibility_if_changed` (perf pass): Bevy's
    // change detection fires on every `DerefMut` of `Visibility`, so writing
    // it unconditionally each frame would dirty the render world needlessly.
    let set_vis =
        |entity: Option<Entity>, hide_above: Option<f32>, vis: &mut Query<&mut Visibility>| {
            let Some(entity) = entity else {
                return;
            };
            let Ok(mut v) = vis.get_mut(entity) else {
                return;
            };
            let next = match hide_above {
                Some(h) if y > h => Visibility::Hidden,
                _ => Visibility::Visible,
            };
            crate::perf::set_visibility_if_changed(&mut v, next, Some(&counters));
        };

    set_vis(state.local_entity, Some(local_hide), &mut visibility);
    set_vis(
        state.collector_entity,
        Some(collector_hide),
        &mut visibility,
    );
    // Arterials: only cull on the fog tiers (above the fog `end` height);
    // otherwise they stay for skyline structure.
    set_vis(state.arterial_entity, fog_end, &mut visibility);
}

/// Flip the 3 road-class materials' `unlit` flag when the quality tier
/// changes, mirroring `buildings.rs`'s `apply_quality_to_buildings_material_
/// system` and `terrain.rs`'s equivalent. Without this, `unlit` — baked in
/// once at `build_roads_system` time — goes stale after a runtime tier
/// change (e.g. dropping to Potato mid-session): roads keep rendering via
/// the LIT path with a directional light, while terrain/buildings correctly
/// switch to flat unlit vertex colors, and the mismatch is visible (found
/// via A/B screenshot diffing while fixing this crate's winding/culling —
/// see the `append_ribbon` comment in mesh_utils.rs). The arterial edge
/// deliberately stays out of this (`RoadClassSurface` excludes it) since
/// it's always lit by design, independent of tier.
fn apply_quality_to_roads_material_system(
    effective: Res<EffectiveKnobs>,
    roads: Query<&MeshMaterial3d<StandardMaterial>, With<RoadClassSurface>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !effective.is_changed() {
        return;
    }
    let unlit = effective.0.unlit_material;
    for handle in &roads {
        if let Some(mat) = materials.get_mut(&handle.0) {
            mat.unlit = unlit;
        }
    }
}
