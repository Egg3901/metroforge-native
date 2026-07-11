//! Build toolbar + route panel (ship-plan #25, v0.2). Draws its own egui
//! panels, distinct from `hud.rs`'s HUD bars/toasts (mission scope: this
//! file must not edit `hud.rs`) - see `MfBuildUiPlugin` for how they're
//! wired in alongside it.
//!
//! Scope boundary this file holds to: `tools.rs` (a parallel worktree, see
//! the `// INTEGRATION STUB` copy in this crate) owns *world* interaction -
//! raycasting/placement, drag-to-draw route building, cost quoting. This
//! file only owns the toolbar/contextual-strip/route-panel widgets and the
//! route-panel's own edit commands (rename/fare/vehicle-count/delete, all
//! `Command::EditRoute`/`Command::DeleteRoute`). It deliberately does NOT
//! read Enter/Esc to confirm/cancel an in-progress route or submit
//! `Command::CreateRoute` itself - the contextual strip's "Enter confirms,
//! Esc cancels" copy is describing the interaction `tools.rs` drives (it
//! owns `route_draft`/`last_cost_quote` and is the natural owner of when to
//! clear them); wiring the same keys here too would risk a double-submit
//! race at integration between two independently-developed systems both
//! watching the same keys.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use mf_net::SimLink;
use mf_protocol::{Command, ToastTone, TransitMode, UiRoute, UiStation};
use mf_state::LatestUi;

use crate::audio::{PlaySfx, Sfx};
use crate::command_bus::{CmdMeta, CommandBus, CommandFeedback};
use crate::design_system as ds;
use crate::hud::ToastLog;
use crate::state::AppState;
use crate::tools::{ActiveTool, ToolState};

// ---------------------------------------------------------------------
// Pure formatting helpers (unit-tested below)
// ---------------------------------------------------------------------
// `hud.rs` has its own `format_thousands`/`format_cash` but both are
// private, so they can't be imported - these are plain reimplementations,
// not a deliberate fork of behavior. `hud.rs` migrates onto `design_system`
// (and, likely, a shared formatting spot) at integration.

/// Comma-grouped integer, e.g. `146015` -> `"146,015"`.
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

/// Fares are sub-dollar-unit prices (e.g. `$1.25`), unlike whole-dollar
/// cash/cost readouts, so this always shows two decimal places.
fn format_fare(value: f64) -> String {
    format!("${:.2}", value.max(0.0))
}

// ---------------------------------------------------------------------
// Local hover-tick (reimplemented per mission brief - `hud.rs`'s copy is
// private and this file must not import from `hud.rs` anyway)
// ---------------------------------------------------------------------

/// One hover tick the first frame the pointer lands on a widget; re-arms
/// when it leaves. Identical behavior to `hud.rs`'s private `hover_tick`,
/// duplicated here rather than shared (see module docs: this file must not
/// depend on `hud.rs` internals).
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
// Local vivid route-color table
// ---------------------------------------------------------------------
// `mf-render`'s `palette::vivid_route_color` is the source of truth the 3D
// scene's route stripes use (art-direction: "native client ignores the
// wire colorTable and keeps its own vivid table indexed by
// routeColorIdx"). `mf-game` doesn't (and per mission scope shouldn't)
// depend on `mf-render`, so this is the same eight hex values duplicated
// as `egui::Color32` for the route panel's swatches - same index, same
// color as the world's route stripe, without a cross-crate dependency.
// Unlike `mf-render`'s version this wraps by modulo past 8 rather than
// extending via golden-angle hue rotation: a route panel swatch is a
// small flat dot, not a rendered stripe, so exact hue fidelity past the
// eighth route isn't worth the HSL math here.
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

fn vivid_route_color(idx: usize) -> egui::Color32 {
    ROUTE_COLORS[idx % ROUTE_COLORS.len()]
}

fn mode_word(mode: TransitMode) -> &'static str {
    match mode {
        TransitMode::Bus => "bus",
        TransitMode::Tram => "tram",
        TransitMode::Metro => "metro",
        TransitMode::Rail => "rail",
    }
}

/// Heuristic for "has the player unlocked Tram yet": `UiState.unlockedModes`
/// (`unlocked_modes` here) is the sidecar's own progression signal - a
/// mode only appears in it once its unlock milestone has actually fired -
/// so this reads that directly rather than inferring unlock from "does any
/// existing station/route already use Tram", which would have a
/// chicken-and-egg gap at the exact moment it unlocks (the player can't
/// have built a tram station yet if the toolbar button that lets them do
/// so is still disabled). Documented here since it's the one place this
/// mission's brief and the wire protocol disagree on the simplest signal.
fn tram_unlocked(ui_state: &LatestUi) -> bool {
    ui_state
        .0
        .as_ref()
        .map(|s| s.unlocked_modes.contains(&TransitMode::Tram))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------
// Route panel state
// ---------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct RoutePanelState {
    pub open: bool,
    pub selected: Option<i64>,
    /// Which route id `name_edit`/`fare_edit` currently mirror; re-seeded
    /// from the live `UiRoute` whenever `selected` changes to a route id
    /// this doesn't match yet.
    edit_for: Option<i64>,
    name_edit: String,
    fare_edit: f64,
    /// Route id armed for a second "Confirm delete" click. Cleared on
    /// selection change; NOT time-limited (v0.2: a stray second click
    /// minutes later still deletes) - acceptable for a first pass, a
    /// timeout could be added later without changing the public shape.
    delete_armed: Option<i64>,
}

// ---------------------------------------------------------------------
// Icon toolbar button
// ---------------------------------------------------------------------

const TOOLBAR_BUTTON_SIZE: egui::Vec2 = egui::vec2(36.0, 32.0);

/// Draws one icon-button and reports whether it was clicked. `active`
/// fills it with the accent color (art-direction: vivid color reserved for
/// interactive elements) and `enabled = false` grays it out and blocks the
/// click. `locked` additionally paints a small padlock badge - used for
/// the Tram Station button before Tram unlocks - distinct from
/// `design_system::IconKind` since it's a toolbar-specific overlay badge,
/// not a general-purpose icon.
#[allow(clippy::too_many_arguments)]
fn icon_button(
    ui: &mut egui::Ui,
    kind: ds::IconKind,
    active: bool,
    enabled: bool,
    locked: bool,
    tooltip: &str,
    hovered: &mut Option<egui::Id>,
    sfx: &mut EventWriter<PlaySfx>,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(TOOLBAR_BUTTON_SIZE, egui::Sense::click());
    let resp = resp.on_hover_text(tooltip);
    if enabled {
        hover_tick(&resp, hovered, sfx);
    }

    let bg = if !enabled {
        ds::inactive_bg()
    } else if active {
        ds::accent()
    } else if resp.hovered() {
        ds::hover_bg()
    } else {
        ds::inactive_bg()
    };
    let fg = if active {
        egui::Color32::WHITE
    } else if !enabled {
        ds::muted()
    } else {
        ds::text()
    };

    let painter = ui.painter();
    painter.rect_filled(rect, ds::CORNER_RADIUS, bg);
    ds::icon(painter, rect.shrink(8.0), kind, fg, 1.6);
    if locked {
        paint_lock_badge(painter, rect, ds::muted());
    }

    enabled && resp.clicked()
}

/// Small padlock badge in a button's bottom-right corner: a stroked
/// shackle arc over a filled body. Kept out of `design_system::IconKind`
/// because it's an overlay badge composed with another icon, not a
/// standalone glyph.
fn paint_lock_badge(painter: &egui::Painter, button_rect: egui::Rect, color: egui::Color32) {
    let badge = egui::Rect::from_min_size(
        button_rect.right_bottom() - egui::vec2(13.0, 13.0),
        egui::vec2(11.0, 11.0),
    );
    let body = egui::Rect::from_min_max(
        egui::pos2(badge.min.x, badge.min.y + badge.height() * 0.45),
        badge.max,
    );
    painter.rect_filled(body, egui::CornerRadius::same(1), color);
    let shackle_center = egui::pos2(badge.center().x, body.min.y);
    painter.circle_stroke(
        shackle_center,
        badge.width() * 0.28,
        egui::Stroke::new(1.2, color),
    );
}

// ---------------------------------------------------------------------
// Toolbar + contextual strip
// ---------------------------------------------------------------------

/// Bottom build toolbar (`build_ui_toolbar`) plus, when a tool is active,
/// a contextual hint strip (`build_ui_context_strip`) stacked above it.
/// Both are `TopBottomPanel::bottom` calls with ids distinct from
/// `hud.rs`'s `hud_top`/`hud_toasts` - which of the two files' bottom
/// panels ends up physically above the other depends on cross-plugin
/// system order, which isn't pinned down here (mission note: "toasts will
/// restack at integration"); the toolbar is shown before the context strip
/// in THIS file specifically so that, all else equal, the strip sits
/// above the toolbar rather than between it and the world.
#[allow(clippy::too_many_arguments)]
fn build_toolbar_system(
    mut contexts: EguiContexts,
    ui_state: Res<LatestUi>,
    mut tools: ResMut<ToolState>,
    mut bus: ResMut<CommandBus>,
    link: Option<Res<SimLink>>,
    mut panel: ResMut<RoutePanelState>,
    mut sfx: EventWriter<PlaySfx>,
    mut hovered: Local<Option<egui::Id>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let tram_ok = tram_unlocked(&ui_state);
    // Copied out once so the click-branches below can freely write
    // `tools.active` without fighting a live borrow from the comparisons
    // (`ActiveTool` is `Copy`, so this is free).
    let current_tool = tools.active;

    egui::TopBottomPanel::bottom("build_ui_toolbar")
        .frame(
            egui::Frame::default()
                .fill(ds::panel_bg())
                .inner_margin(egui::Margin::symmetric(
                    ds::SPACE_SM as i8,
                    ds::SPACE_XS as i8,
                )),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(ds::SPACE_XXS, 0.0);

                if icon_button(
                    ui,
                    ds::IconKind::Cursor,
                    current_tool == ActiveTool::None,
                    true,
                    false,
                    "Select",
                    &mut hovered,
                    &mut sfx,
                ) {
                    tools.active = ActiveTool::None;
                    sfx.write(PlaySfx(Sfx::Confirm));
                }

                if icon_button(
                    ui,
                    ds::IconKind::Bus,
                    current_tool == ActiveTool::PlaceStation(TransitMode::Bus),
                    true,
                    false,
                    "Bus station (1)",
                    &mut hovered,
                    &mut sfx,
                ) {
                    tools.active = ActiveTool::PlaceStation(TransitMode::Bus);
                    sfx.write(PlaySfx(Sfx::Confirm));
                }

                let tram_tooltip = if tram_ok {
                    "Tram station (2)"
                } else {
                    "Tram station (2). Locked until Tram unlocks."
                };
                if icon_button(
                    ui,
                    ds::IconKind::Tram,
                    current_tool == ActiveTool::PlaceStation(TransitMode::Tram),
                    tram_ok,
                    !tram_ok,
                    tram_tooltip,
                    &mut hovered,
                    &mut sfx,
                ) {
                    tools.active = ActiveTool::PlaceStation(TransitMode::Tram);
                    sfx.write(PlaySfx(Sfx::Confirm));
                }

                if icon_button(
                    ui,
                    ds::IconKind::RouteLine,
                    current_tool == ActiveTool::Route,
                    true,
                    false,
                    "Route (3)",
                    &mut hovered,
                    &mut sfx,
                ) {
                    tools.active = ActiveTool::Route;
                    sfx.write(PlaySfx(Sfx::Confirm));
                }

                if icon_button(
                    ui,
                    ds::IconKind::Bulldozer,
                    current_tool == ActiveTool::Bulldoze,
                    true,
                    false,
                    "Bulldoze (4)",
                    &mut hovered,
                    &mut sfx,
                ) {
                    tools.active = ActiveTool::Bulldoze;
                    sfx.write(PlaySfx(Sfx::Confirm));
                }

                ui.add_space(ds::SPACE_SM);
                ui.add(egui::Separator::default().vertical().shrink(6.0));
                ui.add_space(ds::SPACE_SM);

                if icon_button(
                    ui,
                    ds::IconKind::Undo,
                    false,
                    bus.can_undo(),
                    false,
                    "Undo",
                    &mut hovered,
                    &mut sfx,
                ) {
                    let undone = link.as_deref().map(|l| bus.undo_last(l)).unwrap_or(false);
                    sfx.write(PlaySfx(if undone { Sfx::Confirm } else { Sfx::Cancel }));
                }

                ui.add_space(ds::SPACE_SM);
                ui.add(egui::Separator::default().vertical().shrink(6.0));
                ui.add_space(ds::SPACE_SM);

                let routes_button = ui.add(
                    egui::Button::new(egui::RichText::new("Routes").color(if panel.open {
                        egui::Color32::WHITE
                    } else {
                        ds::text()
                    }))
                    .fill(if panel.open {
                        ds::accent()
                    } else {
                        ds::inactive_bg()
                    })
                    .corner_radius(ds::CORNER_RADIUS),
                );
                hover_tick(&routes_button, &mut hovered, &mut sfx);
                if routes_button.clicked() {
                    panel.open = !panel.open;
                    sfx.write(PlaySfx(if panel.open {
                        Sfx::Confirm
                    } else {
                        Sfx::Cancel
                    }));
                }
            });
        });

    if let Some((text, color)) = contextual_strip_text(&tools, &ui_state) {
        egui::TopBottomPanel::bottom("build_ui_context_strip")
            .frame(egui::Frame::default().fill(ds::panel_bg()).inner_margin(
                egui::Margin::symmetric(ds::SPACE_MD as i8, ds::SPACE_XXS as i8),
            ))
            .show_separator_line(false)
            .min_height(0.0)
            .show(ctx, |ui| {
                ui.colored_label(color, egui::RichText::new(text).size(ds::TEXT_SM));
            });
    }

    Ok(())
}

/// The contextual hint shown above the toolbar for the currently active
/// tool, and its text color (warning-tinted for Bulldoze per the brief).
fn contextual_strip_text(
    tools: &ToolState,
    ui_state: &LatestUi,
) -> Option<(String, egui::Color32)> {
    match tools.active {
        ActiveTool::None => None,
        ActiveTool::PlaceStation(mode) => {
            let cash = ui_state.0.as_ref().map(|s| s.cash).unwrap_or(0.0);
            Some((
                format!(
                    "Click to place a {} station. Cash on hand: {}",
                    mode_word(mode),
                    format_cash(cash)
                ),
                ds::text(),
            ))
        }
        ActiveTool::Route => {
            let count = tools.route_draft.len();
            let quote = tools
                .last_cost_quote
                .map(format_cash)
                .unwrap_or_else(|| "not quoted yet".to_string());
            Some((
                format!(
                    "Click stations to add. Enter confirms, Esc cancels. {count} station(s) selected. Estimated cost: {quote}."
                ),
                ds::text(),
            ))
        }
        ActiveTool::Bulldoze => Some((
            "Click a station or track to demolish.".to_string(),
            ds::WARN,
        )),
    }
}

// ---------------------------------------------------------------------
// Route panel
// ---------------------------------------------------------------------

/// Right-side route list/editor (`build_ui_route_panel`), toggled by the
/// toolbar's "Routes" button.
#[allow(clippy::too_many_arguments)]
fn route_panel_system(
    mut contexts: EguiContexts,
    ui_state: Res<LatestUi>,
    link: Option<Res<SimLink>>,
    mut bus: ResMut<CommandBus>,
    mut panel: ResMut<RoutePanelState>,
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

    egui::SidePanel::right("build_ui_route_panel")
        .frame(
            egui::Frame::default()
                .fill(ds::panel_bg())
                .inner_margin(egui::Margin::symmetric(
                    ds::SPACE_SM as i8,
                    ds::SPACE_SM as i8,
                )),
        )
        .default_width(300.0)
        .min_width(240.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.label(ds::heading("Routes"));
            ui.add_space(ds::SPACE_XS);

            if state.routes.is_empty() {
                ui.label(ds::label_muted(
                    "No routes yet. Use the Route tool to string stations together.",
                ));
                return;
            }

            // Selected route may have been deleted server-side (by this
            // panel's own Delete button, or another client in a future
            // multiplayer mode) - drop a stale selection rather than
            // showing an editor for a route that no longer exists.
            if let Some(selected) = panel.selected {
                if !state.routes.iter().any(|r| r.id == selected) {
                    panel.selected = None;
                    panel.edit_for = None;
                }
            }

            for (idx, route) in state.routes.iter().enumerate() {
                let is_selected = panel.selected == Some(route.id);
                ui.horizontal(|ui| {
                    let (swatch_rect, _) =
                        ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        swatch_rect,
                        egui::CornerRadius::same(2),
                        vivid_route_color(idx),
                    );

                    let display_name = if route.name.trim().is_empty() {
                        format!("Line {}", idx + 1)
                    } else {
                        route.name.clone()
                    };
                    let resp = ui.selectable_label(is_selected, display_name);
                    hover_tick(&resp, &mut hovered, &mut sfx);
                    if resp.clicked() {
                        panel.selected = if is_selected { None } else { Some(route.id) };
                        sfx.write(PlaySfx(Sfx::Confirm));
                    }

                    // Sim-depth (PR #31): a live-crowding chip on the right of
                    // the row, colored green -> amber -> red. Only shown when
                    // the sidecar sends `liveCrowding` (old ones omit it).
                    if let Some(crowding) = route.live_crowding {
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                let (dot, dot_resp) = ui.allocate_exact_size(
                                    egui::vec2(10.0, 10.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    dot,
                                    egui::CornerRadius::same(5),
                                    ds::crowding_color(crowding),
                                );
                                dot_resp.on_hover_text(format!(
                                    "Live crowding {:.0}%",
                                    crowding.clamp(0.0, 1.0) * 100.0
                                ));
                            },
                        );
                    }
                });
                ui.label(ds::label_small(format!(
                    "{} station(s), {} vehicle(s), mode {}",
                    route.station_ids.len(),
                    route.vehicle_count,
                    mode_word(route.mode),
                )));

                if is_selected {
                    route_editor(
                        ui,
                        route,
                        &state.stations,
                        vivid_route_color(idx),
                        &mut panel,
                        &mut bus,
                        link.as_deref(),
                        &mut hovered,
                        &mut sfx,
                    );
                }

                ui.add_space(ds::SPACE_XXS);
                ui.separator();
                ui.add_space(ds::SPACE_XXS);
            }
        });

    Ok(())
}

/// Station labels for [`ds::route_line_diagram`], in the route's own
/// station order: each station's own `name` when it has one (the normal
/// case - `UiStation` does carry a `name` field, see `mf-protocol`'s
/// `types.rs`), else a positional "S<n>" fallback (1-based, matching the
/// existing "Line {n}" 1-based convention a few lines up) for the data-gap
/// case of a blank name. Looking a station up by id rather than assuming
/// `stations` iterates in route order - `state.stations` and a route's
/// `station_ids` are two independently-ordered lists over the wire.
fn route_station_labels(route: &UiRoute, stations: &[UiStation]) -> Vec<String> {
    let by_id: std::collections::HashMap<i64, &UiStation> =
        stations.iter().map(|s| (s.id, s)).collect();
    route
        .station_ids
        .iter()
        .enumerate()
        .map(|(i, id)| match by_id.get(id) {
            Some(st) if !st.name.trim().is_empty() => st.name.clone(),
            _ => format!("S{}", i + 1),
        })
        .collect()
}

/// The expanded per-route editor drawn under a selected route's row:
/// vehicle count stepper, fare drag, name field, delete (2-click confirm).
/// Every mutating control here submits through `CommandBus` with
/// `CmdMeta::EditRoute { route_id }` - including the Delete button, since
/// the given `CmdMeta` enum has no dedicated delete variant and
/// `EditRoute { route_id }` is the closest tag that still carries which
/// route the feedback is about.
#[allow(clippy::too_many_arguments)]
fn route_editor(
    ui: &mut egui::Ui,
    route: &UiRoute,
    stations: &[UiStation],
    color: egui::Color32,
    panel: &mut RoutePanelState,
    bus: &mut CommandBus,
    link: Option<&SimLink>,
    hovered: &mut Option<egui::Id>,
    sfx: &mut EventWriter<PlaySfx>,
) {
    if panel.edit_for != Some(route.id) {
        panel.name_edit = route.name.clone();
        panel.fare_edit = route.fare;
        panel.edit_for = Some(route.id);
        panel.delete_armed = None;
    }

    ui.add_space(ds::SPACE_XXS);

    // Line diagram (ship-plan #25, v0.3 map mode wave): the route's color,
    // its stations in order, and its per-segment load, so a glance shows
    // both the line's shape and where it's crowded without leaving this
    // panel for the 3D scene.
    let labels = route_station_labels(route, stations);
    ds::route_line_diagram(ui, color, &labels, &route.segment_loads);
    ui.add_space(ds::SPACE_XXS);

    // Sim-depth (PR #31): this route's daily farebox vs operating cost, plus
    // a live-crowding readout. Each row is drawn only when the sidecar sends
    // the matching field, so an old sidecar (which omits all three) shows the
    // editor exactly as before.
    if let Some(crowding) = route.live_crowding {
        ui.horizontal(|ui| {
            ui.label(ds::label_muted("Live crowding"));
            let (dot, _) =
                ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(dot, egui::CornerRadius::same(5), ds::crowding_color(crowding));
            ui.label(
                ds::value_strong(format!("{:.0}%", crowding.clamp(0.0, 1.0) * 100.0))
                    .color(ds::crowding_color(crowding)),
            );
        });
    }
    if let Some(farebox) = route.farebox {
        ui.horizontal(|ui| {
            ui.label(ds::label_muted("Farebox / day"));
            ui.label(ds::value_strong(format_cash(farebox)).color(ds::GOOD));
        });
    }
    if let Some(cost) = route.operating_cost {
        ui.horizontal(|ui| {
            ui.label(ds::label_muted("Operating cost / day"));
            ui.label(ds::value_strong(format_cash(cost)).color(ds::BAD));
        });
    }
    // Net line only when BOTH numbers are present, so it isn't computed off a
    // half-populated pair.
    if let (Some(farebox), Some(cost)) = (route.farebox, route.operating_cost) {
        let net = farebox - cost;
        let good = net >= 0.0;
        let prefix = if good { "+" } else { "-" };
        ui.horizontal(|ui| {
            ui.label(ds::label_muted("Net / day"));
            ui.label(
                ds::value_strong(format!("{prefix}{}", format_cash(net.abs())))
                    .color(if good { ds::GOOD } else { ds::BAD }),
            );
        });
    }
    ui.add_space(ds::SPACE_XXS);

    ui.horizontal(|ui| {
        ui.label(ds::label_muted("Vehicles"));
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

    ui.horizontal(|ui| {
        ui.label(ds::label_muted("Fare"));
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
        ui.label(ds::label_small(format!("now {}", format_fare(route.fare))));
    });

    ui.horizontal(|ui| {
        ui.label(ds::label_muted("Name"));
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
    let armed = panel.delete_armed == Some(route.id);
    let delete_resp = ui.add(
        egui::Button::new(
            egui::RichText::new(if armed { "Confirm delete" } else { "Delete" }).color(if armed {
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
            panel.selected = None;
            panel.edit_for = None;
        } else {
            panel.delete_armed = Some(route.id);
        }
    }
}

/// The subset of `Command::EditRoute`'s optional fields the route panel
/// can touch, defaulting every field to "leave unchanged" so each control
/// only sets the one field it owns.
#[derive(Default)]
struct EditFields {
    fare: Option<f64>,
    vehicle_count: Option<u32>,
    name: Option<String>,
}

fn submit_edit(bus: &mut CommandBus, link: Option<&SimLink>, route_id: i64, fields: EditFields) {
    let Some(link) = link else { return };
    bus.submit(
        link,
        Command::EditRoute {
            route_id,
            headway_seconds: None,
            fare: fields.fare,
            vehicle_count: fields.vehicle_count,
            name: fields.name,
            color: None,
        },
        CmdMeta::EditRoute { route_id },
    );
}

// ---------------------------------------------------------------------
// Command feedback -> toasts/sfx
// ---------------------------------------------------------------------

/// On failure, plainly reports the sidecar's error via `hud.rs`'s
/// `ToastLog` (imported, not duplicated - `hud.rs`'s toast rendering
/// already exists and this file shouldn't fork it) plus an error chime; on
/// a successful route creation, a confirm chime. Other successful command
/// kinds don't get their own sound here - the world visibly updating
/// (a new station/track/route appearing) is confirmation enough, and
/// `hud.rs`'s own toast/sfx systems already cover the general case.
fn command_feedback_listener_system(
    mut feedback: EventReader<CommandFeedback>,
    mut toasts: ResMut<ToastLog>,
    mut sfx: EventWriter<PlaySfx>,
) {
    for fb in feedback.read() {
        if !fb.ok {
            let detail = fb.error.as_deref().unwrap_or("unknown error");
            push_toast(
                &mut toasts,
                format!("Cannot build there: {detail}"),
                ToastTone::Warn,
            );
            sfx.write(PlaySfx(Sfx::Error));
        } else if matches!(fb.meta, CmdMeta::CreateRoute { .. }) {
            sfx.write(PlaySfx(Sfx::Confirm));
        }
    }
}

/// Mirrors `hud.rs`'s private `TOAST_LOG_CAP` (20) so this file's pushes
/// can't grow the log unbounded either; duplicated rather than imported
/// since the const isn't `pub` there.
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

pub struct MfBuildUiPlugin;

impl Plugin for MfBuildUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoutePanelState>()
            .add_systems(Update, command_feedback_listener_system)
            .add_systems(
                EguiPrimaryContextPass,
                (build_toolbar_system, route_panel_system)
                    .chain()
                    .run_if(in_state(AppState::InGame))
                    .run_if(|| !crate::design_system::hud_hidden()),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_thousands_groups_by_three() {
        assert_eq!(format_thousands(0.0), "0");
        assert_eq!(format_thousands(999.0), "999");
        assert_eq!(format_thousands(1000.0), "1,000");
        assert_eq!(format_thousands(146_015.0), "146,015");
        assert_eq!(format_thousands(1_000_000.0), "1,000,000");
    }

    #[test]
    fn format_thousands_clamps_negative_to_zero() {
        assert_eq!(format_thousands(-50.0), "0");
    }

    #[test]
    fn format_cash_prefixes_dollar_sign() {
        assert_eq!(format_cash(1234.0), "$1,234");
    }

    #[test]
    fn format_fare_shows_two_decimals() {
        assert_eq!(format_fare(1.5), "$1.50");
        assert_eq!(format_fare(0.0), "$0.00");
        assert_eq!(format_fare(-3.0), "$0.00");
    }

    #[test]
    fn vivid_route_color_wraps_past_eight() {
        assert_eq!(vivid_route_color(0), vivid_route_color(8));
        assert_eq!(vivid_route_color(1), vivid_route_color(9));
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

    // --- route_station_labels ---------------------------------------------

    fn test_station(id: i64, name: &str) -> UiStation {
        UiStation {
            id,
            name: name.to_string(),
            x: 0.0,
            y: 0.0,
            mode: TransitMode::Bus,
            level: 0,
            ridership: 0.0,
            alightings: 0.0,
        }
    }

    fn test_route(station_ids: Vec<i64>) -> UiRoute {
        UiRoute {
            id: 1,
            name: "Test".to_string(),
            color: "#000000".to_string(),
            mode: TransitMode::Bus,
            station_ids,
            headway_seconds: 300.0,
            fare: 2.0,
            vehicle_count: 1,
            daily_ridership: 0.0,
            daily_revenue: 0.0,
            length_meters: 0.0,
            capacity: 0.0,
            load: 0.0,
            crowding: 0.0,
            segment_loads: vec![],
            live_crowding: None,
            operating_cost: None,
            farebox: None,
        }
    }

    #[test]
    fn route_station_labels_uses_station_name_when_present() {
        let stations = vec![test_station(10, "Union Sq"), test_station(20, "Park St")];
        let route = test_route(vec![10, 20]);
        assert_eq!(
            route_station_labels(&route, &stations),
            vec!["Union Sq".to_string(), "Park St".to_string()]
        );
    }

    #[test]
    fn route_station_labels_falls_back_to_positional_when_name_blank() {
        let stations = vec![test_station(10, ""), test_station(20, "  ")];
        let route = test_route(vec![10, 20]);
        assert_eq!(
            route_station_labels(&route, &stations),
            vec!["S1".to_string(), "S2".to_string()]
        );
    }

    #[test]
    fn route_station_labels_falls_back_when_station_id_is_unknown() {
        // Route references a station id not present in `stations` (e.g. a
        // stale/racing update) - must not panic or drop the slot, just fall
        // back to the positional label for that one entry.
        let stations = vec![test_station(10, "Union Sq")];
        let route = test_route(vec![10, 999]);
        assert_eq!(
            route_station_labels(&route, &stations),
            vec!["Union Sq".to_string(), "S2".to_string()]
        );
    }

    #[test]
    fn route_station_labels_looks_up_by_id_not_list_order() {
        // `stations` given in the OPPOSITE order from the route's
        // `station_ids` - labels must still follow the route's order, i.e.
        // this is a real id lookup, not a zip-by-index shortcut.
        let stations = vec![test_station(20, "Park St"), test_station(10, "Union Sq")];
        let route = test_route(vec![10, 20]);
        assert_eq!(
            route_station_labels(&route, &stations),
            vec!["Union Sq".to_string(), "Park St".to_string()]
        );
    }

    #[test]
    fn route_station_labels_empty_route_is_empty() {
        let route = test_route(vec![]);
        assert!(route_station_labels(&route, &[]).is_empty());
    }
}
