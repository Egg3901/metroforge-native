//! App state machine (spec §3.4): `Boot -> ConnectingSim -> MainMenu ->
//! Loading -> InGame`. `mf-net`/`mf-state` don't know about these states —
//! this module is the only place that maps `mf_net::NetStatus` /
//! `mf_state` readiness onto them.

use bevy::app::AppExit;
use bevy::prelude::*;
use mf_net::{NetStatus, ReconnectState, SimEvent, SimLink};
use mf_protocol::envelope::FromSimJson;
use mf_protocol::{HelloInfo, InitPayload, ToSim};
use mf_state::{CurrentCity, LatestFields, LatestUi};

use crate::config::MfConfig;

#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    Boot,
    ConnectingSim,
    MainMenu,
    Loading,
    InGame,
}

/// Sidecar `hello` payload (city list + default world size), once received.
#[derive(Resource, Default)]
pub struct SimHello(pub Option<HelloInfo>);

/// The city + difficulty the player picked in `MainMenu`, carried into
/// `Loading` to build the `init` message.
#[derive(Resource, Clone, Debug)]
pub struct PendingInit {
    pub preset_key: String,
    pub difficulty: mf_protocol::Difficulty,
}

impl Default for PendingInit {
    fn default() -> Self {
        // NYC preselected per spec §3.4/§5.
        PendingInit {
            preset_key: "nyc".to_string(),
            difficulty: mf_protocol::Difficulty::Normal,
        }
    }
}

pub struct MfGameStatePlugin;

impl Plugin for MfGameStatePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppState>()
            .init_resource::<SimHello>()
            .init_resource::<PendingInit>()
            .add_systems(OnEnter(AppState::Boot), boot_system)
            .add_systems(Update, net_status_watchdog)
            .add_systems(
                Update,
                connecting_sim_system.run_if(in_state(AppState::ConnectingSim)),
            )
            .add_systems(OnEnter(AppState::Loading), send_init_system)
            .add_systems(
                Update,
                loading_gate_system.run_if(in_state(AppState::Loading)),
            )
            .add_systems(Update, graceful_quit_system);
    }
}

/// Boot: load config, spawn the sidecar + connect, then move on to
/// `ConnectingSim`. On failure, seed `ReconnectState` so `mf-net`'s own
/// reconnect system (backoff 500ms->4s, 5 attempts) picks up the retry
/// instead of duplicating that policy here.
fn boot_system(
    mut commands: Commands,
    mut reconnect: ResMut<ReconnectState>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    let config = MfConfig::load();
    commands.insert_resource(config);

    match SimLink::spawn_and_connect(None) {
        Ok(link) => {
            commands.insert_resource(link);
            reconnect.status = NetStatus::Connected;
        }
        Err(e) => {
            tracing::warn!("mf-game: initial sidecar spawn failed, deferring to reconnect: {e}");
            reconnect.status = NetStatus::Reconnecting { attempt: 1 };
        }
    }
    next_state.set(AppState::ConnectingSim);
}

/// From any state: if `mf-net` gives up (5 failed reconnect attempts), fall
/// back to `MainMenu` per spec ("v1 restarts at MainMenu") so the player at
/// least sees a menu (a toast/HUD banner surfaces the fatal error — see
/// `hud.rs`), rather than a black screen.
fn net_status_watchdog(
    reconnect: Res<ReconnectState>,
    state: Res<State<AppState>>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if matches!(reconnect.status, NetStatus::Fatal(_))
        && *state.get() != AppState::MainMenu
        && *state.get() != AppState::Boot
    {
        next_state.set(AppState::MainMenu);
    }
}

/// ConnectingSim: send our hello, then wait for the sidecar's hello in
/// return (spec §1.4).
fn connecting_sim_system(
    mut events: EventReader<SimEvent>,
    link: Option<Res<SimLink>>,
    mut sent: Local<bool>,
    mut hello: ResMut<SimHello>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if !*sent {
        if let Some(link) = &link {
            let _ = link
                .transport
                .send(ToSim::Hello(mf_protocol::ClientHelloPayload {
                    client_protocol_version: mf_protocol::PROTOCOL_VERSION,
                }));
            *sent = true;
        }
    }

    for SimEvent(msg) in events.read() {
        if let mf_protocol::FromSimMsg::Json(FromSimJson::Hello(info)) = msg {
            if info.protocol_version != mf_protocol::PROTOCOL_VERSION {
                tracing::error!(
                    "mf-game: sidecar protocol version {} != client {}; aborting",
                    info.protocol_version,
                    mf_protocol::PROTOCOL_VERSION
                );
                continue;
            }
            hello.0 = Some(info.clone());
            *sent = false; // reset for a future reconnect
            next_state.set(AppState::MainMenu);
        }
    }
}

/// OnEnter(Loading): send `init` for whatever `MainMenu` chose.
fn send_init_system(link: Option<Res<SimLink>>, pending: Res<PendingInit>) {
    let Some(link) = link else {
        tracing::warn!("mf-game: entered Loading with no SimLink");
        return;
    };
    let _ = link.transport.send(ToSim::Init(InitPayload {
        seed: rand_seed(),
        difficulty: pending.difficulty,
        size: None,
        preset_key: Some(pending.preset_key.clone()),
        rules: None,
    }));
}

fn rand_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
}

/// Loading -> InGame once `ready` + all flagged masks + first `fields` +
/// first `ui` have all arrived (spec §3.4/§5).
fn loading_gate_system(
    city: Res<CurrentCity>,
    fields: Res<LatestFields>,
    ui: Res<LatestUi>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if city.masks_complete() && fields.0.is_some() && ui.0.is_some() {
        next_state.set(AppState::InGame);
    }
}

/// Graceful quit (spec §3.4): on `AppExit`, tell the sim to shut down;
/// `SidecarProcess::drop` is the backstop kill if it doesn't exit cleanly in
/// time.
fn graceful_quit_system(mut exit_events: EventReader<AppExit>, link: Option<Res<SimLink>>) {
    for _ in exit_events.read() {
        if let Some(link) = &link {
            let _ = link.transport.send(ToSim::Shutdown);
        }
    }
}
