//! F11 debug overlay: entity / mesh / material asset counts plus per-layer
//! cache sizes from [`mf_render::RenderCacheStats`]. Toggle with F11; inert
//! when hidden. Exists so long-session growth regressions are visible at a
//! glance without attaching a profiler.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use mf_render::RenderCacheStats;

use crate::state::AppState;

#[derive(Resource, Default)]
struct DebugOverlayState {
    visible: bool,
}

pub struct MfDebugOverlayPlugin;

impl Plugin for MfDebugOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DebugOverlayState>().add_systems(
            Update,
            toggle_debug_overlay_system.run_if(in_state(AppState::InGame)),
        );
        app.add_systems(
            EguiPrimaryContextPass,
            debug_overlay_ui_system.run_if(in_state(AppState::InGame)),
        );
    }
}

fn toggle_debug_overlay_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<DebugOverlayState>,
) {
    if keys.just_pressed(KeyCode::F11) {
        state.visible = !state.visible;
    }
}

fn debug_overlay_ui_system(
    mut contexts: EguiContexts,
    state: Res<DebugOverlayState>,
    stats: Res<RenderCacheStats>,
    meshes: Res<Assets<Mesh>>,
    materials: Res<Assets<StandardMaterial>>,
    entities: Query<Entity>,
) {
    if !state.visible {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let entity_count = entities.iter().count();
    let mesh_count = meshes.len();
    let material_count = materials.len();

    egui::Window::new("Cache / assets (F11)")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::LEFT_TOP, [12.0, 12.0])
        .show(ctx, |ui| {
            ui.label(format!("entities:          {entity_count}"));
            ui.label(format!("Assets<Mesh>:      {mesh_count}"));
            ui.label(format!("Assets<Material>:  {material_count}"));
            ui.separator();
            ui.label(format!("vehicles slots:    {}", stats.vehicle_slots));
            ui.label(format!(
                "  body mat cache:  {}",
                stats.vehicle_material_cache
            ));
            ui.label(format!(
                "  light mat cache: {}",
                stats.vehicle_light_material_cache
            ));
            ui.label(format!(
                "transit stations:  {}",
                stats.transit_station_entities
            ));
            ui.label(format!(
                "transit tracks:    {}",
                stats.transit_track_entities
            ));
            ui.label(format!(
                "transit routes:    {}",
                stats.transit_route_entities
            ));
            ui.label(format!("roads:             {}", stats.road_entities));
            ui.label(format!("building chunks:   {}", stats.building_chunks));
            ui.label(format!("tree chunks:       {}", stats.tree_chunks));
            ui.label(format!("street-lamp chunks:{}", stats.street_lamp_chunks));
            ui.label(format!("agent entities:    {}", stats.agent_entities));
        });
}
