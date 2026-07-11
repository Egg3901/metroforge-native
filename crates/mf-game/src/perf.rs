//! Measurement-first performance harness for `mf-render`.
//!
//! Entirely inert unless `MF_PERF=1` (or any value) is set. When active —
//! typically paired with `MF_AUTOSTART=nyc` — it:
//!
//! 1. Relies on Bevy's frame-time / entity-count diagnostic plugins + spans
//!    (registered from `main` when `MF_PERF` is set).
//! 2. Waits until `InGame` and building chunks have settled.
//! 3. Samples frame times, draw-call proxies, and entity/mesh/material
//!    counts for [`DEFAULT_SAMPLE_SECS`] wall-clock seconds (override with
//!    `MF_PERF_SECONDS`).
//! 4. Logs percentiles + per-layer counts, optionally asserts CI budgets
//!    (`MF_PERF_ASSERT=1`), then exits.
//!
//! Draw-call proxy: count of `Mesh3d` entities with `ViewVisibility`
//! currently visible. Bevy batches some of these, so this is an upper
//! bound on GPU draw calls — still the right regression signal for the
//! "one mesh per chunk / class" architecture.

use std::cmp::Ordering;
use std::time::{Duration, Instant};

use bevy::diagnostic::{
    DiagnosticsStore, EntityCountDiagnosticsPlugin, FrameTimeDiagnosticsPlugin,
};
use bevy::prelude::*;
use bevy::render::view::ViewVisibility;
use bevy::window::WindowCloseRequested;

use mf_render::{BuildingsDenseCenter, PerfCounters};
use mf_state::LatestFields;

use crate::state::AppState;

/// Default sample window once the scene has settled.
const DEFAULT_SAMPLE_SECS: u64 = 60;
/// Frames to hold after dense-center is known before sampling starts, so
/// static rebuilds (buildings/roads/trees) finish uploading.
const SETTLE_FRAMES: u64 = 30;
/// Hard cap waiting for settle so a hung load can't block CI forever.
const MAX_WAIT_FRAMES: u64 = 1_800;

/// Default CI budgets (override with env). Lavapipe / software Vulkan is
/// orders of magnitude slower than a real GPU — budgets are intentionally
/// loose so `MF_PERF_ASSERT=1` is usable as a smoke gate, not a hardware
/// FPS target. Tighten on a GPU runner later.
const DEFAULT_BUDGET_FRAME_MS_P95: f64 = 100.0;
const DEFAULT_BUDGET_DRAW_CALLS_P95: f64 = 500.0;

pub struct MfPerfPlugin;

impl Plugin for MfPerfPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var_os("MF_PERF").is_none() {
            return;
        }
        app.init_resource::<PerfHarness>().add_systems(
            Update,
            perf_harness_system.run_if(in_state(AppState::InGame)),
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Stage {
    #[default]
    WaitSettle,
    Sampling,
    Done,
}

#[derive(Resource)]
struct PerfHarness {
    stage: Stage,
    frame: u64,
    stage_start_frame: u64,
    sample_started_at: Option<Instant>,
    sample_secs: u64,
    frame_ms: Vec<f64>,
    draw_calls: Vec<u32>,
    mesh_entities: Vec<u32>,
    entities: Vec<u32>,
    meshes: Vec<u32>,
    materials: Vec<u32>,
    layer_counts: LayerCounts,
    system_us: SystemUsAccum,
}

impl Default for PerfHarness {
    fn default() -> Self {
        let sample_secs = std::env::var("MF_PERF_SECONDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_SAMPLE_SECS);
        Self {
            stage: Stage::WaitSettle,
            frame: 0,
            stage_start_frame: 0,
            sample_started_at: None,
            sample_secs,
            frame_ms: Vec::with_capacity(4_096),
            draw_calls: Vec::with_capacity(4_096),
            mesh_entities: Vec::with_capacity(4_096),
            entities: Vec::with_capacity(4_096),
            meshes: Vec::with_capacity(4_096),
            materials: Vec::with_capacity(4_096),
            layer_counts: LayerCounts::default(),
            system_us: SystemUsAccum::default(),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct LayerCounts {
    buildings: u32,
    roads: u32,
    trees: u32,
    street_lamps: u32,
    transit: u32,
    vehicles: u32,
    agents: u32,
    other_mesh: u32,
}

#[derive(Debug, Default, Clone)]
struct SystemUsAccum {
    building_draw_distance: u64,
    tree_draw_distance: u64,
    street_lamp_visibility: u64,
    road_lod: u64,
    transit_update: u64,
    buildings_rebuild: u64,
    roads_rebuild: u64,
    egui_pass: u64,
    visibility_mutations: u64,
    visibility_skips: u64,
    frames: u64,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((p / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn summarize(samples: &[u32]) -> (f64, f64, f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let mut v: Vec<f64> = samples.iter().map(|&x| x as f64).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    (
        mean,
        percentile(&v, 50.0),
        percentile(&v, 95.0),
        percentile(&v, 99.0),
    )
}

fn summarize_f64(samples: &[f64]) -> (f64, f64, f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let mut v = samples.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    (
        mean,
        percentile(&v, 50.0),
        percentile(&v, 95.0),
        percentile(&v, 99.0),
    )
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

#[allow(clippy::too_many_arguments)]
fn perf_harness_system(
    mut harness: ResMut<PerfHarness>,
    counters: Res<PerfCounters>,
    mut egui_timer: ResMut<EguiPerfTimer>,
    diagnostics: Res<DiagnosticsStore>,
    time: Res<Time>,
    dense: Option<Res<BuildingsDenseCenter>>,
    fields: Res<LatestFields>,
    meshes: Res<Assets<Mesh>>,
    std_mats: Res<Assets<StandardMaterial>>,
    visible_meshes: Query<&ViewVisibility, With<Mesh3d>>,
    named: Query<(&Name, &ViewVisibility), With<Mesh3d>>,
    mut exit: EventWriter<AppExit>,
    mut close: EventWriter<WindowCloseRequested>,
    windows: Query<Entity, With<Window>>,
) {
    harness.frame += 1;

    match harness.stage {
        Stage::WaitSettle => {
            let ready = fields.0.is_some() && dense.is_some();
            if !ready {
                if harness.frame >= MAX_WAIT_FRAMES {
                    bevy::log::error!(
                        "MF_PERF: gave up waiting for fields+BuildingsDenseCenter after {} frames",
                        harness.frame
                    );
                    exit.write(AppExit::from_code(2));
                }
                return;
            }
            if harness.stage_start_frame == 0 {
                harness.stage_start_frame = harness.frame;
                bevy::log::info!(
                    "MF_PERF: city settled (dense center present); holding {} frames before sample",
                    SETTLE_FRAMES
                );
            }
            if harness.frame.saturating_sub(harness.stage_start_frame) >= SETTLE_FRAMES {
                harness.stage = Stage::Sampling;
                harness.sample_started_at = Some(Instant::now());
                counters.reset();
                egui_timer.0 = 0;
                bevy::log::info!(
                    "MF_PERF: sampling for {}s (override with MF_PERF_SECONDS)",
                    harness.sample_secs
                );
            }
        }
        Stage::Sampling => {
            let frame_ms = time.delta_secs_f64() * 1_000.0;
            // Bevy 0.16 stores FRAME_TIME already in milliseconds
            // (`delta_secs * 1000` in FrameTimeDiagnosticsPlugin).
            let frame_ms = diagnostics
                .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
                .and_then(|d| d.value())
                .unwrap_or(frame_ms);

            let draw_calls = visible_meshes.iter().filter(|v| v.get()).count() as u32;
            let total_mesh_entities = visible_meshes.iter().count() as u32;
            let entities = diagnostics
                .get(&EntityCountDiagnosticsPlugin::ENTITY_COUNT)
                .and_then(|d| d.value())
                .unwrap_or(0.0) as u32;
            let mesh_count = meshes.iter().count() as u32;
            let material_count = std_mats.iter().count() as u32;

            harness.frame_ms.push(frame_ms);
            harness.draw_calls.push(draw_calls);
            harness.mesh_entities.push(total_mesh_entities);
            harness.entities.push(entities);
            harness.meshes.push(mesh_count);
            harness.materials.push(material_count);

            let mut layers = LayerCounts::default();
            for (name, vis) in &named {
                if !vis.get() {
                    continue;
                }
                let n = name.as_str();
                if n.starts_with("buildings-") {
                    layers.buildings += 1;
                } else if n.starts_with("roads-") || n.starts_with("road-") {
                    layers.roads += 1;
                } else if n.starts_with("park-trees") {
                    layers.trees += 1;
                } else if n.starts_with("street-lamps") {
                    layers.street_lamps += 1;
                } else if n.starts_with("station-")
                    || n.starts_with("track-")
                    || n.starts_with("route-")
                    || n.starts_with("metro-")
                {
                    layers.transit += 1;
                } else if n.starts_with("vehicle") {
                    layers.vehicles += 1;
                } else if n.starts_with("agent") {
                    layers.agents += 1;
                } else {
                    layers.other_mesh += 1;
                }
            }
            harness.layer_counts = layers;

            harness.system_us.building_draw_distance += counters
                .building_draw_distance_us
                .load(std::sync::atomic::Ordering::Relaxed);
            harness.system_us.tree_draw_distance += counters
                .tree_draw_distance_us
                .load(std::sync::atomic::Ordering::Relaxed);
            harness.system_us.street_lamp_visibility += counters
                .street_lamp_visibility_us
                .load(std::sync::atomic::Ordering::Relaxed);
            harness.system_us.road_lod += counters
                .road_lod_us
                .load(std::sync::atomic::Ordering::Relaxed);
            harness.system_us.transit_update += counters
                .transit_update_us
                .load(std::sync::atomic::Ordering::Relaxed);
            harness.system_us.buildings_rebuild += counters
                .buildings_rebuild_us
                .load(std::sync::atomic::Ordering::Relaxed);
            harness.system_us.roads_rebuild += counters
                .roads_rebuild_us
                .load(std::sync::atomic::Ordering::Relaxed);
            harness.system_us.egui_pass += egui_timer.0;
            harness.system_us.visibility_mutations += u64::from(
                counters
                    .visibility_mutations
                    .load(std::sync::atomic::Ordering::Relaxed),
            );
            harness.system_us.visibility_skips += u64::from(
                counters
                    .visibility_skips
                    .load(std::sync::atomic::Ordering::Relaxed),
            );
            harness.system_us.frames += 1;
            counters.reset();
            egui_timer.0 = 0;

            let elapsed = harness
                .sample_started_at
                .map(|t| t.elapsed())
                .unwrap_or(Duration::ZERO);
            if elapsed >= Duration::from_secs(harness.sample_secs) {
                finish_and_exit(harness.as_ref(), &mut exit, &mut close, &windows);
                harness.stage = Stage::Done;
            }
        }
        Stage::Done => {}
    }
}

/// Microseconds spent in egui primary-context systems this frame — written
/// by the in-game HUD when `MF_PERF` is active (resource only inserted then).
#[derive(Resource, Default)]
pub struct EguiPerfTimer(pub u64);

fn finish_and_exit(
    harness: &PerfHarness,
    exit: &mut EventWriter<AppExit>,
    close: &mut EventWriter<WindowCloseRequested>,
    windows: &Query<Entity, With<Window>>,
) {
    let (ft_mean, ft_p50, ft_p95, ft_p99) = summarize_f64(&harness.frame_ms);
    let (dc_mean, dc_p50, dc_p95, dc_p99) = summarize(&harness.draw_calls);
    let (me_mean, _, _, _) = summarize(&harness.mesh_entities);
    let (ent_mean, _, _, _) = summarize(&harness.entities);
    let (mesh_mean, _, _, _) = summarize(&harness.meshes);
    let (mat_mean, _, _, _) = summarize(&harness.materials);
    let n = harness.frame_ms.len();
    let layers = harness.layer_counts;

    bevy::log::info!("========== MF_PERF REPORT ==========");
    bevy::log::info!("samples={n} window={}s", harness.sample_secs);
    bevy::log::info!("frame_ms: mean={ft_mean:.2} p50={ft_p50:.2} p95={ft_p95:.2} p99={ft_p99:.2}");
    bevy::log::info!(
        "draw_calls(visible Mesh3d): mean={dc_mean:.0} p50={dc_p50:.0} p95={dc_p95:.0} p99={dc_p99:.0} (total Mesh3d≈{me_mean:.0})"
    );
    bevy::log::info!(
        "entities≈{ent_mean:.0} meshes≈{mesh_mean:.0} standard_materials≈{mat_mean:.0}"
    );
    bevy::log::info!(
        "visible layers: buildings={} roads={} trees={} lamps={} transit={} vehicles={} agents={} other={}",
        layers.buildings,
        layers.roads,
        layers.trees,
        layers.street_lamps,
        layers.transit,
        layers.vehicles,
        layers.agents,
        layers.other_mesh,
    );

    let frames = harness.system_us.frames.max(1) as f64;
    let mut offenders: Vec<(&str, f64)> = vec![
        (
            "buildings::draw_distance",
            harness.system_us.building_draw_distance as f64 / frames / 1000.0,
        ),
        (
            "trees::draw_distance",
            harness.system_us.tree_draw_distance as f64 / frames / 1000.0,
        ),
        (
            "street_lamps::visibility",
            harness.system_us.street_lamp_visibility as f64 / frames / 1000.0,
        ),
        (
            "roads::lod",
            harness.system_us.road_lod as f64 / frames / 1000.0,
        ),
        (
            "transit::update",
            harness.system_us.transit_update as f64 / frames / 1000.0,
        ),
        (
            "buildings::rebuild",
            harness.system_us.buildings_rebuild as f64 / frames / 1000.0,
        ),
        (
            "roads::rebuild",
            harness.system_us.roads_rebuild as f64 / frames / 1000.0,
        ),
        (
            "egui::pass",
            harness.system_us.egui_pass as f64 / frames / 1000.0,
        ),
    ];
    offenders.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
    bevy::log::info!("top instrumented system CPU (avg ms/frame):");
    for (i, (name, ms)) in offenders.iter().take(5).enumerate() {
        bevy::log::info!("  #{}: {name} = {ms:.3} ms/frame", i + 1);
    }
    bevy::log::info!(
        "visibility writes: mutations={} skips={} (skip ratio={:.1}%)",
        harness.system_us.visibility_mutations,
        harness.system_us.visibility_skips,
        100.0 * harness.system_us.visibility_skips as f64
            / (harness.system_us.visibility_mutations + harness.system_us.visibility_skips).max(1)
                as f64
    );
    bevy::log::info!("====================================");

    let mut failed = false;
    if std::env::var_os("MF_PERF_ASSERT").is_some() {
        let budget_ft = env_f64("MF_PERF_BUDGET_FRAME_MS_P95", DEFAULT_BUDGET_FRAME_MS_P95);
        let budget_dc = env_f64(
            "MF_PERF_BUDGET_DRAW_CALLS_P95",
            DEFAULT_BUDGET_DRAW_CALLS_P95,
        );
        if ft_p95 > budget_ft {
            bevy::log::error!(
                "MF_PERF budget FAIL: frame_ms p95={ft_p95:.2} > budget={budget_ft:.2}"
            );
            failed = true;
        } else {
            bevy::log::info!("MF_PERF budget OK: frame_ms p95={ft_p95:.2} <= {budget_ft:.2}");
        }
        if dc_p95 > budget_dc {
            bevy::log::error!(
                "MF_PERF budget FAIL: draw_calls p95={dc_p95:.0} > budget={budget_dc:.0}"
            );
            failed = true;
        } else {
            bevy::log::info!("MF_PERF budget OK: draw_calls p95={dc_p95:.0} <= {budget_dc:.0}");
        }
    }

    for w in windows.iter() {
        close.write(WindowCloseRequested { window: w });
    }
    exit.write(if failed {
        AppExit::from_code(1)
    } else {
        AppExit::Success
    });
}
