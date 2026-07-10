//! Reconnect policy (spec §3.2): if `!is_alive` for >10 s, or the transport
//! reports it's gone, tear down and respawn the sidecar + reconnect the
//! transport with backoff 500 ms -> 4 s, up to 5 attempts before giving up
//! ("fatal error screen").
//!
//! `mf-net` intentionally does NOT know about `mf-game`'s `AppState`
//! (Boot/MainMenu/InGame/...) — it only exposes [`NetStatus`], which the
//! game shell observes and maps onto its own states (spec: "v1 restarts at
//! MainMenu").

use bevy_ecs::prelude::*;

use crate::plugin::{SimAlive, SimLink};

const BACKOFF_START: std::time::Duration = std::time::Duration::from_millis(500);
const BACKOFF_MAX: std::time::Duration = std::time::Duration::from_secs(4);
const MAX_ATTEMPTS: u32 = 5;

/// Current reconnection status. `mf-game` polls this each frame to decide
/// whether to fall back to `ConnectingSim`/`MainMenu`, or show a fatal error.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum NetStatus {
    #[default]
    Connected,
    Reconnecting {
        attempt: u32,
    },
    /// Exhausted `MAX_ATTEMPTS`; the game shell should show a fatal error
    /// screen instead of retrying further.
    Fatal(String),
}

#[derive(Resource, Default)]
pub struct ReconnectState {
    pub status: NetStatus,
    attempts: u32,
    next_attempt_at: Option<std::time::Instant>,
    /// Headless speed to re-apply to a respawned sidecar, if any (mirrors
    /// whatever Boot originally passed to `SimLink::spawn_and_connect`).
    pub headless_speed: Option<f64>,
}

pub fn reconnect_system(
    mut commands: Commands,
    mut state: ResMut<ReconnectState>,
    alive: Res<SimAlive>,
    link: Option<Res<SimLink>>,
) {
    if matches!(state.status, NetStatus::Fatal(_)) {
        return;
    }

    if alive.0 {
        if !matches!(state.status, NetStatus::Connected) {
            tracing::info!("mf-net: connection restored");
        }
        state.status = NetStatus::Connected;
        state.attempts = 0;
        state.next_attempt_at = None;
        return;
    }

    // Not alive: either we've never connected, or we just lost the link.
    if link.is_none() && state.attempts == 0 {
        // Nothing to reconnect yet (Boot hasn't run); leave state as-is.
        return;
    }

    let now = std::time::Instant::now();
    if let Some(next_at) = state.next_attempt_at {
        if now < next_at {
            return; // still backing off
        }
    }

    if state.attempts >= MAX_ATTEMPTS {
        let msg = format!("could not reconnect to the sim after {MAX_ATTEMPTS} attempts");
        tracing::error!("mf-net: {msg}");
        state.status = NetStatus::Fatal(msg);
        commands.remove_resource::<SimLink>();
        return;
    }

    state.attempts += 1;
    state.status = NetStatus::Reconnecting {
        attempt: state.attempts,
    };
    tracing::warn!(
        "mf-net: reconnect attempt {}/{MAX_ATTEMPTS}",
        state.attempts
    );

    match SimLink::spawn_and_connect(state.headless_speed) {
        Ok(new_link) => {
            commands.insert_resource(new_link);
            // Liveness is re-evaluated next frame via `SimAlive`; keep
            // `Reconnecting` until the drain system sees fresh traffic.
        }
        Err(e) => {
            tracing::warn!("mf-net: reconnect attempt {} failed: {e}", state.attempts);
            commands.remove_resource::<SimLink>();
        }
    }

    let backoff = BACKOFF_START
        .saturating_mul(1 << state.attempts.min(4))
        .min(BACKOFF_MAX);
    state.next_attempt_at = Some(now + backoff);
}
