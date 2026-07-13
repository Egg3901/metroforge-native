//! Routes panel: list, sort, edit, and bulk actions for `UiRoute`s.
//!
//! Kept in its own module (not `hud.rs`) so parallel HUD restyles merge
//! cleanly — `hud.rs` only needs `RoutePanelState::{open,selected}` for the
//! overcrowded-routes chip. The toolbar in `build_ui.rs` toggles `open`.
//!
//! Stop order mutations have no dedicated wire field on `EditRoute`, so
//! apply goes through `DeleteRoute` + `CreateRoute` (+ `BuildTrack` for
//! missing segments) and then restores name/fare/vehicles/color/headway via
//! `EditRoute` once the new id lands. Pause/resume is `vehicle_count = 0`
//! with the prior count remembered client-side (no `pauseRoute` command).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use mf_net::SimLink;
use mf_protocol::{Command, FrameSnapshot, ToastTone, TransitMode, UiRoute, UiStation};
use mf_state::{LatestFrame, LatestUi, RouteFocus};

use crate::audio::{PlaySfx, Sfx};
use crate::command_bus::{CmdMeta, CommandBus, CommandFeedback};
use crate::design_system as ds;
use crate::hud::ToastLog;
use crate::state::AppState;
use crate::tools::{ActiveTool, ToolState};

// ---------------------------------------------------------------------
// Pure helpers (unit-tested)
// ---------------------------------------------------------------------

/// Sort keys for the routes list. Crowding prefers live crowding when the
/// sidecar sends it, else the daily aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RouteSortKey {
    #[default]
    Crowding,
    Riders,
    NetIncome,
}

impl RouteSortKey {
    fn label(self) -> &'static str {
        let s = crate::strings::current();
        match self {
            RouteSortKey::Crowding => s.sort_crowding,
            RouteSortKey::Riders => s.sort_riders,
            RouteSortKey::NetIncome => s.sort_net_income,
        }
    }
}

/// Stable descending sort of route indices by `key`. Ties break on route id
/// ascending so the order does not flicker when two routes share a metric.
pub fn sort_route_indices(routes: &[UiRoute], key: RouteSortKey) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..routes.len()).collect();
    indices.sort_by(|&a, &b| {
        let cmp = match key {
            RouteSortKey::Crowding => {
                route_crowding(&routes[a]).total_cmp(&route_crowding(&routes[b]))
            }
            RouteSortKey::Riders => routes[a]
                .daily_ridership
                .total_cmp(&routes[b].daily_ridership),
            RouteSortKey::NetIncome => {
                route_net_income(&routes[a]).total_cmp(&route_net_income(&routes[b]))
            }
        };
        // Descending metric, then ascending id for stability.
        cmp.reverse().then_with(|| routes[a].id.cmp(&routes[b].id))
    });
    indices
}

fn route_crowding(route: &UiRoute) -> f64 {
    route.live_crowding.unwrap_or(route.crowding)
}

/// Net income/day when both sim-depth fields exist; otherwise `0.0` so
/// routes without the pair sort together at the bottom of a descending list
/// rather than inventing a fake profit.
fn route_net_income(route: &UiRoute) -> f64 {
    match (route.farebox, route.operating_cost) {
        (Some(farebox), Some(cost)) => farebox - cost,
        _ => 0.0,
    }
}

/// Move the element at `from` to index `to` (0-based). No-op when either
/// index is out of range or they are equal. Used by the stop-list drag
/// reorder and the ↑/↓ buttons.
pub fn reorder_index<T>(items: &mut Vec<T>, from: usize, to: usize) {
    if from == to || from >= items.len() || to >= items.len() {
        return;
    }
    let item = items.remove(from);
    items.insert(to, item);
}

/// Insert `station_id` into an ordered stop list. If it is already present,
/// remove it (toggle). Otherwise append. Returns whether the list changed.
/// Used by world-click editing and Shift+click multi-select.
pub fn toggle_station_in_order(stops: &mut Vec<i64>, station_id: i64) -> bool {
    if let Some(pos) = stops.iter().position(|&id| id == station_id) {
        stops.remove(pos);
        true
    } else {
        stops.push(station_id);
        true
    }
}

/// Insert `station_id` after `after` (or at the end when `after` is missing /
/// `None`). No-op duplicate of the immediate predecessor (same rule as the
/// route tool's draft append). Returns the new index when inserted.
pub fn insert_stop_after(
    stops: &mut Vec<i64>,
    station_id: i64,
    after: Option<i64>,
) -> Option<usize> {
    if stops.last() == Some(&station_id) {
        return None;
    }
    // Already on the route elsewhere: treat as a move to the insert point.
    if let Some(pos) = stops.iter().position(|&id| id == station_id) {
        stops.remove(pos);
    }
    let insert_at = match after {
        Some(aid) => stops
            .iter()
            .position(|&id| id == aid)
            .map(|i| i + 1)
            .unwrap_or(stops.len()),
        None => stops.len(),
    };
    stops.insert(insert_at, station_id);
    Some(insert_at)
}

/// Remove `station_id` from the stop list. Returns whether it was present.
pub fn remove_stop(stops: &mut Vec<i64>, station_id: i64) -> bool {
    if let Some(pos) = stops.iter().position(|&id| id == station_id) {
        stops.remove(pos);
        true
    } else {
        false
    }
}

fn format_thousands(value: f64) -> String {
    let rounded = value.round().max(0.0) as u64;
    let digits = rounded.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped
}

fn format_cash(value: f64) -> String {
    format!("${}", format_thousands(value))
}

fn format_fare(value: f64) -> String {
    format!("${:.2}", value.max(0.0))
}

fn mode_word(mode: TransitMode) -> &'static str {
    let s = crate::strings::current();
    match mode {
        TransitMode::Bus => s.mode_bus,
        TransitMode::Tram => s.mode_tram,
        TransitMode::Metro => s.mode_metro,
        TransitMode::Rail => s.mode_rail,
    }
}

fn mode_icon(mode: TransitMode) -> ds::IconKind {
    match mode {
        TransitMode::Bus => ds::IconKind::Bus,
        TransitMode::Tram => ds::IconKind::Tram,
        // Metro/rail share the route-line glyph until dedicated icons land.
        TransitMode::Metro | TransitMode::Rail => ds::IconKind::RouteLine,
    }
}

/// Same eight bricks as `mf-render`'s vivid table / the old `build_ui`
/// swatches — index matches the 3D stripe color for the route's list slot.
const ROUTE_COLORS: [egui::Color32; 8] = [
    egui::Color32::from_rgb(0xff, 0x3b, 0x30),
    egui::Color32::from_rgb(0x00, 0x7a, 0xff),
    egui::Color32::from_rgb(0xff, 0xcc, 0x00),
    egui::Color32::from_rgb(0x34, 0xc7, 0x59),
    egui::Color32::from_rgb(0xff, 0x95, 0x00),
    egui::Color32::from_rgb(0xaf, 0x52, 0xde),
    egui::Color32::from_rgb(0x00, 0xc7, 0xbe),
    egui::Color32::from_rgb(0xff, 0x2d, 0x95),
];

const ROUTE_COLOR_HEX: [&str; 8] = [
    "#ff3b30", "#007aff", "#ffcc00", "#34c759", "#ff9500", "#af52de", "#00c7be", "#ff2d95",
];

fn vivid_route_color(idx: usize) -> egui::Color32 {
    ROUTE_COLORS[idx % ROUTE_COLORS.len()]
}

fn color_index_from_hex(hex: &str) -> Option<usize> {
    let normalized = hex.trim().to_ascii_lowercase();
    ROUTE_COLOR_HEX
        .iter()
        .position(|c| c.eq_ignore_ascii_case(&normalized))
}

fn next_color_index(current_hex: &str, route_list_idx: usize) -> usize {
    let cur = color_index_from_hex(current_hex).unwrap_or(route_list_idx % ROUTE_COLORS.len());
    (cur + 1) % ROUTE_COLORS.len()
}

fn hover_tick(resp: &egui::Response, last: &mut Option<egui::Id>, sfx: &mut EventWriter<PlaySfx>) {
    if resp.hovered() {
        if *last != Some(resp.id) {
            sfx.write(PlaySfx(Sfx::Hover));
            *last = Some(resp.id);
        }
    } else if *last == Some(resp.id) {
        *last = None;
    }
}

// ---------------------------------------------------------------------
// Route line-diagram widget (reusable; service panel will share this)
// ---------------------------------------------------------------------

const DIAGRAM_HEIGHT: f32 = ds::SPACE_XXL;
const DIAGRAM_H_MARGIN: f32 = ds::SPACE_SM;
const DIAGRAM_TICK_HALF: f32 = 5.0;
const DIAGRAM_MIN_THICKNESS: f32 = 2.0;
const DIAGRAM_MAX_THICKNESS: f32 = 8.0;
const DIAGRAM_VEHICLE_RADIUS: f32 = 3.5;
const DIAGRAM_TRANSFER_RING_RADIUS: f32 = 7.0;
const DIAGRAM_LABEL_CAP: usize = 12;
const DIAGRAM_HIT_RADIUS: f32 = 10.0;

/// Evenly spaced station x-offsets (from the strip's left edge) for
/// `station_count` stops across a strip of `width` px inset by `margin`.
pub fn diagram_station_offsets(width: f32, margin: f32, station_count: usize) -> Vec<f32> {
    if station_count == 0 {
        return Vec::new();
    }
    let usable = (width - margin * 2.0).max(0.0);
    if station_count == 1 {
        return vec![margin + usable * 0.5];
    }
    let step = usable / (station_count as f32 - 1.0);
    (0..station_count)
        .map(|i| margin + step * i as f32)
        .collect()
}

/// Map normalized route progress `0.0..=1.0` onto the diagram's x-offset
/// range implied by `offsets` (even station spacing).
pub fn diagram_progress_to_x(progress: f32, offsets: &[f32]) -> Option<f32> {
    if offsets.is_empty() {
        return None;
    }
    if offsets.len() == 1 {
        return Some(offsets[0]);
    }
    let t = progress.clamp(0.0, 1.0);
    Some(offsets[0] + t * (offsets[offsets.len() - 1] - offsets[0]))
}

/// Stations that appear on more than one route (transfer hubs).
pub fn transfer_station_ids(routes: &[UiRoute]) -> HashSet<i64> {
    let mut counts: HashMap<i64, u32> = HashMap::new();
    for route in routes {
        for &sid in &route.station_ids {
            *counts.entry(sid).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .filter(|&(_, c)| c > 1)
        .map(|(id, _)| id)
        .collect()
}

fn diagram_segment_thicknesses(segment_loads: &[f64]) -> Vec<f32> {
    if segment_loads.is_empty() {
        return Vec::new();
    }
    let max_load = segment_loads.iter().cloned().fold(0.0_f64, f64::max);
    if max_load <= 0.0 {
        return vec![DIAGRAM_MIN_THICKNESS; segment_loads.len()];
    }
    segment_loads
        .iter()
        .map(|&load| {
            let t = (load / max_load).clamp(0.0, 1.0) as f32;
            DIAGRAM_MIN_THICKNESS + t * (DIAGRAM_MAX_THICKNESS - DIAGRAM_MIN_THICKNESS)
        })
        .collect()
}

fn closest_on_segment(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> (f64, f64) {
    let dx = bx - ax;
    let dy = by - ay;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-12 {
        let d = (px - ax).hypot(py - ay);
        return (0.0, d * d);
    }
    let t = ((px - ax) * dx + (py - ay) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    let d = (px - cx).hypot(py - cy);
    (t, d * d)
}

/// Project a world point onto the route polyline; returns normalized progress
/// `0.0` at the first stop and `1.0` at the last.
pub fn route_progress_at(
    station_ids: &[i64],
    station_coords: &HashMap<i64, (f64, f64)>,
    x: f64,
    y: f64,
) -> Option<f32> {
    if station_ids.is_empty() {
        return None;
    }
    if station_ids.len() == 1 {
        return Some(0.5);
    }
    let mut seg_lengths = Vec::with_capacity(station_ids.len() - 1);
    let mut total = 0.0_f64;
    for pair in station_ids.windows(2) {
        let (ax, ay) = *station_coords.get(&pair[0])?;
        let (bx, by) = *station_coords.get(&pair[1])?;
        let len = (bx - ax).hypot(by - ay).max(1e-9);
        seg_lengths.push(len);
        total += len;
    }
    if total <= 0.0 {
        return Some(0.5);
    }

    let mut best_dist = f64::INFINITY;
    let mut best_progress = 0.0_f64;
    let mut cum = 0.0_f64;
    for (i, pair) in station_ids.windows(2).enumerate() {
        let (ax, ay) = station_coords[&pair[0]];
        let (bx, by) = station_coords[&pair[1]];
        let (t, dist_sq) = closest_on_segment(x, y, ax, ay, bx, by);
        if dist_sq < best_dist {
            best_dist = dist_sq;
            best_progress = (cum + t * seg_lengths[i]) / total;
        }
        cum += seg_lengths[i];
    }
    Some(best_progress.clamp(0.0, 1.0) as f32)
}

/// Live vehicle progress values `0.0..=1.0` along `route_index` (wire
/// `routeColorIdx` matches the index in `UiState.routes`).
pub fn vehicle_progresses_on_route(
    route_index: usize,
    station_ids: &[i64],
    station_coords: &HashMap<i64, (f64, f64)>,
    frame: &FrameSnapshot,
) -> Vec<f32> {
    let mut out = Vec::new();
    for chunk in frame.vehicles.chunks_exact(6) {
        let color_idx = chunk[5] as usize;
        if color_idx != route_index {
            continue;
        }
        let vx = chunk[1] as f64;
        let vy = chunk[2] as f64;
        if let Some(p) = route_progress_at(station_ids, station_coords, vx, vy) {
            out.push(p);
        }
    }
    out
}

fn station_coord_map(stations: &[UiStation]) -> HashMap<i64, (f64, f64)> {
    stations.iter().map(|s| (s.id, (s.x, s.y))).collect()
}

/// Inputs for [`draw_route_line_diagram`]. Shared by the routes panel and
/// future service panels.
pub struct RouteLineDiagram<'a> {
    pub color: egui::Color32,
    pub route_id: i64,
    pub route_index: usize,
    pub station_ids: &'a [i64],
    pub all_routes: &'a [UiRoute],
    pub stations: &'a [UiStation],
    pub segment_loads: &'a [f64],
    pub frame: Option<&'a FrameSnapshot>,
}

/// Horizontal metro-map strip: route-colored line, per-station tick marks,
/// transfer rings, live vehicle dots, and station names on hover.
pub fn draw_route_line_diagram(ui: &mut egui::Ui, diagram: &RouteLineDiagram<'_>) {
    let RouteLineDiagram {
        color,
        route_id,
        route_index,
        station_ids,
        all_routes,
        stations,
        segment_loads,
        frame,
    } = *diagram;
    let station_count = station_ids.len();
    let desired_size = egui::vec2(ui.available_width(), DIAGRAM_HEIGHT);
    let (rect, _strip) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    if station_count == 0 {
        return;
    }

    let painter = ui.painter_at(rect);
    let line_y = rect.center().y;
    let offsets = diagram_station_offsets(rect.width(), DIAGRAM_H_MARGIN, station_count);
    let transfers = transfer_station_ids(all_routes);
    let labels = route_station_labels_for(station_ids, stations);
    let coords = station_coord_map(stations);

    if station_count > 1 {
        let thicknesses = diagram_segment_thicknesses(segment_loads);
        let aligned = thicknesses.len() == station_count - 1;
        for i in 0..station_count - 1 {
            let thickness = if aligned {
                thicknesses[i]
            } else {
                DIAGRAM_MIN_THICKNESS
            };
            let a = egui::pos2(rect.left() + offsets[i], line_y);
            let b = egui::pos2(rect.left() + offsets[i + 1], line_y);
            painter.line_segment([a, b], egui::Stroke::new(thickness, color));
        }
    }

    for (i, &off) in offsets.iter().enumerate() {
        let center = egui::pos2(rect.left() + off, line_y);
        let tick_top = egui::pos2(center.x, line_y - DIAGRAM_TICK_HALF);
        let tick_bot = egui::pos2(center.x, line_y + DIAGRAM_TICK_HALF);
        painter.line_segment([tick_top, tick_bot], egui::Stroke::new(2.0, color));

        let sid = station_ids[i];
        if transfers.contains(&sid) {
            painter.circle_stroke(
                center,
                DIAGRAM_TRANSFER_RING_RADIUS,
                egui::Stroke::new(1.5, ds::text()),
            );
        }

        let hit = egui::Rect::from_center_size(
            center,
            egui::vec2(DIAGRAM_HIT_RADIUS * 2.0, DIAGRAM_HIT_RADIUS * 2.0),
        );
        let resp = ui.interact(
            hit,
            ui.id().with(("route_diagram_stop", route_id, i)),
            egui::Sense::hover(),
        );
        if let Some(label) = labels.get(i) {
            resp.on_hover_text(label);
        }
    }

    if station_count > DIAGRAM_LABEL_CAP {
        let caption = crate::strings::current().route_diagram_stops(station_count);
        painter.text(
            egui::pos2(rect.left(), rect.bottom() - ds::SPACE_XXS),
            egui::Align2::LEFT_BOTTOM,
            caption,
            ds::body_font(ds::TEXT_XS),
            ds::muted(),
        );
    }

    if let Some(frame) = frame {
        let progresses = vehicle_progresses_on_route(route_index, station_ids, &coords, frame);
        for progress in progresses {
            if let Some(off) = diagram_progress_to_x(progress, &offsets) {
                let pos = egui::pos2(rect.left() + off, line_y);
                painter.circle_filled(pos, DIAGRAM_VEHICLE_RADIUS, ds::text());
                painter.circle_stroke(pos, DIAGRAM_VEHICLE_RADIUS, egui::Stroke::new(1.0, color));
            }
        }
    }
}

// ---------------------------------------------------------------------
// Panel state
// ---------------------------------------------------------------------

/// Props to restore onto a freshly recreated route after a stop-order apply
/// (`DeleteRoute` + `CreateRoute` loses the old id and its EditRoute fields).
#[derive(Clone, Debug)]
struct PendingRouteRestore {
    name: String,
    fare: f64,
    vehicle_count: u32,
    headway_seconds: f64,
    color: String,
    /// Seq of the in-flight `CreateRoute` we are waiting on.
    create_seq: u32,
}

#[derive(Resource, Default)]
pub struct RoutePanelState {
    pub open: bool,
    pub selected: Option<i64>,
    edit_for: Option<i64>,
    name_edit: String,
    fare_edit: f64,
    delete_armed: Option<i64>,
    sort_key: RouteSortKey,
    /// Local stop-order draft while editing. `None` means "mirror live
    /// `UiRoute.station_ids`". Dirty when it differs from the live list.
    pub(crate) edit_stops: Option<Vec<i64>>,
    /// Prior vehicle counts for routes the player paused (`vehicle_count=0`).
    paused_counts: HashMap<i64, u32>,
    /// In-flight stop-order apply waiting on `CreateRoute`'s `created_id`.
    pending_restore: Option<PendingRouteRestore>,
    /// Index being dragged in the stop list, if any.
    drag_from: Option<usize>,
}

impl RoutePanelState {
    fn select(&mut self, id: Option<i64>) {
        if self.selected != id {
            self.selected = id;
            self.edit_for = None;
            self.delete_armed = None;
            self.edit_stops = None;
            self.drag_from = None;
        }
    }

    fn stops_for<'a>(&'a self, route: &'a UiRoute) -> &'a [i64] {
        self.edit_stops
            .as_deref()
            .unwrap_or(route.station_ids.as_slice())
    }

    fn ensure_edit_stops(&mut self, route: &UiRoute) -> &mut Vec<i64> {
        if self.edit_stops.is_none() {
            self.edit_stops = Some(route.station_ids.clone());
        }
        self.edit_stops.as_mut().expect("just seeded")
    }

    fn stops_dirty(&self, route: &UiRoute) -> bool {
        match &self.edit_stops {
            Some(draft) => draft.as_slice() != route.station_ids.as_slice(),
            None => false,
        }
    }
}

// ---------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------

fn sync_route_focus_system(panel: Res<RoutePanelState>, mut focus: ResMut<RouteFocus>) {
    let editing = panel.selected.is_some() && panel.open;
    let desired = if panel.open { panel.selected } else { None };
    if focus.route_id != desired || focus.editing != editing {
        match desired {
            Some(id) => focus.focus(id, editing),
            None => focus.clear(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn route_panel_system(
    mut contexts: EguiContexts,
    ui_state: Res<LatestUi>,
    frame: Res<LatestFrame>,
    link: Option<Res<SimLink>>,
    mut bus: ResMut<CommandBus>,
    mut panel: ResMut<RoutePanelState>,
    mut tools: ResMut<ToolState>,
    mut sfx: EventWriter<PlaySfx>,
    mut hovered: Local<Option<egui::Id>>,
) -> Result {
    if !panel.open {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let Some(state) = &ui_state.0 else {
        return Ok(());
    };

    egui::SidePanel::right("routes_panel")
        .frame(
            egui::Frame::default()
                .fill(ds::panel_bg())
                .inner_margin(egui::Margin::symmetric(
                    ds::SPACE_SM as i8,
                    ds::SPACE_SM as i8,
                )),
        )
        .default_width(320.0)
        .min_width(260.0)
        .resizable(true)
        .show(ctx, |ui| {
            let s = crate::strings::current();
            ui.horizontal(|ui| {
                ui.label(ds::heading(s.routes));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let close = ui.small_button(s.close);
                    hover_tick(&close, &mut hovered, &mut sfx);
                    if close.clicked() {
                        panel.open = false;
                        panel.select(None);
                    }
                });
            });
            ui.add_space(ds::SPACE_XS);

            if state.routes.is_empty() {
                ui.label(ds::label_muted(s.no_routes_panel_hint));
                ui.add_space(ds::SPACE_XS);
                ui.label(ds::label_small(s.multi_select_hint));
                return;
            }

            if let Some(selected) = panel.selected {
                if !state.routes.iter().any(|r| r.id == selected) {
                    panel.select(None);
                }
            }

            // Sort controls
            ui.horizontal(|ui| {
                ui.label(ds::label_muted(s.sort));
                for key in [
                    RouteSortKey::Crowding,
                    RouteSortKey::Riders,
                    RouteSortKey::NetIncome,
                ] {
                    let selected = panel.sort_key == key;
                    let resp = ui.selectable_label(selected, key.label());
                    hover_tick(&resp, &mut hovered, &mut sfx);
                    if resp.clicked() {
                        panel.sort_key = key;
                        sfx.write(PlaySfx(Sfx::Confirm));
                    }
                }
            });
            ui.add_space(ds::SPACE_XS);
            ui.separator();
            ui.add_space(ds::SPACE_XS);

            let order = sort_route_indices(&state.routes, panel.sort_key);
            let frame_snap = frame.0.as_deref();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for &idx in &order {
                    let route = &state.routes[idx];
                    let is_selected = panel.selected == Some(route.id);
                    draw_route_row(
                        ui,
                        route,
                        idx,
                        is_selected,
                        &mut panel,
                        &mut bus,
                        link.as_deref(),
                        &mut tools,
                        &state.stations,
                        &state.tracks,
                        &state.routes,
                        frame_snap,
                        &mut hovered,
                        &mut sfx,
                    );
                }
            });
        });

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_route_row(
    ui: &mut egui::Ui,
    route: &UiRoute,
    list_idx: usize,
    is_selected: bool,
    panel: &mut RoutePanelState,
    bus: &mut CommandBus,
    link: Option<&SimLink>,
    tools: &mut ToolState,
    stations: &[UiStation],
    tracks: &[mf_protocol::UiTrack],
    all_routes: &[UiRoute],
    frame: Option<&FrameSnapshot>,
    hovered: &mut Option<egui::Id>,
    sfx: &mut EventWriter<PlaySfx>,
) {
    let color = vivid_route_color(list_idx);
    ui.horizontal(|ui| {
        let (swatch_rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(swatch_rect, egui::CornerRadius::same(2), color);

        let (icon_rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
        ds::icon(
            ui.painter(),
            icon_rect,
            mode_icon(route.mode),
            ds::muted(),
            1.4,
        );

        let display_name = if route.name.trim().is_empty() {
            crate::strings::current().line(list_idx + 1)
        } else {
            route.name.clone()
        };
        let resp = ui.selectable_label(is_selected, display_name);
        hover_tick(&resp, hovered, sfx);
        if resp.clicked() {
            if is_selected {
                panel.select(None);
            } else {
                panel.select(Some(route.id));
            }
            sfx.write(PlaySfx(Sfx::Confirm));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(crowding) = route.live_crowding.or(Some(route.crowding)) {
                let (dot, dot_resp) =
                    ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    dot,
                    egui::CornerRadius::same(5),
                    ds::crowding_color(crowding),
                );
                dot_resp.on_hover_text(
                    crate::strings::current().crowding_pct(crowding.clamp(0.0, 1.0) * 100.0),
                );
            }
        });
    });

    let paused = route.vehicle_count == 0;
    ui.label(ds::label_small(
        crate::strings::current().route_row_subtitle(
            route.station_ids.len(),
            &format_thousands(route.daily_ridership),
            mode_word(route.mode),
            paused,
        ),
    ));

    if is_selected {
        route_editor(
            ui, route, list_idx, stations, tracks, all_routes, frame, color, panel, bus, link,
            tools, hovered, sfx,
        );
    }

    ui.add_space(ds::SPACE_XXS);
    ui.separator();
    ui.add_space(ds::SPACE_XXS);
}

#[allow(clippy::too_many_arguments)]
fn route_editor(
    ui: &mut egui::Ui,
    route: &UiRoute,
    list_idx: usize,
    stations: &[UiStation],
    tracks: &[mf_protocol::UiTrack],
    all_routes: &[UiRoute],
    frame: Option<&FrameSnapshot>,
    color: egui::Color32,
    panel: &mut RoutePanelState,
    bus: &mut CommandBus,
    link: Option<&SimLink>,
    tools: &mut ToolState,
    hovered: &mut Option<egui::Id>,
    sfx: &mut EventWriter<PlaySfx>,
) {
    if panel.edit_for != Some(route.id) {
        panel.name_edit = route.name.clone();
        panel.fare_edit = route.fare;
        panel.edit_for = Some(route.id);
        panel.delete_armed = None;
        panel.edit_stops = None;
        panel.drag_from = None;
    }

    ui.add_space(ds::SPACE_XXS);

    let loads = if panel.stops_dirty(route) {
        // Draft order: diagram without load weights (segment_loads align to
        // the live order only).
        vec![0.0; panel.stops_for(route).len().saturating_sub(1)]
    } else {
        route.segment_loads.clone()
    };
    draw_route_line_diagram(
        ui,
        &RouteLineDiagram {
            color,
            route_id: route.id,
            route_index: list_idx,
            station_ids: panel.stops_for(route),
            all_routes,
            stations,
            segment_loads: &loads,
            frame,
        },
    );
    ui.add_space(ds::SPACE_XXS);

    let s = crate::strings::current();
    if let Some(crowding) = route.live_crowding {
        ui.horizontal(|ui| {
            ui.label(ds::label_muted(s.live_crowding));
            let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().rect_filled(
                dot,
                egui::CornerRadius::same(5),
                ds::crowding_color(crowding),
            );
            ui.label(
                ds::value_strong(format!("{:.0}%", crowding.clamp(0.0, 1.0) * 100.0))
                    .color(ds::crowding_color(crowding)),
            );
        });
    }
    if let Some(farebox) = route.farebox {
        ui.horizontal(|ui| {
            ui.label(ds::label_muted(s.farebox_per_day));
            ui.label(ds::value_strong(format_cash(farebox)).color(ds::GOOD));
        });
    }
    if let Some(cost) = route.operating_cost {
        ui.horizontal(|ui| {
            ui.label(ds::label_muted(s.operating_cost_per_day));
            ui.label(ds::value_strong(format_cash(cost)).color(ds::BAD));
        });
    }
    if let (Some(farebox), Some(cost)) = (route.farebox, route.operating_cost) {
        let net = farebox - cost;
        let good = net >= 0.0;
        let prefix = if good { "+" } else { "-" };
        ui.horizontal(|ui| {
            ui.label(ds::label_muted(s.net_per_day));
            ui.label(
                ds::value_strong(format!("{prefix}{}", format_cash(net.abs()))).color(if good {
                    ds::GOOD
                } else {
                    ds::BAD
                }),
            );
        });
    }
    ui.add_space(ds::SPACE_XXS);

    // Stop list with drag reorder + numbered stops
    ui.label(ds::label_muted(s.stops));
    ui.label(ds::label_small(s.stops_edit_hint));
    draw_stop_list(ui, route, stations, panel, hovered, sfx);
    if panel.stops_dirty(route) {
        ui.horizontal(|ui| {
            let apply = ui.add(
                egui::Button::new(
                    egui::RichText::new(s.apply_stop_order).color(egui::Color32::WHITE),
                )
                .fill(ds::accent())
                .corner_radius(ds::CORNER_RADIUS),
            );
            hover_tick(&apply, hovered, sfx);
            if apply.clicked() {
                apply_stop_order(panel, route, tracks, bus, link, sfx);
            }
            let revert = ui.small_button(s.revert);
            hover_tick(&revert, hovered, sfx);
            if revert.clicked() {
                panel.edit_stops = None;
                panel.drag_from = None;
            }
        });
    }
    ui.add_space(ds::SPACE_XXS);

    // Enter world edit mode: Route tool seeded with current stops so clicks
    // toggle membership against the draft.
    ui.horizontal(|ui| {
        let edit_world = ui.small_button(s.edit_stops_in_world);
        hover_tick(&edit_world, hovered, sfx);
        if edit_world.clicked() {
            let stops = panel.stops_for(route).to_vec();
            tools.active = ActiveTool::Route;
            tools.route_mode = route.mode;
            tools.route_draft = stops.clone();
            tools.editing_route_id = Some(route.id);
            panel.ensure_edit_stops(route);
            sfx.write(PlaySfx(Sfx::Confirm));
        }
    });
    ui.add_space(ds::SPACE_XXS);

    // Vehicles
    ui.horizontal(|ui| {
        ui.label(ds::label_muted(s.vehicles));
        let minus = ui.small_button("-");
        hover_tick(&minus, hovered, sfx);
        if minus.clicked() && route.vehicle_count > 0 {
            submit_edit(
                bus,
                link,
                route.id,
                EditFields {
                    vehicle_count: Some(route.vehicle_count - 1),
                    ..Default::default()
                },
            );
        }
        ui.label(ds::value_strong(route.vehicle_count.to_string()));
        let plus = ui.small_button("+");
        hover_tick(&plus, hovered, sfx);
        if plus.clicked() {
            submit_edit(
                bus,
                link,
                route.id,
                EditFields {
                    vehicle_count: Some(route.vehicle_count + 1),
                    ..Default::default()
                },
            );
        }
    });

    // Fare
    ui.horizontal(|ui| {
        ui.label(ds::label_muted(s.fare));
        let resp = ui.add(
            egui::DragValue::new(&mut panel.fare_edit)
                .range(0.0..=50.0)
                .speed(0.02)
                .prefix("$")
                .fixed_decimals(2),
        );
        if resp.drag_stopped() || (resp.lost_focus() && !resp.dragged()) {
            submit_edit(
                bus,
                link,
                route.id,
                EditFields {
                    fare: Some(panel.fare_edit),
                    ..Default::default()
                },
            );
        }
        ui.label(ds::label_small(s.now_fare(&format_fare(route.fare))));
    });

    // Name
    ui.horizontal(|ui| {
        ui.label(ds::label_muted(s.name));
        let resp = ui.add(egui::TextEdit::singleline(&mut panel.name_edit).desired_width(140.0));
        if resp.lost_focus() {
            let trimmed = panel.name_edit.trim();
            if !trimmed.is_empty() && trimmed != route.name {
                submit_edit(
                    bus,
                    link,
                    route.id,
                    EditFields {
                        name: Some(trimmed.to_string()),
                        ..Default::default()
                    },
                );
            }
        }
    });

    ui.add_space(ds::SPACE_XXS);

    // Bulk actions: pause/resume, color, delete
    ui.horizontal(|ui| {
        let paused = route.vehicle_count == 0;
        let pause_label = if paused { s.resume } else { s.pause };
        let pause_resp = ui.small_button(pause_label);
        hover_tick(&pause_resp, hovered, sfx);
        if pause_resp.clicked() {
            if paused {
                let restore = panel.paused_counts.remove(&route.id).unwrap_or(1).max(1);
                submit_edit(
                    bus,
                    link,
                    route.id,
                    EditFields {
                        vehicle_count: Some(restore),
                        ..Default::default()
                    },
                );
            } else {
                panel
                    .paused_counts
                    .insert(route.id, route.vehicle_count.max(1));
                submit_edit(
                    bus,
                    link,
                    route.id,
                    EditFields {
                        vehicle_count: Some(0),
                        ..Default::default()
                    },
                );
            }
            sfx.write(PlaySfx(Sfx::Confirm));
        }

        let color_resp = ui.small_button(s.next_color);
        hover_tick(&color_resp, hovered, sfx);
        if color_resp.clicked() {
            let next = next_color_index(&route.color, list_idx);
            submit_edit(
                bus,
                link,
                route.id,
                EditFields {
                    color: Some(ROUTE_COLOR_HEX[next].to_string()),
                    ..Default::default()
                },
            );
            sfx.write(PlaySfx(Sfx::Confirm));
        }
    });

    ui.add_space(ds::SPACE_XXS);
    let armed = panel.delete_armed == Some(route.id);
    let delete_resp = ui.add(
        egui::Button::new(
            egui::RichText::new(if armed {
                s.confirm_delete
            } else {
                s.delete_route
            })
            .color(if armed {
                egui::Color32::WHITE
            } else {
                ds::text()
            }),
        )
        .fill(if armed { ds::BAD } else { ds::inactive_bg() })
        .corner_radius(ds::CORNER_RADIUS),
    );
    hover_tick(&delete_resp, hovered, sfx);
    if delete_resp.clicked() {
        if armed {
            if let Some(link) = link {
                bus.submit(
                    link,
                    Command::DeleteRoute { route_id: route.id },
                    CmdMeta::EditRoute { route_id: route.id },
                );
            }
            panel.delete_armed = None;
            panel.select(None);
            tools.editing_route_id = None;
        } else {
            panel.delete_armed = Some(route.id);
        }
    }
}

fn draw_stop_list(
    ui: &mut egui::Ui,
    route: &UiRoute,
    stations: &[UiStation],
    panel: &mut RoutePanelState,
    hovered: &mut Option<egui::Id>,
    sfx: &mut EventWriter<PlaySfx>,
) {
    let stops = panel.stops_for(route).to_vec();
    let by_id: HashMap<i64, &UiStation> = stations.iter().map(|s| (s.id, s)).collect();
    let mut reorder: Option<(usize, usize)> = None;
    let mut remove: Option<i64> = None;

    for (i, &sid) in stops.iter().enumerate() {
        let label = match by_id.get(&sid) {
            Some(st) if !st.name.trim().is_empty() => st.name.clone(),
            _ => format!("S{}", i + 1),
        };
        ui.horizontal(|ui| {
            let handle = ui.add(
                egui::Label::new(ds::label_muted(format!("{}.", i + 1))).sense(egui::Sense::drag()),
            );
            if handle.drag_started() {
                panel.drag_from = Some(i);
            }
            if handle.dragged() {
                panel.drag_from = Some(i);
            }
            if handle.drag_stopped() {
                if let (Some(from), Some(pointer)) =
                    (panel.drag_from, ui.ctx().pointer_interact_pos())
                {
                    // Drop onto whichever row the pointer is over by Y.
                    let row_h = handle.rect.height().max(1.0);
                    let top = handle.rect.top() - (i as f32) * row_h;
                    let target = ((pointer.y - top) / row_h).floor() as isize;
                    let to = target.clamp(0, (stops.len().saturating_sub(1)) as isize) as usize;
                    if from != to {
                        reorder = Some((from, to));
                    }
                }
                panel.drag_from = None;
            }

            ui.label(ds::label_body(label));

            let up = ui.small_button("↑");
            hover_tick(&up, hovered, sfx);
            if up.clicked() && i > 0 {
                reorder = Some((i, i - 1));
            }
            let down = ui.small_button("↓");
            hover_tick(&down, hovered, sfx);
            if down.clicked() && i + 1 < stops.len() {
                reorder = Some((i, i + 1));
            }
            let rm = ui.small_button("×");
            hover_tick(&rm, hovered, sfx);
            if rm.clicked() {
                remove = Some(sid);
            }
        });
    }

    if let Some((from, to)) = reorder {
        let draft = panel.ensure_edit_stops(route);
        reorder_index(draft, from, to);
        sfx.write(PlaySfx(Sfx::Confirm));
    }
    if let Some(sid) = remove {
        let draft = panel.ensure_edit_stops(route);
        remove_stop(draft, sid);
        sfx.write(PlaySfx(Sfx::Confirm));
    }
}

fn route_station_labels_for(stops: &[i64], stations: &[UiStation]) -> Vec<String> {
    let by_id: HashMap<i64, &UiStation> = stations.iter().map(|s| (s.id, s)).collect();
    stops
        .iter()
        .enumerate()
        .map(|(i, id)| match by_id.get(id) {
            Some(st) if !st.name.trim().is_empty() => st.name.clone(),
            _ => format!("S{}", i + 1),
        })
        .collect()
}

fn missing_track_pairs_for(draft: &[i64], tracks: &[mf_protocol::UiTrack]) -> Vec<(i64, i64)> {
    draft
        .windows(2)
        .map(|pair| (pair[0], pair[1]))
        .filter(|(a, b)| {
            !tracks.iter().any(|t| {
                (t.from_station_id == *a && t.to_station_id == *b)
                    || (t.from_station_id == *b && t.to_station_id == *a)
            })
        })
        .collect()
}

fn apply_stop_order(
    panel: &mut RoutePanelState,
    route: &UiRoute,
    tracks: &[mf_protocol::UiTrack],
    bus: &mut CommandBus,
    link: Option<&SimLink>,
    sfx: &mut EventWriter<PlaySfx>,
) {
    let Some(link) = link else { return };
    let Some(draft) = panel.edit_stops.clone() else {
        return;
    };
    if draft.len() < 2 {
        return;
    }
    if draft.as_slice() == route.station_ids.as_slice() {
        panel.edit_stops = None;
        return;
    }

    for (a, b) in missing_track_pairs_for(&draft, tracks) {
        let _ = bus.submit(
            link,
            Command::BuildTrack {
                mode: route.mode,
                grade: mf_protocol::TrackGrade::Surface,
                from_station_id: a,
                to_station_id: b,
                waypoints: Vec::new(),
            },
            CmdMeta::BuildTrack { from: a, to: b },
        );
    }

    bus.submit(
        link,
        Command::DeleteRoute { route_id: route.id },
        CmdMeta::EditRoute { route_id: route.id },
    );

    let create_seq = bus.submit(
        link,
        Command::CreateRoute {
            mode: route.mode,
            station_ids: draft.clone(),
        },
        CmdMeta::CreateRoute {
            mode: route.mode,
            station_ids: draft,
        },
    );

    panel.pending_restore = Some(PendingRouteRestore {
        name: route.name.clone(),
        fare: route.fare,
        vehicle_count: route.vehicle_count,
        headway_seconds: route.headway_seconds,
        color: route.color.clone(),
        create_seq,
    });
    panel.edit_stops = None;
    panel.select(None);
    sfx.write(PlaySfx(Sfx::Confirm));
}

#[derive(Default)]
struct EditFields {
    fare: Option<f64>,
    vehicle_count: Option<u32>,
    name: Option<String>,
    color: Option<String>,
    headway_seconds: Option<f64>,
}

fn submit_edit(bus: &mut CommandBus, link: Option<&SimLink>, route_id: i64, fields: EditFields) {
    let Some(link) = link else { return };
    bus.submit(
        link,
        Command::EditRoute {
            route_id,
            headway_seconds: fields.headway_seconds,
            fare: fields.fare,
            vehicle_count: fields.vehicle_count,
            name: fields.name,
            color: fields.color,
        },
        CmdMeta::EditRoute { route_id },
    );
}

fn route_panel_feedback_system(
    mut feedback: EventReader<CommandFeedback>,
    mut panel: ResMut<RoutePanelState>,
    mut bus: ResMut<CommandBus>,
    link: Option<Res<SimLink>>,
    mut toasts: ResMut<ToastLog>,
    mut sfx: EventWriter<PlaySfx>,
) {
    for fb in feedback.read() {
        if !fb.ok {
            // Only toast failures that belong to our pending restore / edits
            // when we have a pending restore matching this seq, or EditRoute.
            let ours = panel
                .pending_restore
                .as_ref()
                .is_some_and(|p| p.create_seq == fb.seq)
                || matches!(
                    fb.meta,
                    CmdMeta::EditRoute { .. } | CmdMeta::CreateRoute { .. }
                );
            if ours {
                let s = crate::strings::current();
                let detail = fb.error.as_deref().unwrap_or(s.unknown_error);
                push_toast(&mut toasts, s.route_update_failed(detail), ToastTone::Warn);
                sfx.write(PlaySfx(Sfx::Error));
            }
            if panel
                .pending_restore
                .as_ref()
                .is_some_and(|p| p.create_seq == fb.seq)
            {
                panel.pending_restore = None;
            }
            continue;
        }

        if let Some(pending) = panel.pending_restore.as_ref() {
            if pending.create_seq == fb.seq {
                if let Some(new_id) = fb.created_id {
                    if let Some(link) = link.as_deref() {
                        bus.submit(
                            link,
                            Command::EditRoute {
                                route_id: new_id,
                                headway_seconds: Some(pending.headway_seconds),
                                fare: Some(pending.fare),
                                vehicle_count: Some(pending.vehicle_count),
                                name: if pending.name.trim().is_empty() {
                                    None
                                } else {
                                    Some(pending.name.clone())
                                },
                                color: Some(pending.color.clone()),
                            },
                            CmdMeta::EditRoute { route_id: new_id },
                        );
                    }
                    panel.pending_restore = None;
                    panel.select(Some(new_id));
                    panel.open = true;
                    sfx.write(PlaySfx(Sfx::Confirm));
                } else {
                    panel.pending_restore = None;
                }
            }
        }
    }
}

const TOAST_LOG_CAP: usize = 20;

fn push_toast(toasts: &mut ToastLog, message: String, tone: ToastTone) {
    toasts.0.push((message, tone));
    if toasts.0.len() > TOAST_LOG_CAP {
        let excess = toasts.0.len() - TOAST_LOG_CAP;
        toasts.0.drain(0..excess);
    }
}

// ---------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------

pub struct MfRoutesPanelPlugin;

impl Plugin for MfRoutesPanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoutePanelState>()
            .add_systems(
                EguiPrimaryContextPass,
                (route_panel_system, sync_route_focus_system)
                    .chain()
                    .run_if(in_state(AppState::InGame))
                    .run_if(|| !crate::design_system::hud_hidden()),
            )
            .add_systems(
                Update,
                route_panel_feedback_system.run_if(in_state(AppState::InGame)),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_route(
        id: i64,
        crowding: f64,
        riders: f64,
        farebox: Option<f64>,
        cost: Option<f64>,
    ) -> UiRoute {
        UiRoute {
            on_time_pct: None,
            avg_delay_sec: None,
            in_service_vehicles: None,
            frequency: None,
            peak_units_required: None,
            avg_effective_speed: None,
            id,
            name: format!("R{id}"),
            color: "#007aff".to_string(),
            mode: TransitMode::Bus,
            station_ids: vec![1, 2],
            headway_seconds: 300.0,
            fare: 2.0,
            vehicle_count: 2,
            daily_ridership: riders,
            daily_revenue: 0.0,
            length_meters: 0.0,
            capacity: 0.0,
            load: 0.0,
            crowding,
            segment_loads: vec![],
            live_crowding: None,
            operating_cost: cost,
            farebox,
        }
    }

    #[test]
    fn sort_by_crowding_descending_stable_on_ties() {
        let routes = vec![
            test_route(1, 0.2, 10.0, None, None),
            test_route(2, 0.9, 10.0, None, None),
            test_route(3, 0.9, 10.0, None, None),
            test_route(4, 0.1, 10.0, None, None),
        ];
        let order = sort_route_indices(&routes, RouteSortKey::Crowding);
        assert_eq!(order, vec![1, 2, 0, 3]); // ids 2,3 tied → id ascending
    }

    #[test]
    fn sort_by_riders_descending() {
        let routes = vec![
            test_route(1, 0.0, 100.0, None, None),
            test_route(2, 0.0, 500.0, None, None),
            test_route(3, 0.0, 200.0, None, None),
        ];
        assert_eq!(
            sort_route_indices(&routes, RouteSortKey::Riders),
            vec![1, 2, 0]
        );
    }

    #[test]
    fn sort_by_net_income_uses_farebox_minus_cost() {
        let routes = vec![
            test_route(1, 0.0, 0.0, Some(100.0), Some(80.0)), // +20
            test_route(2, 0.0, 0.0, Some(50.0), Some(90.0)),  // -40
            test_route(3, 0.0, 0.0, Some(200.0), Some(10.0)), // +190
            test_route(4, 0.0, 0.0, None, None),              // 0 (missing)
        ];
        assert_eq!(
            sort_route_indices(&routes, RouteSortKey::NetIncome),
            vec![2, 0, 3, 1]
        );
    }

    #[test]
    fn sort_prefers_live_crowding_when_present() {
        let mut routes = vec![
            test_route(1, 0.9, 0.0, None, None),
            test_route(2, 0.1, 0.0, None, None),
        ];
        routes[0].live_crowding = Some(0.05);
        routes[1].live_crowding = Some(0.95);
        assert_eq!(
            sort_route_indices(&routes, RouteSortKey::Crowding),
            vec![1, 0]
        );
    }

    #[test]
    fn sort_empty_is_empty() {
        assert!(sort_route_indices(&[], RouteSortKey::Crowding).is_empty());
    }

    #[test]
    fn reorder_index_moves_element() {
        let mut v = vec![10, 20, 30, 40];
        reorder_index(&mut v, 0, 2);
        assert_eq!(v, vec![20, 30, 10, 40]);
        reorder_index(&mut v, 3, 1);
        assert_eq!(v, vec![20, 40, 30, 10]);
    }

    #[test]
    fn reorder_index_noop_on_bad_indices() {
        let mut v = vec![1, 2, 3];
        reorder_index(&mut v, 5, 0);
        reorder_index(&mut v, 0, 5);
        reorder_index(&mut v, 1, 1);
        assert_eq!(v, vec![1, 2, 3]);
    }

    #[test]
    fn toggle_station_adds_and_removes() {
        let mut stops = vec![1, 2];
        assert!(toggle_station_in_order(&mut stops, 3));
        assert_eq!(stops, vec![1, 2, 3]);
        assert!(toggle_station_in_order(&mut stops, 2));
        assert_eq!(stops, vec![1, 3]);
    }

    #[test]
    fn insert_stop_after_appends_or_inserts() {
        let mut stops = vec![1, 2, 3];
        assert_eq!(insert_stop_after(&mut stops, 4, Some(2)), Some(2));
        assert_eq!(stops, vec![1, 2, 4, 3]);
        assert_eq!(insert_stop_after(&mut stops, 5, None), Some(4));
        assert_eq!(stops, vec![1, 2, 4, 3, 5]);
    }

    #[test]
    fn insert_stop_after_rejects_duplicate_tail() {
        let mut stops = vec![1, 2];
        assert_eq!(insert_stop_after(&mut stops, 2, None), None);
        assert_eq!(stops, vec![1, 2]);
    }

    #[test]
    fn remove_stop_returns_presence() {
        let mut stops = vec![1, 2, 3];
        assert!(remove_stop(&mut stops, 2));
        assert_eq!(stops, vec![1, 3]);
        assert!(!remove_stop(&mut stops, 9));
    }

    #[test]
    fn next_color_index_cycles() {
        assert_eq!(next_color_index("#ff3b30", 0), 1);
        assert_eq!(next_color_index("#ff2d95", 0), 0);
        // Unknown hex falls back to list index then advances.
        assert_eq!(next_color_index("#abcdef", 3), 4);
    }

    #[test]
    fn mode_word_has_no_dashes() {
        for mode in [
            TransitMode::Bus,
            TransitMode::Tram,
            TransitMode::Metro,
            TransitMode::Rail,
        ] {
            assert!(!mode_word(mode).contains(['-', '\u{2013}', '\u{2014}']));
        }
    }

    // --- route line-diagram layout ---------------------------------------

    #[test]
    fn diagram_station_offsets_zero_stations_is_empty() {
        assert!(diagram_station_offsets(300.0, 12.0, 0).is_empty());
    }

    #[test]
    fn diagram_station_offsets_one_station_is_centered() {
        let offsets = diagram_station_offsets(300.0, 12.0, 1);
        assert_eq!(offsets.len(), 1);
        assert!((offsets[0] - 150.0).abs() < 0.001);
    }

    #[test]
    fn diagram_station_offsets_span_usable_width_evenly() {
        let offsets = diagram_station_offsets(210.0, 10.0, 4);
        assert_eq!(offsets.len(), 4);
        assert!((offsets[0] - 10.0).abs() < 0.001);
        assert!((offsets[3] - 200.0).abs() < 0.001);
        let step = offsets[1] - offsets[0];
        for pair in offsets.windows(2) {
            assert!((pair[1] - pair[0] - step).abs() < 0.001);
        }
    }

    #[test]
    fn diagram_progress_to_x_maps_endpoints() {
        let offsets = diagram_station_offsets(200.0, 10.0, 3);
        assert!((diagram_progress_to_x(0.0, &offsets).unwrap() - offsets[0]).abs() < 0.001);
        assert!((diagram_progress_to_x(1.0, &offsets).unwrap() - offsets[2]).abs() < 0.001);
        let mid = diagram_progress_to_x(0.5, &offsets).unwrap();
        assert!((mid - offsets[1]).abs() < 0.001);
    }

    #[test]
    fn transfer_station_ids_marks_shared_stops_only() {
        let routes = vec![
            UiRoute {
                on_time_pct: None,
                avg_delay_sec: None,
                in_service_vehicles: None,
                frequency: None,
                peak_units_required: None,
                station_ids: vec![1, 2, 3],
                ..test_route(1, 0.0, 0.0, None, None)
            },
            UiRoute {
                on_time_pct: None,
                avg_delay_sec: None,
                in_service_vehicles: None,
                frequency: None,
                peak_units_required: None,
                station_ids: vec![3, 4],
                ..test_route(2, 0.0, 0.0, None, None)
            },
            UiRoute {
                on_time_pct: None,
                avg_delay_sec: None,
                in_service_vehicles: None,
                frequency: None,
                peak_units_required: None,
                station_ids: vec![5],
                ..test_route(3, 0.0, 0.0, None, None)
            },
        ];
        let transfers = transfer_station_ids(&routes);
        assert_eq!(transfers, HashSet::from([3]));
    }

    #[test]
    fn route_progress_at_endpoints_and_midpoint() {
        let mut coords = HashMap::new();
        coords.insert(1, (0.0, 0.0));
        coords.insert(2, (100.0, 0.0));
        coords.insert(3, (200.0, 0.0));
        let stops = vec![1, 2, 3];
        assert!((route_progress_at(&stops, &coords, 0.0, 0.0).unwrap() - 0.0).abs() < 0.001);
        assert!((route_progress_at(&stops, &coords, 200.0, 0.0).unwrap() - 1.0).abs() < 0.001);
        assert!((route_progress_at(&stops, &coords, 100.0, 5.0).unwrap() - 0.5).abs() < 0.05);
    }

    #[test]
    fn vehicle_progresses_on_route_filters_by_color_index() {
        let mut coords = HashMap::new();
        coords.insert(1, (0.0, 0.0));
        coords.insert(2, (100.0, 0.0));
        let frame = FrameSnapshot {
            tick: 1,
            vehicle_count: 2,
            agent_count: 0,
            color_table: vec![],
            vehicles: vec![
                1.0, 50.0, 0.0, 0.0, 0.0, 0.0, // route 0, midpoint
                2.0, 50.0, 0.0, 0.0, 0.0, 1.0, // route 1, ignored
            ],
            agents: vec![],
        };
        let progresses = vehicle_progresses_on_route(0, &[1, 2], &coords, &frame);
        assert_eq!(progresses.len(), 1);
        assert!((progresses[0] - 0.5).abs() < 0.05);
    }
}
