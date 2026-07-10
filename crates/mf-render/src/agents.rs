//! Pedestrian/passenger agents (spec §3.3 `agents.rs`): one mesh of small
//! flat quads, rebuilt every frame from `LatestFrame`'s stride-3 agent
//! array, capped per tier (`QualityTier::knobs().agent_cap`; potato = 0,
//! i.e. agents are entirely disabled on the weakest tier).

use bevy::prelude::*;

use mf_state::{HeightAt, LatestFrame, QualityTier};

use crate::mesh_utils::MeshBuffers;

const AGENT_SIZE: f32 = 2.2;
const AGENT_Y_OFFSET: f32 = 0.8;

// Phase: 0 walk, 1 ride, 2 wait (spec §1.2).
fn phase_color(phase: f32) -> Color {
    if phase < 0.5 {
        Color::srgb(0.55, 0.57, 0.6) // walk: neutral grey
    } else if phase < 1.5 {
        Color::srgb(0.20, 0.78, 0.35) // ride: green
    } else {
        Color::srgb(1.0, 0.6, 0.0) // wait: amber
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
        let mat = materials.add(StandardMaterial {
            double_sided: true,
            cull_mode: None,
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
        buf.push_flat_quad(
            Vec3::new(x - half, ground_y, y - half),
            Vec3::new(x + half, ground_y, y - half),
            Vec3::new(x + half, ground_y, y + half),
            Vec3::new(x - half, ground_y, y + half),
            Vec3::Y,
            color,
        );
    }

    let mesh_handle = meshes.add(buf.build());
    commands.entity(entity).insert(Mesh3d(mesh_handle));
    if let Ok(mut vis) = visibility.get_mut(entity) {
        *vis = Visibility::Visible;
    }
}
