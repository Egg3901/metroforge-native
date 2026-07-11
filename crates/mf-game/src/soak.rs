//! Long-session soak harness for unbounded cache / asset growth.
//!
//! Entirely inert unless `MF_SOAK=<seconds>` is set (pair with
//! `MF_AUTOSTART=<presetKey>` so the run reaches `InGame` without a menu).
//! When armed it:
//!
//! 1. Runs the sim at 20x so day/night cycles churn the night-bucket path.
//! 2. Orbits the camera continuously around the dense center.
//! 3. Logs entity / mesh / material / per-layer cache counts every minute.
//! 4. After a short warmup, rejects **superlinear** growth: if the growth
//!    rate in the second half of post-warmup samples exceeds 1.5× the first
//!    half **and** absolute growth clears a small noise floor, the process
//!    exits nonzero. Plateau (or mild linear growth from the grow-only
//!    vehicle pool) passes.
//!
//! See `docs/DEVELOPMENT.md` ("Soak harness") for the full recipe.

use bevy::prelude::*;
use mf_net::SimLink;
use mf_protocol::{SetSpeedPayload, ToSim};
use mf_render::{BuildingsDenseCenter, RenderCacheStats};

use crate::camera::CameraRig;
use crate::state::AppState;

/// Sim speed while soaking — fast enough to cross many dusk/dawn boundaries
/// without making the render loop unreadable under lavapipe.
const SOAK_SPEED: f64 = 20.0;
/// Radians of yaw advanced per wall-clock second of camera orbit.
const ORBIT_RAD_PER_SEC: f32 = 0.15;
/// Seconds of InGame settle before the first sample (statics + first frame).
const WARMUP_SECS: f64 = 90.0;
/// How often to sample and log counters.
const SAMPLE_INTERVAL_SECS: f64 = 60.0;
/// Absolute growth below this is treated as noise (asset GC lag, one-off
/// pool growth) and never fails the superlinear check on its own.
const GROWTH_NOISE_FLOOR: f64 = 8.0;
/// Second-half growth rate must exceed this multiple of the first-half
/// rate (with both halves clearing the noise floor) to count as superlinear.
const SUPERLINEAR_RATIO: f64 = 1.5;

pub struct MfSoakPlugin;

impl Plugin for MfSoakPlugin {
    fn build(&self, app: &mut App) {
        let Some(duration) = soak_duration_secs() else {
            return;
        };
        app.insert_resource(SoakState::new(duration))
            .add_systems(Update, soak_system.run_if(in_state(AppState::InGame)));
        tracing::info!(
            "mf-game: MF_SOAK={duration}s armed (20x sim, camera orbit, per-minute samples)"
        );
    }
}

fn soak_duration_secs() -> Option<f64> {
    let raw = std::env::var("MF_SOAK").ok()?;
    let secs: f64 = raw.parse().ok()?;
    (secs > 0.0).then_some(secs)
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    t_secs: f64,
    entities: usize,
    meshes: usize,
    materials: usize,
    vehicle_mat_cache: usize,
    vehicle_light_cache: usize,
    transit_entities: usize,
}

#[derive(Resource)]
struct SoakState {
    duration_secs: f64,
    /// Wall-clock seconds spent in `InGame` (sum of `Time::delta_secs`).
    elapsed: f64,
    speed_sent: bool,
    last_sample_at: f64,
    samples: Vec<Sample>,
    failed: bool,
}

impl SoakState {
    fn new(duration_secs: f64) -> Self {
        Self {
            duration_secs,
            elapsed: 0.0,
            speed_sent: false,
            last_sample_at: -SAMPLE_INTERVAL_SECS, // sample immediately after warmup
            samples: Vec::new(),
            failed: false,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn soak_system(
    time: Res<Time>,
    link: Option<Res<SimLink>>,
    dense: Res<BuildingsDenseCenter>,
    stats: Res<RenderCacheStats>,
    meshes: Res<Assets<Mesh>>,
    materials: Res<Assets<StandardMaterial>>,
    entities: Query<Entity>,
    mut rigs: Query<&mut CameraRig>,
    mut state: ResMut<SoakState>,
    mut exit: EventWriter<AppExit>,
) {
    if state.failed {
        return;
    }
    state.elapsed += time.delta_secs_f64();

    if !state.speed_sent {
        if let Some(link) = link.as_ref() {
            let _ = link
                .transport
                .send(ToSim::SetSpeed(SetSpeedPayload { speed: SOAK_SPEED }));
            state.speed_sent = true;
        }
    }

    // Continuous yaw orbit around the dense center once we know it.
    if dense.0 != Vec2::ZERO {
        for mut rig in &mut rigs {
            rig.target = dense.0;
            rig.target_goal = dense.0;
            if rig.distance < 800.0 {
                rig.distance = 1400.0;
                rig.distance_goal = 1400.0;
            }
            let dyaw = ORBIT_RAD_PER_SEC * time.delta_secs();
            rig.yaw += dyaw;
            rig.yaw_goal = rig.yaw;
        }
    }

    if state.elapsed < WARMUP_SECS {
        return;
    }

    if state.elapsed - state.last_sample_at >= SAMPLE_INTERVAL_SECS {
        state.last_sample_at = state.elapsed;
        let sample = Sample {
            t_secs: state.elapsed,
            entities: entities.iter().count(),
            meshes: meshes.len(),
            materials: materials.len(),
            vehicle_mat_cache: stats.vehicle_material_cache,
            vehicle_light_cache: stats.vehicle_light_material_cache,
            transit_entities: stats.transit_station_entities
                + stats.transit_track_entities
                + stats.transit_route_entities,
        };
        tracing::info!(
            "mf-soak t={:.0}s entities={} meshes={} materials={} veh_mat={} veh_light={} transit={}",
            sample.t_secs,
            sample.entities,
            sample.meshes,
            sample.materials,
            sample.vehicle_mat_cache,
            sample.vehicle_light_cache,
            sample.transit_entities,
        );
        state.samples.push(sample);

        if let Some(reason) = detect_superlinear(&state.samples) {
            tracing::error!("mf-soak FAIL: {reason}");
            state.failed = true;
            exit.write(AppExit::from_code(1));
            return;
        }
    }

    if state.elapsed >= state.duration_secs {
        tracing::info!(
            "mf-soak PASS after {:.0}s ({} samples)",
            state.elapsed,
            state.samples.len()
        );
        exit.write(AppExit::Success);
    }
}

/// Pure growth check used by the soak system and unit-tested below.
/// Returns `Some(reason)` when any tracked counter accelerates past
/// [`SUPERLINEAR_RATIO`] across the two halves of the post-warmup series.
fn detect_superlinear(samples: &[Sample]) -> Option<String> {
    // Need enough points that each half has ≥2 samples for a rate.
    if samples.len() < 4 {
        return None;
    }
    let mid = samples.len() / 2;
    let first = &samples[..mid];
    let second = &samples[mid..];

    check_counter("entities", first, second, |s| s.entities as f64)
        .or_else(|| check_counter("meshes", first, second, |s| s.meshes as f64))
        .or_else(|| check_counter("materials", first, second, |s| s.materials as f64))
        .or_else(|| {
            check_counter("vehicle_mat_cache", first, second, |s| {
                s.vehicle_mat_cache as f64
            })
        })
        .or_else(|| {
            check_counter("vehicle_light_cache", first, second, |s| {
                s.vehicle_light_cache as f64
            })
        })
        .or_else(|| {
            check_counter("transit_entities", first, second, |s| {
                s.transit_entities as f64
            })
        })
}

fn check_counter(
    name: &str,
    first: &[Sample],
    second: &[Sample],
    pick: impl Fn(&Sample) -> f64,
) -> Option<String> {
    let g1 = growth(first, &pick);
    let g2 = growth(second, &pick);
    if g1 > GROWTH_NOISE_FLOOR && g2 > GROWTH_NOISE_FLOOR && g2 > g1 * SUPERLINEAR_RATIO {
        Some(format!(
            "{name} grew superlinearly (first-half Δ={g1:.1}, second-half Δ={g2:.1})"
        ))
    } else {
        None
    }
}

fn growth(samples: &[Sample], pick: &impl Fn(&Sample) -> f64) -> f64 {
    let first = pick(samples.first().expect("non-empty"));
    let last = pick(samples.last().expect("non-empty"));
    (last - first).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(t: f64, entities: usize, meshes: usize, materials: usize) -> Sample {
        Sample {
            t_secs: t,
            entities,
            meshes,
            materials,
            vehicle_mat_cache: 0,
            vehicle_light_cache: 0,
            transit_entities: 0,
        }
    }

    #[test]
    fn plateau_passes() {
        let samples: Vec<Sample> = (0..8)
            .map(|i| sample(90.0 + i as f64 * 60.0, 1000, 200, 300))
            .collect();
        assert!(detect_superlinear(&samples).is_none());
    }

    #[test]
    fn linear_growth_passes() {
        // +10 entities per sample, constant rate — linear, not superlinear.
        let samples: Vec<Sample> = (0..8)
            .map(|i| sample(90.0 + i as f64 * 60.0, 1000 + i * 10, 200, 300))
            .collect();
        assert!(detect_superlinear(&samples).is_none());
    }

    #[test]
    fn accelerating_growth_fails() {
        // First half: +10/sample (clears the noise floor); second half: +80/sample.
        let mut samples = Vec::new();
        let mut n = 1000usize;
        for i in 0..4 {
            samples.push(sample(90.0 + i as f64 * 60.0, n, 200, 300));
            n += 10;
        }
        for i in 4..8 {
            samples.push(sample(90.0 + i as f64 * 60.0, n, 200, 300));
            n += 80;
        }
        let reason = detect_superlinear(&samples).expect("should fail");
        assert!(reason.contains("entities"), "{reason}");
    }

    #[test]
    fn too_few_samples_is_inconclusive() {
        let samples = vec![
            sample(90.0, 1000, 200, 300),
            sample(150.0, 2000, 400, 600),
            sample(210.0, 4000, 800, 1200),
        ];
        assert!(detect_superlinear(&samples).is_none());
    }
}
