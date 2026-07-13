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
    Command, FromSimJson, FromSimMsg, QueryTrackCostPayload, RoadDto, StaticCityJson, ToSim,
    TrackGrade, TransitMode, UiState, UiStation, UiTrack, Vec2 as WireVec2,
};
use mf_state::{CurrentCity, HeightAt, LatestFields, LatestUi};

use crate::audio::{PlaySfx, Sfx};
use crate::camera::{screen_to_ground, CLICK_DRAG_THRESHOLD_PX};
use crate::command_bus::{CmdMeta, CommandBus, CommandFeedback};
use crate::routes_panel::{self, RoutePanelState};

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
/// Place-station: how close (meters) the cursor's snapped cell must be to a
/// road polyline before the ghost snaps to the road *frontage* (the nearest
/// point ON the road) instead of the plain grid cell center. Buildings in a
/// real city front a street; a station wants to sit on the road it serves.
/// Comfortably larger than a cell so a click "near" a road still grabs it.
const ROAD_SNAP_RADIUS_M: f32 = 45.0;
/// Place-station: minimum spacing (meters) from an existing station for a
/// placement to be considered valid. Below this the target cell reads as
/// "occupied" and the ghost tints invalid. Matched to the station ghost
/// footprint so two stations can't visually overlap.
const STATION_MIN_SEPARATION_M: f32 = STATION_GHOST_RADIUS_M * 2.0;
/// Valid-placement ghost tint (green): the cell is buildable.
fn valid_green() -> Color {
    Color::srgb_u8(0x34, 0xc7, 0x59)
}
/// Subtle snap-guide gray: drawn from the raw grid cell to the road frontage
/// point the ghost snapped to, so the player can see WHY the ghost jumped.
fn snap_guide_gray() -> Color {
    Color::srgba(0.85, 0.85, 0.9, 0.6)
}

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
    /// Operations (v0.9 A4): place a maintenance depot for one mode. The sim
    /// allows one depot per mode; a second placement for the same mode bounces
    /// with an error chime, tool left active to retry.
    PlaceDepot(TransitMode),
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
    /// Place-station ghost facing, in 90-degree steps (0..4), cycled by `R`
    /// while the place tool is active. When no place tool is active, `R`
    /// instead activates the Route tool (see empty-state copy in the routes
    /// panel: "place two stations and press R").
    pub ghost_quarter_turns: u8,
    /// Screen-space cursor at the last RIGHT-button press, mirroring
    /// `press_pos` for the left button: lets `tool_click_system` tell a
    /// right-CLICK (cancel the active tool) from a right-DRAG (camera orbit,
    /// handled untouched in `camera.rs`).
    right_press_pos: Option<Vec2>,
    /// When set, the Route tool is editing an existing route's stops (seeded
    /// from the routes panel). Confirm applies the draft back to the panel's
    /// `edit_stops` instead of firing `CreateRoute` immediately — the panel
    /// owns the Delete+Create apply step so name/fare/vehicles survive.
    pub editing_route_id: Option<i64>,
    /// Mode the next depot placement will use (v0.9 A4). The toolbar's depot
    /// mode picker writes it; the main Depot button re-activates whatever was
    /// last chosen. Defaults to Bus (the always-unlocked starter mode).
    pub depot_mode: TransitMode,
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
            ghost_quarter_turns: 0,
            right_press_pos: None,
            editing_route_id: None,
            depot_mode: TransitMode::Bus,
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
        tool.editing_route_id = None;
    } else if keys.just_pressed(KeyCode::Digit2) {
        tool.active = ActiveTool::Route;
    } else if keys.just_pressed(KeyCode::Digit3) {
        tool.active = ActiveTool::Bulldoze;
        tool.editing_route_id = None;
    }
    // R: rotate place-station ghost while placing; otherwise activate the
    // Route tool so "place two stations and press R" matches the empty-state
    // guidance in the routes panel.
    if keys.just_pressed(KeyCode::KeyR) {
        if matches!(tool.active, ActiveTool::PlaceStation(_)) {
            tool.ghost_quarter_turns = next_ghost_quarter_turns(tool.ghost_quarter_turns);
        } else {
            tool.active = ActiveTool::Route;
        }
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
    city: Res<CurrentCity>,
    fields: Res<LatestFields>,
    link: Option<Res<SimLink>>,
    mut bus: ResMut<CommandBus>,
    mut sfx: EventWriter<PlaySfx>,
    mut selected: ResMut<SelectedTarget>,
    mut panel: ResMut<RoutePanelState>,
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

    // Right-click (press+release under the drag threshold) cancels the active
    // build tool, the standard city-builder "put the tool away" gesture. A
    // right-DRAG is left untouched for `camera.rs`'s orbit — we only act when
    // the pointer barely moved, exactly as the left click-vs-drag split does.
    if mouse_buttons.just_pressed(MouseButton::Right) {
        tool.right_press_pos = window.cursor_position();
    }
    if mouse_buttons.just_released(MouseButton::Right) {
        if let (Some(press), Some(release)) = (tool.right_press_pos, window.cursor_position()) {
            if press.distance(release) < CLICK_DRAG_THRESHOLD_PX
                && !over_egui
                && tool.active != ActiveTool::None
            {
                tool.active = ActiveTool::None;
                tool.route_draft.clear();
                tool.last_cost_quote = None;
                tool.editing_route_id = None;
                sfx.write(PlaySfx(Sfx::Cancel));
            }
        }
        tool.right_press_pos = None;
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
    let shift_held = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

    let Some(link) = link.as_deref() else {
        return; // No live sim connection: nothing any tool does is meaningful.
    };

    if let (Some(ground), Some(ui)) = (clicked_ground, ui_state.0.as_ref()) {
        match tool.active {
            // Select tool: plain click picks one station for the inspector.
            // Shift+click multi-selects into `route_draft` (ordered) so the
            // player can connect several stations without entering the Route
            // tool first — Enter then builds tracks + creates the route.
            ActiveTool::None => {
                if shift_held {
                    if let Some(station_id) =
                        nearest_station_id(&ui.stations, ground, SELECT_STATION_RADIUS_M)
                    {
                        routes_panel::toggle_station_in_order(&mut tool.route_draft, station_id);
                        *selected = SelectedTarget::Station(station_id);
                        sfx.write(PlaySfx(Sfx::Confirm));
                    }
                } else {
                    *selected =
                        match nearest_station_id(&ui.stations, ground, SELECT_STATION_RADIUS_M) {
                            Some(id) => {
                                sfx.write(PlaySfx(Sfx::Confirm));
                                SelectedTarget::Station(id)
                            }
                            None => SelectedTarget::None,
                        };
                }
            }
            ActiveTool::PlaceStation(mode) => {
                // Snap the raw click to the city grid / road frontage, then
                // gate on validity: a placement over water, off the map, or
                // on top of an existing station is rejected with an error
                // chime instead of firing a doomed BuildStation the sim would
                // just bounce. The tool stays active so the player can retry.
                let placement = city
                    .static_city
                    .as_ref()
                    .map(|c| resolve_placement(ground, c));
                let build_pos = placement.map(|p| p.pos).unwrap_or(ground);
                let valid = placement_valid(
                    build_pos,
                    city.static_city.as_ref(),
                    fields.0.as_ref().map(|f| f.water.as_slice()),
                    &ui.stations,
                );
                if !valid {
                    sfx.write(PlaySfx(Sfx::Error));
                } else {
                    let pos = WireVec2 {
                        x: build_pos.x as f64,
                        y: build_pos.y as f64,
                    };
                    let seq = bus.submit(
                        link,
                        Command::BuildStation { mode, pos },
                        CmdMeta::BuildStation { mode, pos },
                    );
                    tool.pending_seqs.insert(seq);
                }
            }
            ActiveTool::Route => {
                handle_route_click(
                    &mut tool,
                    ground,
                    ui,
                    link,
                    &mut bus,
                    &mut panel,
                    double_click,
                    shift_held,
                    &mut sfx,
                );
            }
            ActiveTool::Bulldoze => {
                handle_bulldoze_click(&mut tool, ground, ui, link, &mut bus);
            }
            ActiveTool::PlaceDepot(mode) => {
                // Depots snap to the plain grid cell (no road frontage): they
                // are yards, not street-fronting stops. Reject water up front
                // with an error chime; the sim's one-per-mode rule surfaces as
                // an error reply, handled in `command_feedback_system`.
                let valid = depot_placement_valid(
                    ground,
                    city.static_city.as_ref(),
                    fields.0.as_ref().map(|f| f.water.as_slice()),
                );
                if !valid {
                    sfx.write(PlaySfx(Sfx::Error));
                } else {
                    let pos = WireVec2 {
                        x: ground.x as f64,
                        y: ground.y as f64,
                    };
                    let seq = bus.submit(
                        link,
                        Command::BuildDepot { mode, pos },
                        CmdMeta::BuildDepot { mode },
                    );
                    tool.pending_seqs.insert(seq);
                }
            }
        }
    }

    if enter_confirm {
        // Multi-select from the Select tool, or a Route draft: same confirm.
        let can_confirm = tool.route_draft.len() >= 2
            && (tool.active == ActiveTool::Route
                || tool.active == ActiveTool::None
                || tool.editing_route_id.is_some());
        if can_confirm {
            if let Some(ui) = ui_state.0.as_ref() {
                if tool.editing_route_id.is_some() {
                    // Hand the draft back to the panel as `edit_stops`; the
                    // player hits Apply there (Delete+Create) so props survive.
                    panel.edit_stops = Some(tool.route_draft.clone());
                    panel.open = true;
                    if let Some(id) = tool.editing_route_id {
                        panel.selected = Some(id);
                    }
                    tool.route_draft.clear();
                    tool.editing_route_id = None;
                    tool.active = ActiveTool::None;
                    tool.last_cost_quote = None;
                    sfx.write(PlaySfx(Sfx::Confirm));
                } else {
                    confirm_route(&mut tool, ui, link, &mut bus);
                    tool.active = ActiveTool::Route;
                    sfx.write(PlaySfx(Sfx::Confirm));
                }
            }
        }
    }
}

/// Route tool click: pick the nearest station within `ROUTE_PICK_RADIUS_M`.
/// Plain click appends (same as before). Shift+click toggles membership so
/// multi-select does not require a strict sequential chain. When editing an
/// existing route (`editing_route_id`), clicks toggle against the draft and
/// sync into the panel's `edit_stops`. Double-click confirms when the draft
/// has at least two stations.
#[allow(clippy::too_many_arguments)]
fn handle_route_click(
    tool: &mut ToolState,
    ground: Vec2,
    ui: &UiState,
    link: &SimLink,
    bus: &mut CommandBus,
    panel: &mut RoutePanelState,
    double_click: bool,
    shift_held: bool,
    sfx: &mut EventWriter<PlaySfx>,
) {
    let Some(station_id) = nearest_station_id(&ui.stations, ground, ROUTE_PICK_RADIUS_M) else {
        return;
    };
    if shift_held || tool.editing_route_id.is_some() {
        routes_panel::toggle_station_in_order(&mut tool.route_draft, station_id);
        if tool.editing_route_id.is_some() {
            panel.edit_stops = Some(tool.route_draft.clone());
        }
        sfx.write(PlaySfx(Sfx::Confirm));
    } else {
        let after = tool.route_draft.last().copied();
        if routes_panel::insert_stop_after(&mut tool.route_draft, station_id, after).is_some() {
            if tool.route_draft.len() >= 2 {
                query_latest_segment_cost(tool, &ui.stations, link);
            }
            sfx.write(PlaySfx(Sfx::Confirm));
        }
    }
    if double_click && tool.route_draft.len() >= 2 {
        if tool.editing_route_id.is_some() {
            panel.edit_stops = Some(tool.route_draft.clone());
            panel.open = true;
            if let Some(id) = tool.editing_route_id {
                panel.selected = Some(id);
            }
            tool.route_draft.clear();
            tool.editing_route_id = None;
            tool.last_cost_quote = None;
        } else {
            confirm_route(tool, ui, link, bus);
        }
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

/// Confirm/Error/Placement chime for any `CommandFeedback` this module itself
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
        let kind = if fb.ok {
            match fb.meta {
                CmdMeta::BuildStation { .. } | CmdMeta::BuildDepot { .. } => Sfx::Placement,
                _ => Sfx::Confirm,
            }
        } else {
            Sfx::Error
        };
        sfx.write(PlaySfx(kind));
    }
}

/// Stashes a `trackCost` reply into `last_cost_quote` for the toolbar UI to
/// display, but only if its seq matches the query currently in flight: a
/// reply for an already-superseded segment (the player clicked again before
/// the sidecar answered) is stale and dropped.
fn track_cost_feedback_system(mut tool: ResMut<ToolState>, mut events: EventReader<SimEvent>) {
    for SimEvent(msg) in events.read() {
        if let FromSimMsg::Json(FromSimJson::TrackCost { seq, cost, .. }) = msg {
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

/// Color for the NEXT route about to be created, indexed by how many
/// routes already exist (`LatestUi.routes.len()`), same index/same color
/// convention `mf-render`'s finished routes use — including golden-angle
/// extension past the fixed eight.
fn route_ghost_color(next_route_idx: usize) -> Color {
    mf_render::palette::vivid_route_color(next_route_idx)
}

/// Accent blue for the place-station ghost: metro mode accent from the
/// shared palette (theme-aware).
fn accent_blue() -> Color {
    mf_render::palette::mode_accent(mf_protocol::TransitMode::Metro)
}

/// Bulldoze ghost red: first vivid route brick from the shared palette.
fn bulldoze_red() -> Color {
    mf_render::palette::vivid_route_color(0)
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
#[allow(clippy::too_many_arguments)]
fn tool_gizmo_system(
    mut gizmos: Gizmos,
    mut egui_contexts: EguiContexts,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    height_at: Res<HeightAt>,
    tool: Res<ToolState>,
    ui_state: Res<LatestUi>,
    city: Res<CurrentCity>,
    fields: Res<LatestFields>,
    focus: Res<mf_state::RouteFocus>,
) {
    let drafting = !tool.route_draft.is_empty();
    let editing_focus = focus.editing && focus.route_id.is_some();
    if tool.active == ActiveTool::None && !drafting && !editing_focus {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let over_egui = egui_contexts
        .ctx_mut()
        .map(|ctx| ctx.wants_pointer_input())
        .unwrap_or(false);
    if over_egui && tool.active == ActiveTool::None && !editing_focus {
        // Still allow draft preview under the cursor-free case below.
    }

    let cursor = window.cursor_position();
    let cursor_world = match (cursor, cameras.single()) {
        (Some(pos), Ok((camera, camera_transform))) => {
            screen_to_ground(camera, camera_transform, &height_at, pos)
                .map(|g| Vec3::new(g.x, height_at.sample(g.x, g.y), g.y))
        }
        _ => None,
    };

    // Stop numbers + direction ticks for the focused route while editing.
    if editing_focus {
        if let (Some(ui), Some(route_id)) = (ui_state.0.as_ref(), focus.route_id) {
            if let Some(route) = ui.routes.iter().find(|r| r.id == route_id) {
                let stops =
                    if tool.editing_route_id == Some(route_id) && !tool.route_draft.is_empty() {
                        tool.route_draft.as_slice()
                    } else {
                        route.station_ids.as_slice()
                    };
                draw_numbered_stops(&mut gizmos, &ui.stations, stops, &height_at);
                draw_direction_ticks(&mut gizmos, &ui.stations, stops, &height_at);
            }
        }
    }

    // Multi-select draft preview while the Select tool is active.
    if tool.active == ActiveTool::None && drafting {
        if let Some(ui) = ui_state.0.as_ref() {
            let color = route_ghost_color(ui.routes.len());
            draw_draft_polyline(
                &mut gizmos,
                &ui.stations,
                &tool.route_draft,
                cursor_world,
                color,
                &height_at,
            );
            draw_numbered_stops(&mut gizmos, &ui.stations, &tool.route_draft, &height_at);
        }
        return;
    }

    let Some(cursor_world) = cursor_world else {
        return;
    };
    if over_egui {
        return;
    }

    match tool.active {
        ActiveTool::None => {}
        ActiveTool::PlaceStation(_) => {
            let stations = ui_state
                .0
                .as_ref()
                .map(|u| u.stations.as_slice())
                .unwrap_or(&[]);
            let placement = city
                .static_city
                .as_ref()
                .map(|c| resolve_placement(Vec2::new(cursor_world.x, cursor_world.z), c));
            let snapped = placement
                .map(|p| p.pos)
                .unwrap_or(Vec2::new(cursor_world.x, cursor_world.z));
            let valid = placement_valid(
                snapped,
                city.static_city.as_ref(),
                fields.0.as_ref().map(|f| f.water.as_slice()),
                stations,
            );
            let tint = if valid { valid_green() } else { bulldoze_red() };
            let snapped_world =
                Vec3::new(snapped.x, height_at.sample(snapped.x, snapped.y), snapped.y);
            draw_ground_circle(&mut gizmos, snapped_world, STATION_GHOST_RADIUS_M, tint);
            gizmos.line(
                snapped_world,
                snapped_world + Vec3::Y * STATION_GHOST_HEIGHT_M,
                tint,
            );
            // Facing tick: a short spoke pointing in the R-rotated direction,
            // so the rotate gesture reads on screen even for a round marker.
            let facing_xz = ghost_facing_xz(tool.ghost_quarter_turns);
            let facing = Vec3::new(facing_xz.x, 0.0, facing_xz.y) * (STATION_GHOST_RADIUS_M * 1.4);
            gizmos.line(snapped_world, snapped_world + facing, tint);
            if let Some(guide_from) = placement.and_then(|p| p.guide_from) {
                let from_world = Vec3::new(
                    guide_from.x,
                    height_at.sample(guide_from.x, guide_from.y),
                    guide_from.y,
                );
                gizmos.line(from_world, snapped_world, snap_guide_gray());
            }
        }
        ActiveTool::Route => {
            let Some(ui) = ui_state.0.as_ref() else {
                return;
            };
            let color = route_ghost_color(ui.routes.len());
            draw_draft_polyline(
                &mut gizmos,
                &ui.stations,
                &tool.route_draft,
                Some(cursor_world),
                color,
                &height_at,
            );
            draw_numbered_stops(&mut gizmos, &ui.stations, &tool.route_draft, &height_at);
            if tool.route_draft.len() >= 2 {
                draw_direction_ticks(&mut gizmos, &ui.stations, &tool.route_draft, &height_at);
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
        ActiveTool::PlaceDepot(_) => {
            let ground = Vec2::new(cursor_world.x, cursor_world.z);
            let valid = depot_placement_valid(
                ground,
                city.static_city.as_ref(),
                fields.0.as_ref().map(|f| f.water.as_slice()),
            );
            let tint = if valid { valid_green() } else { bulldoze_red() };
            // A slightly larger footprint than a station reads as "yard".
            draw_ground_circle(
                &mut gizmos,
                cursor_world,
                STATION_GHOST_RADIUS_M * 1.4,
                tint,
            );
            gizmos.line(
                cursor_world,
                cursor_world + Vec3::Y * STATION_GHOST_HEIGHT_M,
                tint,
            );
        }
    }
}

fn draw_draft_polyline(
    gizmos: &mut Gizmos,
    stations: &[UiStation],
    draft: &[i64],
    cursor_world: Option<Vec3>,
    color: Color,
    height_at: &HeightAt,
) {
    let mut points: Vec<Vec3> = draft
        .iter()
        .filter_map(|id| stations.iter().find(|s| s.id == *id))
        .map(|s| {
            Vec3::new(
                s.x as f32,
                height_at.sample(s.x as f32, s.y as f32),
                s.y as f32,
            )
        })
        .collect();
    if let Some(c) = cursor_world {
        points.push(c);
    }
    if points.len() >= 2 {
        gizmos.linestrip(points, color);
    }
}

/// Numbered stop markers (1-based) for route draft / edit mode. Drawn as
/// stacked short vertical ticks whose count encodes the stop index so we
/// stay on Bevy gizmos (no egui world-text dependency).
fn draw_numbered_stops(
    gizmos: &mut Gizmos,
    stations: &[UiStation],
    stops: &[i64],
    height_at: &HeightAt,
) {
    let mark = Color::srgb_u8(0xff, 0xff, 0xff);
    for (i, sid) in stops.iter().enumerate() {
        let Some(s) = stations.iter().find(|st| st.id == *sid) else {
            continue;
        };
        let base = Vec3::new(
            s.x as f32,
            height_at.sample(s.x as f32, s.y as f32) + 2.0,
            s.y as f32,
        );
        draw_ground_circle(gizmos, base, 6.0, mark);
        // Index encoding: N short vertical pips (capped) plus a taller stem
        // so stop order reads at a glance without a text atlas.
        let n = (i + 1).min(8);
        for p in 0..n {
            let y0 = base + Vec3::Y * (8.0 + p as f32 * 3.0);
            gizmos.line(y0, y0 + Vec3::Y * 2.0, mark);
        }
        gizmos.line(
            base,
            base + Vec3::Y * (10.0 + n as f32 * 3.0),
            accent_blue(),
        );
    }
}

/// Direction ticks along consecutive stop pairs (edit-mode polish alongside
/// the mesh chevrons on finished routes).
fn draw_direction_ticks(
    gizmos: &mut Gizmos,
    stations: &[UiStation],
    stops: &[i64],
    height_at: &HeightAt,
) {
    let color = accent_blue();
    for w in stops.windows(2) {
        let (Some(a), Some(b)) = (
            stations.iter().find(|s| s.id == w[0]),
            stations.iter().find(|s| s.id == w[1]),
        ) else {
            continue;
        };
        let pa = Vec2::new(a.x as f32, a.y as f32);
        let pb = Vec2::new(b.x as f32, b.y as f32);
        let delta = pb - pa;
        let len = delta.length();
        if len < 1.0 {
            continue;
        }
        let dir = delta / len;
        let mid = pa + dir * (len * 0.5);
        let y = height_at.sample(mid.x, mid.y) + 3.0;
        let tip = mid + dir * 10.0;
        let perp = Vec2::new(-dir.y, dir.x);
        let left = tip - dir * 8.0 + perp * 4.0;
        let right = tip - dir * 8.0 - perp * 4.0;
        let tip3 = Vec3::new(tip.x, y, tip.y);
        gizmos.line(tip3, Vec3::new(left.x, y, left.y), color);
        gizmos.line(tip3, Vec3::new(right.x, y, right.y), color);
        gizmos.line(
            Vec3::new(mid.x, y, mid.y) - Vec3::new(dir.x, 0.0, dir.y) * 10.0,
            tip3,
            color,
        );
    }
}

// ---------------------------------------------------------------------
// Pure helpers (no ECS types), unit-tested directly with hand data.
// ---------------------------------------------------------------------

/// Advance the place-station ghost's R-rotate state one quarter-turn,
/// wrapping `3 -> 0`. Extracted from `keybind_system` so the wrap is
/// unit-testable without an input event.
fn next_ghost_quarter_turns(turns: u8) -> u8 {
    (turns + 1) % 4
}

/// Unit facing direction in world XZ for a given quarter-turn state
/// (0 = +Z / "north", 1 = +X / "east", 2 = -Z / "south", 3 = -X / "west"),
/// matching the gizmo spoke drawn by `tool_gizmo_system`.
fn ghost_facing_xz(quarter_turns: u8) -> Vec2 {
    let ang = (quarter_turns % 4) as f32 * std::f32::consts::FRAC_PI_2;
    Vec2::new(ang.sin(), ang.cos())
}

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

// ---------------------------------------------------------------------
// Placement snapping + validity (grid, road frontage, water/occupancy).
// ---------------------------------------------------------------------

/// The city's regular cell grid, lifted out of `StaticCityJson` so the snap
/// math has a small, testable surface instead of threading four raw fields
/// through every helper. Coordinates follow the usual convention: world X /
/// world Y (= Bevy Z).
#[derive(Clone, Copy, Debug)]
struct CityGrid {
    origin: Vec2,
    cell_size: f32,
    field_w: i32,
    field_h: i32,
}

impl CityGrid {
    fn from_city(c: &StaticCityJson) -> Self {
        CityGrid {
            origin: Vec2::new(c.origin_x as f32, c.origin_y as f32),
            cell_size: c.cell_size as f32,
            field_w: c.field_w as i32,
            field_h: c.field_h as i32,
        }
    }

    /// Integer cell containing `pos` (may be out of `[0, field)` if the point
    /// is off the map — callers gate on [`Self::in_bounds`]).
    fn cell_of(&self, pos: Vec2) -> (i32, i32) {
        let g = (pos - self.origin) / self.cell_size.max(f32::MIN_POSITIVE);
        (g.x.floor() as i32, g.y.floor() as i32)
    }

    /// World-space center of cell `(cx, cy)`.
    fn cell_center(&self, cx: i32, cy: i32) -> Vec2 {
        self.origin
            + Vec2::new(
                (cx as f32 + 0.5) * self.cell_size,
                (cy as f32 + 0.5) * self.cell_size,
            )
    }

    /// Snap a world point to the center of the cell it falls in.
    fn snap(&self, pos: Vec2) -> Vec2 {
        let (cx, cy) = self.cell_of(pos);
        self.cell_center(cx, cy)
    }

    fn in_bounds(&self, cx: i32, cy: i32) -> bool {
        cx >= 0 && cy >= 0 && cx < self.field_w && cy < self.field_h
    }
}

/// A resolved place-station target: where the ghost sits, and (when it
/// snapped to road frontage rather than a bare grid cell) the plain grid
/// point it jumped FROM, so the gizmo can draw a snap guide between the two.
#[derive(Clone, Copy, Debug)]
struct Placement {
    pos: Vec2,
    guide_from: Option<Vec2>,
}

/// Resolve a raw cursor-ground point into a snapped station placement: prefer
/// the nearest road frontage within [`ROAD_SNAP_RADIUS_M`] of the grid-snapped
/// cell (buildings front a street), else the plain grid cell center.
fn resolve_placement(cursor: Vec2, city: &StaticCityJson) -> Placement {
    let grid = CityGrid::from_city(city);
    let grid_pos = grid.snap(cursor);
    if let Some(front) = nearest_point_on_roads(grid_pos, &city.roads, ROAD_SNAP_RADIUS_M) {
        Placement {
            pos: front,
            guide_from: Some(grid_pos),
        }
    } else {
        Placement {
            pos: grid_pos,
            guide_from: None,
        }
    }
}

/// Whether a station may be built at `pos`: on the map, not over water, and
/// not on top of an existing station. A `None` city (not loaded yet) is
/// permissive — there is nothing to validate against — matching the old
/// unconditional build behavior for that pre-load window.
fn placement_valid(
    pos: Vec2,
    city: Option<&StaticCityJson>,
    water: Option<&[u8]>,
    stations: &[UiStation],
) -> bool {
    if let Some(city) = city {
        let grid = CityGrid::from_city(city);
        let (cx, cy) = grid.cell_of(pos);
        if !grid.in_bounds(cx, cy) {
            return false;
        }
        if let Some(water) = water {
            let idx = (cy * grid.field_w + cx) as usize;
            if water.get(idx).copied().unwrap_or(0) >= 1 {
                return false;
            }
        }
    }
    nearest_station_id(stations, pos, STATION_MIN_SEPARATION_M).is_none()
}

/// Whether a depot may be placed at `pos`: on the map and not over water.
/// Mirrors the sim's `buildDepot` guard (it only rejects water), so unlike a
/// station a depot has no minimum-separation or occupancy check here. The
/// one-per-mode rule lives in the sim and surfaces as an error chime.
fn depot_placement_valid(pos: Vec2, city: Option<&StaticCityJson>, water: Option<&[u8]>) -> bool {
    if let Some(city) = city {
        let grid = CityGrid::from_city(city);
        let (cx, cy) = grid.cell_of(pos);
        if !grid.in_bounds(cx, cy) {
            return false;
        }
        if let Some(water) = water {
            let idx = (cy * grid.field_w + cx) as usize;
            if water.get(idx).copied().unwrap_or(0) >= 1 {
                return false;
            }
        }
    }
    true
}

/// Closest point ON segment `a..b` to `p` (clamped to the endpoints).
/// Companion to [`point_segment_distance`], which is just the length of
/// `p - closest_point_on_segment(p, a, b)`.
fn closest_point_on_segment(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq < 1e-6 {
        return a;
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    a + ab * t
}

/// Nearest point lying on ANY road polyline within `max_dist` of `pos`, or
/// `None` if every road is farther than that. Roads store flat x,y pairs
/// (`RoadDto::points`), same layout as tracks.
fn nearest_point_on_roads(pos: Vec2, roads: &[RoadDto], max_dist: f32) -> Option<Vec2> {
    let max_sq = max_dist * max_dist;
    let mut best: Option<(f32, Vec2)> = None;
    for road in roads {
        if road.points.len() < 4 {
            continue;
        }
        let verts: Vec<Vec2> = road
            .points
            .chunks_exact(2)
            .map(|c| Vec2::new(c[0] as f32, c[1] as f32))
            .collect();
        for w in verts.windows(2) {
            let cp = closest_point_on_segment(pos, w[0], w[1]);
            let d_sq = (pos - cp).length_squared();
            if d_sq <= max_sq && best.is_none_or(|(bd, _)| d_sq < bd) {
                best = Some((d_sq, cp));
            }
        }
    }
    best.map(|(_, p)| p)
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
    fn route_ghost_color_extends_past_eight_via_golden_angle() {
        // Shared palette uses golden-angle HSL past the fixed eight bricks
        // (same as finished routes), not modulo wrap.
        assert_ne!(route_ghost_color(8), route_ghost_color(0));
        assert_ne!(route_ghost_color(8), route_ghost_color(7));
    }

    // --- placement snapping / validity ------------------------------------

    fn grid_city(roads: Vec<RoadDto>) -> StaticCityJson {
        // 10x10 cells of 10m, origin at 0,0 -> world spans [0, 100).
        StaticCityJson {
            field_w: 10,
            field_h: 10,
            cell_size: 10.0,
            origin_x: 0.0,
            origin_y: 0.0,
            world_size: 100.0,
            road_scale: 1.0,
            mask_res: None,
            has_water_mask: false,
            has_park_mask: false,
            has_building_mask: false,
            labels: None,
            poi_anchors: None,
            roads,
        }
    }

    fn road(points: Vec<f64>) -> RoadDto {
        RoadDto {
            cls: "local".to_string(),
            points,
            grade_level: 0,
            is_bridge: false,
            is_tunnel: false,
            name: None,
            wikidata: None,
        }
    }

    #[test]
    fn grid_snaps_to_cell_center() {
        let grid = CityGrid::from_city(&grid_city(vec![]));
        // A point anywhere inside cell (2,3) snaps to its center (25, 35).
        assert_eq!(grid.snap(Vec2::new(23.0, 31.0)), Vec2::new(25.0, 35.0));
        assert_eq!(grid.snap(Vec2::new(29.9, 39.9)), Vec2::new(25.0, 35.0));
    }

    #[test]
    fn grid_cell_of_and_bounds() {
        let grid = CityGrid::from_city(&grid_city(vec![]));
        assert_eq!(grid.cell_of(Vec2::new(0.0, 0.0)), (0, 0));
        assert!(grid.in_bounds(9, 9));
        assert!(!grid.in_bounds(10, 0));
        assert!(!grid.in_bounds(-1, 5));
    }

    #[test]
    fn placement_without_road_falls_back_to_grid() {
        let city = grid_city(vec![]);
        let p = resolve_placement(Vec2::new(23.0, 31.0), &city);
        assert_eq!(p.pos, Vec2::new(25.0, 35.0));
        assert!(p.guide_from.is_none());
    }

    #[test]
    fn placement_snaps_to_nearby_road_frontage() {
        // A horizontal road along y=50 across the map. A click near cell
        // (2,4) (center 25,45) is within ROAD_SNAP_RADIUS_M (45) of the road,
        // so it snaps onto the road at x=25, y=50 and records a guide.
        let city = grid_city(vec![road(vec![0.0, 50.0, 100.0, 50.0])]);
        let p = resolve_placement(Vec2::new(23.0, 44.0), &city);
        assert_eq!(p.pos, Vec2::new(25.0, 50.0));
        assert_eq!(p.guide_from, Some(Vec2::new(25.0, 45.0)));
    }

    #[test]
    fn placement_ignores_a_road_too_far_away() {
        // Road along y=0; a click way up at cell center (25,95) is far past
        // the snap radius, so no frontage snap happens.
        let city = grid_city(vec![road(vec![0.0, 0.0, 100.0, 0.0])]);
        let p = resolve_placement(Vec2::new(25.0, 95.0), &city);
        assert_eq!(p.pos, Vec2::new(25.0, 95.0));
        assert!(p.guide_from.is_none());
    }

    #[test]
    fn placement_over_water_is_invalid() {
        let city = grid_city(vec![]);
        // 10x10 water grid, all dry except cell (2,3) = index 3*10+2 = 32.
        let mut water = vec![0u8; 100];
        water[32] = 1;
        // Point inside cell (2,3): invalid.
        assert!(!placement_valid(
            Vec2::new(25.0, 35.0),
            Some(&city),
            Some(&water),
            &[]
        ));
        // A dry neighbouring cell: valid.
        assert!(placement_valid(
            Vec2::new(35.0, 35.0),
            Some(&city),
            Some(&water),
            &[]
        ));
    }

    #[test]
    fn placement_off_map_is_invalid() {
        let city = grid_city(vec![]);
        assert!(!placement_valid(
            Vec2::new(500.0, 500.0),
            Some(&city),
            None,
            &[]
        ));
    }

    #[test]
    fn placement_on_existing_station_is_invalid() {
        let city = grid_city(vec![]);
        let stations = vec![station(1, 25.0, 35.0)];
        // Right on top: within STATION_MIN_SEPARATION_M -> occupied.
        assert!(!placement_valid(
            Vec2::new(26.0, 36.0),
            Some(&city),
            None,
            &stations
        ));
        // Far enough away in a different cell -> free.
        assert!(placement_valid(
            Vec2::new(85.0, 85.0),
            Some(&city),
            None,
            &stations
        ));
    }

    #[test]
    fn placement_with_no_city_is_permissive() {
        // Pre-load window: nothing to validate against, so any empty-ground
        // placement is allowed (mirrors the old unconditional build).
        assert!(placement_valid(Vec2::new(9999.0, 9999.0), None, None, &[]));
    }

    #[test]
    fn closest_point_on_segment_projects_and_clamps() {
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(10.0, 0.0);
        assert_eq!(
            closest_point_on_segment(Vec2::new(5.0, 5.0), a, b),
            Vec2::new(5.0, 0.0)
        );
        // Past the far end clamps to the endpoint.
        assert_eq!(closest_point_on_segment(Vec2::new(20.0, 5.0), a, b), b);
    }

    #[test]
    fn nearest_point_on_roads_picks_the_closer_road() {
        let roads = vec![
            road(vec![0.0, 0.0, 100.0, 0.0]),
            road(vec![0.0, 100.0, 100.0, 100.0]),
        ];
        let p = nearest_point_on_roads(Vec2::new(40.0, 10.0), &roads, 45.0).unwrap();
        assert_eq!(p, Vec2::new(40.0, 0.0));
    }

    #[test]
    fn nearest_point_on_roads_out_of_range_is_none() {
        let roads = vec![road(vec![0.0, 0.0, 100.0, 0.0])];
        assert_eq!(
            nearest_point_on_roads(Vec2::new(50.0, 500.0), &roads, 45.0),
            None
        );
    }

    // --- rotation states ---------------------------------------------------

    #[test]
    fn ghost_quarter_turns_wrap_three_to_zero() {
        assert_eq!(next_ghost_quarter_turns(0), 1);
        assert_eq!(next_ghost_quarter_turns(1), 2);
        assert_eq!(next_ghost_quarter_turns(2), 3);
        assert_eq!(next_ghost_quarter_turns(3), 0);
        // A full spin returns to the start.
        let mut t = 0u8;
        for _ in 0..4 {
            t = next_ghost_quarter_turns(t);
        }
        assert_eq!(t, 0);
    }

    #[test]
    fn ghost_facing_covers_cardinal_directions() {
        let expected = [
            Vec2::new(0.0, 1.0),  // 0: +Z / north
            Vec2::new(1.0, 0.0),  // 1: +X / east
            Vec2::new(0.0, -1.0), // 2: -Z / south
            Vec2::new(-1.0, 0.0), // 3: -X / west
        ];
        for (turns, want) in expected.into_iter().enumerate() {
            let got = ghost_facing_xz(turns as u8);
            assert!(
                (got - want).length() < 1e-5,
                "turns={turns}: got {got:?}, want {want:?}"
            );
        }
    }

    // --- CityGrid bounds / snap edge cases ---------------------------------

    #[test]
    fn grid_cell_of_at_exact_cell_boundaries() {
        let grid = CityGrid::from_city(&grid_city(vec![]));
        // Left/bottom edge of cell (2,3) is at world (20, 30); floor maps
        // that exact boundary into (2,3), not the previous cell.
        assert_eq!(grid.cell_of(Vec2::new(20.0, 30.0)), (2, 3));
        // One ulp inside the next cell.
        assert_eq!(grid.cell_of(Vec2::new(29.999, 39.999)), (2, 3));
        assert_eq!(grid.cell_of(Vec2::new(30.0, 40.0)), (3, 4));
    }

    #[test]
    fn grid_in_bounds_rejects_negative_and_past_far_edge() {
        let grid = CityGrid::from_city(&grid_city(vec![]));
        assert!(grid.in_bounds(0, 0));
        assert!(grid.in_bounds(9, 9));
        assert!(!grid.in_bounds(-1, 0));
        assert!(!grid.in_bounds(0, -1));
        assert!(!grid.in_bounds(10, 9));
        assert!(!grid.in_bounds(9, 10));
        // cell_of of a point past the far edge is out of bounds.
        let (cx, cy) = grid.cell_of(Vec2::new(100.0, 100.0));
        assert!(!grid.in_bounds(cx, cy));
    }

    #[test]
    fn road_snap_includes_exact_radius_excludes_just_beyond() {
        // Horizontal road on y=0. Grid-snapped cursor at (50, 45) is exactly
        // ROAD_SNAP_RADIUS_M from the road; (50, 45+ε) is not.
        let roads = vec![road(vec![0.0, 0.0, 100.0, 0.0])];
        let on_boundary = nearest_point_on_roads(
            Vec2::new(50.0, ROAD_SNAP_RADIUS_M),
            &roads,
            ROAD_SNAP_RADIUS_M,
        );
        assert_eq!(on_boundary, Some(Vec2::new(50.0, 0.0)));

        let just_beyond = nearest_point_on_roads(
            Vec2::new(50.0, ROAD_SNAP_RADIUS_M + 0.5),
            &roads,
            ROAD_SNAP_RADIUS_M,
        );
        assert_eq!(just_beyond, None);
    }

    #[test]
    fn road_snap_skips_degenerate_polylines() {
        // Fewer than 2 vertices (4 floats) must be ignored, not panic.
        let roads = vec![road(vec![0.0, 0.0]), road(vec![0.0, 0.0, 100.0, 0.0])];
        let p = nearest_point_on_roads(Vec2::new(40.0, 5.0), &roads, 45.0);
        assert_eq!(p, Some(Vec2::new(40.0, 0.0)));
    }

    #[test]
    fn station_separation_boundary_is_inclusive_invalid() {
        let city = grid_city(vec![]);
        let stations = vec![station(1, 50.0, 50.0)];
        // Exactly STATION_MIN_SEPARATION_M away: `<=` in nearest_station_id
        // means occupied / invalid.
        let on_ring = Vec2::new(50.0 + STATION_MIN_SEPARATION_M, 50.0);
        assert!(!placement_valid(on_ring, Some(&city), None, &stations));
        let outside = Vec2::new(50.0 + STATION_MIN_SEPARATION_M + 1.0, 50.0);
        assert!(placement_valid(outside, Some(&city), None, &stations));
    }
}
