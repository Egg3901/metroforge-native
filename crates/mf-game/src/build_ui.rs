//! Build toolbar (ship-plan #25, v0.2). Draws its own egui panels, distinct
//! from `hud.rs`'s HUD bars/toasts. The routes list/editor lives in
//! [`crate::routes_panel`] so HUD restyles and route-panel work merge
//! without fighting over `hud.rs` / a single mega-file.
//!
//! Scope boundary this file holds to: `tools.rs` owns *world* interaction —
//! raycasting/placement, route drafting, cost quoting. This file only owns
//! the toolbar / contextual-strip widgets. Route edit commands
//! (rename/fare/vehicles/delete/color/stop order) live in `routes_panel.rs`.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use mf_net::SimLink;
use mf_protocol::{ToastTone, TransitMode};
use mf_state::LatestUi;

use crate::audio::{PlaySfx, Sfx};
use crate::command_bus::{CmdMeta, CommandBus, CommandFeedback};
use crate::design_system as ds;
use crate::hud::ToastLog;
use crate::state::AppState;
use crate::tools::{ActiveTool, ToolState};

// Re-export so `hud.rs` (overcrowded-routes chip) keeps a stable import
// path across the panel extraction.
pub use crate::routes_panel::RoutePanelState;

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

fn mode_word(mode: TransitMode) -> &'static str {
    let s = crate::strings::current();
    match mode {
        TransitMode::Bus => s.mode_bus,
        TransitMode::Tram => s.mode_tram,
        TransitMode::Metro => s.mode_metro,
        TransitMode::Rail => s.mode_rail,
    }
}

/// Title-case mode label for a button face (the lowercase `mode_word` reads
/// oddly on a button). Plain words, no dashes.
fn mode_title(mode: TransitMode) -> &'static str {
    match mode {
        TransitMode::Bus => "Bus",
        TransitMode::Tram => "Tram",
        TransitMode::Metro => "Metro",
        TransitMode::Rail => "Rail",
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
    mode_unlocked(ui_state, TransitMode::Tram)
}

/// Generalized form of [`tram_unlocked`]: whether `mode` is in the sidecar's
/// `unlocked_modes` progression list. Bus is the always-available starter
/// mode, so it reads as unlocked even in the pre-load window (no `UiState`
/// yet) where every other mode reads locked.
fn mode_unlocked(ui_state: &LatestUi, mode: TransitMode) -> bool {
    if mode == TransitMode::Bus {
        return true;
    }
    ui_state
        .0
        .as_ref()
        .map(|s| s.unlocked_modes.contains(&mode))
        .unwrap_or(false)
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
    let s = crate::strings::current();
    let tram_ok = tram_unlocked(&ui_state);
    // Copied out once so the click-branches below can freely write
    // `tools.active` without fighting a live borrow from the comparisons
    // (`ActiveTool` is `Copy`, so this is free).
    let current_tool = tools.active;

    egui::TopBottomPanel::bottom("build_ui_toolbar")
        .frame(
            egui::Frame::NONE
                .fill(ds::panel_bg())
                .inner_margin(egui::Margin::symmetric(
                    ds::SPACE_SM as i8,
                    ds::SPACE_XS as i8,
                ))
                .stroke(egui::Stroke::new(ds::ACCENT_EDGE_PX, ds::accent())),
        )
        .show_separator_line(false)
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(ds::SPACE_XXS, 0.0);

                if icon_button(
                    ui,
                    ds::IconKind::Cursor,
                    current_tool == ActiveTool::None,
                    true,
                    false,
                    s.tool_select,
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
                    s.tool_bus_station,
                    &mut hovered,
                    &mut sfx,
                ) {
                    tools.active = ActiveTool::PlaceStation(TransitMode::Bus);
                    sfx.write(PlaySfx(Sfx::Confirm));
                }

                let tram_tooltip = if tram_ok {
                    s.tool_tram_station
                } else {
                    s.tool_tram_station_locked
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
                    s.tool_route,
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
                    s.tool_bulldoze,
                    &mut hovered,
                    &mut sfx,
                ) {
                    tools.active = ActiveTool::Bulldoze;
                    sfx.write(PlaySfx(Sfx::Confirm));
                }

                if icon_button(
                    ui,
                    ds::IconKind::Depot,
                    matches!(current_tool, ActiveTool::PlaceDepot(_)),
                    true,
                    false,
                    s.tool_depot,
                    &mut hovered,
                    &mut sfx,
                ) {
                    tools.active = ActiveTool::PlaceDepot(tools.depot_mode);
                    sfx.write(PlaySfx(Sfx::Confirm));
                }

                // Depot mode picker (v0.9 A4): only while the depot tool is
                // active, a compact segmented control lets the player place a
                // depot for any UNLOCKED mode (the sim allows one per mode).
                // Locked modes are shown disabled so the progression reads.
                if matches!(tools.active, ActiveTool::PlaceDepot(_)) {
                    for mode in [
                        TransitMode::Bus,
                        TransitMode::Tram,
                        TransitMode::Metro,
                        TransitMode::Rail,
                    ] {
                        let unlocked = mode_unlocked(&ui_state, mode);
                        let selected = tools.active == ActiveTool::PlaceDepot(mode);
                        let resp = ui
                            .add_enabled_ui(unlocked, |ui| {
                                ds::button(ui, mode_title(mode), ds::ButtonKind::Toggle(selected))
                            })
                            .inner;
                        hover_tick(&resp, &mut hovered, &mut sfx);
                        if resp.clicked() {
                            tools.depot_mode = mode;
                            tools.active = ActiveTool::PlaceDepot(mode);
                            sfx.write(PlaySfx(Sfx::Confirm));
                        }
                    }
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
                    s.tool_undo,
                    &mut hovered,
                    &mut sfx,
                ) {
                    let undone = link.as_deref().map(|l| bus.undo_last(l)).unwrap_or(false);
                    sfx.write(PlaySfx(if undone { Sfx::Confirm } else { Sfx::Cancel }));
                }

                ui.add_space(ds::SPACE_SM);
                ui.add(egui::Separator::default().vertical().shrink(6.0));
                ui.add_space(ds::SPACE_SM);

                let routes_button = ds::button(ui, s.routes, ds::ButtonKind::Toggle(panel.open));
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
            .frame(
                egui::Frame::NONE
                    .fill(ds::panel_bg())
                    .inner_margin(egui::Margin::symmetric(
                        ds::SPACE_MD as i8,
                        ds::SPACE_XXS as i8,
                    ))
                    .stroke(egui::Stroke::new(ds::ACCENT_EDGE_PX, ds::accent())),
            )
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
    let s = crate::strings::current();
    match tools.active {
        ActiveTool::None => {
            let count = tools.route_draft.len();
            if count == 0 {
                None
            } else {
                Some((
                    format!(
                        "Shift click toggles stations. {count} selected. Enter connects as a route, or press R for the Route tool."
                    ),
                    ds::text(),
                ))
            }
        }
        ActiveTool::PlaceStation(mode) => {
            let cash = ui_state.0.as_ref().map(|st| st.cash).unwrap_or(0.0);
            Some((
                s.place_station_context(mode_word(mode), &format_cash(cash)),
                ds::text(),
            ))
        }
        ActiveTool::Route => {
            let count = tools.route_draft.len();
            let quote = tools
                .last_cost_quote
                .map(format_cash)
                .unwrap_or_else(|| s.not_quoted_yet.to_string());
            Some((
                format!(
                    "Click stations to add (Shift click toggles). Enter confirms, Esc cancels. {count} station(s) selected. Estimated cost: {quote}."
                ),
                ds::text(),
            ))
        }
        ActiveTool::Bulldoze => Some((s.bulldoze_context.to_string(), ds::WARN)),
        ActiveTool::PlaceDepot(mode) => Some((
            format!("{} {}", mode_title(mode), s.tool_depot_context),
            ds::text(),
        )),
    }
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
            let s = crate::strings::current();
            let detail = fb.error.as_deref().unwrap_or(s.unknown_error);
            push_toast(&mut toasts, s.cannot_build(detail), ToastTone::Warn);
            sfx.write(PlaySfx(Sfx::Error));
        } else if matches!(fb.meta, CmdMeta::CreateRoute { .. }) {
            sfx.write(PlaySfx(Sfx::Confirm));
        }
    }
}

/// Mirrors the capped [`ToastLog::push`] helper so this file's command
/// feedback can't grow the log unbounded either.
fn push_toast(toasts: &mut ToastLog, message: String, tone: ToastTone) {
    toasts.push(message, tone);
}

// ---------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------

pub struct MfBuildUiPlugin;

impl Plugin for MfBuildUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, command_feedback_listener_system)
            .add_systems(
                EguiPrimaryContextPass,
                build_toolbar_system
                    .run_if(in_state(AppState::InGame))
                    .run_if(crate::egui_idle::egui_content_active)
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
}
