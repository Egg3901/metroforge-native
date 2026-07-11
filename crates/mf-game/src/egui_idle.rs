//! Idle-aware egui paint-job cache for the in-game HUD.
//!
//! bevy_egui tessellates every frame even when the HUD content is unchanged.
//! When the fingerprint of HUD-relevant state is stable and there is no
//! pointer/keyboard activity, in-game egui systems skip via
//! [`egui_content_active`] and we restore the previous
//! [`EguiRenderOutput::paint_jobs`] after egui's process-output pass —
//! same pixels, no tessellation work.

use std::hash::{Hash, Hasher};

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPostUpdateSet, EguiRenderOutput};

use mf_state::{LatestUi, SubwayView};

use crate::goals::GoalsPanelOpen;
use crate::hud::ToastLog;
use crate::state::{AppState, PauseState};

pub struct MfEguiIdlePlugin;

impl Plugin for MfEguiIdlePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EguiIdleState>()
            .add_systems(
                Update,
                mark_egui_idle_system.run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                PostUpdate,
                (restore_egui_idle_paint_jobs, cache_egui_paint_jobs_system)
                    .chain()
                    .after(EguiPostUpdateSet::ProcessOutput)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

#[derive(Resource, Default)]
pub struct EguiIdleState {
    /// When true, in-game egui systems should skip widget construction.
    pub idle: bool,
    fingerprint: u64,
    cached_paint_jobs: Vec<egui::ClippedPrimitive>,
}

/// `run_if` for in-game egui systems: false while the idle cache is serving
/// the previous frame's paint jobs.
pub fn egui_content_active(idle: Option<Res<EguiIdleState>>) -> bool {
    !idle.is_some_and(|i| i.idle)
}

fn hud_fingerprint(
    ui: &LatestUi,
    subway: &SubwayView,
    goals: &GoalsPanelOpen,
    toasts: &ToastLog,
    pause: Option<&PauseState>,
) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let Some(s) = &ui.0 {
        // Minute bucket: second-level clock churn must not defeat idle.
        let minute = (s.tick % 1200) / 50;
        s.day.hash(&mut h);
        minute.hash(&mut h);
        (s.cash as i64).hash(&mut h);
        ((s.approval * 10.0) as i32).hash(&mut h);
        ((s.population * 10.0) as i64).hash(&mut h);
        ((s.speed * 100.0) as i32).hash(&mut h);
    }
    subway.active.hash(&mut h);
    goals.0.hash(&mut h);
    for (msg, _tone) in &toasts.0 {
        msg.hash(&mut h);
    }
    toasts.0.len().hash(&mut h);
    if let Some(p) = pause {
        p.active.hash(&mut h);
    }
    h.finish()
}

fn mark_egui_idle_system(
    mut idle: ResMut<EguiIdleState>,
    ui: Res<LatestUi>,
    subway: Res<SubwayView>,
    goals: Res<GoalsPanelOpen>,
    toasts: Res<ToastLog>,
    pause: Option<Res<PauseState>>,
    mut contexts: EguiContexts,
) {
    let input_busy = contexts
        .ctx_mut()
        .ok()
        .map(|ctx| {
            ctx.wants_pointer_input()
                || ctx.wants_keyboard_input()
                || ctx.input(|i| i.pointer.any_pressed() || !i.keys_down.is_empty())
        })
        .unwrap_or(false);
    let fp = hud_fingerprint(&ui, &subway, &goals, &toasts, pause.as_deref());
    let unchanged = idle.fingerprint != 0 && fp == idle.fingerprint;
    idle.idle = unchanged && !input_busy && !idle.cached_paint_jobs.is_empty();
    if !idle.idle {
        idle.fingerprint = fp;
    }
}

fn restore_egui_idle_paint_jobs(
    idle: Res<EguiIdleState>,
    mut outputs: Query<&mut EguiRenderOutput>,
) {
    if !idle.idle {
        return;
    }
    for mut output in &mut outputs {
        output.paint_jobs.clone_from(&idle.cached_paint_jobs);
        output.textures_delta = egui::TexturesDelta::default();
    }
}

fn cache_egui_paint_jobs_system(
    mut idle: ResMut<EguiIdleState>,
    outputs: Query<&EguiRenderOutput>,
) {
    if idle.idle {
        return;
    }
    if let Ok(output) = outputs.single() {
        if !output.paint_jobs.is_empty() {
            idle.cached_paint_jobs.clone_from(&output.paint_jobs);
        }
    }
}
