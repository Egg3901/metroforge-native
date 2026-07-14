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
use mf_sim::geometry::{dist, Vec2};
use mf_sim::rng::Rng;
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
/// Host-side visual passenger pool cap.
const MAX_AGENTS: usize = 1600;
/// Agent visual ride speed in m/s.
const AGENT_RIDE_SPEED: f64 = 16.0;
/// Max road A* builds each resample.
const AGENT_PATH_BUDGET: i32 = 160;

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
    /// Host-side visual passenger particles (presentation only).
    agents: AgentPool,
}

#[derive(Clone)]
struct FlowPath {
    pts: Vec<Vec2>,
    cum: Vec<f64>,
    seg_phase: Vec<f32>,
    total: f64,
}

struct Agent {
    path_idx: usize,
    d: f64,
}

struct AgentPool {
    paths: Vec<FlowPath>,
    agents: Vec<Agent>,
    rng: Rng,
    buffer: Vec<f32>,
    count: usize,
}

impl AgentPool {
    fn new() -> Self {
        Self {
            paths: Vec::new(),
            agents: Vec::new(),
            rng: Rng::from_seed(0x0a9e17),
            buffer: vec![0.0; MAX_AGENTS * 3],
            count: 0,
        }
    }

    fn clear(&mut self) {
        self.paths.clear();
        self.agents.clear();
        self.count = 0;
    }

    fn resample(&mut self, state: &GameState) {
        self.clear();
        if state.flows.is_empty() {
            return;
        }
        let total_trips: f64 = state.flows.iter().map(|f| f.transit_trips).sum();
        if total_trips <= 0.0 {
            return;
        }
        let station_by_id: std::collections::BTreeMap<u32, Vec2> =
            state.stations.iter().map(|s| (s.id, s.pos)).collect();
        let district_by_id: std::collections::BTreeMap<u32, Vec2> =
            state.districts.iter().map(|d| (d.id, d.centroid)).collect();
        let mut track_by_pair: std::collections::BTreeMap<(u32, u32), Vec<Vec2>> =
            std::collections::BTreeMap::new();
        for t in &state.tracks {
            let forward = t.polyline.points.clone();
            let mut reverse = forward.clone();
            reverse.reverse();
            track_by_pair.insert((t.from_station_id, t.to_station_id), forward);
            track_by_pair.insert((t.to_station_id, t.from_station_id), reverse);
        }

        let weights: Vec<f64> = state.flows.iter().map(|f| f.transit_trips).collect();
        let tod_scale = mf_sim::transit::cohorts::cohort_demand_factor(state.tick).clamp(0.15, 1.8);
        let target = ((total_trips / 35.0) * tod_scale).round() as usize;
        let target = target.min(MAX_AGENTS);
        let mut budget = AGENT_PATH_BUDGET;
        let mut path_idx_by_flow: Vec<i64> = vec![-2; state.flows.len()];

        for _ in 0..target {
            let fi = self.rng.weighted(&weights);
            if fi >= state.flows.len() {
                continue;
            }
            let path_idx = if path_idx_by_flow[fi] >= 0 {
                path_idx_by_flow[fi] as usize
            } else if path_idx_by_flow[fi] == -1 {
                continue;
            } else if let Some(path) = build_flow_path(
                state,
                &state.flows[fi],
                &station_by_id,
                &district_by_id,
                &track_by_pair,
                &mut budget,
            ) {
                self.paths.push(path);
                let idx = self.paths.len() - 1;
                path_idx_by_flow[fi] = idx as i64;
                idx
            } else {
                path_idx_by_flow[fi] = -1;
                continue;
            };
            let Some(path) = self.paths.get(path_idx) else {
                continue;
            };
            if path.total <= 0.0 {
                continue;
            }
            self.agents.push(Agent {
                path_idx,
                d: self.rng.next_f64() * path.total,
            });
        }
    }

    fn update(&mut self, dt_game_seconds: f64) {
        let mut idx = 0usize;
        for a in &mut self.agents {
            let Some(path) = self.paths.get(a.path_idx) else {
                continue;
            };
            if path.pts.len() < 2 || path.cum.len() < 2 || path.total <= 0.0 {
                continue;
            }
            let mut seg = 1usize;
            while seg < path.cum.len() - 1 && path.cum[seg] < a.d {
                seg += 1;
            }
            let phase = path.seg_phase.get(seg - 1).copied().unwrap_or(0.0);
            let speed = if (phase - 1.0).abs() <= f32::EPSILON {
                AGENT_RIDE_SPEED
            } else {
                mf_sim::constants::WALK_SPEED * 2.4
            };
            a.d += speed * dt_game_seconds;
            while a.d >= path.total {
                a.d -= path.total;
                seg = 1;
            }
            while seg < path.cum.len() - 1 && path.cum[seg] < a.d {
                seg += 1;
            }
            let d0 = path.cum[seg - 1];
            let seg_len = (path.cum[seg] - d0).max(1e-6);
            let t = ((a.d - d0) / seg_len).clamp(0.0, 1.0);
            let p0 = path.pts[seg - 1];
            let p1 = path.pts[seg];
            if idx >= MAX_AGENTS {
                break;
            }
            self.buffer[idx * 3] = (p0.x + (p1.x - p0.x) * t) as f32;
            self.buffer[idx * 3 + 1] = (p0.y + (p1.y - p0.y) * t) as f32;
            self.buffer[idx * 3 + 2] = phase;
            idx += 1;
        }
        self.count = idx;
    }

    fn snapshot(&self) -> (&[f32], u32) {
        (&self.buffer[..self.count * 3], self.count as u32)
    }
}

fn build_flow_path(
    state: &GameState,
    flow: &mf_sim::types::FlowResult,
    station_by_id: &std::collections::BTreeMap<u32, Vec2>,
    district_by_id: &std::collections::BTreeMap<u32, Vec2>,
    track_by_pair: &std::collections::BTreeMap<(u32, u32), Vec<Vec2>>,
    budget: &mut i32,
) -> Option<FlowPath> {
    let origin = district_by_id.get(&flow.origin_district).copied()?;
    let dest = district_by_id.get(&flow.dest_district).copied()?;
    let stops: Vec<Vec2> = flow
        .station_ids
        .iter()
        .filter_map(|id| station_by_id.get(id).copied())
        .collect();
    if stops.is_empty() {
        return None;
    }
    let mut legs: Vec<(Vec<Vec2>, f32)> = Vec::new();
    let walk = |a: Vec2, b: Vec2, budget: &mut i32| -> Vec<Vec2> {
        if *budget > 0 {
            *budget -= 1;
            mf_sim::transit::road_graph::find_road_path(&state.roads, a, b)
                .unwrap_or_else(|| vec![a, b])
        } else {
            vec![a, b]
        }
    };
    legs.push((walk(origin, stops[0], budget), 0.0));
    for pair in flow.station_ids.windows(2) {
        let sid_a = pair[0];
        let sid_b = pair[1];
        let poly = track_by_pair.get(&(sid_a, sid_b)).cloned().or_else(|| {
            let a = station_by_id.get(&sid_a).copied()?;
            let b = station_by_id.get(&sid_b).copied()?;
            Some(vec![a, b])
        })?;
        legs.push((poly, 1.0));
    }
    legs.push((walk(*stops.last()?, dest, budget), 0.0));

    let mut pts: Vec<Vec2> = Vec::new();
    let mut seg_phase: Vec<f32> = Vec::new();
    for (poly, phase) in legs {
        for pt in poly {
            if pts.is_empty() {
                pts.push(pt);
                continue;
            }
            if dist(*pts.last().unwrap(), pt) < 1.0 {
                continue;
            }
            pts.push(pt);
            seg_phase.push(phase);
        }
    }
    if pts.len() < 2 {
        return None;
    }
    let mut cum = vec![0.0];
    let mut total = 0.0;
    for i in 1..pts.len() {
        total += dist(pts[i - 1], pts[i]);
        cum.push(total);
    }
    Some(FlowPath {
        pts,
        cum,
        seg_phase,
        total,
    })
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
            agents: AgentPool::new(),
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

fn rules_to_wire(r: &mf_sim::types::ScenarioRules) -> mf_protocol::ScenarioRules {
    mf_protocol::ScenarioRules {
        scenario_id: r.scenario_id.clone(),
        starting_modes: r
            .starting_modes
            .iter()
            .map(|&m| host::mode_to_wire(m))
            .collect(),
        lock_modes: r.lock_modes,
        max_day: r.max_day,
        approval_floor: r.approval_floor,
        starting_cash: r.starting_cash,
        daily_subsidy: r.daily_subsidy,
        era_label: r.era_label.clone(),
    }
}

fn scenario_from_init(
    p: &mf_protocol::envelope::InitPayload,
) -> Option<&'static mf_sim::scenario::evaluate::ScenarioDef> {
    p.scenario_id
        .as_deref()
        .or_else(|| p.rules.as_ref().and_then(|r| r.scenario_id.as_deref()))
        .and_then(mf_sim::scenario::catalog::playable_scenario)
}

fn apply_osm_bundle(state: &mut GameState, osm: mf_sim::city::osm::OsmCityData) {
    let mut labels = osm.labels;
    let mut anchors = osm.poi_anchors;
    let n = (osm.mask_res as usize) * (osm.mask_res as usize);
    state.osm_water_mask = Some(mf_sim::city::osm::decode_b64_mask(
        &osm.water_mask,
        n,
        osm.mask_packed,
    ));
    state.osm_park_mask = osm
        .park_mask
        .as_deref()
        .map(|m| mf_sim::city::osm::decode_b64_mask(m, n, osm.mask_packed));
    state.osm_building_mask = osm
        .building_mask
        .as_deref()
        .map(|m| mf_sim::city::osm::decode_b64_mask(m, n, osm.mask_packed));
    state.osm_mask_res = Some(osm.mask_res);
    state.osm_elevation = match (osm.elevation.as_deref(), osm.elev_res) {
        (Some(e), Some(res)) => Some(mf_sim::city::osm::decode_elevation(e, res)),
        _ => None,
    };
    state.osm_elev_res = osm.elev_res;
    state.osm_labels = if labels.is_empty() {
        None
    } else {
        Some(std::mem::take(&mut labels))
    };
    state.poi_anchors = if anchors.is_empty() {
        None
    } else {
        Some(std::mem::take(&mut anchors))
    };
}

fn extract_save_blob(json: &str) -> Result<(String, Option<String>), String> {
    let parsed: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("invalid save JSON: {e}"))?;
    let Some(obj) = parsed.as_object() else {
        return Ok((json.to_string(), None));
    };
    let is_wrapped = obj
        .get("mfSaveV")
        .and_then(|v| v.as_u64())
        .is_some_and(|v| v == 2)
        && obj.get("sim").is_some();
    if !is_wrapped {
        return Ok((json.to_string(), None));
    }
    let sim = obj
        .get("sim")
        .ok_or_else(|| "save wrapper missing sim payload".to_string())?;
    let sim_json = serde_json::to_string(sim)
        .map_err(|e| format!("failed to encode wrapped sim JSON: {e}"))?;
    let preset_key = obj
        .get("presetKey")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok((sim_json, preset_key))
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
            let scenario = scenario_from_init(&p);
            let preset_key = p
                .preset_key
                .clone()
                .or_else(|| scenario.map(|s| s.city_key.clone()));
            let opts = NewGameOptions {
                size: p.size.map(size_from_wire),
                preset_key: preset_key.clone(),
                rules: p
                    .rules
                    .as_ref()
                    .map(rules_from_wire)
                    .or_else(|| scenario.map(mf_sim::scenario::evaluate::rules_from_scenario)),
                // real-city bundle (None for procedural keys -> procgen)
                osm: crate::cities::resolve_city(preset_key.as_deref()),
                scenario: scenario.map(|d| mf_sim::types::ScenarioDef { id: d.id.clone() }),
            };
            host.preset_key = preset_key;
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
            host.agents.clear();
            let mut ok = tx
                .send(FromSimMsg::Json(FromSimJson::Ready(host::build_ready(
                    &state,
                ))))
                .is_ok();
            // real-city static binary frames (masks/elevation), right after
            // `ready`, mirroring the sidecar's `sendStatic` ordering.
            for mask in host::build_masks(&state) {
                ok = ok && tx.send(FromSimMsg::Mask(mask)).is_ok();
            }
            if let Some(elev) = host::build_elevation(&state) {
                ok = ok && tx.send(FromSimMsg::Elevation(Arc::new(elev))).is_ok();
            }
            if let Some(buildings) = host::build_static_buildings(host.preset_key.as_deref()) {
                ok = ok && tx.send(FromSimMsg::Buildings(buildings)).is_ok();
            }
            ok = ok
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
            let replayed = mf_sim::replay::replay_sync(mf_sim::replay::ReplayInput {
                seed: state.seed,
                difficulty: state.difficulty,
                size: host.size.map(size_from_wire),
                preset_key: host.preset_key.clone(),
                rules: state.scenario_rules.clone(),
                command_log: state.command_log.clone(),
                final_tick: Some(state.tick),
                osm: crate::cities::resolve_city(host.preset_key.as_deref()),
            });
            let live_hash = state.state_hash();
            if replayed.hash != live_hash || replayed.failed != state.failed {
                let _ = tx.send(FromSimMsg::Json(FromSimJson::Toast(
                    mf_protocol::ToastPayload {
                        message: "Replay validation mismatch detected for this run.".to_string(),
                        tone: mf_protocol::ToastTone::Warn,
                    },
                )));
            }
            let payload = mf_protocol::ReplayPayload {
                seed: state.seed as u64,
                difficulty: difficulty_to_wire(state.difficulty),
                preset_key: host.preset_key.clone(),
                size: host.size,
                rules: state.scenario_rules.as_ref().map(rules_to_wire),
                command_log: host::command_log_to_wire(&state.command_log),
                final_tick: state.tick,
                state_hash: live_hash as i64,
                score_hint: state.stats.daily_transit_trips.round(),
            };
            tx.send(FromSimMsg::Json(FromSimJson::Replay(payload)))
                .is_ok()
        }
        ToSim::RequestSave => {
            let Some(state) = host.state.as_ref() else {
                return true;
            };
            let sim_json = match mf_sim::save::serialize(state) {
                Ok(s) => s,
                Err(e) => {
                    return tx
                        .send(FromSimMsg::Json(FromSimJson::Toast(
                            mf_protocol::ToastPayload {
                                message: format!("Save failed: {e}"),
                                tone: mf_protocol::ToastTone::Warn,
                            },
                        )))
                        .is_ok();
                }
            };
            let sim_value: serde_json::Value = match serde_json::from_str(&sim_json) {
                Ok(v) => v,
                Err(e) => {
                    return tx
                        .send(FromSimMsg::Json(FromSimJson::Toast(
                            mf_protocol::ToastPayload {
                                message: format!("Save failed: {e}"),
                                tone: mf_protocol::ToastTone::Warn,
                            },
                        )))
                        .is_ok();
                }
            };
            let wrapped = serde_json::json!({
                "mfSaveV": 2,
                "presetKey": host.preset_key.clone(),
                "sim": sim_value,
            });
            tx.send(FromSimMsg::Json(FromSimJson::Saved(
                mf_protocol::envelope::SavedPayload {
                    json: wrapped.to_string(),
                },
            )))
            .is_ok()
        }
        ToSim::LoadSave(payload) => {
            let (sim_json, wrapped_preset_key) = match extract_save_blob(&payload.json) {
                Ok(v) => v,
                Err(e) => {
                    return tx
                        .send(FromSimMsg::Json(FromSimJson::Toast(
                            mf_protocol::ToastPayload {
                                message: format!("Load failed: {e}"),
                                tone: mf_protocol::ToastTone::Warn,
                            },
                        )))
                        .is_ok();
                }
            };
            let mut state = match mf_sim::save::deserialize(&sim_json) {
                Ok(v) => v,
                Err(e) => {
                    return tx
                        .send(FromSimMsg::Json(FromSimJson::Toast(
                            mf_protocol::ToastPayload {
                                message: format!("Load failed: {e}"),
                                tone: mf_protocol::ToastTone::Warn,
                            },
                        )))
                        .is_ok();
                }
            };
            let preset_key = wrapped_preset_key.or_else(|| state.city_key.clone());
            if preset_key.is_some() {
                state.city_key = preset_key.clone();
            }
            if let Some(osm) = crate::cities::resolve_city(preset_key.as_deref()) {
                apply_osm_bundle(&mut state, osm);
            }
            host.preset_key = preset_key;
            host.size = None;
            host.bankrupt = state.failed == Some(mf_sim::types::FailReason::Bankrupt);
            host.won = state.scenario_won == Some(true);
            host.fields_version += 1;
            host.accumulator = 0.0;
            host.ui_countdown = 0;
            host.agents.clear();
            let mut ok = tx
                .send(FromSimMsg::Json(FromSimJson::Ready(host::build_ready(
                    &state,
                ))))
                .is_ok();
            for mask in host::build_masks(&state) {
                ok = ok && tx.send(FromSimMsg::Mask(mask)).is_ok();
            }
            if let Some(elev) = host::build_elevation(&state) {
                ok = ok && tx.send(FromSimMsg::Elevation(Arc::new(elev))).is_ok();
            }
            if let Some(buildings) = host::build_static_buildings(host.preset_key.as_deref()) {
                ok = ok && tx.send(FromSimMsg::Buildings(buildings)).is_ok();
            }
            ok = ok
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
    let mut overlays_dirty = false;
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
        if events.assignment_refreshed {
            overlays_dirty = true;
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

    if overlays_dirty {
        host.agents.resample(state);
        if let Some(traffic) = host::build_traffic(state) {
            if tx.send(FromSimMsg::Traffic(traffic)).is_err() {
                return false;
            }
        }
        if let Some(demand) = host::build_demand(state) {
            if tx
                .send(FromSimMsg::Json(FromSimJson::Demand(demand)))
                .is_err()
            {
                return false;
            }
        }
    }

    host.agents.update(host.speed / 20.0);
    let (agent_buf, agent_count) = host.agents.snapshot();
    // frame every step.
    if tx
        .send(FromSimMsg::Frame(Arc::new(host::build_frame_with_agents(
            state,
            agent_buf,
            agent_count,
        ))))
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
    use mf_sim::types::Difficulty;
    use mf_sim::{apply_command, new_game, NewGameOptions, SimCommand};

    fn wait_for<F, T>(t: &EmbeddedTransport, timeout: Duration, mut f: F) -> Option<T>
    where
        F: FnMut(FromSimMsg) -> Option<T>,
    {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            while let Some(msg) = t.try_recv() {
                if let Some(v) = f(msg) {
                    return Some(v);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    fn scripted_bus_route(s: &mut GameState) {
        let pts: Vec<Vec2> = s
            .roads
            .iter()
            .flat_map(|r| r.polyline.points.iter().copied())
            .collect();
        let p0 = pts[0];
        let p1 = *pts
            .iter()
            .find(|p| {
                let d = dist(p0, **p);
                (400.0..3000.0).contains(&d)
            })
            .expect("second road point");
        let a = apply_command(
            s,
            &SimCommand::BuildStation {
                mode: mf_sim::types::TransitMode::Bus,
                pos: p0,
            },
        );
        let b = apply_command(
            s,
            &SimCommand::BuildStation {
                mode: mf_sim::types::TransitMode::Bus,
                pos: p1,
            },
        );
        assert!(a.ok && b.ok, "stations built");
        let sa = a.created_id.unwrap();
        let sb = b.created_id.unwrap();
        let t = apply_command(
            s,
            &SimCommand::BuildTrack {
                mode: mf_sim::types::TransitMode::Bus,
                grade: mf_sim::types::TrackGrade::Surface,
                from_station_id: sa,
                to_station_id: sb,
                waypoints: vec![],
            },
        );
        assert!(t.ok, "track built");
        let r = apply_command(
            s,
            &SimCommand::CreateRoute {
                mode: mf_sim::types::TransitMode::Bus,
                station_ids: vec![sa, sb],
            },
        );
        assert!(r.ok, "route built");
    }

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

    #[test]
    fn embedded_save_and_load_round_trip_preserves_replay_hash() {
        let t = EmbeddedTransport::connect();
        t.send(ToSim::Init(InitPayload {
            seed: 12345,
            difficulty: mf_protocol::Difficulty::Normal,
            size: None,
            preset_key: Some("nyc".to_string()),
            rules: None,
            scenario_id: None,
        }))
        .unwrap();
        t.send(ToSim::SetSpeed(SetSpeedPayload { speed: 120.0 }))
            .unwrap();

        // Let the sim tick for a bit before snapshotting.
        wait_for(&t, Duration::from_secs(6), |msg| match msg {
            FromSimMsg::Json(FromSimJson::Ui(ui)) if ui.tick > 30 => Some(()),
            _ => None,
        })
        .expect("ui advanced");
        t.send(ToSim::SetSpeed(SetSpeedPayload { speed: 0.0 }))
            .unwrap();
        wait_for(&t, Duration::from_secs(2), |msg| match msg {
            FromSimMsg::Json(FromSimJson::Ui(_)) => Some(()),
            _ => None,
        })
        .expect("ui after pause");

        t.send(ToSim::RequestReplay).unwrap();
        let hash_before = wait_for(&t, Duration::from_secs(3), |msg| match msg {
            FromSimMsg::Json(FromSimJson::Replay(p)) => Some(p.state_hash),
            _ => None,
        })
        .expect("replay before save");

        t.send(ToSim::RequestSave).unwrap();
        let save_json = wait_for(&t, Duration::from_secs(3), |msg| match msg {
            FromSimMsg::Json(FromSimJson::Saved(p)) => Some(p.json),
            _ => None,
        })
        .expect("saved payload");

        t.send(ToSim::LoadSave(mf_protocol::envelope::LoadSavePayload {
            json: save_json,
        }))
        .unwrap();
        wait_for(&t, Duration::from_secs(3), |msg| match msg {
            FromSimMsg::Json(FromSimJson::Ready(_)) => Some(()),
            _ => None,
        })
        .expect("ready after load");

        t.send(ToSim::RequestReplay).unwrap();
        let hash_after = wait_for(&t, Duration::from_secs(3), |msg| match msg {
            FromSimMsg::Json(FromSimJson::Replay(p)) => Some(p.state_hash),
            _ => None,
        })
        .expect("replay after load");

        assert_eq!(hash_before, hash_after, "save/load changed replay hash");
    }

    #[test]
    fn agent_pool_resamples_and_emits_particles_for_active_flows() {
        let mut s = new_game(12345, Difficulty::Normal, NewGameOptions::default());
        scripted_bus_route(&mut s);
        let origin = s
            .districts
            .first()
            .map(|d| d.id)
            .expect("districts available");
        let dest = s.districts.get(1).map(|d| d.id).unwrap_or(origin);
        let stations: Vec<u32> = s.stations.iter().map(|st| st.id).collect();
        s.flows = vec![mf_sim::types::FlowResult {
            origin_district: origin,
            dest_district: dest,
            transit_trips: 500.0,
            car_trips: 0.0,
            transit_cost: 15.0,
            route_ids: vec![],
            station_ids: stations,
        }];
        let mut pool = AgentPool::new();
        pool.resample(&s);
        pool.update(1.0);
        let (_, count) = pool.snapshot();
        assert!(count > 0, "expected visual agents for active transit flows");
    }
}
