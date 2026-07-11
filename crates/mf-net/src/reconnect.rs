//! Reconnect policy (1.0): detect sidecar death via process exit **or**
//! websocket silence > 5 s (distinguished in [`SidecarDeathReason`]), then
//! tear down and respawn with backoff 500 ms -> 4 s, up to 3 attempts.
//!
//! Mid-game recovery does **not** bounce to `MainMenu`: when
//! [`ResumePolicy::InGameSession`] is set, a successful respawn leaves
//! [`NetStatus::Reconnecting`] in the `Handshaking`/`Reloading` phases until
//! the game shell calls [`ReconnectState::mark_recovered`] after re-hello +
//! autosave/city restore. Exhausting attempts surfaces
//! [`NetStatus::Fatal`] with the sidecar log tail for the diagnostics screen.
//!
//! `mf-net` intentionally does NOT know about `mf-game`'s `AppState` — it
//! only exposes [`NetStatus`], which the game shell observes.

use std::time::{Duration, Instant};

use bevy_ecs::prelude::*;

use crate::plugin::{SimAlive, SimLink};
use crate::sidecar::SidecarDeathReason;
use crate::ws_transport::LIVENESS_WINDOW;

const BACKOFF_START: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(4);
/// 1.0: three failed restarts then the friendly error screen — never a
/// silent freeze.
pub const MAX_ATTEMPTS: u32 = 3;

/// What "connection restored" means after a respawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResumePolicy {
    /// Fresh boot / menu: transport liveness alone is enough; the game shell
    /// runs the normal hello → MainMenu path.
    #[default]
    ToMenu,
    /// Mid-game: after respawn the game must re-handshake and reload the
    /// city/autosave before we call the link healthy again.
    InGameSession,
}

/// Phase of an in-flight reconnect (only meaningful while `Reconnecting`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectPhase {
    /// Waiting for backoff / about to spawn.
    Respawning,
    /// New `SimLink` is up; game shell must exchange hello.
    Handshaking,
    /// Hello done; game shell is loading autosave / re-initing the city.
    Reloading,
}

/// Diagnostics bundled into a fatal reconnect failure for the error screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FatalDiagnostics {
    pub message: String,
    pub reason: SidecarDeathReason,
    pub log_tail: String,
}

impl FatalDiagnostics {
    /// Plain-text blob for the "Copy diagnostics" button.
    pub fn clipboard_text(&self) -> String {
        format!(
            "MetroForge sidecar failure\n\
             reason: {} ({})\n\
             detail: {}\n\
             ---\n\
             sidecar log tail:\n\
             {}",
            self.reason.label(),
            self.message,
            self.reason.detail(),
            if self.log_tail.trim().is_empty() {
                "(empty)"
            } else {
                self.log_tail.trim()
            }
        )
    }
}

/// Current reconnection status. `mf-game` polls this each frame.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum NetStatus {
    #[default]
    Connected,
    Reconnecting {
        attempt: u32,
        reason: SidecarDeathReason,
        phase: ReconnectPhase,
    },
    /// Exhausted [`MAX_ATTEMPTS`]; the game shell shows the fatal error
    /// screen (log tail + copy-diagnostics) instead of retrying further.
    Fatal(FatalDiagnostics),
}

#[derive(Resource, Default)]
pub struct ReconnectState {
    pub status: NetStatus,
    attempts: u32,
    next_attempt_at: Option<Instant>,
    /// Headless speed to re-apply to a respawned sidecar, if any (mirrors
    /// whatever Boot originally passed to `SimLink::spawn_and_connect`).
    pub headless_speed: Option<f64>,
    /// Set by `mf-game` when entering `InGame` so mid-session crashes resume
    /// in place rather than bouncing to the menu.
    pub resume_policy: ResumePolicy,
    /// Last observed death reason (retained across attempts for the fatal
    /// screen even if a later attempt fails for a different reason).
    pub last_death_reason: Option<SidecarDeathReason>,
    /// Sidecar stderr captured at the moment we declared death / gave up.
    pub last_log_tail: String,
}

impl ReconnectState {
    /// Clear a fatal status so Boot/MainMenu can start fresh after the
    /// diagnostics screen's "Back to menu" action.
    pub fn clear_fatal(&mut self) {
        self.status = NetStatus::Connected;
        self.attempts = 0;
        self.next_attempt_at = None;
        self.last_death_reason = None;
        self.last_log_tail.clear();
        self.resume_policy = ResumePolicy::ToMenu;
    }

    /// Game shell finished re-handshake + city/autosave restore — promote
    /// back to [`NetStatus::Connected`] and clear attempt counters.
    pub fn mark_recovered(&mut self) {
        tracing::info!("mf-net: in-game session recovered");
        self.status = NetStatus::Connected;
        self.attempts = 0;
        self.next_attempt_at = None;
        self.last_death_reason = None;
        self.last_log_tail.clear();
    }

    /// Advance a mid-game reconnect from Handshaking → Reloading once the
    /// game shell has exchanged hello and staged the city restore.
    pub fn mark_reloading(&mut self) {
        if let NetStatus::Reconnecting {
            attempt, reason, ..
        } = &self.status
        {
            self.status = NetStatus::Reconnecting {
                attempt: *attempt,
                reason: reason.clone(),
                phase: ReconnectPhase::Reloading,
            };
        }
    }

    /// Record a freshly observed death (process exit or WS silence) before
    /// the respawn loop starts. No-op if already reconnecting/fatal.
    pub fn note_death(&mut self, reason: SidecarDeathReason, log_tail: String) {
        if matches!(
            self.status,
            NetStatus::Reconnecting { .. } | NetStatus::Fatal(_)
        ) {
            return;
        }
        tracing::warn!("mf-net: sidecar death detected ({})", reason.detail());
        self.last_death_reason = Some(reason.clone());
        self.last_log_tail = log_tail;
        self.attempts = 0;
        self.next_attempt_at = None;
        self.status = NetStatus::Reconnecting {
            attempt: 0,
            reason,
            phase: ReconnectPhase::Respawning,
        };
    }
}

pub fn reconnect_system(
    mut commands: Commands,
    mut state: ResMut<ReconnectState>,
    alive: Res<SimAlive>,
    link: Option<ResMut<SimLink>>,
) {
    if matches!(state.status, NetStatus::Fatal(_)) {
        return;
    }

    // Mid-game: transport liveness alone must NOT clear Reconnecting — the
    // game shell still has to hello + reload. Boot/menu path still treats
    // a live transport as Connected.
    if alive.0 && matches!(state.resume_policy, ResumePolicy::ToMenu) {
        if !matches!(state.status, NetStatus::Connected) {
            tracing::info!("mf-net: connection restored (menu path)");
        }
        state.status = NetStatus::Connected;
        state.attempts = 0;
        state.next_attempt_at = None;
        return;
    }

    if alive.0 && matches!(state.status, NetStatus::Connected) {
        return;
    }

    // Detect death while we thought we were connected.
    if matches!(state.status, NetStatus::Connected) {
        let death = match link {
            Some(mut link) => detect_death(&mut link).map(|reason| {
                let log_tail = link
                    .sidecar
                    .as_ref()
                    .map(|s| s.log_tail())
                    .unwrap_or_default();
                (reason, log_tail)
            }),
            None if state.resume_policy == ResumePolicy::InGameSession => Some((
                SidecarDeathReason::WebsocketSilence {
                    silence_ms: LIVENESS_WINDOW.as_millis() as u64,
                },
                String::new(),
            )),
            None => None,
        };
        if let Some((reason, log_tail)) = death {
            commands.remove_resource::<SimLink>();
            state.note_death(reason, log_tail);
        }
        // Boot hasn't inserted a link yet, or we're still healthy — either
        // way, spawn runs next frame once status is Reconnecting.
        return;
    }

    let NetStatus::Reconnecting {
        reason,
        phase,
        attempt: current_attempt,
    } = state.status.clone()
    else {
        return;
    };

    let now = Instant::now();

    // Already spawned a link for this attempt — wait for the game shell,
    // unless the fresh link died again (then fall back to Respawning).
    if matches!(
        phase,
        ReconnectPhase::Handshaking | ReconnectPhase::Reloading
    ) {
        let Some(mut link) = link else {
            // `insert_resource` from the spawn path is deferred until the
            // end of the schedule — a missing link for a frame is normal,
            // not a fresh death.
            return;
        };
        if let Some(new_reason) = detect_death(&mut link) {
            let log_tail = link
                .sidecar
                .as_ref()
                .map(|s| s.log_tail())
                .unwrap_or_default();
            tracing::warn!(
                "mf-net: sidecar died again during {:?}: {}",
                phase,
                new_reason.detail()
            );
            commands.remove_resource::<SimLink>();
            state.last_death_reason = Some(new_reason.clone());
            if !log_tail.is_empty() {
                state.last_log_tail = log_tail;
            }
            let backoff = BACKOFF_START
                .saturating_mul(1 << current_attempt.min(4))
                .min(BACKOFF_MAX);
            state.next_attempt_at = Some(now + backoff);
            state.status = NetStatus::Reconnecting {
                attempt: current_attempt,
                reason: new_reason,
                phase: ReconnectPhase::Respawning,
            };
        }
        return;
    }

    if let Some(next_at) = state.next_attempt_at {
        if now < next_at {
            return;
        }
    }

    if state.attempts >= MAX_ATTEMPTS {
        let reason = state.last_death_reason.clone().unwrap_or(reason);
        let msg = format!(
            "could not reconnect to the simulation after {MAX_ATTEMPTS} attempts ({})",
            reason.label()
        );
        tracing::error!("mf-net: {msg}");
        state.status = NetStatus::Fatal(FatalDiagnostics {
            message: msg,
            reason,
            log_tail: state.last_log_tail.clone(),
        });
        commands.remove_resource::<SimLink>();
        return;
    }

    state.attempts += 1;
    let attempt = state.attempts;
    state.status = NetStatus::Reconnecting {
        attempt,
        reason: reason.clone(),
        phase: ReconnectPhase::Respawning,
    };
    tracing::warn!(
        "mf-net: reconnect attempt {attempt}/{MAX_ATTEMPTS} ({})",
        reason.label()
    );

    // Ensure any prior link is gone before spawning.
    commands.remove_resource::<SimLink>();

    match SimLink::spawn_and_connect(state.headless_speed) {
        Ok(new_link) => {
            commands.insert_resource(new_link);
            state.status = NetStatus::Reconnecting {
                attempt,
                reason,
                phase: ReconnectPhase::Handshaking,
            };
            if matches!(state.resume_policy, ResumePolicy::ToMenu) {
                // Menu path: drain/ping will mark alive and the branch at the
                // top of this system promotes to Connected next frame.
            }
        }
        Err(e) => {
            tracing::warn!("mf-net: reconnect attempt {attempt} failed: {e}");
            let backoff = BACKOFF_START
                .saturating_mul(1 << attempt.min(4))
                .min(BACKOFF_MAX);
            state.next_attempt_at = Some(now + backoff);
            state.status = NetStatus::Reconnecting {
                attempt,
                reason,
                phase: ReconnectPhase::Respawning,
            };
        }
    }
}

/// Prefer process-exit detection (immediate) over websocket silence.
fn detect_death(link: &mut SimLink) -> Option<SidecarDeathReason> {
    if let Some(sidecar) = link.sidecar.as_mut() {
        if let Some(status) = sidecar.try_exit_status() {
            return Some(SidecarDeathReason::ProcessExited {
                code: status.code(),
            });
        }
    }
    if !link.transport.is_alive() {
        let silence_ms = link.transport.silence_duration().as_millis() as u64;
        return Some(SidecarDeathReason::WebsocketSilence { silence_ms });
    }
    None
}
