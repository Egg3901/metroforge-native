//! Pedestrian/passenger agents (spec §3.3 `agents.rs`): one mesh of small
//! flat quads, rebuilt from `LatestFrame`'s stride-3 agent array whenever it
//! changes, capped per tier (`QualityTier::knobs().agent_cap`; potato = 0,
//! i.e. agents are entirely disabled on the weakest tier).
//!
//! One `Mesh` asset lives for the app's whole life (created once); a rebuild
//! overwrites its attributes/indices in place via `Assets<Mesh>::get_mut`
//! rather than `meshes.add`-ing a fresh asset every pass — the latter would
//! be a brand-new GPU buffer allocation + upload plus an old-asset teardown
//! every single frame, when `LatestFrame` (and thus the agent positions)
//! only actually changes at the sim's tick rate.

use bevy::prelude::*;
use bevy::render::mesh::PrimitiveTopology;
use bevy::render::render_asset::RenderAssetUsages;

use mf_state::{HeightAt, LatestFrame, QualityTier};

use crate::mesh_utils::MeshBuffers;

const AGENT_SIZE: f32 = 2.2;
const AGENT_Y_OFFSET: f32 = 0.8;

// Phase: 0 walk, 1 ride, 2 wait (spec §1.2).
// Art direction: vivid color is reserved for the transit network — agents
// stay greyscale, with phase readable via brightness only.
fn phase_color(phase: f32) -> Color {
    if phase < 0.5 {
        Color::srgb(0.55, 0.57, 0.6) // walk: mid grey
    } else if phase < 1.5 {
        Color::srgb(0.72, 0.74, 0.76) // ride: lighter grey
    } else {
        Color::srgb(0.40, 0.42, 0.45) // wait: darker grey
    }
}

pub struct MfAgentsPlugin;

impl Plugin for MfAgentsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AgentsState>().add_systems(
            Update,
            (
                update_agents_system,
                apply_quality_to_agents_material_system,
            )
                .in_set(crate::MfRenderSet::Dynamic),
        );
    }
}

#[derive(Resource, Default)]
struct AgentsState {
    entity: Option<Entity>,
    material: Option<Handle<StandardMaterial>>,
    /// Created lazily the first time there's at least one agent to draw, then
    /// reused for the rest of the app's life — its attributes are overwritten
    /// in place on each rebuild instead of allocating a new `Mesh` asset.
    mesh: Option<Handle<Mesh>>,
}

/// Flip the shared agents material's `unlit` flag when the quality tier
/// changes, without waiting for the next `update_agents_system` rebuild.
fn apply_quality_to_agents_material_system(
    quality: Res<QualityTier>,
    state: Res<AgentsState>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !quality.is_changed() {
        return;
    }
    let Some(handle) = &state.material else {
        return;
    };
    if let Some(mat) = materials.get_mut(handle) {
        mat.unlit = quality.knobs().unlit_material;
    }
}

#[allow(clippy::too_many_arguments)]
fn update_agents_system(
    frame: Res<LatestFrame>,
    height_at: Res<HeightAt>,
    quality: Res<QualityTier>,
    mut state: ResMut<AgentsState>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut visibility: Query<&mut Visibility>,
) {
    let cap = quality.knobs().agent_cap as usize;
    let entity = if let Some(e) = state.entity {
        e
    } else {
        // Flat +Y quads viewed only from above (top-down camera) — verified
        // CCW-from-+Y below (fixed to match), so single-sided is correct.
        let mat = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            unlit: quality.knobs().unlit_material,
            ..default()
        });
        let e = commands
            .spawn((
                Mesh3d(Handle::default()),
                MeshMaterial3d(mat.clone()),
                Transform::IDENTITY,
                Visibility::Hidden,
                Name::new("agents"),
            ))
            .id();
        state.entity = Some(e);
        state.material = Some(mat);
        e
    };

    if cap == 0 {
        if let Ok(mut vis) = visibility.get_mut(entity) {
            *vis = Visibility::Hidden;
        }
        return;
    }

    // `LatestFrame` arrives at the sim's tick rate, well under render frame
    // rate; a `QualityTier` change can move `cap` (and thus `draw_count`)
    // without any new frame data. Neither changing means the agent mesh
    // can't possibly need to look any different from what's already built.
    if !frame.is_changed() && !quality.is_changed() {
        return;
    }
    let Some(f) = &frame.0 else {
        return;
    };
    let draw_count = (f.agent_count as usize).min(cap);
    if draw_count == 0 {
        if let Ok(mut vis) = visibility.get_mut(entity) {
            *vis = Visibility::Hidden;
        }
        return;
    }

    let mut buf = MeshBuffers::new();
    let half = AGENT_SIZE * 0.5;
    for i in 0..draw_count {
        let base = i * 3;
        let (Some(&x), Some(&y), Some(&phase)) = (
            f.agents.get(base),
            f.agents.get(base + 1),
            f.agents.get(base + 2),
        ) else {
            break;
        };
        let ground_y = height_at.sample(x, y) + AGENT_Y_OFFSET;
        let color = phase_color(phase);
        // Winding vs the declared `+Y` normal: same corner pattern as
        // `terrain.rs`'s grid quad ((x0,z0),(x1,z0),(x1,z1),(x0,z1)), which
        // works out to a `-Y` cross product (CCW from below, not above) —
        // see the comment there for the derivation. Swapping the middle two
        // corners to ((x0,z0),(x0,z1),(x1,z1),(x1,z0)) reverses the quad and
        // flips the cross product to `+Y`, matching the declared normal.
        buf.push_flat_quad(
            Vec3::new(x - half, ground_y, y - half),
            Vec3::new(x - half, ground_y, y + half),
            Vec3::new(x + half, ground_y, y + half),
            Vec3::new(x + half, ground_y, y - half),
            Vec3::Y,
            color,
        );
    }

    // Build the new attributes as a throwaway CPU-side `Mesh` (no asset
    // registration, no GPU cost), then transplant them into the one
    // long-lived asset via `get_mut` so this rebuild re-uploads the existing
    // GPU buffers instead of allocating fresh ones and tearing down the old.
    let is_new_mesh = state.mesh.is_none();
    let mesh_handle = state
        .mesh
        .get_or_insert_with(|| {
            meshes.add(Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            ))
        })
        .clone();
    if is_new_mesh {
        commands.entity(entity).insert(Mesh3d(mesh_handle.clone()));
    }
    let mut built = buf.build();
    if let Some(mesh) = meshes.get_mut(&mesh_handle) {
        if let Some(v) = built.remove_attribute(Mesh::ATTRIBUTE_POSITION) {
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, v);
        }
        if let Some(v) = built.remove_attribute(Mesh::ATTRIBUTE_NORMAL) {
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, v);
        }
        if let Some(v) = built.remove_attribute(Mesh::ATTRIBUTE_COLOR) {
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, v);
        }
        if let Some(idx) = built.remove_indices() {
            mesh.insert_indices(idx);
        }
    }
    if let Ok(mut vis) = visibility.get_mut(entity) {
        *vis = Visibility::Visible;
    }
}
