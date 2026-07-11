//! In-game graphics benchmark + FPS overlay.
//!
//! The 10-second benchmark orbits the loaded city camera, samples frame
//! times from Bevy's `FrameTimeDiagnosticsPlugin`, then reports average /
//! 1%-low frame times and a recommended [`QualityTier`].

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use mf_state::{recommend_tier_from_frame_times, QualityTier};

use crate::camera::CameraRig;
use crate::config::MfConfig;
use crate::design_system;
use crate::state::AppState;

/// How long the built-in benchmark runs.
pub const BENCHMARK_DURATION_SECS: f32 = 10.0;

/// On-screen FPS counter preference (mirrors `MfConfig::show_fps`).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShowFps(pub bool);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BenchmarkResult {
    pub avg_ms: f32,
    pub low_1pct_ms: f32,
    pub recommended: QualityTier,
}

#[derive(Debug, Clone, PartialEq, Default)]
enum BenchmarkPhase {
    #[default]
    Idle,
    Running {
        elapsed: f32,
        samples_ms: Vec<f32>,
        /// Camera yaw at start so we can restore after the sweep.
        start_yaw: f32,
        start_yaw_goal: f32,
    },
    Done(BenchmarkResult),
}

/// Player-triggered 10s camera-sweep benchmark.
#[derive(Resource, Debug, Clone, PartialEq, Default)]
pub struct GraphicsBenchmark {
    phase: BenchmarkPhase,
}

impl GraphicsBenchmark {
    pub fn is_running(&self) -> bool {
        matches!(self.phase, BenchmarkPhase::Running { .. })
    }

    pub fn result(&self) -> Option<BenchmarkResult> {
        match self.phase {
            BenchmarkPhase::Done(r) => Some(r),
            _ => None,
        }
    }

    pub fn clear_result(&mut self) {
        if matches!(self.phase, BenchmarkPhase::Done(_)) {
            self.phase = BenchmarkPhase::Idle;
        }
    }

    pub fn start(&mut self, start_yaw: f32, start_yaw_goal: f32) {
        self.phase = BenchmarkPhase::Running {
            elapsed: 0.0,
            samples_ms: Vec::with_capacity(512),
            start_yaw,
            start_yaw_goal,
        };
    }
}

pub struct MfGraphicsPerfPlugin;

impl Plugin for MfGraphicsPerfPlugin {
    fn build(&self, app: &mut App) {
        // Always register frame-time diagnostics so the FPS overlay and
        // benchmark can read them; logging stays behind MF_PERF_LOG.
        if !app.is_plugin_added::<FrameTimeDiagnosticsPlugin>() {
            app.add_plugins(FrameTimeDiagnosticsPlugin::default());
        }
        app.init_resource::<ShowFps>()
            .init_resource::<GraphicsBenchmark>()
            .add_systems(
                Update,
                (
                    sync_show_fps_from_config_system,
                    run_graphics_benchmark_system.run_if(in_state(AppState::InGame)),
                ),
            )
            .add_systems(
                EguiPrimaryContextPass,
                fps_overlay_system
                    .run_if(in_state(AppState::InGame))
                    .run_if(|show: Res<ShowFps>| show.0),
            );
    }
}

fn sync_show_fps_from_config_system(config: Res<MfConfig>, mut show: ResMut<ShowFps>) {
    if config.is_changed() || show.is_added() {
        show.0 = config.show_fps;
    }
}

fn run_graphics_benchmark_system(
    time: Res<Time>,
    diagnostics: Res<DiagnosticsStore>,
    mut bench: ResMut<GraphicsBenchmark>,
    mut rigs: Query<&mut CameraRig>,
) {
    let BenchmarkPhase::Running {
        elapsed,
        samples_ms,
        start_yaw,
        start_yaw_goal,
    } = &mut bench.phase
    else {
        return;
    };

    let dt = time.delta_secs();
    *elapsed += dt;

    // Full 360° yaw sweep over the benchmark window.
    let t = (*elapsed / BENCHMARK_DURATION_SECS).clamp(0.0, 1.0);
    let yaw = *start_yaw + t * std::f32::consts::TAU;
    for mut rig in &mut rigs {
        rig.yaw = yaw;
        rig.yaw_goal = yaw;
        // Slight pitch bob so more of the city enters/leaves the frustum.
        let pitch = 0.45 + (t * std::f32::consts::TAU).sin() * 0.12;
        rig.pitch = pitch;
        rig.pitch_goal = pitch;
    }

    if let Some(fps) = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
    {
        if fps.is_finite() && fps > 1.0 {
            samples_ms.push(1000.0 / fps as f32);
        }
    } else if let Some(ft) = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(|d| d.smoothed())
    {
        if ft.is_finite() && ft > 0.0 {
            samples_ms.push(ft as f32 * 1000.0);
        }
    }

    if *elapsed < BENCHMARK_DURATION_SECS {
        return;
    }

    let restore_yaw = *start_yaw;
    let restore_yaw_goal = *start_yaw_goal;
    let result = summarize_samples(samples_ms);
    for mut rig in &mut rigs {
        rig.yaw = restore_yaw;
        rig.yaw_goal = restore_yaw_goal;
    }
    bench.phase = BenchmarkPhase::Done(result);
    tracing::info!(
        "mf-game: graphics benchmark avg={:.2}ms 1%low={:.2}ms recommended={:?}",
        result.avg_ms,
        result.low_1pct_ms,
        result.recommended
    );
}

fn summarize_samples(samples_ms: &[f32]) -> BenchmarkResult {
    if samples_ms.is_empty() {
        return BenchmarkResult {
            avg_ms: 999.0,
            low_1pct_ms: 999.0,
            recommended: QualityTier::Potato,
        };
    }
    let sum: f32 = samples_ms.iter().sum();
    let avg_ms = sum / samples_ms.len() as f32;
    let mut sorted = samples_ms.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    // 1% low = average of the slowest 1% of frames (highest ms).
    let n = sorted.len();
    let worst_count = (n / 100).max(1);
    let low_1pct_ms = sorted[n - worst_count..].iter().sum::<f32>() / worst_count as f32;
    let recommended = recommend_tier_from_frame_times(avg_ms, low_1pct_ms);
    BenchmarkResult {
        avg_ms,
        low_1pct_ms,
        recommended,
    }
}

fn fps_overlay_system(
    mut contexts: EguiContexts,
    diagnostics: Res<DiagnosticsStore>,
    bench: Res<GraphicsBenchmark>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0);
    let frame_ms = if fps > 1.0 { 1000.0 / fps } else { 0.0 };

    egui::Area::new(egui::Id::new("fps_overlay"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::LEFT_TOP, egui::vec2(12.0, 12.0))
        .show(ctx, |ui| {
            let label = if bench.is_running() {
                format!("BENCHMARK  {fps:.0} fps  ({frame_ms:.1} ms)")
            } else {
                format!("{fps:.0} fps  ({frame_ms:.1} ms)")
            };
            ui.label(
                egui::RichText::new(label)
                    .size(13.0)
                    .color(design_system::current_colors().text),
            );
        });
    Ok(())
}

/// Start the benchmark from Settings (needs a live `CameraRig`).
pub fn begin_benchmark(bench: &mut GraphicsBenchmark, rigs: &mut Query<&mut CameraRig>) {
    let Ok(rig) = rigs.single() else {
        return;
    };
    let yaw = rig.yaw;
    let yaw_goal = rig.yaw_goal;
    bench.start(yaw, yaw_goal);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_empty_recommends_potato() {
        let r = summarize_samples(&[]);
        assert_eq!(r.recommended, QualityTier::Potato);
    }

    #[test]
    fn summarize_fast_frames_recommends_high() {
        let samples: Vec<f32> = (0..100).map(|_| 8.0).collect();
        let r = summarize_samples(&samples);
        assert_eq!(r.recommended, QualityTier::High);
        assert!((r.avg_ms - 8.0).abs() < 0.01);
    }
}
