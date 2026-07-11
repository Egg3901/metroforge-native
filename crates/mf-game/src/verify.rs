//! Dev/CI end-to-end verification harness — **not** part of the spec's v1
//! feature list; added while implementing/verifying `mf-render` so this box
//! (xvfb + lavapipe software Vulkan, no way to click through an egui
//! `MainMenu` headlessly) can drive the game all the way to `InGame` and
//! capture screenshots of what `mf-render` actually draws, without a human
//! at a display.
//!
//! Entirely inert unless `MF_VERIFY_DIR` is set (paired with
//! `MF_AUTOSTART=<presetKey>` in `state.rs` to skip the menu). When set, it
//! drives a fixed sequence once `InGame` is reached:
//!
//! 1. Run the sim at 120x so the day/night cycle reaches daylight quickly,
//!    and wait for both a daytime hour (so screenshots show the Mirror's
//!    Edge white-city look, not a night reading) and `mf_render`'s
//!    `BuildingsDenseCenter` (the city's densest built-up chunk, e.g.
//!    Manhattan for NYC — the origin alone is frequently open water).
//! 2. If `MF_VERIFY_NETWORK` is set, build a small multi-line network
//!    around the dense center over the wire and let it spin up for a bit
//!    (`NetworkBuild` / `NetworkSettle`, see below) before continuing; this
//!    is what puts route stripes, chevrons and moving vehicles into every
//!    screenshot from here on, plus one extra `transit.png`.
//! 3. Frame an elevated 3/4 view over that dense area -> `default.png`.
//! 4. Dolly down low over the same area (street level, buildings on both
//!    sides) -> `street.png`.
//! 5. Restore the elevated framing, toggle subway view -> `subway.png`
//!    (subway view is about the *world* changing, not the camera, so it
//!    reuses the `default` framing rather than the street one).
//! 6. Drop to potato quality, same elevated framing -> `potato.png` -> quit.
//!
//! Frame counts are generous since software rasterization on this box is
//! slow (seconds per frame is fine).
//!
//! **`MF_VERIFY_NETWORK`** (opt-in, additive to `MF_VERIFY_DIR`): the base
//! harness above never issues a single game command, so it has never
//! exercised route stripes, chevrons or vehicles (`mf-render`'s `transit.rs`
//! / `vehicles.rs`). When this env var is set, two stages are inserted
//! between the daytime gate and the elevated shot:
//!
//! - `NetworkBuild`: drives [`build_plan`] over `SimLink` one `ToSim::Command`
//!   at a time (see [`NetworkBuildState`]), correlating each `commandResult`
//!   back to the command that produced it by `seq`.
//! - `NetworkSettle`: runs the sim briefly so vehicles spawn and spread
//!   along the new routes, then freezes it again (`LatestFrame` retains the
//!   last snapshot it saw, so vehicles stay visible in every subsequent
//!   frozen screenshot exactly like the daytime lighting already does) and
//!   takes one closer `transit.png` before the normal sequence continues.

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use mf_net::{SimEvent, SimLink};
use mf_protocol::{
    Command, CommandResult, FromSimJson, FromSimMsg, SetSpeedPayload, ToSim, TrackGrade,
    TransitMode, Vec2 as WireVec2,
};
use mf_render::BuildingsDenseCenter;
use mf_state::{CurrentCity, LatestUi, QualityTier, SubwayView};

use crate::camera::CameraRig;
use crate::state::AppState;

/// Minimum frames to hold a given camera/world configuration before
/// screenshotting, so static layers (roads/buildings/transit rebuilds,
/// subway ease transition) have settled. Deliberately small: software
/// rendering is slow in wall-clock terms (each frame can be 100-300ms), and
/// at the 120x sim speed used to reach daylight quickly, even this many
/// frames' worth of real time is enough to cycle through several sim
/// hours — see the speed=0 freeze below, which is what actually keeps the
/// later screenshots' lighting consistent with the moment daylight was
/// detected.
const SETTLE_FRAMES: u64 = 20;
/// Hard cap on how long we'll wait for the "daytime + dense-center known"
/// gate before proceeding anyway, so a pathological sim state can't hang
/// this indefinitely in CI.
const MAX_WAIT_FRAMES: u64 = 900;

const TICKS_PER_DAY: u64 = 1200;
/// Daytime window (hours) we're willing to screenshot in — wide enough to
/// hit reliably within a couple of seconds at 120x, centered on noon.
// Narrowed from 9-15: at ~09:30 the low sun renders distant unshaded
// faces white-on-white (issue #16 readability item), making screenshots
// unreadable AND non-comparable between runs. Nearer midday is stable.
const DAY_HOUR_MIN: f64 = 10.5;
const DAY_HOUR_MAX: f64 = 14.5;

/// How many frames to run the sim at [`NETWORK_SPIN_UP_SPEED`] once the
/// network-demo build plan finishes, so vehicles actually spawn and spread
/// out along their new routes before everything freezes again for the rest
/// of the screenshot sequence.
const NETWORK_SPIN_UP_FRAMES: u64 = 40;
const NETWORK_SPIN_UP_SPEED: f64 = 120.0;
/// Frames to wait for a single in-flight `commandResult` before giving up on
/// it and moving on to the next plan step. Generous (this box's software
/// rasterizer, not the wire, is the slow part of a verify run), but finite:
/// a dropped reply must not hang the harness forever.
const NETWORK_COMMAND_TIMEOUT_FRAMES: u64 = 120;
/// Hard cap on the whole `NetworkBuild` stage, mirroring [`MAX_WAIT_FRAMES`]'s
/// role for the daytime gate: a pathologically stuck plan (e.g. every command
/// failing and retriggering the per-command timeout) still can't hang CI.
const NETWORK_BUILD_MAX_FRAMES: u64 = 6_000;

pub struct MfVerifyPlugin;

impl Plugin for MfVerifyPlugin {
    fn build(&self, app: &mut App) {
        // Dedicated HUD design-system gallery capture (MF_HUD_SCENE=1).
        // Independent of the full InGame verify sequence so a single Xvfb
        // run can screenshot every chrome component on one frame.
        if crate::design_system::hud_scene_enabled() {
            if let Some(dir) = std::env::var_os("MF_VERIFY_DIR") {
                app.insert_resource(HudSceneCapture {
                    dir: dir.to_string_lossy().into_owned(),
                    frame: 0,
                    done: false,
                })
                .add_systems(Update, hud_scene_screenshot_system);
            }
            return;
        }
        if std::env::var_os("MF_VERIFY_DIR").is_none() && std::env::var_os("MF_PROMO_DIR").is_none()
        {
            return; // inert in every normal build/run
        }
        app.init_resource::<VerifyState>()
            .add_systems(
                Update,
                verify_sequence_system.run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                menu_screenshot_system.run_if(in_state(AppState::MainMenu)),
            );
    }
}

#[derive(Resource)]
struct HudSceneCapture {
    dir: String,
    frame: u64,
    done: bool,
}

/// Wait for the design-system gallery to paint, then capture `hud.png`.
fn hud_scene_screenshot_system(
    mut capture: ResMut<HudSceneCapture>,
    mut commands: Commands,
    mut exit: EventWriter<AppExit>,
) {
    if capture.done {
        return;
    }
    capture.frame += 1;
    // Settle long enough for the 150ms panel fade to finish and egui to
    // lay out the gallery (software rasterizer is slow).
    if capture.frame == 45 {
        take_screenshot(&mut commands, format!("{}/hud.png", capture.dir));
    }
    if capture.frame == 75 {
        capture.done = true;
        exit.write(AppExit::Success);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Stage {
    #[default]
    WaitForDayAndCity,
    /// Only entered when `MF_VERIFY_NETWORK` is set; see [`NetworkBuildState`].
    NetworkBuild,
    /// Only entered when `MF_VERIFY_NETWORK` is set; runs the sim briefly so
    /// the network built in `NetworkBuild` spins up vehicles, then freezes
    /// again and takes `transit.png`.
    NetworkSettle,
    NetworkShot,
    Parked,
    Elevated,
    Street,
    Subway,
    Potato,
    Pause,
    Done,
}

#[derive(Resource, Default)]
struct VerifyState {
    frame: u64,
    stage: Stage,
    /// Frame count at the start of the current stage — every stage's
    /// "have we settled" check is relative to this, not the global frame
    /// counter.
    stage_start: u64,
    speed_sent: bool,
    /// Populated once, on entry to `Stage::NetworkBuild`, only when
    /// `MF_VERIFY_NETWORK` is set. `None` for the whole rest of a normal
    /// (network-demo-disabled) run.
    network: Option<NetworkBuildState>,
}

fn is_daytime(ui: &LatestUi) -> bool {
    let Some(state) = &ui.0 else { return false };
    let hour = (state.tick % TICKS_PER_DAY) as f64 / TICKS_PER_DAY as f64 * 24.0;
    (DAY_HOUR_MIN..DAY_HOUR_MAX).contains(&hour)
}

fn frame_elevated(rig: &mut CameraRig, center: Vec2) {
    rig.target = center;
    rig.distance = 1400.0;
    rig.pitch = 0.62; // a clear 3/4 angle over the skyline
    rig.yaw = 0.5;
}

/// Nearest arterial/collector polyline vertex to `center`: with real
/// footprints, the dense-center point itself is usually inside a tower, so
/// the street shot must anchor on an actual street.
fn nearest_road_point(city: &mf_state::CurrentCity, center: Vec2) -> Vec2 {
    let Some(cj) = &city.static_city else {
        return center;
    };
    let mut best = center;
    let mut best_d2 = f32::MAX;
    for road in &cj.roads {
        if road.cls != "arterial" && road.cls != "collector" {
            continue;
        }
        for c in road.points.chunks_exact(2) {
            let p = Vec2::new(c[0] as f32, c[1] as f32);
            let d2 = p.distance_squared(center);
            if d2 < best_d2 {
                best_d2 = d2;
                best = p;
            }
        }
    }
    best
}

fn frame_street(rig: &mut CameraRig, center: Vec2) {
    rig.target = center;
    rig.distance = 220.0;
    rig.pitch = 0.28; // low, looking mostly along the ground
    rig.yaw = 0.5;
}

/// Elevated framing for `transit.png`, closer in than [`frame_elevated`] so
/// the freshly built stripes/chevrons/vehicles read clearly in one shot.
fn frame_transit(rig: &mut CameraRig, center: Vec2) {
    rig.target = center;
    rig.distance = 650.0;
    rig.pitch = 0.78;
    rig.yaw = 0.5;
}

/// Half the world's side length on both axes (world spans
/// `[-half, half]` centered on the origin, matching `buildings.rs`'s chunk
/// indexing convention). Falls back to effectively unbounded when the city
/// hasn't loaded yet, which never happens in practice here since this stage
/// only runs after `WaitForDayAndCity` already required a known dense center.
fn world_half(city: &CurrentCity) -> f32 {
    city.static_city
        .as_ref()
        .map(|c| c.world_size as f32 / 2.0)
        .unwrap_or(f32::MAX)
}

// ---- MF_VERIFY_NETWORK build plan ------------------------------------------

/// One line of the network-demo build plan: an ordered station list plus the
/// transit mode and vehicle count the finished route gets. Positions are
/// world meters in the same ground-plane (x, y) convention as `UiStation` and
/// `BuildingsDenseCenter` (this module never touches height; `HeightAt`
/// resolves the ground y at render time).
#[derive(Debug, Clone, PartialEq)]
struct PlannedLine {
    mode: TransitMode,
    positions: Vec<Vec2>,
    vehicle_count: u32,
    /// `Some((source_line, from_idx, to_idx))`: build no stations or tracks,
    /// create the route over that slice of an earlier line's station ids.
    /// Several routes over the same station pairs is what triggers
    /// `transit.rs`'s parallel-offset bundling (the rainbow corridor).
    reuse: Option<(usize, usize, usize)>,
}

/// Station spacing shared by every line below.
const NETWORK_LINE_STEP: f32 = 600.0;
/// Half-span of the west-east and north-south bus lines (5 stations each,
/// `center +/- 1200` stepping by `NETWORK_LINE_STEP`).
const NETWORK_LINE_HALF_SPAN: f32 = 1200.0;
/// Half-span of the diagonal bus line and the tram line (4 stations each,
/// `+/- 900` stepping by `NETWORK_LINE_STEP`).
const NETWORK_SHORT_HALF_SPAN: f32 = 900.0;
/// How far the north-south line is nudged off `center.x` so its middle
/// station doesn't land exactly on top of the west-east line's middle
/// station. `buildStation` always creates a new station rather than
/// reusing one at a coincident position, so two stations stacked on the
/// same point is a degenerate case worth just avoiding here.
const NETWORK_CROSS_OFFSET: f32 = 250.0;
/// Perpendicular shift of the diagonal line so none of its stations land on
/// the west-east or north-south lines' stations (the sim rejects stations
/// too close to an existing one of the same mode).
const NETWORK_DIAGONAL_SHIFT: f32 = 300.0;

/// Bus/tram vehicle counts handed to `editRoute` once each line's route is
/// created, so vehicles actually deploy onto it.
const NETWORK_BUS_VEHICLES: u32 = 5;

/// Positions `start..=end` stepping by `step`, both ends included by
/// construction (every call site below picks `end - start` as an exact
/// multiple of `step`). The small epsilon guards against float
/// accumulation stopping one step short of `end`.
fn axis_positions(start: f32, end: f32, step: f32) -> Vec<f32> {
    let mut out = Vec::new();
    let mut v = start;
    while v <= end + 0.001 {
        out.push(v);
        v += step;
    }
    out
}

fn clamp_to_world(p: Vec2, half: f32) -> Vec2 {
    Vec2::new(p.x.clamp(-half, half), p.y.clamp(-half, half))
}

/// Builds the small, colorful, multi-line network the render-handoff demo
/// screenshots need: a west-east bus line and a north-south bus line
/// crossing near `center`, a diagonal bus line, and a tram line sharing
/// (offset) the west-east corridor. Every station lands within 1500m of
/// `center` before the `[-world_half, world_half]` clamp on both axes.
fn build_plan(center: Vec2, world_half: f32) -> Vec<PlannedLine> {
    let clamp = |p: Vec2| clamp_to_world(p, world_half);

    let line_west_east = PlannedLine {
        mode: TransitMode::Bus,
        positions: axis_positions(
            center.x - NETWORK_LINE_HALF_SPAN,
            center.x + NETWORK_LINE_HALF_SPAN,
            NETWORK_LINE_STEP,
        )
        .into_iter()
        .map(|x| clamp(Vec2::new(x, center.y)))
        .collect(),
        vehicle_count: NETWORK_BUS_VEHICLES,
        reuse: None,
    };

    let line_north_south = PlannedLine {
        mode: TransitMode::Bus,
        positions: axis_positions(
            center.y - NETWORK_LINE_HALF_SPAN,
            center.y + NETWORK_LINE_HALF_SPAN,
            NETWORK_LINE_STEP,
        )
        .into_iter()
        .map(|y| clamp(Vec2::new(center.x + NETWORK_CROSS_OFFSET, y)))
        .collect(),
        vehicle_count: NETWORK_BUS_VEHICLES,
        reuse: None,
    };

    let line_diagonal = PlannedLine {
        mode: TransitMode::Bus,
        positions: axis_positions(
            -NETWORK_SHORT_HALF_SPAN,
            NETWORK_SHORT_HALF_SPAN,
            NETWORK_LINE_STEP,
        )
        .into_iter()
        // X-only shift: on the 600m station grid a symmetric +-shift lands a
        // diagonal station exactly on a crossing line's station; +300 in X
        // alone keeps every diagonal station >=300m from both other lines.
        .map(|d| {
            clamp(Vec2::new(
                center.x + d + NETWORK_DIAGONAL_SHIFT,
                center.y + d,
            ))
        })
        .collect(),
        vehicle_count: NETWORK_BUS_VEHICLES,
        reuse: None,
    };

    // Two extra routes over the west-east line's existing stations: three
    // routes sharing those pairs bundle side by side (transit.rs pair_users
    // offsetting), which is the rainbow-corridor read the art direction
    // promises. Trams are progression-locked at game start, so every demo
    // line is a bus.
    let route_express = PlannedLine {
        mode: TransitMode::Bus,
        positions: Vec::new(),
        vehicle_count: NETWORK_BUS_VEHICLES,
        reuse: Some((0, 0, 4)),
    };
    let route_short_turn = PlannedLine {
        mode: TransitMode::Bus,
        positions: Vec::new(),
        vehicle_count: NETWORK_BUS_VEHICLES,
        reuse: Some((0, 1, 3)),
    };

    vec![
        line_west_east,
        line_north_south,
        line_diagonal,
        route_express,
        route_short_turn,
    ]
}

/// Which wire command a [`NetworkBuildState`] is currently waiting on a
/// `commandResult` for, and where its outcome (a created id, or nothing)
/// goes once that result arrives.
#[derive(Debug, Clone, Copy)]
enum PendingKind {
    Station { line: usize, station: usize },
    Track { line: usize, pair: usize },
    Route { line: usize },
    EditRoute { line: usize },
}

/// Where a line's build sequence currently is: stations first, then tracks
/// between each consecutive pair, then the route itself, then its vehicle
/// count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinePhase {
    Station(usize),
    Track(usize),
    Route,
    EditRoute,
    Done,
}

/// Drives the `MF_VERIFY_NETWORK` build plan one command at a time: at most
/// one `ToSim::Command` in flight, matched back to its
/// `FromSimJson::CommandResult` by `seq` (the sidecar echoes the client's
/// `requestId`, see `mf-protocol`'s `envelope.rs`; `mf-net`'s smoke test in
/// `metroforge/sidecar/smoke-test.ts` correlates commands the same way). A
/// failed command (`ok: false`) is logged and skipped rather than retried,
/// and a missing reply eventually times out (see
/// `NETWORK_COMMAND_TIMEOUT_FRAMES` in the caller), so one bad line can
/// never hang the harness.
struct NetworkBuildState {
    lines: Vec<PlannedLine>,
    line_idx: usize,
    phase: LinePhase,
    /// Per line, per station index: the id `buildStation` returned (`None`
    /// if that station failed).
    station_ids: Vec<Vec<Option<i64>>>,
    /// Per line: the id `createRoute` returned (`None` if route creation was
    /// skipped or failed).
    route_ids: Vec<Option<i64>>,
    next_seq: u32,
    /// The command currently in flight, if any.
    pending: Option<(u32, PendingKind)>,
    /// The `frame` (`VerifyState.frame`) the current `pending` command was
    /// sent at, so the caller can time it out.
    pending_since: Option<u64>,
}

impl NetworkBuildState {
    fn new(lines: Vec<PlannedLine>) -> Self {
        let station_ids = lines
            .iter()
            .map(|l| vec![None; l.positions.len()])
            .collect();
        let route_ids = vec![None; lines.len()];
        NetworkBuildState {
            lines,
            line_idx: 0,
            phase: LinePhase::Station(0),
            station_ids,
            route_ids,
            next_seq: 1,
            pending: None,
            pending_since: None,
        }
    }

    /// True once every line has been walked to `Done` and there is no
    /// in-flight command left.
    fn is_finished(&self) -> bool {
        self.pending.is_none() && self.line_idx >= self.lines.len()
    }

    /// If no command is currently in flight, advances the plan as far as it
    /// can for free (skipping steps that need no wire round-trip, e.g. a
    /// track whose station already failed to build) and returns the next
    /// command to send, remembering it as pending at `current_frame`.
    /// Returns `None` once the whole plan is exhausted (or a command is
    /// already in flight).
    fn next_command(&mut self, current_frame: u64) -> Option<(u32, Command)> {
        if self.pending.is_some() {
            return None;
        }
        loop {
            if self.line_idx >= self.lines.len() {
                return None;
            }
            let line_idx = self.line_idx;
            let line = &self.lines[line_idx];
            match self.phase {
                LinePhase::Station(i) => {
                    if line.reuse.is_some() {
                        self.phase = LinePhase::Route;
                        continue;
                    }
                    if i >= line.positions.len() {
                        self.phase = LinePhase::Track(0);
                        continue;
                    }
                    let pos = line.positions[i];
                    self.phase = LinePhase::Station(i + 1);
                    return Some(self.send(
                        current_frame,
                        PendingKind::Station {
                            line: line_idx,
                            station: i,
                        },
                        Command::BuildStation {
                            mode: line.mode,
                            pos: WireVec2 {
                                x: pos.x as f64,
                                y: pos.y as f64,
                            },
                        },
                    ));
                }
                LinePhase::Track(pair) => {
                    let pair_count = line.positions.len().saturating_sub(1);
                    if pair >= pair_count {
                        self.phase = LinePhase::Route;
                        continue;
                    }
                    let from_id = self.station_ids[line_idx][pair];
                    let to_id = self.station_ids[line_idx][pair + 1];
                    self.phase = LinePhase::Track(pair + 1);
                    let (Some(from_id), Some(to_id)) = (from_id, to_id) else {
                        tracing::warn!(
                            "verify: line {line_idx} skipping track {pair}-{}, a station failed to build",
                            pair + 1
                        );
                        continue; // no command sent, no seq consumed
                    };
                    return Some(self.send(
                        current_frame,
                        PendingKind::Track {
                            line: line_idx,
                            pair,
                        },
                        Command::BuildTrack {
                            mode: line.mode,
                            grade: TrackGrade::Surface,
                            from_station_id: from_id,
                            to_station_id: to_id,
                            waypoints: Vec::new(),
                        },
                    ));
                }
                LinePhase::Route => {
                    let ids: Vec<i64> = match line.reuse {
                        Some((src, from, to)) => self.station_ids[src]
                            [from..=to.min(self.station_ids[src].len() - 1)]
                            .iter()
                            .filter_map(|s| *s)
                            .collect(),
                        None => self.station_ids[line_idx]
                            .iter()
                            .filter_map(|s| *s)
                            .collect(),
                    };
                    self.phase = LinePhase::EditRoute;
                    if ids.len() < 2 {
                        tracing::warn!(
                            "verify: line {line_idx} skipping route creation, only {} of {} stations built",
                            ids.len(),
                            line.positions.len()
                        );
                        continue;
                    }
                    return Some(self.send(
                        current_frame,
                        PendingKind::Route { line: line_idx },
                        Command::CreateRoute {
                            mode: line.mode,
                            station_ids: ids,
                        },
                    ));
                }
                LinePhase::EditRoute => {
                    let route_id = self.route_ids[line_idx];
                    self.phase = LinePhase::Done;
                    let Some(route_id) = route_id else {
                        continue; // route creation failed or was skipped
                    };
                    return Some(self.send(
                        current_frame,
                        PendingKind::EditRoute { line: line_idx },
                        Command::EditRoute {
                            route_id,
                            headway_seconds: None,
                            fare: None,
                            vehicle_count: Some(line.vehicle_count),
                            name: None,
                            color: None,
                        },
                    ));
                }
                LinePhase::Done => {
                    self.line_idx += 1;
                    self.phase = LinePhase::Station(0);
                    continue;
                }
            }
        }
    }

    /// Assigns the next `seq`, records the command as pending, and returns
    /// it ready to hand to `SimLink.transport.send`.
    fn send(&mut self, current_frame: u64, kind: PendingKind, cmd: Command) -> (u32, Command) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.pending = Some((seq, kind));
        self.pending_since = Some(current_frame);
        (seq, cmd)
    }

    /// Applies an incoming `commandResult` if its `seq` matches the command
    /// currently in flight; a mismatched seq (e.g. a stale reply arriving
    /// after `drop_pending` already gave up on it) is ignored.
    fn handle_result(&mut self, seq: u32, result: &CommandResult) {
        let Some((pending_seq, kind)) = self.pending else {
            return;
        };
        if pending_seq != seq {
            return;
        }
        self.pending = None;
        self.pending_since = None;
        if !result.ok {
            let where_ = match kind {
                PendingKind::Station { line, station } => {
                    format!("line {line} station {station}")
                }
                PendingKind::Track { line, pair } => format!("line {line} track {pair}"),
                PendingKind::Route { line } => format!("line {line} route"),
                PendingKind::EditRoute { line } => format!("line {line} editRoute"),
            };
            tracing::warn!(
                "verify: network-demo command failed ({where_}): {:?}",
                result.error
            );
            return;
        }
        match kind {
            PendingKind::Station { line, station } => {
                self.station_ids[line][station] = result.created_id;
            }
            PendingKind::Route { line } => {
                self.route_ids[line] = result.created_id;
            }
            PendingKind::Track { .. } | PendingKind::EditRoute { .. } => {}
        }
    }

    /// Drops the in-flight command without applying any result, so
    /// `next_command` can move on after a timeout.
    fn drop_pending(&mut self) {
        if let Some((seq, _)) = self.pending.take() {
            self.pending_since = None;
            tracing::warn!("verify: network-demo command seq={seq} timed out waiting for a result");
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn verify_sequence_system(
    mut state: ResMut<VerifyState>,
    mut commands: Commands,
    mut rigs: Query<&mut CameraRig>,
    mut subway: ResMut<SubwayView>,
    mut quality: ResMut<QualityTier>,
    mut exit: EventWriter<AppExit>,
    link: Option<Res<SimLink>>,
    ui: Res<LatestUi>,
    dense_center: Res<BuildingsDenseCenter>,
    mut pause: ResMut<crate::state::PauseState>,
    city: Res<CurrentCity>,
    mut sim_events: EventReader<SimEvent>,
) {
    // Promo mode (MF_PROMO_DIR without MF_VERIFY_DIR): this machine only
    // drives the network build, then parks; promo.rs owns every camera and
    // screenshot from there.
    let promo_only = std::env::var_os("MF_VERIFY_DIR").is_none();
    let Some(dir) = std::env::var_os("MF_VERIFY_DIR")
        .or_else(|| std::env::var_os("MF_PROMO_DIR"))
        .map(|s| s.to_string_lossy().into_owned())
    else {
        return;
    };
    let network_enabled = std::env::var_os("MF_VERIFY_NETWORK").is_some();
    state.frame += 1;

    if !state.speed_sent {
        state.speed_sent = true;
        if let Some(link) = &link {
            let _ = link
                .transport
                .send(ToSim::SetSpeed(SetSpeedPayload { speed: 120.0 }));
        }
    }

    // Feed any `commandResult` replies to the in-flight network-build
    // command, if one is in flight. Reading this only while actually
    // building the network keeps every other stage (and every run with
    // `MF_VERIFY_NETWORK` unset) byte-identical to before this was added.
    if state.stage == Stage::NetworkBuild {
        for SimEvent(msg) in sim_events.read() {
            if let FromSimMsg::Json(FromSimJson::CommandResult {
                seq: Some(seq),
                result,
            }) = msg
            {
                if let Some(network) = state.network.as_mut() {
                    network.handle_result(*seq, result);
                }
            }
        }
    }

    let elapsed_in_stage = state.frame - state.stage_start;
    let mut advance_to = None;

    match state.stage {
        Stage::WaitForDayAndCity => {
            let ready = (is_daytime(&ui) && dense_center.0 != Vec2::ZERO)
                || elapsed_in_stage > MAX_WAIT_FRAMES;
            if ready {
                if let Ok(mut rig) = rigs.single_mut() {
                    frame_elevated(&mut rig, dense_center.0);
                }
                // Freeze the clock right here: at 120x, even the handful of
                // real seconds the remaining stages take would otherwise
                // cycle through several more sim hours, so every later
                // screenshot would land at an arbitrary (possibly nighttime)
                // moment again. Speed 0 holds `hour` steady for the rest of
                // the sequence.
                if let Some(link) = &link {
                    let _ = link
                        .transport
                        .send(ToSim::SetSpeed(SetSpeedPayload { speed: 0.0 }));
                }
                if network_enabled {
                    state.network = Some(NetworkBuildState::new(build_plan(
                        dense_center.0,
                        world_half(&city),
                    )));
                    advance_to = Some(Stage::NetworkBuild);
                } else {
                    advance_to = Some(Stage::Elevated);
                }
            }
        }
        Stage::NetworkBuild => {
            let mut plan_done = true;
            let current_frame = state.frame;
            if let Some(network) = state.network.as_mut() {
                if let Some(pending_since) = network.pending_since {
                    if current_frame - pending_since > NETWORK_COMMAND_TIMEOUT_FRAMES {
                        network.drop_pending();
                    }
                }
                if let Some((seq, cmd)) = network.next_command(current_frame) {
                    if let Some(link) = &link {
                        let _ = link.transport.send(ToSim::Command { seq, cmd });
                    }
                }
                plan_done = network.is_finished();
            }
            if plan_done || elapsed_in_stage > NETWORK_BUILD_MAX_FRAMES {
                if !plan_done {
                    tracing::warn!(
                        "verify: network-demo build plan hit its {NETWORK_BUILD_MAX_FRAMES}-frame cap, continuing anyway"
                    );
                }
                advance_to = Some(Stage::NetworkSettle);
            }
        }
        Stage::NetworkSettle => {
            if elapsed_in_stage == 1 {
                if let Some(link) = &link {
                    let _ = link.transport.send(ToSim::SetSpeed(SetSpeedPayload {
                        speed: NETWORK_SPIN_UP_SPEED,
                    }));
                }
            }
            // The build + spin-up ran the sim clock well past dusk on slow
            // software rasterizers, which shot the first demo at 20:45 and
            // dimmed the whole city to its night palette. Keep the sim
            // running until the clock is back in the daytime window, THEN
            // freeze and shoot: the demo's whole point is daytime color.
            if elapsed_in_stage > NETWORK_SPIN_UP_FRAMES && is_daytime(&ui) {
                // Re-freeze: `LatestFrame` retains the last snapshot it saw
                // (see `mf-state`'s `frame.rs`), so vehicles stay put at
                // their spun-up positions in every screenshot from here on,
                // the same way speed=0 already keeps the lighting steady.
                if let Some(link) = &link {
                    let _ = link
                        .transport
                        .send(ToSim::SetSpeed(SetSpeedPayload { speed: 0.0 }));
                }
                if let Ok(mut rig) = rigs.single_mut() {
                    frame_transit(&mut rig, dense_center.0);
                }
                advance_to = Some(Stage::NetworkShot);
            }
            if elapsed_in_stage > NETWORK_BUILD_MAX_FRAMES {
                advance_to = Some(Stage::NetworkShot); // shoot whatever we have
            }
        }
        Stage::NetworkShot => {
            if promo_only {
                advance_to = Some(Stage::Parked);
            } else if elapsed_in_stage == SETTLE_FRAMES {
                take_screenshot(&mut commands, format!("{dir}/transit.png"));
                advance_to = Some(Stage::Elevated);
            }
        }
        Stage::Parked => {
            // promo.rs is in charge now; never exit, never screenshot.
        }
        Stage::Elevated => {
            // Re-framed on entry (not just relied on from whatever stage
            // came before): `NetworkSettle` leaves the camera on the closer
            // `transit.png` framing, so this must reclaim the standard
            // elevated view before `default.png`. A no-op when the previous
            // stage was `WaitForDayAndCity` (already framed identically).
            if elapsed_in_stage == 1 {
                if let Ok(mut rig) = rigs.single_mut() {
                    frame_elevated(&mut rig, dense_center.0);
                }
            }
            if elapsed_in_stage == SETTLE_FRAMES {
                take_screenshot(&mut commands, format!("{dir}/default.png"));
                advance_to = Some(Stage::Street);
            }
        }
        Stage::Street => {
            if elapsed_in_stage == 5 {
                if let Ok(mut rig) = rigs.single_mut() {
                    frame_street(&mut rig, nearest_road_point(&city, dense_center.0));
                }
            }
            if elapsed_in_stage == SETTLE_FRAMES {
                take_screenshot(&mut commands, format!("{dir}/street.png"));
                advance_to = Some(Stage::Subway);
            }
        }
        Stage::Subway => {
            if elapsed_in_stage == 5 {
                if let Ok(mut rig) = rigs.single_mut() {
                    frame_elevated(&mut rig, dense_center.0);
                }
                subway.toggle();
            }
            if elapsed_in_stage == SETTLE_FRAMES {
                take_screenshot(&mut commands, format!("{dir}/subway.png"));
                advance_to = Some(Stage::Potato);
            }
        }
        Stage::Potato => {
            if elapsed_in_stage == 5 {
                *quality = QualityTier::Potato;
            }
            if elapsed_in_stage == SETTLE_FRAMES {
                take_screenshot(&mut commands, format!("{dir}/potato.png"));
                advance_to = Some(Stage::Pause);
            }
        }
        Stage::Pause => {
            // Direct flag write (not `toggle_pause`): the overlay render path
            // is what's under test, and the sim was already frozen at speed 0
            // back in WaitForDayAndCity, so no SetSpeed round-trip is needed.
            if elapsed_in_stage == 5 {
                pause.active = true;
            }
            if elapsed_in_stage == SETTLE_FRAMES {
                take_screenshot(&mut commands, format!("{dir}/pause.png"));
                advance_to = Some(Stage::Done);
            }
        }
        Stage::Done => {
            // A little extra headroom so the last screenshot's async
            // GPU->CPU readback finishes before the process exits.
            if elapsed_in_stage == 30 {
                exit.write(AppExit::Success);
            }
        }
    }

    if let Some(next) = advance_to {
        state.stage = next;
        state.stage_start = state.frame;
    }
}

/// Screenshot the main menu itself (`menu.png`). `state.rs`'s autostart holds
/// at MainMenu for ~30 frames when `MF_VERIFY_DIR` is set precisely so this
/// can run — the menu being invisible (no camera = no egui context) shipped
/// in v0.1.0-alpha because no verify path ever rendered it.
fn menu_screenshot_system(mut state: Local<u64>, mut commands: Commands) {
    // Verify-only: promo runs (MF_PROMO_DIR alone) take no menu shot.
    let Some(dir) = std::env::var_os("MF_VERIFY_DIR").map(|s| s.to_string_lossy().into_owned())
    else {
        return;
    };
    *state += 1;
    if *state == 15 {
        take_screenshot(&mut commands, format!("{dir}/menu.png"));
    }
}

fn take_screenshot(commands: &mut Commands, path: String) {
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path));
}

#[cfg(test)]
mod build_plan_tests {
    use super::*;

    const CENTER: Vec2 = Vec2::new(1000.0, -500.0);

    #[test]
    fn five_lines_with_expected_station_counts_and_reuse() {
        let plan = build_plan(CENTER, f32::MAX);
        assert_eq!(
            plan.len(),
            5,
            "expected 5 lines (west-east/north-south/diagonal/express/short-turn)"
        );
        assert_eq!(plan[0].positions.len(), 5, "west-east bus line");
        assert_eq!(plan[1].positions.len(), 5, "north-south bus line");
        assert_eq!(plan[2].positions.len(), 4, "diagonal bus line");
        for line in &plan {
            assert_eq!(line.mode, TransitMode::Bus);
            assert_eq!(line.vehicle_count, NETWORK_BUS_VEHICLES);
        }
        for built in &plan[0..3] {
            assert!(built.reuse.is_none());
        }
        assert_eq!(
            plan[3].reuse,
            Some((0, 0, 4)),
            "express reuses line 0 fully"
        );
        assert_eq!(
            plan[4].reuse,
            Some((0, 1, 3)),
            "short turn reuses line 0's middle"
        );
        for reused in &plan[3..5] {
            assert!(reused.positions.is_empty(), "reuse lines build nothing");
        }
    }

    #[test]
    fn every_station_lands_within_1500m_of_center_before_clamp() {
        let plan = build_plan(CENTER, f32::MAX);
        for line in &plan {
            for p in &line.positions {
                let d = p.distance(CENTER);
                assert!(
                    d <= 1500.0 + 0.01,
                    "station {p:?} is {d}m from center, want <=1500"
                );
            }
        }
    }

    #[test]
    fn stations_clamp_to_world_bounds() {
        // A tight world_half forces every generated coordinate to have been
        // pulled inside it, even though the unclamped offsets (up to 1200m)
        // would otherwise overshoot a world this small.
        let world_half = 100.0;
        let plan = build_plan(Vec2::ZERO, world_half);
        for line in &plan {
            for p in &line.positions {
                assert!(
                    p.x.abs() <= world_half + 0.01,
                    "x {} outside +/-{world_half}",
                    p.x
                );
                assert!(
                    p.y.abs() <= world_half + 0.01,
                    "y {} outside +/-{world_half}",
                    p.y
                );
            }
        }
    }

    #[test]
    fn consecutive_station_pairs_are_ordered_and_distinct() {
        let plan = build_plan(CENTER, f32::MAX);
        for line in &plan {
            // Stations within a line must be strictly increasing along
            // their axis of travel (west-east/north-south by x/y, diagonal
            // and tram by x), so `buildTrack` pairs form a real polyline
            // rather than a zero-length or backtracking segment.
            for pair in line.positions.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                assert!(
                    a.distance(b) > 0.0,
                    "adjacent stations {a:?}/{b:?} must not coincide"
                );
                assert!(
                    b.x > a.x || b.y > a.y,
                    "stations must advance along the line: {a:?} -> {b:?}"
                );
            }
        }
    }

    #[test]
    fn north_south_line_does_not_collide_with_west_east_line_at_center() {
        let plan = build_plan(Vec2::ZERO, f32::MAX);
        // Line 1 (west-east)'s middle station sits exactly at `center`;
        // line 2 (north-south) must be nudged off it, not stacked on top.
        let west_east_mid = plan[0].positions[2];
        let north_south_mid = plan[1].positions[2];
        assert_eq!(west_east_mid, Vec2::ZERO);
        assert_ne!(west_east_mid, north_south_mid);
    }

    #[test]
    fn diagonal_line_is_shifted_off_the_crossing_lines_stations() {
        let plan = build_plan(Vec2::ZERO, f32::MAX);
        // Every diagonal station must stay clear of both crossing lines'
        // station positions (the sim rejects same-mode stations that are
        // too close together, which killed the first demo run).
        for d in &plan[2].positions {
            for other in plan[0].positions.iter().chain(plan[1].positions.iter()) {
                assert!(
                    d.distance(*other) > 200.0,
                    "diagonal station {d:?} within 200m of {other:?}"
                );
            }
        }
    }

    #[test]
    fn axis_positions_is_inclusive_of_both_ends() {
        let xs = axis_positions(-1200.0, 1200.0, NETWORK_LINE_STEP);
        assert_eq!(xs, vec![-1200.0, -600.0, 0.0, 600.0, 1200.0]);
        let short = axis_positions(-900.0, 900.0, NETWORK_LINE_STEP);
        assert_eq!(short, vec![-900.0, -300.0, 300.0, 900.0]);
    }
}

#[cfg(test)]
mod network_build_state_tests {
    use super::*;

    fn one_line_plan() -> Vec<PlannedLine> {
        vec![PlannedLine {
            mode: TransitMode::Bus,
            positions: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(600.0, 0.0),
                Vec2::new(1200.0, 0.0),
            ],
            vehicle_count: 5,
            reuse: None,
        }]
    }

    #[test]
    fn happy_path_builds_stations_tracks_route_and_edits_it() {
        let mut net = NetworkBuildState::new(one_line_plan());

        // 3 buildStation commands.
        for want_station in 0..3 {
            let (seq, cmd) = net
                .next_command(1)
                .expect("expected a buildStation command");
            match cmd {
                Command::BuildStation { .. } => {}
                other => panic!("expected BuildStation, got {other:?}"),
            }
            assert!(
                net.next_command(1).is_none(),
                "only one command in flight at a time"
            );
            net.handle_result(
                seq,
                &CommandResult {
                    ok: true,
                    error: None,
                    created_id: Some(want_station as i64 + 100),
                },
            );
        }

        // 2 buildTrack commands (one per consecutive pair).
        for _ in 0..2 {
            let (seq, cmd) = net.next_command(1).expect("expected a buildTrack command");
            match cmd {
                Command::BuildTrack {
                    from_station_id,
                    to_station_id,
                    ..
                } => {
                    assert!(from_station_id >= 100 && to_station_id >= 100);
                }
                other => panic!("expected BuildTrack, got {other:?}"),
            }
            net.handle_result(
                seq,
                &CommandResult {
                    ok: true,
                    error: None,
                    created_id: None,
                },
            );
        }

        // 1 createRoute command.
        let (seq, cmd) = net.next_command(1).expect("expected a createRoute command");
        match cmd {
            Command::CreateRoute { station_ids, .. } => assert_eq!(station_ids.len(), 3),
            other => panic!("expected CreateRoute, got {other:?}"),
        }
        net.handle_result(
            seq,
            &CommandResult {
                ok: true,
                error: None,
                created_id: Some(500),
            },
        );

        // 1 editRoute command.
        let (seq, cmd) = net.next_command(1).expect("expected an editRoute command");
        match cmd {
            Command::EditRoute {
                route_id,
                vehicle_count,
                ..
            } => {
                assert_eq!(route_id, 500);
                assert_eq!(vehicle_count, Some(5));
            }
            other => panic!("expected EditRoute, got {other:?}"),
        }
        net.handle_result(
            seq,
            &CommandResult {
                ok: true,
                error: None,
                created_id: None,
            },
        );

        assert!(net.next_command(1).is_none());
        assert!(net.is_finished());
    }

    #[test]
    fn a_failed_station_skips_its_dependent_track_and_route_without_hanging() {
        let mut net = NetworkBuildState::new(one_line_plan());

        // Fail every buildStation.
        for _ in 0..3 {
            let (seq, _) = net.next_command(1).unwrap();
            net.handle_result(
                seq,
                &CommandResult {
                    ok: false,
                    error: Some("no room".to_string()),
                    created_id: None,
                },
            );
        }

        // With every station id unknown, both track pairs and route
        // creation must be skipped without ever sending a command for
        // them (there's nothing to build tracks/routes between), and the
        // plan must still terminate rather than hang.
        assert!(net.next_command(1).is_none());
        assert!(net.is_finished());
    }

    #[test]
    fn mismatched_seq_is_ignored_so_a_stale_reply_cannot_corrupt_state() {
        let mut net = NetworkBuildState::new(one_line_plan());
        let (real_seq, _) = net.next_command(1).unwrap();
        // A stale/foreign seq must not be treated as resolving the pending
        // command.
        net.handle_result(
            real_seq + 999,
            &CommandResult {
                ok: true,
                error: None,
                created_id: Some(42),
            },
        );
        assert!(
            net.pending.is_some(),
            "the real pending command is still in flight"
        );
        // The real reply still resolves it correctly afterward.
        net.handle_result(
            real_seq,
            &CommandResult {
                ok: true,
                error: None,
                created_id: Some(42),
            },
        );
        assert!(net.pending.is_none());
    }
}
