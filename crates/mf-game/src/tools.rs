//! In-world build tools (ship-plan #25, v0.2 §3.4 `tools.rs`): the tool
//! state machine, click-vs-drag handling, ghost/preview rendering, and the
//! pick logic (nearest station / nearest track polyline) that the three v0.2
//! tools (place-station, route, bulldoze) all share.
//!
//! Two independent request-id spaces flow out of this module over the wire:
//! player *commands* (`BuildStation`/`BuildTrack`/`CreateRoute`/`Demolish*`)
//! go through `command_bus::CommandBus`, which owns its own seq counter and
//! resolves replies into `CommandFeedback` events; `ToSim::QueryTrackCost`
//! is NOT a `Command` (see `mf-protocol`'s `envelope.rs`), so it can't go
//! through the bus and gets this module's own small seq counter instead.
//! The two spaces can collide numerically without any ambiguity, because
//! `commandResult` and `trackCost` are distinct wire message types that each
//! side matches independently (`command_feedback_system` only ever sees
//! `CommandFeedback` events off the bus; `track_cost_feedback_system` only
//! ever looks at `FromSimJson::TrackCost`).
//!
//! `MfToolsPlugin` is deliberately NOT registered in `main.rs` yet (v0.2
//! integration wires it in alongside the toolbar UI and the real
//! `command_bus`, both landing separately); until then every system this
//! module defines is unreachable from `main`, same situation
//! `camera.rs`'s `CLICK_DRAG_THRESHOLD_PX` was in before this module
//! existed to consume it. The blanket allow below mirrors that precedent
//! rather than sprinkling per-item annotations across an entire module
//! that is one `add_plugins` call away from every one of them being live.
#![allow(dead_code)]

use bevy::prelude::*;
use bevy_egui::EguiContexts;
use std::collections::HashSet;

use mf_net::{SimEvent, SimLink};
use mf_protocol::{
    Command, FromSimJson, FromSimMsg, QueryTrackCostPayload, ToSim, TrackGrade, TransitMode,
    UiState, UiStation, UiTrack, Vec2 as WireVec2,
};
use mf_state::{HeightAt, LatestUi};

use crate::audio::{PlaySfx, Sfx};
use crate::camera::{screen_to_ground, CLICK_DRAG_THRESHOLD_PX};
use crate::command_bus::{CmdMeta, CommandBus, CommandFeedback};

// ---------------------------------------------------------------------
// Tunable constants (comments explain the "why", not just the number).
// ---------------------------------------------------------------------

/// Route tool: how close (meters) a click needs to land to an existing
/// station to pick it. Generous enough that clicking "near" a station on a
/// zoomed-out camera still registers, per spec §5's "nearest station within
/// 80m" build tool brief.
const ROUTE_PICK_RADIUS_M: f32 = 80.0;
/// Bulldoze tool: station pick radius. Tighter than the route tool's 80m,
/// since demolishing is destructive: the click should land closer to the
/// actual marker before it's willing to guess "that one".
const BULLDOZE_STATION_RADIUS_M: f32 = 60.0;
/// Bulldoze tool: track polyline pick radius (perpendicular distance to the
/// nearest segment), tighter still since tracks are thin lines rather than
/// station-sized blobs.
const BULLDOZE_TRACK_RADIUS_M: f32 = 30.0;
/// Ghost circle radius for the place-station preview: "station-ish" per the
/// spec brief, matched to `mf-render`'s actual station marker scale.
const STATION_GHOST_RADIUS_M: f32 = 14.0;
/// Height of the place-station ghost's vertical locator line, tall enough
/// to read against the ground plane from the default camera pitch without
/// competing with nearby building silhouettes.
const STATION_GHOST_HEIGHT_M: f32 = 30.0;
/// Bulldoze ghost circle radius: drawn at the (larger of the two) station
/// pick radius so the circle honestly represents what a click there will
/// catch, rather than an arbitrary cosmetic size.
const BULLDOZE_GHOST_RADIUS_M: f32 = BULLDOZE_STATION_RADIUS_M;
/// Window within which a second clean click counts as a double-click
/// (Route tool confirm gesture), rather than two independent clicks.
const DOUBLE_CLICK_WINDOW_SECS: f32 = 0.35;
/// Select tool (v0.3, ship-plan #25 §4): nearest station pick radius for a
/// clean click while no build tool is active. Its own named constant
/// (rather than reusing [`BULLDOZE_STATION_RADIUS_M`], which happens to
/// share the same 60m value) since selecting and demolishing are
/// conceptually independent actions that could diverge later even though
/// they start out numerically identical.
const SELECT_STATION_RADIUS_M: f32 = 60.0;

/// Which build tool is active, if any. Read/written by the toolbar UI (a
/// separate v0.2 agent's HUD panel) as well as this module's own keybind
/// system, so both stay in lockstep with a single source of truth.
#[derive(Default, PartialEq, Clone, Copy, Debug)]
pub enum ActiveTool {
    #[default]
    None,
    PlaceStation(TransitMode),
    Route,
    Bulldoze,
}

/// What a clean world click picked while the Select tool (`ActiveTool::None`)
/// is active - `panels.rs`'s station inspection window reads this to decide
/// what (if anything) to show. v0.3 scope is stations only (see
/// `tool_click_system`'s `ActiveTool::None` arm): route-stripe picking is
/// fiddlier (nearest-segment math against a rendered stripe rather than a
/// single point) and left for a later wave, so a miss on the station check
/// always resolves to `None` rather than attempting a route fallback.
#[derive(Resource, Default, PartialEq, Clone, Copy, Debug)]
pub enum SelectedTarget {
    #[default]
    None,
    Station(i64),
}

/// Shared state for the active build tool. `active`/`route_draft`/
/// `route_mode`/`last_cost_quote` are `pub`: the toolbar UI reads all four
/// to render its panel (which tool is selected, the in-progress route, a
/// live cost estimate) and writes `active`/`route_mode` when the player
/// clicks a toolbar button. Everything else here is this module's own
/// click/feedback bookkeeping and deliberately private.
#[derive(Resource)]
pub struct ToolState {
    pub active: ActiveTool,
    /// Station ids picked so far for the in-progress route, oldest first.
    pub route_draft: Vec<i64>,
    /// Transit mode the in-progress (or next) route draft will use.
    pub route_mode: TransitMode,
    /// Latest `trackCost` quote for the draft's newest segment, if any
    /// reply has come back yet. Cleared whenever the draft changes shape
    /// (confirm or cancel) so a stale quote never lingers on screen.
    pub last_cost_quote: Option<f64>,
    /// Screen-space cursor position at the last left-button press; `None`
    /// once the button is released. Lets `tool_click_system` tell a clean
    /// click from a drag-pan without stealing `camera.rs`'s own read of the
    /// same button (Bevy `Res<ButtonInput<_>>` is freely shared).
    press_pos: Option<Vec2>,
    /// Seqs this module itself handed to `CommandBus::submit`, so
    /// `command_feedback_system` only chimes for ITS OWN commands: a
    /// toolbar rename/loan/edit action rides the same bus and must not
    /// trigger a tool confirm/error sound.
    pending_seqs: HashSet<u32>,
    /// Seq of the currently in-flight `ToSim::QueryTrackCost`, if any. A
    /// `trackCost` reply whose seq doesn't match this is a stale answer for
    /// an already-superseded segment and is ignored.
    pending_track_cost_seq: Option<u32>,
    /// This module's own request-id counter for `QueryTrackCost` (see the
    /// module doc for why it's separate from `CommandBus`'s).
    next_track_cost_seq: u32,
    /// Wall-clock time + screen position of the last recognized clean
    /// click, for double-click detection (Route tool confirm gesture).
    last_click: Option<(f32, Vec2)>,
}

impl Default for ToolState {
    fn default() -> Self {
        ToolState {
            active: ActiveTool::default(),
            route_draft: Vec::new(),
            // Bus preselected per the v0.2 keybind brief (`1` also maps to
            // PlaceStation(Bus)); TransitMode has no Default upstream in
            // mf-protocol (out of scope to add here), hence the manual impl.
            route_mode: TransitMode::Bus,
            last_cost_quote: None,
            press_pos: None,
            pending_seqs: HashSet::new(),
            pending_track_cost_seq: None,
            next_track_cost_seq: 1,
            last_click: None,
        }
    }
}

pub struct MfToolsPlugin;

impl Plugin for MfToolsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ToolState>()
            .init_resource::<SelectedTarget>()
            .add_systems(
                Update,
                (
                    keybind_system,
                    tool_click_system,
                    command_feedback_system,
                    track_cost_feedback_system,
                    tool_gizmo_system,
                )
                    .run_if(in_state(crate::state::AppState::InGame)),
            );
    }
}

// ---------------------------------------------------------------------
// Keybinds: 1/2/3 select a tool. Tram is deliberately absent (progression-
// locked; toolbar-only per the ship-plan) and Esc lives in `input.rs`
// alongside the pause toggle since it has to arbitrate priority against it.
// ---------------------------------------------------------------------

fn keybind_system(keys: Res<ButtonInput<KeyCode>>, mut tool: ResMut<ToolState>) {
    if keys.just_pressed(KeyCode::Digit1) {
        tool.active = ActiveTool::PlaceStation(TransitMode::Bus);
    } else if keys.just_pressed(KeyCode::Digit2) {
        tool.active = ActiveTool::Route;
    } else if keys.just_pressed(KeyCode::Digit3) {
        tool.active = ActiveTool::Bulldoze;
    }
}

// ---------------------------------------------------------------------
// Click vs. drag + per-tool dispatch.
// ---------------------------------------------------------------------

/// Left-press/release -> world click detection, then dispatches the click
/// (or an Enter keypress) to whichever tool is active. Deliberately reads
/// (never consumes) the same `ButtonInput<MouseButton>`/cursor position
/// `camera.rs`'s drag-pan reads, so panning keeps working untouched: this
/// system only ever ACTS on a release that moved less than
/// `CLICK_DRAG_THRESHOLD_PX`, which a real pan never satisfies.
#[allow(clippy::too_many_arguments)]
fn tool_click_system(
    time: Res<Time>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut egui_contexts: EguiContexts,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    height_at: Res<HeightAt>,
    mut tool: ResMut<ToolState>,
    ui_state: Res<LatestUi>,
    link: Option<Res<SimLink>>,
    mut bus: ResMut<CommandBus>,
    mut sfx: EventWriter<PlaySfx>,
    mut selected: ResMut<SelectedTarget>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let over_egui = egui_contexts
        .ctx_mut()
        .map(|ctx| ctx.wants_pointer_input())
        .unwrap_or(false);

    if mouse_buttons.just_pressed(MouseButton::Left) {
        tool.press_pos = window.cursor_position();
    }

    let mut clicked_ground: Option<Vec2> = None;
    let mut double_click = false;
    if mouse_buttons.just_released(MouseButton::Left) {
        let release_pos = window.cursor_position();
        if let (Some(press), Some(release)) = (tool.press_pos, release_pos) {
            if press.distance(release) < CLICK_DRAG_THRESHOLD_PX && !over_egui {
                if let Ok((camera, camera_transform)) = cameras.single() {
                    clicked_ground =
                        screen_to_ground(camera, camera_transform, &height_at, release);
                }
                let now = time.elapsed_secs();
                double_click = tool.last_click.is_some_and(|(t, pos)| {
                    now - t < DOUBLE_CLICK_WINDOW_SECS
                        && pos.distance(release) < CLICK_DRAG_THRESHOLD_PX * 2.0
                });
                tool.last_click = Some((now, release));
            }
        }
        tool.press_pos = None;
    }

    let enter_confirm =
        keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter);

    let Some(link) = link.as_deref() else {
        return; // No live sim connection: nothing any tool does is meaningful.
    };

    if let (Some(ground), Some(ui)) = (clicked_ground, ui_state.0.as_ref()) {
        match tool.active {
            // Select tool (v0.3): mirrors `PlaceStation`'s "consume a clean
            // click" plumbing below, just picking instead of building.
            // Nearest station within `SELECT_STATION_RADIUS_M` wins; a miss
            // clears the selection SILENTLY (no sfx/toast - clicking empty
            // ground is a deliberate "deselect", not an error) rather than
            // falling back to route-stripe picking, which the mission scopes
            // out of v0.3 (see `SelectedTarget`'s doc).
            ActiveTool::None => {
                *selected = match nearest_station_id(&ui.stations, ground, SELECT_STATION_RADIUS_M)
                {
                    Some(id) => {
                        sfx.write(PlaySfx(Sfx::Confirm));
                        SelectedTarget::Station(id)
                    }
                    None => SelectedTarget::None,
                };
            }
            ActiveTool::PlaceStation(mode) => {
                let pos = WireVec2 {
                    x: ground.x as f64,
                    y: ground.y as f64,
                };
                let seq = bus.submit(
                    link,
                    Command::BuildStation { mode, pos },
                    CmdMeta::BuildStation { mode, pos },
                );
                tool.pending_seqs.insert(seq);
            }
            ActiveTool::Route => {
                handle_route_click(&mut tool, ground, ui, link, &mut bus, double_click);
            }
            ActiveTool::Bulldoze => {
                handle_bulldoze_click(&mut tool, ground, ui, link, &mut bus);
            }
        }
    }

    if enter_confirm {
        if let (ActiveTool::Route, Some(ui)) = (tool.active, ui_state.0.as_ref()) {
            if tool.route_draft.len() >= 2 {
                confirm_route(&mut tool, ui, link, &mut bus);
                sfx.write(PlaySfx(Sfx::Confirm));
            }
        }
    }
}

/// Route tool click: pick the nearest station within `ROUTE_PICK_RADIUS_M`
/// (a soft no-op if none is close enough), append it to the draft unless
/// it would duplicate the draft's current tail, fire a track-cost query
/// once the draft has a segment to price, and, on a double-click, confirm
/// the draft (the double-clicked station itself need not be newly
/// appended: re-clicking the existing last station to finish a route is
/// the expected gesture, and `should_append_to_draft` already keeps that
/// from duplicating it).
fn handle_route_click(
    tool: &mut ToolState,
    ground: Vec2,
    ui: &UiState,
    link: &SimLink,
    bus: &mut CommandBus,
    double_click: bool,
) {
    let Some(station_id) = nearest_station_id(&ui.stations, ground, ROUTE_PICK_RADIUS_M) else {
        return;
    };
    if should_append_to_draft(&tool.route_draft, station_id) {
        tool.route_draft.push(station_id);
        if tool.route_draft.len() >= 2 {
            query_latest_segment_cost(tool, &ui.stations, link);
        }
    }
    if double_click && tool.route_draft.len() >= 2 {
        confirm_route(tool, ui, link, bus);
    }
}

/// Bulldoze tool click: nearest station within `BULLDOZE_STATION_RADIUS_M`
/// wins over a track (stations are the more common, and more
/// consequential, target), falling back to the nearest track polyline
/// within `BULLDOZE_TRACK_RADIUS_M`. A miss on both is a soft no-op.
fn handle_bulldoze_click(
    tool: &mut ToolState,
    ground: Vec2,
    ui: &UiState,
    link: &SimLink,
    bus: &mut CommandBus,
) {
    if let Some(station_id) = nearest_station_id(&ui.stations, ground, BULLDOZE_STATION_RADIUS_M) {
        let seq = bus.submit(
            link,
            Command::DemolishStation { station_id },
            CmdMeta::Demolish,
        );
        tool.pending_seqs.insert(seq);
        return;
    }
    if let Some(track_id) = nearest_track_id(&ui.tracks, ground, BULLDOZE_TRACK_RADIUS_M) {
        let seq = bus.submit(link, Command::DemolishTrack { track_id }, CmdMeta::Demolish);
        tool.pending_seqs.insert(seq);
    }
}

/// Fires `ToSim::QueryTrackCost` for the draft's newest (last) segment only
/// (cheap, and the only segment whose price the player hasn't already
/// seen). Silently does nothing if either endpoint's station has vanished
/// from `LatestUi` since it was drafted (e.g. bulldozed by a stray click).
fn query_latest_segment_cost(tool: &mut ToolState, stations: &[UiStation], link: &SimLink) {
    let n = tool.route_draft.len();
    let (Some(&a), Some(&b)) = (tool.route_draft.get(n - 2), tool.route_draft.get(n - 1)) else {
        return;
    };
    let (Some(sa), Some(sb)) = (
        stations.iter().find(|s| s.id == a),
        stations.iter().find(|s| s.id == b),
    ) else {
        return;
    };
    tool.next_track_cost_seq += 1;
    let seq = tool.next_track_cost_seq;
    tool.pending_track_cost_seq = Some(seq);
    let _ = link.transport.send(ToSim::QueryTrackCost {
        seq,
        payload: QueryTrackCostPayload {
            mode: tool.route_mode,
            grade: TrackGrade::Surface,
            points: vec![WireVec2 { x: sa.x, y: sa.y }, WireVec2 { x: sb.x, y: sb.y }],
        },
    });
}

/// Confirms the current route draft: builds whichever consecutive pairs
/// don't already have a track between them (either direction), then
/// creates the route over the full draft, then clears the draft. Takes the
/// draft via `mem::take` so the pair-building loop can walk it by
/// reference without cloning while the field is briefly empty.
fn confirm_route(tool: &mut ToolState, ui: &UiState, link: &SimLink, bus: &mut CommandBus) {
    let draft = std::mem::take(&mut tool.route_draft);
    for (a, b) in missing_track_pairs(&draft, &ui.tracks) {
        let seq = bus.submit(
            link,
            Command::BuildTrack {
                mode: tool.route_mode,
                grade: TrackGrade::Surface,
                from_station_id: a,
                to_station_id: b,
                waypoints: Vec::new(),
            },
            CmdMeta::BuildTrack { from: a, to: b },
        );
        tool.pending_seqs.insert(seq);
    }
    let seq = bus.submit(
        link,
        Command::CreateRoute {
            mode: tool.route_mode,
            station_ids: draft.clone(),
        },
        CmdMeta::CreateRoute {
            mode: tool.route_mode,
            station_ids: draft,
        },
    );
    tool.pending_seqs.insert(seq);
    tool.last_cost_quote = None;
}

// ---------------------------------------------------------------------
// Feedback: CommandBus results -> SFX, TrackCost replies -> the quote.
// ---------------------------------------------------------------------

/// Confirm/Error chime for any `CommandFeedback` this module itself
/// requested (see `ToolState::pending_seqs`'s doc). On failure the active
/// tool is deliberately left as-is (spec: "keep tool active on failure") so
/// a bad click can just be retried without re-selecting the tool.
fn command_feedback_system(
    mut tool: ResMut<ToolState>,
    mut feedback: EventReader<CommandFeedback>,
    mut sfx: EventWriter<PlaySfx>,
) {
    for fb in feedback.read() {
        if !tool.pending_seqs.remove(&fb.seq) {
            continue; // Not ours: some other CommandBus caller's action.
        }
        sfx.write(PlaySfx(if fb.ok { Sfx::Confirm } else { Sfx::Error }));
    }
}

/// Stashes a `trackCost` reply into `last_cost_quote` for the toolbar UI to
/// display, but only if its seq matches the query currently in flight: a
/// reply for an already-superseded segment (the player clicked again before
/// the sidecar answered) is stale and dropped.
fn track_cost_feedback_system(mut tool: ResMut<ToolState>, mut events: EventReader<SimEvent>) {
    for SimEvent(msg) in events.read() {
        if let FromSimMsg::Json(FromSimJson::TrackCost { seq, cost }) = msg {
            if tool.pending_track_cost_seq.is_some() && *seq == tool.pending_track_cost_seq {
                tool.last_cost_quote = Some(*cost);
                tool.pending_track_cost_seq = None;
            }
        }
    }
}

// ---------------------------------------------------------------------
// Ghost/preview rendering (bevy Gizmos: immediate-mode, zero asset churn).
// ---------------------------------------------------------------------

/// Vivid route color table, duplicated from `mf-render`'s private
/// `palette::vivid_route_color` (that module isn't visible to `mf-game`,
/// and this crate deliberately doesn't gain a dependency on `mf-render`'s
/// internals just for eight numbers). TODO(unify): hoist this into a small
/// shared color crate both renderers can depend on, so the two tables can
/// never drift apart. Kept identical value-for-value with `mf-render`'s
/// table so a route's draft ghost previews in the SAME color it will
/// finish in once built.
const VIVID_ROUTE_COLORS: [(u8, u8, u8); 8] = [
    (0xff, 0x3b, 0x30),
    (0x00, 0x7a, 0xff),
    (0xff, 0xcc, 0x00),
    (0x34, 0xc7, 0x59),
    (0xff, 0x95, 0x00),
    (0xaf, 0x52, 0xde),
    (0x00, 0xc7, 0xbe),
    (0xff, 0x2d, 0x95),
];

fn hex_color(r: u8, g: u8, b: u8) -> Color {
    Color::srgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

/// Color for the NEXT route about to be created, indexed by how many
/// routes already exist (`LatestUi.routes.len()`), same index/same color
/// convention `mf-render`'s finished routes use. Wraps modulo the fixed
/// eight rather than `mf-render`'s golden-angle extension past index 7:
/// good enough for a transient ghost preview, called out in the TODO above.
fn route_ghost_color(next_route_idx: usize) -> Color {
    let (r, g, b) = VIVID_ROUTE_COLORS[next_route_idx % VIVID_ROUTE_COLORS.len()];
    hex_color(r, g, b)
}

/// Accent blue for the place-station ghost: literally
/// `VIVID_ROUTE_COLORS[1]` / `mf-render::palette::mode_accent(Metro)`, kept
/// as its own named function rather than a magic index for readability at
/// the call site.
fn accent_blue() -> Color {
    hex_color(0x00, 0x7a, 0xff)
}

/// Bulldoze ghost red: `VIVID_ROUTE_COLORS[0]`, named for the same reason.
fn bulldoze_red() -> Color {
    hex_color(0xff, 0x3b, 0x30)
}

/// Draws a flat circle lying in the ground (XZ) plane: `Gizmos::circle`
/// draws in the XY plane of its isometry by default, so a quarter-turn
/// about X puts it flat on the ground instead of standing up like a coin.
fn draw_ground_circle(gizmos: &mut Gizmos, center: Vec3, radius: f32, color: Color) {
    let rotation = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    gizmos.circle(Isometry3d::new(center, rotation), radius, color);
}

/// Draws whichever tool's ghost/preview is relevant this frame. Gated (at
/// the call site's plugin registration) to `InGame`, and here to "a tool is
/// actually active" and "egui doesn't have the pointer" (a hovering egui
/// panel means the cursor isn't over the world, so a ground ghost there
/// would be misleading rather than helpful).
fn tool_gizmo_system(
    mut gizmos: Gizmos,
    mut egui_contexts: EguiContexts,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    height_at: Res<HeightAt>,
    tool: Res<ToolState>,
    ui_state: Res<LatestUi>,
) {
    if tool.active == ActiveTool::None {
        return;
    }
    let over_egui = egui_contexts
        .ctx_mut()
        .map(|ctx| ctx.wants_pointer_input())
        .unwrap_or(false);
    if over_egui {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_screen) = window.cursor_position() else {
        return;
    };
    let Ok((camera, camera_transform)) = cameras.single() else {
        return;
    };
    let Some(cursor_ground) = screen_to_ground(camera, camera_transform, &height_at, cursor_screen)
    else {
        return;
    };
    let cursor_world = Vec3::new(
        cursor_ground.x,
        height_at.sample(cursor_ground.x, cursor_ground.y),
        cursor_ground.y,
    );

    match tool.active {
        ActiveTool::None => {}
        ActiveTool::PlaceStation(_) => {
            draw_ground_circle(
                &mut gizmos,
                cursor_world,
                STATION_GHOST_RADIUS_M,
                accent_blue(),
            );
            gizmos.line(
                cursor_world,
                cursor_world + Vec3::Y * STATION_GHOST_HEIGHT_M,
                accent_blue(),
            );
        }
        ActiveTool::Route => {
            let Some(ui) = ui_state.0.as_ref() else {
                return;
            };
            let color = route_ghost_color(ui.routes.len());
            let mut points: Vec<Vec3> = tool
                .route_draft
                .iter()
                .filter_map(|id| ui.stations.iter().find(|s| s.id == *id))
                .map(|s| {
                    Vec3::new(
                        s.x as f32,
                        height_at.sample(s.x as f32, s.y as f32),
                        s.y as f32,
                    )
                })
                .collect();
            points.push(cursor_world);
            if points.len() >= 2 {
                gizmos.linestrip(points, color);
            }
        }
        ActiveTool::Bulldoze => {
            draw_ground_circle(
                &mut gizmos,
                cursor_world,
                BULLDOZE_GHOST_RADIUS_M,
                bulldoze_red(),
            );
        }
    }
}

// ---------------------------------------------------------------------
// Pure helpers (no ECS types), unit-tested directly with hand data.
// ---------------------------------------------------------------------

/// Nearest station to `point` within `max_dist` meters, or `None` if every
/// station is farther than that (including the trivial case of an empty
/// list). Squared distances throughout so the hot path never calls `sqrt`.
fn nearest_station_id(stations: &[UiStation], point: Vec2, max_dist: f32) -> Option<i64> {
    let max_dist_sq = max_dist * max_dist;
    stations
        .iter()
        .map(|s| {
            let delta = Vec2::new(s.x as f32, s.y as f32) - point;
            (s.id, delta.length_squared())
        })
        .filter(|(_, dist_sq)| *dist_sq <= max_dist_sq)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// Shortest distance from `p` to the segment `a..b` (0 if `p` lies on it,
/// the distance to whichever endpoint is nearer if `p` projects outside the
/// segment). Degenerates gracefully to point-to-point distance if `a == b`.
fn point_segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq < 1e-6 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    let closest = a + ab * t;
    (p - closest).length()
}

/// Minimum point-to-segment distance over a track's flattened `points`
/// (`UiTrack::points`: flat x,y pairs). `None` for a degenerate track with
/// fewer than two vertices (shouldn't happen, safe to skip if it does).
fn track_min_distance(points: &[f64], point: Vec2) -> Option<f32> {
    if points.len() < 4 {
        return None;
    }
    let verts: Vec<Vec2> = points
        .chunks_exact(2)
        .map(|c| Vec2::new(c[0] as f32, c[1] as f32))
        .collect();
    verts
        .windows(2)
        .map(|w| point_segment_distance(point, w[0], w[1]))
        .min_by(|a, b| a.total_cmp(b))
}

/// Nearest track polyline to `point` within `max_dist` meters, or `None` if
/// none qualifies.
fn nearest_track_id(tracks: &[UiTrack], point: Vec2, max_dist: f32) -> Option<i64> {
    tracks
        .iter()
        .filter_map(|t| track_min_distance(&t.points, point).map(|dist| (t.id, dist)))
        .filter(|(_, dist)| *dist <= max_dist)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// Whether `tracks` already has a track connecting `a` and `b`, in either
/// direction (the sim treats a track as undirected for this purpose).
fn track_exists(tracks: &[UiTrack], a: i64, b: i64) -> bool {
    tracks.iter().any(|t| {
        (t.from_station_id == a && t.to_station_id == b)
            || (t.from_station_id == b && t.to_station_id == a)
    })
}

/// True if appending `id` to `draft` would NOT duplicate its current tail:
/// re-clicking the same station twice in a row is a no-op, but
/// revisiting an EARLIER station later (a loop route) is allowed.
fn should_append_to_draft(draft: &[i64], id: i64) -> bool {
    draft.last() != Some(&id)
}

/// Every consecutive pair in `draft` that doesn't already have a track
/// between its two stations: exactly the set `confirm_route` needs to
/// build before creating the route itself.
fn missing_track_pairs(draft: &[i64], tracks: &[UiTrack]) -> Vec<(i64, i64)> {
    draft
        .windows(2)
        .map(|pair| (pair[0], pair[1]))
        .filter(|(a, b)| !track_exists(tracks, *a, *b))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(id: i64, x: f64, y: f64) -> UiStation {
        UiStation {
            id,
            name: format!("s{id}"),
            x,
            y,
            mode: TransitMode::Bus,
            level: 1,
            ridership: 0.0,
            alightings: 0.0,
        }
    }

    fn track(id: i64, from: i64, to: i64, points: Vec<f64>) -> UiTrack {
        UiTrack {
            id,
            mode: TransitMode::Bus,
            grade: "surface".to_string(),
            points,
            from_station_id: from,
            to_station_id: to,
        }
    }

    // --- nearest_station_id -------------------------------------------

    #[test]
    fn nearest_station_picks_the_closest_within_range() {
        let stations = vec![station(1, 0.0, 0.0), station(2, 100.0, 0.0)];
        let id = nearest_station_id(&stations, Vec2::new(10.0, 0.0), 80.0);
        assert_eq!(id, Some(1));
    }

    #[test]
    fn nearest_station_ignores_everything_out_of_range() {
        let stations = vec![station(1, 500.0, 500.0)];
        assert_eq!(
            nearest_station_id(&stations, Vec2::new(0.0, 0.0), 80.0),
            None
        );
    }

    #[test]
    fn nearest_station_empty_list_is_none() {
        assert_eq!(nearest_station_id(&[], Vec2::ZERO, 80.0), None);
    }

    // --- point_segment_distance ----------------------------------------

    #[test]
    fn point_segment_distance_perpendicular_to_middle() {
        let d = point_segment_distance(
            Vec2::new(5.0, 5.0),
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
        );
        assert!((d - 5.0).abs() < 0.001, "got {d}");
    }

    #[test]
    fn point_segment_distance_clamps_past_the_far_endpoint() {
        let d = point_segment_distance(
            Vec2::new(15.0, 0.0),
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
        );
        assert!((d - 5.0).abs() < 0.001, "got {d}");
    }

    #[test]
    fn point_segment_distance_zero_when_on_the_segment() {
        let d = point_segment_distance(
            Vec2::new(5.0, 0.0),
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
        );
        assert!(d < 0.001, "got {d}");
    }

    #[test]
    fn point_segment_distance_degenerate_segment_is_point_distance() {
        let d = point_segment_distance(Vec2::new(3.0, 4.0), Vec2::ZERO, Vec2::ZERO);
        assert!((d - 5.0).abs() < 0.001, "got {d}");
    }

    // --- nearest_track_id ------------------------------------------------

    #[test]
    fn nearest_track_finds_the_closer_of_two() {
        let tracks = vec![
            track(1, 10, 20, vec![0.0, 0.0, 10.0, 0.0]),
            track(2, 30, 40, vec![0.0, 100.0, 10.0, 100.0]),
        ];
        let id = nearest_track_id(&tracks, Vec2::new(5.0, 1.0), 30.0);
        assert_eq!(id, Some(1));
    }

    #[test]
    fn nearest_track_out_of_range_is_none() {
        let tracks = vec![track(1, 10, 20, vec![0.0, 0.0, 10.0, 0.0])];
        assert_eq!(
            nearest_track_id(&tracks, Vec2::new(500.0, 500.0), 30.0),
            None
        );
    }

    #[test]
    fn track_min_distance_uses_the_closer_of_multiple_segments() {
        // An L-shaped track: (0,0)->(10,0)->(10,10). A point near the
        // corner should read close to zero, not the (larger) distance to
        // either segment's midpoint.
        let dist = track_min_distance(&[0.0, 0.0, 10.0, 0.0, 10.0, 10.0], Vec2::new(10.0, 1.0));
        assert!(dist.unwrap() < 1.5, "got {dist:?}");
    }

    #[test]
    fn track_min_distance_degenerate_points_is_none() {
        assert_eq!(track_min_distance(&[0.0, 0.0], Vec2::ZERO), None);
    }

    // --- track_exists -----------------------------------------------------

    #[test]
    fn track_exists_matches_either_direction() {
        let tracks = vec![track(1, 5, 6, vec![0.0, 0.0, 1.0, 1.0])];
        assert!(track_exists(&tracks, 5, 6));
        assert!(track_exists(&tracks, 6, 5));
        assert!(!track_exists(&tracks, 5, 7));
    }

    // --- should_append_to_draft ------------------------------------------

    #[test]
    fn draft_dedup_rejects_consecutive_repeat() {
        assert!(!should_append_to_draft(&[1, 2], 2));
    }

    #[test]
    fn draft_dedup_allows_new_station() {
        assert!(should_append_to_draft(&[1, 2], 3));
    }

    #[test]
    fn draft_dedup_allows_revisiting_an_earlier_station_for_a_loop() {
        assert!(should_append_to_draft(&[1, 2, 3], 1));
    }

    #[test]
    fn draft_dedup_empty_draft_allows_anything() {
        assert!(should_append_to_draft(&[], 42));
    }

    // --- missing_track_pairs ----------------------------------------------

    #[test]
    fn missing_track_pairs_skips_existing_tracks() {
        let tracks = vec![track(1, 1, 2, vec![0.0, 0.0, 1.0, 1.0])];
        let pairs = missing_track_pairs(&[1, 2, 3], &tracks);
        assert_eq!(pairs, vec![(2, 3)]);
    }

    #[test]
    fn missing_track_pairs_all_missing_when_no_tracks_exist() {
        let pairs = missing_track_pairs(&[1, 2, 3], &[]);
        assert_eq!(pairs, vec![(1, 2), (2, 3)]);
    }

    #[test]
    fn missing_track_pairs_single_station_draft_has_none() {
        let pairs = missing_track_pairs(&[1], &[]);
        assert!(pairs.is_empty());
    }

    // --- route_ghost_color -------------------------------------------------

    #[test]
    fn route_ghost_color_differs_between_first_two_indices() {
        assert_ne!(route_ghost_color(0), route_ghost_color(1));
    }

    #[test]
    fn route_ghost_color_wraps_at_eight() {
        assert_eq!(route_ghost_color(8), route_ghost_color(0));
    }
}
