//! App state machine (spec §3.4): `Boot -> ConnectingSim -> MainMenu ->
//! Loading -> InGame`, plus `SimError` for exhausted sidecar reconnects.
//! Mid-game sidecar death stays in `InGame` with a reconnect overlay,
//! re-handshakes, and restores from the latest autosave (or re-inits the
//! current city) without bouncing to the main menu.
//!
//! `mf-net`/`mf-state` don't know about these states — this module is the
//! only place that maps `mf_net::NetStatus` / `mf_state` readiness onto them.

use bevy::app::AppExit;
use bevy::prelude::*;
use mf_net::{
    NetStatus, ReconnectPhase, ReconnectState, ResumePolicy, SidecarDeathReason, SimEvent, SimLink,
    MAX_ATTEMPTS,
};
use mf_protocol::envelope::FromSimJson;
use mf_protocol::{HelloInfo, InitPayload, LoadSavePayload, SetSpeedPayload, ToSim};
use mf_state::{CurrentCity, LatestFields, LatestUi};

use crate::attract::AttractState;
use crate::config::MfConfig;
use crate::saves::SaveManager;

#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    Boot,
    ConnectingSim,
    MainMenu,
    Loading,
    InGame,
    /// Exhausted sidecar reconnect attempts — friendly diagnostics screen,
    /// never a silent freeze.
    SimError,
}

/// Which screen `hud.rs`'s `AppState::MainMenu` systems are showing right
/// now. Not a second `States` machine (that's overkill for a handful of
/// egui panels) — a plain resource `hud.rs` reads/writes directly, since
/// nothing outside the menu (net status, sim init, etc.) reacts to it.
///
/// Owner feedback ("takes me right to city select"): the player must land
/// on `Title` first and explicitly click Play before seeing city cards.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuScreen {
    #[default]
    Title,
    CitySelect,
    /// Save browser: per-slot metadata for continue/load from the title screen.
    LoadGame,
    Settings,
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

/// Whether the pause overlay (`hud.rs`) is showing, and the speed to
/// restore on resume. The sim clock itself is authoritative — pausing is
/// just asking it to run at 0x and remembering what to ask for afterward —
/// so this resource carries no other in-game state.
#[derive(Resource, Debug, Clone, Copy)]
pub struct PauseState {
    pub active: bool,
    pub resume_speed: f64,
}

impl Default for PauseState {
    fn default() -> Self {
        // 1.0 rather than 0.0: a resume before any pause has ever set this
        // (shouldn't happen, but cheap to guard) must not silently relock
        // the clock at 0x.
        PauseState {
            active: false,
            resume_speed: 1.0,
        }
    }
}

/// Toggle pause. Shared by `input.rs` (Esc) and `hud.rs` (the "Resume"
/// button) so both call sites agree on what "current speed" means and
/// can't drift into disagreeing about whether we're paused.
/// Returns true when the toggle actually happened (a missing `SimLink`
/// means neither the freeze nor the resume was sent, so callers must not
/// react as if it did, e.g. by playing a pause sound).
pub fn toggle_pause(pause: &mut PauseState, ui: &LatestUi, link: Option<&SimLink>) -> bool {
    let Some(link) = link else {
        return false;
    };
    if pause.active {
        pause.active = false;
        let _ = link.transport.send(ToSim::SetSpeed(SetSpeedPayload {
            speed: pause.resume_speed,
        }));
    } else {
        // Only latch a fresh resume_speed on the pause->active transition;
        // toggling only ever alternates active on/off, so this branch can't
        // run twice in a row and clobber a good value with 0.
        let current = ui.0.as_ref().map(|s| s.speed).unwrap_or(0.0);
        pause.resume_speed = if current > 0.0 { current } else { 1.0 };
        pause.active = true;
        let _ = link
            .transport
            .send(ToSim::SetSpeed(SetSpeedPayload { speed: 0.0 }));
    }
    true
}

pub struct MfGameStatePlugin;

impl Plugin for MfGameStatePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppState>()
            .init_resource::<SimHello>()
            .init_resource::<PendingInit>()
            .init_resource::<PauseState>()
            // `attract.rs`'s `MfAttractPlugin` isn't wired into `main.rs`'s
            // plugin tuple yet (see that module's integration-handoff doc),
            // but `send_init_system` below already reads `AttractState` —
            // `init_resource` is idempotent (a no-op if `MfAttractPlugin`
            // already inserted it), so eagerly ensuring it exists here means
            // this crate behaves correctly (attract simply never marks a
            // preset inited, so every `Loading` entry inits exactly as it
            // did before this wave) whether or not that wiring has landed.
            .init_resource::<AttractState>()
            .init_resource::<MenuScreen>()
            .add_systems(OnEnter(AppState::Boot), boot_system)
            .add_systems(OnEnter(AppState::MainMenu), reset_menu_screen_system)
            .add_systems(OnEnter(AppState::InGame), on_enter_ingame_system)
            .add_systems(OnExit(AppState::InGame), on_exit_ingame_system)
            .add_systems(Update, net_status_watchdog)
            .add_systems(
                Update,
                connecting_sim_system.run_if(in_state(AppState::ConnectingSim)),
            )
            .add_systems(
                Update,
                ingame_reconnect_system.run_if(in_state(AppState::InGame)),
            )
            .add_systems(OnEnter(AppState::Loading), send_init_system)
            .add_systems(
                Update,
                loading_gate_system.run_if(in_state(AppState::Loading)),
            )
            .add_systems(
                Update,
                autostart_system.run_if(in_state(AppState::MainMenu)),
            )
            .add_systems(Update, graceful_quit_system);
    }
}

/// Re-entering `MainMenu` (fresh boot, or returning from `SimError`) must
/// always start at `Title` — never resume wherever the player happened to
/// leave the menu screen state last time.
fn reset_menu_screen_system(mut screen: ResMut<MenuScreen>) {
    *screen = menu_screen_override().unwrap_or(MenuScreen::Title);
}

/// Verify/screenshot-tooling escape hatch: `MF_MENU_SCREEN=title|city|
/// settings` forces which `MenuScreen` `MainMenu` opens on, so the verify
/// harness can capture all three screens without a player driving egui
/// clicks by hand (no display server input under `xvfb-run`). Unset (the
/// normal player path) always lands on `Title` per the fixed state
/// machine above. A malformed/unknown value degrades to "unset" for the
/// same reason `MF_AUTOSTART`'s parsing does — a stray env var must not
/// strand anything.
fn menu_screen_override() -> Option<MenuScreen> {
    match std::env::var("MF_MENU_SCREEN").ok()?.trim() {
        "title" => Some(MenuScreen::Title),
        "city" => Some(MenuScreen::CitySelect),
        "load" => Some(MenuScreen::LoadGame),
        "settings" => Some(MenuScreen::Settings),
        _ => None,
    }
}

/// Boot: load config, spawn the sidecar + connect, then move on to
/// `ConnectingSim`. On failure, seed `ReconnectState` so `mf-net`'s own
/// reconnect system (backoff 500ms->4s, 3 attempts) picks up the retry
/// instead of duplicating that policy here.
fn boot_system(
    mut commands: Commands,
    config: Res<MfConfig>,
    mut reconnect: ResMut<ReconnectState>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    // `MfConfig` is loaded and inserted in `main` before the window is
    // created (so size/position/fullscreen apply on first frame). Boot only
    // mirrors the weather preference into the render-facing resource.
    commands.insert_resource(mf_state::WeatherEffects {
        enabled: config.weather_effects,
    });

    match SimLink::spawn_and_connect(None) {
        Ok(link) => {
            commands.insert_resource(link);
            reconnect.status = NetStatus::Connected;
        }
        Err(e) => {
            tracing::warn!("mf-game: initial sidecar spawn failed, deferring to reconnect: {e}");
            reconnect.status = NetStatus::Reconnecting {
                attempt: 0,
                reason: SidecarDeathReason::ProcessExited { code: None },
                phase: ReconnectPhase::Respawning,
            };
        }
    }
    next_state.set(AppState::ConnectingSim);
}

fn on_enter_ingame_system(mut reconnect: ResMut<ReconnectState>) {
    // From here on, a sidecar death must resume in place — not bounce to
    // MainMenu after a successful respawn.
    reconnect.resume_policy = ResumePolicy::InGameSession;
}

fn on_exit_ingame_system(mut pause: ResMut<PauseState>, mut reconnect: ResMut<ReconnectState>) {
    pause.active = false;
    // Default back to menu policy; SimError entry happens via watchdog in
    // the same frame and does not need InGameSession anymore.
    reconnect.resume_policy = ResumePolicy::ToMenu;
}

/// Fatal reconnect → `SimError` (diagnostics screen). Never silently freeze,
/// and never dump a mid-game player back to MainMenu on sidecar death.
fn net_status_watchdog(
    reconnect: Res<ReconnectState>,
    state: Res<State<AppState>>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if !matches!(reconnect.status, NetStatus::Fatal(_)) {
        return;
    }
    if matches!(*state.get(), AppState::SimError | AppState::Boot) {
        return;
    }
    next_state.set(AppState::SimError);
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
    // A reconnect replaces the SimLink resource wholesale; the fresh sidecar
    // has never seen our hello, so the send-once latch must reset with it or
    // the handshake never completes and we sit here forever.
    if link.as_ref().map(|l| l.is_added()).unwrap_or(false) {
        *sent = false;
    }
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

/// Mid-game sidecar recovery: while `NetStatus::Reconnecting` is in
/// Handshaking/Reloading, re-exchange hello, stage autosave (or re-init the
/// current city), and call [`ReconnectState::mark_recovered`] once the
/// readiness gate passes — without leaving `InGame`.
#[allow(clippy::too_many_arguments)]
fn ingame_reconnect_system(
    mut events: EventReader<SimEvent>,
    link: Option<Res<SimLink>>,
    mut reconnect: ResMut<ReconnectState>,
    mut hello: ResMut<SimHello>,
    mut saves: ResMut<SaveManager>,
    pending: Res<PendingInit>,
    mut city: ResMut<CurrentCity>,
    mut fields: ResMut<LatestFields>,
    mut ui: ResMut<LatestUi>,
    mut hello_sent: Local<bool>,
    mut load_staged: Local<bool>,
) {
    let NetStatus::Reconnecting { phase, attempt, .. } = reconnect.status.clone() else {
        *hello_sent = false;
        *load_staged = false;
        return;
    };

    match phase {
        ReconnectPhase::Respawning => {
            *hello_sent = false;
            *load_staged = false;
        }
        ReconnectPhase::Handshaking => {
            if link.as_ref().map(|l| l.is_added()).unwrap_or(false) {
                *hello_sent = false;
                *load_staged = false;
            }
            if !*hello_sent {
                if let Some(link) = &link {
                    let _ = link
                        .transport
                        .send(ToSim::Hello(mf_protocol::ClientHelloPayload {
                            client_protocol_version: mf_protocol::PROTOCOL_VERSION,
                        }));
                    *hello_sent = true;
                    tracing::info!(
                        "mf-game: reconnect hello sent (attempt {attempt}/{MAX_ATTEMPTS})"
                    );
                }
            }
            for SimEvent(msg) in events.read() {
                let mf_protocol::FromSimMsg::Json(FromSimJson::Hello(info)) = msg else {
                    continue;
                };
                if info.protocol_version != mf_protocol::PROTOCOL_VERSION {
                    tracing::error!(
                        "mf-game: reconnect hello version mismatch ({} != {})",
                        info.protocol_version,
                        mf_protocol::PROTOCOL_VERSION
                    );
                    continue;
                }
                hello.0 = Some(info.clone());
                // Clear stale readiness so we don't mark recovered on
                // pre-crash city/fields/ui still sitting in resources.
                *city = CurrentCity::default();
                fields.0 = None;
                ui.0 = None;
                if !*load_staged {
                    stage_session_restore(&mut saves, &pending, link.as_deref());
                    *load_staged = true;
                }
                reconnect.mark_reloading();
            }
        }
        ReconnectPhase::Reloading => {
            if !*load_staged {
                if let Some(link) = &link {
                    stage_session_restore(&mut saves, &pending, Some(link));
                    *load_staged = true;
                }
            }
            if city.masks_complete() && fields.0.is_some() && ui.0.is_some() {
                tracing::info!("mf-game: in-game session restored after sidecar reconnect");
                reconnect.mark_recovered();
                *hello_sent = false;
                *load_staged = false;
            }
        }
    }
}

/// Prefer the autosave slot; if none exists yet (crash before the first
/// 10-day autosave), re-init the current city so the player still lands
/// back in-world rather than at the menu.
fn stage_session_restore(saves: &mut SaveManager, pending: &PendingInit, link: Option<&SimLink>) {
    let Some(link) = link else {
        tracing::warn!("mf-game: reconnect has no SimLink to restore session");
        return;
    };
    // Mid-game reconnect never races an OnEnter(Loading) Init, so LoadSave
    // can go on the wire immediately (unlike the menu Continue path).
    if let Some(sim_json) = saves.take_autosave_json_for_reconnect() {
        tracing::info!("mf-game: reconnect restoring from autosave");
        let _ = link
            .transport
            .send(ToSim::LoadSave(LoadSavePayload { json: sim_json }));
        return;
    }
    tracing::info!(
        "mf-game: reconnect re-initing city '{}' (no autosave)",
        pending.preset_key
    );
    let _ = link.transport.send(ToSim::Init(InitPayload {
        seed: rand_seed(),
        difficulty: pending.difficulty,
        size: None,
        preset_key: Some(pending.preset_key.clone()),
        rules: None,
    }));
}

/// OnEnter(Loading): send `init` for whatever `MainMenu` chose.
///
/// `attract: Option<ResMut<AttractState>>` (not a hard `Res`/`ResMut`, same
/// convention as `link: Option<Res<SimLink>>` just above): `attract.rs`'s
/// `MfAttractPlugin` — the only inserter of `AttractState` — isn't wired
/// into `main.rs`'s plugin tuple yet (see that module's integration-handoff
/// doc), so this must degrade gracefully to "attract never ran" rather than
/// panicking on a missing resource until that wiring lands.
fn send_init_system(
    link: Option<Res<SimLink>>,
    pending: Res<PendingInit>,
    attract: Option<ResMut<AttractState>>,
    saves: Option<Res<SaveManager>>,
) {
    // A staged slot load supersedes a fresh init entirely: LoadSave carries
    // the whole sim state, so the fresh city this would build is thrown
    // away the same frame (and briefly wastes sidecar work).
    if saves.as_deref().is_some_and(SaveManager::has_pending_load) {
        return;
    }
    let Some(link) = link else {
        tracing::warn!("mf-game: entered Loading with no SimLink");
        return;
    };
    if let Some(mut attract) = attract {
        if can_reuse_attract_city(
            &attract.inited_preset,
            &pending.preset_key,
            pending.difficulty,
        ) {
            // `attract.rs` already streamed exactly this city while the
            // player sat at the MainMenu diorama (verified: the sidecar's
            // `handleInit` always reinitializes fresh) — re-sending `init`
            // here would throw away everything that streamed in during the
            // orbit and restart the sim from scratch. Just normalize the
            // clock back down from attract mode's 30x cinematic speed.
            let _ = link
                .transport
                .send(ToSim::SetSpeed(SetSpeedPayload { speed: 1.0 }));
            return;
        }
        // Whatever attract-mode had inited (if anything) doesn't match what
        // is actually being started (different city, or a non-Normal
        // difficulty pick), so a real `Loading` init supersedes it below —
        // clear the stale marker.
        attract.inited_preset = None;
    }
    let _ = link.transport.send(ToSim::Init(InitPayload {
        seed: rand_seed(),
        difficulty: pending.difficulty,
        size: None,
        preset_key: Some(pending.preset_key.clone()),
        rules: None,
    }));
}

/// Pure decision for `send_init_system`'s attract-reuse fast path: the
/// already-streamed attract city can only stand in for the real init when
/// BOTH the preset matches what attract streamed AND the player left the
/// difficulty at Normal — attract always inits at `Difficulty::Normal` (see
/// `attract.rs`'s `send_attract_init`), so skipping the re-init on an
/// Easy/Hard pick would silently hand the player a Normal-difficulty city.
fn can_reuse_attract_city(
    inited_preset: &Option<String>,
    pending_preset: &str,
    pending_difficulty: mf_protocol::Difficulty,
) -> bool {
    inited_preset.as_deref() == Some(pending_preset)
        && pending_difficulty == mf_protocol::Difficulty::Normal
}

pub(crate) fn rand_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
}

/// Dev/CI convenience (not in the original spec's v1 IN-list, added while
/// implementing `mf-render`'s verification pass): `MF_AUTOSTART=<presetKey>`
/// skips the egui `MainMenu` city picker entirely and jumps straight to
/// `Loading` with that city, Normal difficulty. Headless/CI environments
/// (this box included — no way to click through egui without a display)
/// need a non-interactive path to `InGame`, and it's a reasonable product
/// feature too (fast-boot for screenshots, automated smoke tests, demo
/// kiosks). A malformed/empty value is treated as "unset" so a stray env
/// var can't silently strand a real player at a blank screen.
fn autostart_system(
    mut pending: ResMut<PendingInit>,
    mut next_state: ResMut<NextState<AppState>>,
    mut done: Local<bool>,
    mut frames_in_menu: Local<u32>,
) {
    if *done {
        return;
    }
    // When the verify harness is active, hold at MainMenu for a few frames
    // first so verify.rs can screenshot the menu itself — v0.1.0-alpha
    // shipped with the menu never having been rendered by ANY test (autostart
    // skipped it in every verify run, and it was in fact invisible).
    *frames_in_menu += 1;
    if std::env::var_os("MF_VERIFY_DIR").is_some() && *frames_in_menu < 30 {
        return;
    }
    *done = true;
    let Ok(preset) = std::env::var("MF_AUTOSTART") else {
        return;
    };
    let preset = preset.trim();
    if preset.is_empty() {
        return;
    }
    pending.preset_key = preset.to_string();
    next_state.set(AppState::Loading);
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

#[cfg(test)]
mod tests {
    use super::*;
    use mf_protocol::Difficulty;

    // --- attract-reuse fast path (see `can_reuse_attract_city`) ------------

    #[test]
    fn reuses_attract_city_when_preset_matches_at_normal_difficulty() {
        assert!(can_reuse_attract_city(
            &Some("nyc".to_string()),
            "nyc",
            Difficulty::Normal
        ));
    }

    #[test]
    fn does_not_reuse_when_preset_differs() {
        assert!(!can_reuse_attract_city(
            &Some("nyc".to_string()),
            "boston",
            Difficulty::Normal
        ));
    }

    #[test]
    fn does_not_reuse_when_attract_never_inited() {
        assert!(!can_reuse_attract_city(&None, "nyc", Difficulty::Normal));
    }

    #[test]
    fn does_not_reuse_on_non_normal_difficulty() {
        // Attract always inits at Normal (see attract.rs's
        // `send_attract_init`) — an Easy/Hard pick must force a real init or
        // the player silently gets a Normal-difficulty city.
        assert!(!can_reuse_attract_city(
            &Some("nyc".to_string()),
            "nyc",
            Difficulty::Hard
        ));
        assert!(!can_reuse_attract_city(
            &Some("nyc".to_string()),
            "nyc",
            Difficulty::Easy
        ));
    }
}
