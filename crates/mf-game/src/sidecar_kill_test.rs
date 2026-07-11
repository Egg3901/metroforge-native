//! Integration harness: `MF_TEST_KILL_SIDECAR=<seconds>` kills the owned
//! sidecar that many wall-clock seconds after `InGame` is reached, then
//! asserts the client recovers (`NetStatus::Connected` again with a live
//! transport) without bouncing to the main menu / `SimError`.
//!
//! Entirely inert unless the env var is set. Wired as an optional CI job
//! (see `.github/workflows/ci.yml` `sidecar-recovery`) once a sidecar binary
//! is available — not part of the default `cargo test` suite.
//!
//! Typical invocation:
//! ```sh
//! MF_AUTOSTART=nyc MF_TEST_KILL_SIDECAR=30 MF_SIDECAR_PATH=./metroforge-sidecar \
//!   cargo run -p mf-game --release
//! ```
//! Exit code 0 = recovered; non-zero = failed (see stderr / result file).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use bevy::app::AppExit;
use bevy::prelude::*;
use mf_net::{NetStatus, ReconnectState, SimLink};
use mf_state::{CurrentCity, LatestUi};

use crate::state::AppState;

/// Hard cap after the kill so a wedged reconnect can't hang CI forever.
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(90);
/// After recovery, require this many consecutive Connected frames with a
/// fresh `LatestUi` tick bump before declaring success (avoids racing the
/// first post-restore frame).
const STABLE_FRAMES: u32 = 30;

pub struct MfSidecarKillTestPlugin;

impl Plugin for MfSidecarKillTestPlugin {
    fn build(&self, app: &mut App) {
        let Some(secs) = parse_kill_after_secs() else {
            return;
        };
        tracing::info!(
            "mf-game: MF_TEST_KILL_SIDECAR={secs} — will kill sidecar {secs}s after InGame"
        );
        app.insert_resource(KillTestState::new(secs))
            .add_systems(
                Update,
                kill_sidecar_harness_system.run_if(in_state(AppState::InGame)),
            )
            .add_systems(Update, kill_sidecar_fail_on_sim_error);
    }
}

fn parse_kill_after_secs() -> Option<u64> {
    let raw = std::env::var("MF_TEST_KILL_SIDECAR").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<u64>().ok().filter(|s| *s > 0)
}

#[derive(Resource)]
struct KillTestState {
    kill_after: Duration,
    entered_ingame_at: Option<Instant>,
    killed_at: Option<Instant>,
    stable_frames: u32,
    /// Ui tick observed at kill time — recovery must produce a newer one.
    tick_at_kill: Option<u64>,
}

impl KillTestState {
    fn new(secs: u64) -> Self {
        Self {
            kill_after: Duration::from_secs(secs),
            entered_ingame_at: None,
            killed_at: None,
            stable_frames: 0,
            tick_at_kill: None,
        }
    }
}

fn kill_sidecar_harness_system(
    mut state: ResMut<KillTestState>,
    mut link: Option<ResMut<SimLink>>,
    reconnect: Res<ReconnectState>,
    ui: Res<LatestUi>,
    city: Res<CurrentCity>,
    mut exit: EventWriter<AppExit>,
) {
    let now = Instant::now();
    if state.entered_ingame_at.is_none() {
        state.entered_ingame_at = Some(now);
        tracing::info!("mf-game: kill-sidecar harness armed");
    }
    let entered = state.entered_ingame_at.unwrap();

    if state.killed_at.is_none() {
        if now.duration_since(entered) < state.kill_after {
            return;
        }
        let Some(link) = link.as_mut() else {
            fail_and_exit(
                &mut exit,
                "MF_TEST_KILL_SIDECAR: no SimLink to kill at deadline",
            );
            return;
        };
        state.tick_at_kill = ui.0.as_ref().map(|s| s.tick);
        tracing::warn!(
            "mf-game: MF_TEST_KILL_SIDECAR — killing sidecar now (tick={:?})",
            state.tick_at_kill
        );
        link.kill_sidecar_for_test();
        state.killed_at = Some(now);
        return;
    }

    let killed_at = state.killed_at.unwrap();
    if now.duration_since(killed_at) > RECOVERY_TIMEOUT {
        fail_and_exit(
            &mut exit,
            &format!(
                "MF_TEST_KILL_SIDECAR: did not recover within {RECOVERY_TIMEOUT:?} (status={:?})",
                reconnect.status
            ),
        );
        return;
    }

    // Still reconnecting — wait.
    if !matches!(reconnect.status, NetStatus::Connected) {
        state.stable_frames = 0;
        return;
    }

    // Must have a live city + ui again, and ui must have advanced past the
    // pre-kill tick when we had one (fresh sim traffic, not a stale retain).
    let Some(ui_state) = &ui.0 else {
        state.stable_frames = 0;
        return;
    };
    if !city.masks_complete() {
        state.stable_frames = 0;
        return;
    }
    if let Some(tick_at_kill) = state.tick_at_kill {
        if ui_state.tick <= tick_at_kill {
            state.stable_frames = 0;
            return;
        }
    }

    state.stable_frames += 1;
    if state.stable_frames < STABLE_FRAMES {
        return;
    }

    let msg = format!(
        "MF_TEST_KILL_SIDECAR: recovered ok after {:?} (ui tick {})",
        now.duration_since(killed_at),
        ui_state.tick
    );
    tracing::info!("mf-game: {msg}");
    write_result_file(true, &msg);
    exit.write(AppExit::Success);
}

fn kill_sidecar_fail_on_sim_error(
    state: Option<Res<KillTestState>>,
    app_state: Res<State<AppState>>,
    reconnect: Res<ReconnectState>,
    mut exit: EventWriter<AppExit>,
) {
    let Some(state) = state else {
        return;
    };
    if state.killed_at.is_none() {
        return;
    }
    if *app_state.get() == AppState::SimError || matches!(reconnect.status, NetStatus::Fatal(_)) {
        fail_and_exit(
            &mut exit,
            &format!(
                "MF_TEST_KILL_SIDECAR: landed on fatal/SimError instead of recovering ({:?})",
                reconnect.status
            ),
        );
    }
}

fn fail_and_exit(exit: &mut EventWriter<AppExit>, msg: &str) {
    tracing::error!("mf-game: {msg}");
    write_result_file(false, msg);
    // AppExit::from_code is available on recent Bevy; fall back to Success
    // only if we somehow lack it — CI checks the result file either way.
    exit.write(AppExit::from_code(1));
}

fn write_result_file(ok: bool, detail: &str) {
    let path = std::env::var_os("MF_TEST_KILL_SIDECAR_RESULT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("sidecar-recovery-result.txt"));
    let body = format!("ok={}\n{}\n", if ok { "1" } else { "0" }, detail);
    if let Err(e) = std::fs::write(&path, body) {
        tracing::warn!("mf-game: failed to write {}: {e}", path.display());
    }
}
