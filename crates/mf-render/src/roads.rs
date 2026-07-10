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
// Widened ~1.5x from real-world-ish 40/24/13: at overview zoom the true
// widths are a few pixels and vanish into the bright ground (the oldest
// render-backlog item, owner-flagged twice). Slight exaggeration is the
// standard map-style tradeoff.
const ARTERIAL_WIDTH: f64 = 60.0;
const COLLECTOR_WIDTH: f64 = 36.0;
const LOCAL_WIDTH: f64 = 20.0;
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
                apply_quality_to_roads_material_system.in_set(crate::MfRenderSet::Dynamic),
            ),
        );
    }
}

#[derive(Resource, Default)]
struct RoadsState {
    /// Cheap structural signature: `(fields version, roads.len(), total
    /// point count)`. Road geometry never changes after `ready` in v1, but
    /// the terrain the ribbons drape over rebuilds on every fields version —
    /// baking only once left roads buried under relief that arrived in a
    /// later version (the residual half of the roads race).
    signature: Option<(u32, usize, usize)>,
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
    quality: Res<QualityTier>,
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
    let signature = (f.version, city_json.roads.len(), total_points);
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
        // Follow the terrain, not just the sparse simplified vertices.
        let pts = crate::mesh_utils::densify_polyline(&pts, 24.0);
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
        // `Blend`, not `Opaque`: this surface renders at alpha 1.0 except
        // during the subway-view fade (`subway.rs`'s
        // `fade_road_and_stripe_alpha_system`, which lowers alpha toward
        // `FADED_ALPHA`). An earlier version of this fix created this
        // `Opaque` and had `subway.rs` flip it to `Blend` only while
        // actually translucent, to skip the transparent pass (no depth
        // write, per-entity sort, guaranteed overdraw) for the overwhelming
        // majority of opaque-alpha frames. That broke rendering in
        // practice — verified via headless screenshot A/B diffing, not just
        // suspected — regardless of whether the mode was flipped by
        // mutating the existing material asset in place or by swapping in a
        // freshly-added material and reassigning the entity's
        // `MeshMaterial3d` handle (which should force Bevy to re-queue the
        // entity into the correct render phase, and didn't fix it either):
        // both left the road/stripe geometry either invisible or wrongly
        // blended once subway view settled. Root cause not fully isolated
        // within this fix's scope; staying `Blend` unconditionally here
        // (this crate's second blanket-material decision, `roads.rs`
        // rebuild_routes's normal stripe in `transit.rs` is the other one)
        // is the correctness-first fallback so the rendered scene doesn't
        // change.
        //
        // `cull_mode`/`double_sided` are fixed below (Part 2 of issue #5):
        // `append_ribbon`'s winding is verified CCW-from-+Y (mesh_utils.rs),
        // making single-sided/back-face-culled correct for a ribbon only
        // ever seen from above. This alone wasn't enough during
        // verification — A/B screenshot diffing turned up a brightness
        // regression in the subway+Potato combination, root-caused to
        // `unlit` going stale (baked in once at build time, never updated
        // on a runtime tier change, unlike buildings.rs/terrain.rs) so roads
        // stayed on the LIT path after switching to Potato; combined with
        // `double_sided`'s back-face normal flip, the "corrected" winding
        // changed which normal direction actually receives direct light,
        // visibly brightening the surface. `apply_quality_to_roads_material_
        // system` below fixes that root cause (keeps `unlit` in sync with
        // the tier, like buildings/terrain already do), which is what makes
        // single-siding safe here.
        // Color at the MATERIAL level, not vertex colors: vertex colors do
        // not reach the shader for alpha-blended StandardMaterials in this
        // Bevy 0.16 setup (terrain's vertex colors work fine - it is
        // Opaque), which is why roads rendered white-on-white for so long
        // ("streets barely read", the oldest item in the render backlog).
        // A single-color ribbon never needed per-vertex color anyway.
        let material = materials.add(StandardMaterial {
            base_color: road_color,
            unlit,
            alpha_mode: AlphaMode::Blend,
            // Fully matte: with the ribbon winding fixed and single-siding
            // on, these surfaces now receive direct sun for the first time,
            // and the default reflectance (4% F0) paints a specular sheen
            // that blows near-black asphalt out to white at high sun angles.
            perceptual_roughness: 1.0,
            reflectance: 0.0,
            ..default()
        });
        let mut entity_commands = commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            Visibility::default(),
            RoadSurface,
            RoadClassSurface,
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
        // `Blend` for the same reason as the class materials above.
        // `double_sided`/`cull_mode` stay as-is (unlike the class materials):
        // this one is *always* lit by design (`unlit: false` hardcoded, not
        // tier-driven — it only exists on medium/high tier to begin with),
        // so there's no stale-`unlit` root cause to fix here the way there
        // was for the class materials, and it wasn't independently
        // re-verified as single-siding-safe under the always-lit path. Low
        // stakes either way: a 1-3m hairline accent, not the road fill.
        let material = materials.add(StandardMaterial {
            // Same transparent-pass vertex-color caveat as the class
            // materials above: color the material directly.
            base_color: palette::road_edge(),
            unlit: false,
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 1.0,
            reflectance: 0.0,
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
    quality: Res<QualityTier>,
    roads: Query<&MeshMaterial3d<StandardMaterial>, With<RoadClassSurface>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !quality.is_changed() {
        return;
    }
    let unlit = quality.knobs().unlit_material;
    for handle in &roads {
        if let Some(mat) = materials.get_mut(&handle.0) {
            mat.unlit = unlit;
        }
    }
}
