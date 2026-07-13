//! `EmbeddedTransport` — the in-process Rust sim behind the `SimTransport`
//! seam (P4). It is a drop-in peer of [`crate::ws_transport::WsTransport`]: same
//! trait, same crossbeam-channel + background-thread lifecycle, same liveness
//! model. The difference is that instead of a blocking WebSocket to the Bun
//! sidecar, the worker thread owns a [`mf_sim::GameState`] and drives
//! `sim_tick` directly, translating wire messages to/from the sim through
//! [`crate::host`].
//!
//! No tokio (matches the existing model). The sim itself stays deterministic:
//! only the tick CADENCE uses a wall clock (a 20 Hz step timer, exactly like the
//! TS `sim.worker.ts` `setInterval(..., 50)`); the sim math is seeded and
//! wall-clock-free.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver, Sender, TryRecvError};
use mf_protocol::envelope::FromSimJson;
use mf_protocol::{FromSimMsg, ToSim};
use mf_sim::{apply_command, new_game, sim_tick, GameState, NewGameOptions};

use crate::host;
use crate::transport::SimTransport;
use crate::ws_transport::LIVENESS_WINDOW;

/// Host step cadence: 20 steps/sec, mirroring the TS worker's 50ms interval.
const STEP: Duration = Duration::from_millis(50);
/// Worker wake granularity: small enough that inbound commands are picked up
/// with low latency (like `WsTransport`'s 4ms poll) without busy-spinning.
const WAKE: Duration = Duration::from_millis(4);
/// UI (`ui`) is emitted every Nth step -> 2 Hz, matching the TS worker's
/// `uiCountdown = 10`.
const UI_EVERY_STEPS: u32 = 10;
/// Max ticks advanced in a single step at high speed (matches the TS worker's
/// `ticksRun < 400` guard).
const MAX_TICKS_PER_STEP: u32 = 400;

/// In-process Rust-sim transport. See module docs.
pub struct EmbeddedTransport {
    to_sim_tx: Sender<ToSim>,
    from_sim_rx: Receiver<FromSimMsg>,
    last_inbound_millis: Arc<AtomicU64>,
    started_at: Instant,
    closed: Arc<AtomicBool>,
    _worker: JoinHandle<()>,
}

impl EmbeddedTransport {
    /// Spawn the sim worker thread and immediately queue the `hello` handshake
    /// (the client waits on it before sending `init`, exactly as with the
    /// sidecar). Never fails to connect: there is no socket to open.
    pub fn connect() -> Self {
        let (to_sim_tx, to_sim_rx) = unbounded::<ToSim>();
        let (from_sim_tx, from_sim_rx) = unbounded::<FromSimMsg>();
        let last_inbound_millis = Arc::new(AtomicU64::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let started_at = Instant::now();

        let worker_last = last_inbound_millis.clone();
        let worker_closed = closed.clone();
        let worker = std::thread::Builder::new()
            .name("mf-net-embedded".into())
            .spawn(move || {
                run_worker(
                    to_sim_rx,
                    from_sim_tx,
                    worker_last,
                    started_at,
                    worker_closed,
                );
            })
            .expect("failed to spawn mf-net-embedded thread");

        EmbeddedTransport {
            to_sim_tx,
            from_sim_rx,
            last_inbound_millis,
            started_at,
            closed,
            _worker: worker,
        }
    }
}

impl SimTransport for EmbeddedTransport {
    fn send(&self, msg: ToSim) -> anyhow::Result<()> {
        self.to_sim_tx
            .send(msg)
            .map_err(|e| anyhow::anyhow!("mf-net embedded worker gone: {e}"))
    }

    fn try_recv(&self) -> Option<FromSimMsg> {
        match self.from_sim_rx.try_recv() {
            Ok(msg) => Some(msg),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }

    fn is_alive(&self) -> bool {
        if self.closed.load(Ordering::Relaxed) {
            return false;
        }
        self.silence_duration() < LIVENESS_WINDOW
    }

    fn silence_duration(&self) -> Duration {
        let last = self.last_inbound_millis.load(Ordering::Relaxed);
        if last == 0 {
            return self.started_at.elapsed();
        }
        let elapsed_ms = self.started_at.elapsed().as_millis() as u64;
        Duration::from_millis(elapsed_ms.saturating_sub(last))
    }
}

/// The worker's mutable sim host state (mirrors the module-level `let`s in
/// `sim.worker.ts`).
struct Host {
    state: Option<GameState>,
    speed: f64,
    fields_version: u32,
    bankrupt: bool,
    won: bool,
    accumulator: f64,
    ui_countdown: u32,
    /// Preset key from the last `init`, echoed into replays.
    preset_key: Option<String>,
    /// Procedural size from the last `init`, echoed into replays.
    size: Option<mf_protocol::CitySize>,
}

impl Host {
    fn new() -> Self {
        Host {
            state: None,
            speed: 1.0,
            fields_version: 1,
            bankrupt: false,
            won: false,
            accumulator: 0.0,
            ui_countdown: 0,
            preset_key: None,
            size: None,
        }
    }
}

fn size_from_wire(s: mf_protocol::CitySize) -> mf_sim::city::presets::MapSize {
    use mf_protocol::CitySize as C;
    use mf_sim::city::presets::MapSize as M;
    match s {
        C::Small => M::Small,
        C::Medium => M::Medium,
        C::Large => M::Large,
    }
}

fn rules_from_wire(r: &mf_protocol::ScenarioRules) -> mf_sim::types::ScenarioRules {
    mf_sim::types::ScenarioRules {
        scenario_id: r.scenario_id.clone(),
        starting_modes: r
            .starting_modes
            .iter()
            .map(|&m| host::mode_from_wire(m))
            .collect(),
        lock_modes: r.lock_modes,
        max_day: r.max_day,
        approval_floor: r.approval_floor,
        starting_cash: r.starting_cash,
        daily_subsidy: r.daily_subsidy,
        era_label: r.era_label.clone(),
    }
}

fn run_worker(
    to_sim_rx: Receiver<ToSim>,
    from_sim_tx: Sender<FromSimMsg>,
    last_inbound_millis: Arc<AtomicU64>,
    started_at: Instant,
    closed: Arc<AtomicBool>,
) {
    let mut host = Host::new();

    // Emit `hello` immediately so the client handshake completes on connect.
    let emit = |tx: &Sender<FromSimMsg>, msg: FromSimMsg| -> bool { tx.send(msg).is_ok() };
    if !emit(
        &from_sim_tx,
        FromSimMsg::Json(FromSimJson::Hello(host::hello_info())),
    ) {
        closed.store(true, Ordering::Relaxed);
        return;
    }

    let mut last_step = Instant::now();
    loop {
        // 1. drain all queued inbound wire messages (client -> sim).
        loop {
            match to_sim_rx.try_recv() {
                Ok(msg) => {
                    if matches!(msg, ToSim::Shutdown) {
                        let _ = from_sim_tx.send(FromSimMsg::Json(FromSimJson::Bye));
                        closed.store(true, Ordering::Relaxed);
                        return;
                    }
                    if !handle_inbound(&mut host, msg, &from_sim_tx) {
                        closed.store(true, Ordering::Relaxed);
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    closed.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }

        // 2. fixed 20 Hz step pump.
        let now = Instant::now();
        while now.duration_since(last_step) >= STEP {
            last_step += STEP;
            if !step(&mut host, &from_sim_tx, &last_inbound_millis, started_at) {
                closed.store(true, Ordering::Relaxed);
                return;
            }
        }

        std::thread::sleep(WAKE);
    }
}

/// Mark the "sidecar" as alive: the embedded worker sets this on every outbound
/// batch so `is_alive()`/`silence_duration()` behave exactly like `WsTransport`.
fn mark_alive(last_inbound_millis: &Arc<AtomicU64>, started_at: Instant) {
    let now_ms = started_at.elapsed().as_millis() as u64;
    last_inbound_millis.store(now_ms, Ordering::Relaxed);
}

/// Handle one inbound wire message. Returns false only if the outbound channel
/// closed (client dropped) and the worker should exit.
fn handle_inbound(host: &mut Host, msg: ToSim, tx: &Sender<FromSimMsg>) -> bool {
    match msg {
        ToSim::Hello(_) => true, // client hello; we already advertised ours.
        ToSim::Ping => tx.send(FromSimMsg::Json(FromSimJson::Pong)).is_ok(),
        ToSim::SetSpeed(p) => {
            host.speed = p.speed;
            true
        }
        ToSim::Init(p) => {
            let opts = NewGameOptions {
                size: p.size.map(size_from_wire),
                preset_key: p.preset_key.clone(),
                rules: p.rules.as_ref().map(rules_from_wire),
            };
            host.preset_key = p.preset_key.clone();
            host.size = p.size;
            let state = new_game(
                p.seed as u32,
                host::difficulty_from_wire(p.difficulty),
                opts,
            );
            host.bankrupt = false;
            host.won = false;
            host.fields_version += 1;
            host.accumulator = 0.0;
            host.ui_countdown = 0;
            let ok = tx
                .send(FromSimMsg::Json(FromSimJson::Ready(host::build_ready(
                    &state,
                ))))
                .is_ok()
                && tx
                    .send(FromSimMsg::Fields(Arc::new(host::build_fields(
                        &state,
                        host.fields_version,
                    ))))
                    .is_ok()
                && tx
                    .send(FromSimMsg::Json(FromSimJson::Ui(host::build_ui_state(
                        &state,
                        host.speed,
                        host.fields_version,
                        host.bankrupt,
                    ))))
                    .is_ok();
            host.state = Some(state);
            ok
        }
        ToSim::Command { seq, cmd } => {
            let Some(state) = host.state.as_mut() else {
                return true;
            };
            let result = match host::command_to_sim(&cmd) {
                Some(sim_cmd) => host::command_result_to_wire(&apply_command(state, &sim_cmd)),
                None => mf_protocol::CommandResult {
                    ok: false,
                    error: Some("unrecognized command".to_string()),
                    created_id: None,
                },
            };
            let ui = host::build_ui_state(state, host.speed, host.fields_version, host.bankrupt);
            tx.send(FromSimMsg::Json(FromSimJson::CommandResult {
                seq: Some(seq),
                result,
            }))
            .is_ok()
                && tx.send(FromSimMsg::Json(FromSimJson::Ui(ui))).is_ok()
        }
        ToSim::QueryTrackCost { seq, payload } => {
            let Some(state) = host.state.as_ref() else {
                return true;
            };
            let points: Vec<mf_sim::geometry::Vec2> = payload
                .points
                .iter()
                .map(|p| mf_sim::geometry::Vec2 { x: p.x, y: p.y })
                .collect();
            let cost = mf_sim::transit::build::track_cost(
                state,
                host::mode_from_wire(payload.mode),
                grade_from_wire_local(payload.grade),
                &points,
            );
            // breakdown (v0.8 per-component split) is not surfaced yet: TODO(P5).
            tx.send(FromSimMsg::Json(FromSimJson::TrackCost {
                seq: Some(seq),
                cost,
                breakdown: None,
            }))
            .is_ok()
        }
        ToSim::StrataProbe { seq, payload } => {
            let Some(state) = host.state.as_ref() else {
                return true;
            };
            let col = mf_sim::geology::column_at(
                state.city_key.as_deref(),
                state.seed,
                mf_sim::constants::WORLD_SIZE,
                state.osm_elevation.as_deref(),
                state.osm_elev_res,
                mf_sim::geometry::Vec2 {
                    x: payload.x,
                    y: payload.y,
                },
            );
            let bands = col
                .bands
                .iter()
                .map(|b| mf_protocol::StrataBandDto {
                    kind: b.kind.as_str().to_string(),
                    top: b.top,
                    bottom: b.bottom,
                })
                .collect();
            tx.send(FromSimMsg::Json(FromSimJson::StrataProbe {
                seq: Some(seq),
                result: mf_protocol::StrataProbeResultPayload {
                    bands,
                    water_table: col.water_table_depth,
                    rock_hardness: col.rock_hardness,
                    surface_elevation: col.surface_elevation,
                },
            }))
            .is_ok()
        }
        ToSim::RequestReplay => {
            let Some(state) = host.state.as_ref() else {
                return true;
            };
            let payload = mf_protocol::ReplayPayload {
                seed: state.seed as u64,
                difficulty: difficulty_to_wire(state.difficulty),
                preset_key: host.preset_key.clone(),
                size: host.size,
                rules: None,
                command_log: Vec::new(), // TODO(P5): mirror command_log (needs SimCommand->wire).
                final_tick: state.tick,
                state_hash: state.state_hash() as i64,
                score_hint: state.stats.daily_transit_trips.round(),
            };
            tx.send(FromSimMsg::Json(FromSimJson::Replay(payload)))
                .is_ok()
        }
        // Save/load need mf-sim's serde feature (not enabled here). Flagged for
        // P5: surface a toast rather than silently doing nothing.
        ToSim::RequestSave | ToSim::LoadSave(_) => tx
            .send(FromSimMsg::Json(FromSimJson::Toast(
                mf_protocol::ToastPayload {
                    message: "Saves are not available with the in-process sim yet.".to_string(),
                    tone: mf_protocol::ToastTone::Warn,
                },
            )))
            .is_ok(),
        ToSim::Shutdown => false, // handled by caller before reaching here.
    }
}

fn grade_from_wire_local(g: mf_protocol::TrackGrade) -> mf_sim::types::TrackGrade {
    match g {
        mf_protocol::TrackGrade::Surface => mf_sim::types::TrackGrade::Surface,
        mf_protocol::TrackGrade::Elevated => mf_sim::types::TrackGrade::Elevated,
        mf_protocol::TrackGrade::Tunnel => mf_sim::types::TrackGrade::Tunnel,
    }
}

fn difficulty_to_wire(d: mf_sim::types::Difficulty) -> mf_protocol::Difficulty {
    match d {
        mf_sim::types::Difficulty::Easy => mf_protocol::Difficulty::Easy,
        mf_sim::types::Difficulty::Normal => mf_protocol::Difficulty::Normal,
        mf_sim::types::Difficulty::Hard => mf_protocol::Difficulty::Hard,
    }
}

/// One 20 Hz host step: advance the tick timer, run whole ticks, emit a frame
/// every step and a UI at 2 Hz. Port of the `setInterval` body in
/// `sim.worker.ts`. Returns false only if the outbound channel closed.
fn step(
    host: &mut Host,
    tx: &Sender<FromSimMsg>,
    last_inbound_millis: &Arc<AtomicU64>,
    started_at: Instant,
) -> bool {
    let Some(state) = host.state.as_mut() else {
        return true;
    };
    if host.bankrupt || state.failed.is_some() || host.won || state.scenario_won == Some(true) {
        return true;
    }

    host.accumulator += host.speed / 20.0;
    let mut ticks_run = 0u32;
    let mut fields_dirty = false;
    while host.accumulator >= 1.0 && ticks_run < MAX_TICKS_PER_STEP {
        let events = sim_tick(state);
        host.accumulator -= 1.0;
        ticks_run += 1;

        for m in &events.messages {
            if !send_toast(tx, m, mf_protocol::ToastTone::Info) {
                return false;
            }
        }
        for t in &events.toasts {
            let tone = match t.tone {
                mf_sim::sim::ToastTone::Good => mf_protocol::ToastTone::Good,
                mf_sim::sim::ToastTone::Warn => mf_protocol::ToastTone::Warn,
                mf_sim::sim::ToastTone::Info => mf_protocol::ToastTone::Info,
            };
            if !send_toast(tx, &t.message, tone) {
                return false;
            }
        }
        if let Some(label) = &events.mode_unlocked {
            if !send_toast(
                tx,
                &format!("{label} unlocked!"),
                mf_protocol::ToastTone::Good,
            ) {
                return false;
            }
        }
        if events.won {
            host.won = true;
        }
        if events.bankrupt || events.failed.is_some() {
            host.bankrupt = events.bankrupt;
        }
        if let Some(day) = events.day_completed {
            if day % 7 == 0 {
                fields_dirty = true;
            }
        }
    }

    if fields_dirty {
        host.fields_version += 1;
        if tx
            .send(FromSimMsg::Fields(Arc::new(host::build_fields(
                state,
                host.fields_version,
            ))))
            .is_err()
        {
            return false;
        }
    }

    // frame every step.
    if tx
        .send(FromSimMsg::Frame(Arc::new(host::build_frame(state))))
        .is_err()
    {
        return false;
    }

    // UI at 2 Hz.
    if host.ui_countdown == 0 {
        host.ui_countdown = UI_EVERY_STEPS;
        if tx
            .send(FromSimMsg::Json(FromSimJson::Ui(host::build_ui_state(
                state,
                host.speed,
                host.fields_version,
                host.bankrupt,
            ))))
            .is_err()
        {
            return false;
        }
    }
    host.ui_countdown -= 1;

    mark_alive(last_inbound_millis, started_at);
    true
}

fn send_toast(tx: &Sender<FromSimMsg>, message: &str, tone: mf_protocol::ToastTone) -> bool {
    tx.send(FromSimMsg::Json(FromSimJson::Toast(
        mf_protocol::ToastPayload {
            message: message.to_string(),
            tone,
        },
    )))
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_protocol::{InitPayload, SetSpeedPayload};

    /// Boot the transport, init a game, run for a bit, and assert it emits a
    /// hello, a UiState, and a FrameSnapshot without panicking.
    #[test]
    fn embedded_boots_and_emits_ui_and_frame() {
        let t = EmbeddedTransport::connect();
        // hello should arrive quickly.
        t.send(ToSim::Init(InitPayload {
            seed: 12345,
            difficulty: mf_protocol::Difficulty::Normal,
            size: None,
            preset_key: None,
            rules: None,
            scenario_id: None,
        }))
        .unwrap();
        t.send(ToSim::SetSpeed(SetSpeedPayload { speed: 120.0 }))
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut saw_hello = false;
        let mut saw_ui = false;
        let mut saw_frame = false;
        while Instant::now() < deadline && !(saw_hello && saw_ui && saw_frame) {
            while let Some(msg) = t.try_recv() {
                match msg {
                    FromSimMsg::Json(FromSimJson::Hello(_)) => saw_hello = true,
                    FromSimMsg::Json(FromSimJson::Ui(ui)) => {
                        // The very first UI (emitted on `init`) predates the
                        // `setSpeed`; only count a UI that reflects the new speed
                        // so we also verify SetSpeed propagation.
                        if ui.speed == 120.0 {
                            saw_ui = true;
                        }
                    }
                    FromSimMsg::Frame(_) => saw_frame = true,
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(saw_hello, "hello emitted");
        assert!(saw_ui, "UiState emitted");
        assert!(saw_frame, "FrameSnapshot emitted");
        assert!(t.is_alive(), "transport reports alive after activity");
    }
}
