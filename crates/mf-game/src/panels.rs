//! Station inspection + finance panels (ship-plan #25, v0.3): the sim
//! already computes everything these two windows show (`UiStation`'s
//! ridership/alightings, `UiState`'s ledger/net-history/insights) - nothing
//! in the native client rendered any of it before this file existed.
//!
//! Scope boundary this file holds to: `tools.rs` (this wave's other
//! addition) owns *picking* a station via `SelectedTarget`; this file only
//! reads that resource and draws. It does not touch `hud.rs`/`build_ui.rs`.
//!
//! `MfPanelsPlugin` is deliberately NOT registered in `main.rs` this wave
//! (only `mod panels;` was added there, for compilation) - same "lands on
//! its own branch, wired in at integration" convention `tools.rs`'s and
//! `command_bus.rs`'s own module docs describe for v0.2 (see
//! `v02/integration`, which is what actually assembled those two branches
//! together; this repo already has a parallel `v03/demand-overlay` branch,
//! so a `v03/integration` pass is the expected place to add
//! `panels::MfPanelsPlugin` to `main.rs`'s `add_plugins` call). Until then
//! every system below is unreachable from `main`, hence the blanket allow.
#![allow(dead_code)]

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use mf_net::SimLink;
use mf_protocol::{
    Command, DayLedger, FailReason, TransitMode, UiDistrict, UiRoute, UiState, UiStation,
};
use mf_state::LatestUi;

use crate::command_bus::{CmdMeta, CommandBus};
use crate::design_system as ds;
use crate::state::AppState;
use crate::tools::SelectedTarget;

// ---------------------------------------------------------------------
// Tunable constants
// ---------------------------------------------------------------------

/// Vertical offset (px) both floating windows anchor below, so neither one
/// visually starts on top of `hud.rs`'s `hud_top` bar. That bar's own
/// height isn't exposed as a constant either file can read (it's an
/// egui-computed auto-height), so this is a generous fixed guess - a
/// floating `egui::Window` is user-draggable afterward regardless, this
/// only sets where it first appears.
const PANEL_TOP_OFFSET_PX: f32 = 56.0;
/// Station panel default width; narrower than the route panel's 300px
/// `SidePanel` default since this window carries less content per station.
const STATION_PANEL_WIDTH: f32 = 260.0;
const FINANCE_PANEL_WIDTH: f32 = 300.0;
/// Station level cap, mirrored from `metroforge/src/core/commands.ts`'s
/// `upgradeStation` handler (`if (station.level >= 5) return {..error..}`).
/// Duplicated as a plain constant rather than added to the wire protocol:
/// the client only needs it to stop drawing an Upgrade button once further
/// upgrades are meaningless, it never needs to compute cost itself (the
/// sidecar quotes/authorizes the actual charge).
const MAX_STATION_LEVEL: u32 = 5;

// ---------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------

/// Finance panel open/closed. Independent of `SelectedTarget`'s station
/// panel: either can be open, both, or neither.
#[derive(Resource, Default)]
pub struct FinancePanelState {
    pub open: bool,
}

// ---------------------------------------------------------------------
// Pure helpers (no ECS/egui types beyond plain data), unit-tested below.
// ---------------------------------------------------------------------

/// Comma-grouped integer, e.g. `146015` -> `"146,015"`. `hud.rs` and
/// `build_ui.rs` each already carry a private copy of this same helper
/// (neither is `pub`, so neither can be imported here) - this is a third
/// plain reimplementation, not a deliberate fork of behavior.
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

fn mode_word(mode: TransitMode) -> &'static str {
    let s = crate::strings::current();
    match mode {
        TransitMode::Bus => s.mode_bus,
        TransitMode::Tram => s.mode_tram,
        TransitMode::Metro => s.mode_metro,
        TransitMode::Rail => s.mode_rail,
    }
}

/// Maps a station's transit mode onto the closest existing toolbar glyph.
/// `design_system::IconKind` only has hand-drawn vehicle silhouettes for
/// `Bus`/`Tram` (the two starting modes) - `Metro`/`Rail` have no dedicated
/// icon yet, and drawing one is out of scope for this wave (the mission
/// scopes `design_system.rs`'s extension to just the new `sparkline`
/// helper, which is reusable in a way a mode-specific icon glyph isn't).
/// Falling back to `StationPin` for those two modes still reads correctly
/// as "this is a station," just without a mode-specific silhouette.
fn station_icon_kind(mode: TransitMode) -> ds::IconKind {
    match mode {
        TransitMode::Bus => ds::IconKind::Bus,
        TransitMode::Tram => ds::IconKind::Tram,
        TransitMode::Metro | TransitMode::Rail => ds::IconKind::StationPin,
    }
}

/// Vivid route color table - duplicated from `mf-render`'s private
/// `palette::vivid_route_color` (that module isn't visible to `mf-game`)
/// and, separately, from `tools.rs`'s and `build_ui.rs`'s own already-
/// duplicated copies of the same eight values. A fourth copy rather than a
/// shared crate follows the precedent those two files already set (see
/// `build_ui.rs`'s own note on its copy) instead of introducing a new
/// sharing mechanism in this wave.
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

/// Indices (position within `routes`, matching the same 0-based index
/// `build_ui.rs`'s route panel and `tools.rs`'s ghost preview both already
/// use for `vivid_route_color`) of every route whose `station_ids` includes
/// `station_id`. Order-preserving, so a station's serving routes always
/// list in the same stable order `routes` itself is in.
fn routes_serving_station(routes: &[UiRoute], station_id: i64) -> Vec<usize> {
    routes
        .iter()
        .enumerate()
        .filter(|(_, r)| r.station_ids.contains(&station_id))
        .map(|(idx, _)| idx)
        .collect()
}

/// Nearest catchment district (sim-depth, PR #31) to a point, by squared
/// euclidean distance to each district centroid. Returns `None` when no
/// districts are present (old sidecars send none), so the caller can simply
/// skip the catchment line rather than inventing one.
fn nearest_district(districts: &[UiDistrict], x: f64, y: f64) -> Option<&UiDistrict> {
    districts.iter().min_by(|a, b| {
        let da = (a.x - x).powi(2) + (a.y - y).powi(2);
        let db = (b.x - x).powi(2) + (b.y - y).powi(2);
        da.total_cmp(&db)
    })
}

/// `last_day`'s net for the day: fares + subsidy (income) minus operations,
/// maintenance and interest (costs). Mirrors
/// `metroforge/src/core/sim.ts`'s `runDailyEconomy` exactly (`const net =
/// fares + subsidy - operations - maintenance - interest`) - the wire
/// `DayLedger` doesn't carry this precomputed since `UiState.netHistory`
/// already exists for the rolling series, but the panel wants today's own
/// net as a labeled row too.
fn day_ledger_net(ledger: &DayLedger) -> f64 {
    ledger.fares + ledger.subsidy - ledger.operations - ledger.maintenance - ledger.interest
}

/// Plain-language banner text for a run that has ended, or `None` for a
/// still-live one. `ui.failed` is the authoritative reason once set; the
/// separate `ui.bankrupt` bool is checked too as a defensive fallback (the
/// sidecar's own `sim.worker.ts` sets `bankrupt` and `failed` from the same
/// event in the same tick, but nothing here should assume wire fields can
/// never race against each other by a frame). Copy is deliberately plain
/// sentences, no dashes, matching the sim's own `computeInsights` style.
fn fail_banner_text(ui: &UiState) -> Option<&'static str> {
    let s = crate::strings::current();
    if let Some(reason) = ui.failed {
        return Some(match reason {
            FailReason::Bankrupt => s.bankrupt_banner,
            FailReason::Approval => s.approval_collapsed_banner,
            FailReason::Time => s.time_up_banner,
        });
    }
    if ui.bankrupt {
        return Some(s.bankrupt_banner);
    }
    None
}

fn station_title(station: &UiStation) -> String {
    if station.name.trim().is_empty() {
        crate::strings::current().station(station.id)
    } else {
        station.name.clone()
    }
}

fn route_display_name(route: &UiRoute, idx: usize) -> String {
    if route.name.trim().is_empty() {
        crate::strings::current().line(idx + 1)
    } else {
        route.name.clone()
    }
}

// ---------------------------------------------------------------------
// Small egui row helpers (thin wrappers around `design_system`, kept here
// since they're one-line-of-content-specific rather than general-purpose).
// ---------------------------------------------------------------------

fn stat_row(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
    ui.horizontal(|ui| {
        ui.label(ds::label_muted(label));
        ui.label(ds::value_strong(value.into()));
    });
}

/// A cash row signed by its OWN VALUE's sign (not an external flag) -
/// used both for the day's five ledger categories (whose signed value is
/// computed as +magnitude for income, -magnitude for a cost, see call
/// sites below) and for the "Net" row, whose sign is whatever the actual
/// computed net happens to be.
fn money_row(ui: &mut egui::Ui, label: &str, signed_value: f64) {
    ui.horizontal(|ui| {
        ui.label(ds::label_muted(label));
        let good = signed_value >= 0.0;
        let prefix = if good { "+" } else { "-" };
        let color = if good { ds::GOOD } else { ds::BAD };
        ui.label(
            ds::value_strong(format!("{prefix}{}", format_cash(signed_value.abs()))).color(color),
        );
    });
}

fn insight_row(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        let (bar_rect, _) = ui.allocate_exact_size(egui::vec2(3.0, 18.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(bar_rect, egui::CornerRadius::ZERO, ds::WARN);
        ui.add_space(ds::SPACE_XXS);
        ui.label(ds::label_body(text));
    });
}

/// The Upgrade button, or a "max level" note once the station can no
/// longer be upgraded. Submits `Command::UpgradeStation` through the shared
/// `CommandBus`.
///
/// `CmdMeta` decision: the enum (see `command_bus.rs`) has no dedicated
/// upgrade variant, and none of the existing ones fit semantically -
/// `BuildStation`/`BuildTrack`/`CreateRoute` all pair with an inverse
/// demolish/delete command via `created_id` (`UpgradeStation` returns no
/// `createdId`, and the sim has no "downgrade" command to invert into
/// anyway - see `metroforge/src/core/commands.ts`'s handler, which just
/// increments `station.level` in place), and `EditRoute`/`Demolish` both
/// name a DIFFERENT concrete action a toast/log line would misreport this
/// as. `CmdMeta::Query` is the closest fit: it's `command_bus.rs`'s catch-
/// all "not undoable" bucket (`inverse_for` already maps it, `EditRoute`,
/// `Demolish` and `Undo` all to `None`) and, unlike those three, it isn't
/// actually used by any real call site today - `tools.rs`'s own
/// `ToSim::QueryTrackCost` bypasses `CommandBus` entirely per that module's
/// doc comment, so `Query` is otherwise dead outside `command_bus.rs`'s own
/// tests. Reusing it here doesn't collide with anything and costs nothing;
/// adding a dedicated `CmdMeta::Upgrade { station_id }` variant instead
/// would be the more honest long-term fix but is out of this file's owned
/// surface (`command_bus.rs` isn't in this wave's file list).
fn station_upgrade_button(
    ui: &mut egui::Ui,
    station: &UiStation,
    link: Option<&SimLink>,
    bus: &mut CommandBus,
) {
    let s = crate::strings::current();
    if station.level >= MAX_STATION_LEVEL {
        ui.label(ds::label_small(s.station_max_level));
        return;
    }
    let resp = ui.add(
        egui::Button::new(
            egui::RichText::new(s.upgrade_to_level(station.level + 1))
                .color(egui::Color32::WHITE),
        )
        .fill(ds::accent())
        .corner_radius(ds::CORNER_RADIUS),
    );
    if resp.clicked() {
        if let Some(link) = link {
            bus.submit(
                link,
                Command::UpgradeStation {
                    station_id: station.id,
                },
                CmdMeta::Query,
            );
        }
    }
}

// ---------------------------------------------------------------------
// Station inspection panel
// ---------------------------------------------------------------------

/// Floating window shown while `SelectedTarget::Station(id)` names a
/// station still present in `LatestUi`. Uses `egui::Window` (not a
/// `SidePanel` like `build_ui.rs`'s route panel) since it's transient and
/// keyed by whatever the player last clicked, rather than a persistent
/// toggle-able panel - a `Window` is also independently draggable, so it
/// never has to fight the route panel's `SidePanel` for the same screen
/// column if both happen to be open at once.
#[allow(clippy::too_many_arguments)]
fn station_panel_system(
    mut contexts: EguiContexts,
    ui_state: Res<LatestUi>,
    link: Option<Res<SimLink>>,
    mut bus: ResMut<CommandBus>,
    mut selected: ResMut<SelectedTarget>,
) -> Result {
    let SelectedTarget::Station(station_id) = *selected else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let Some(state) = &ui_state.0 else {
        return Ok(());
    };
    let Some(station) = state.stations.iter().find(|s| s.id == station_id) else {
        // The selected station vanished (bulldozed by this client, or a
        // future multiplayer client) - same stale-selection drop
        // `build_ui.rs`'s route panel already does for a deleted route,
        // just clearing `tools.rs`'s selection resource instead of this
        // panel's own local state.
        *selected = SelectedTarget::None;
        return Ok(());
    };

    let mut open = true;
    egui::Window::new(station_title(station))
        // Fixed id: the title text changes per selected station, and
        // `egui::Window::new`'s own doc is explicit that a changing title
        // requires `.id(...)` with a value that doesn't change, or the
        // window loses its position/collapsed state every time the
        // selection changes.
        .id(egui::Id::new("mf_station_inspection_panel"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(STATION_PANEL_WIDTH)
        .anchor(
            egui::Align2::RIGHT_TOP,
            egui::vec2(-ds::SPACE_MD, PANEL_TOP_OFFSET_PX),
        )
        .frame(
            egui::Frame::default()
                .fill(ds::panel_bg())
                .inner_margin(egui::Margin::symmetric(
                    ds::SPACE_SM as i8,
                    ds::SPACE_SM as i8,
                )),
        )
        .show(ctx, |ui| {
            let s = crate::strings::current();
            ui.horizontal(|ui| {
                let (icon_rect, _) =
                    ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
                ds::icon(
                    ui.painter(),
                    icon_rect,
                    station_icon_kind(station.mode),
                    ds::accent(),
                    1.6,
                );
                ui.label(ds::heading(station_title(station)));
            });
            ui.label(ds::label_small(s.level_mode(station.level, mode_word(station.mode))));
            ui.add_space(ds::SPACE_SM);

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(ds::label_muted(s.boarding_per_day));
                    ui.label(ds::value_strong(format_thousands(station.ridership)));
                });
                ui.add_space(ds::SPACE_MD);
                ui.vertical(|ui| {
                    ui.label(ds::label_muted(s.arriving_per_day));
                    ui.label(ds::value_strong(format_thousands(station.alightings)));
                });
            });

            // Sim-depth (PR #31): the district this station sits in, so the
            // player can see the population/jobs it draws from. Only drawn
            // when the sidecar sends districts (old ones send none).
            if let Some(district) = nearest_district(&state.districts, station.x, station.y) {
                ui.add_space(ds::SPACE_SM);
                ui.separator();
                ui.add_space(ds::SPACE_XXS);
                ui.label(ds::label_muted(s.catchment_district));
                ui.label(ds::value_strong(district.name.clone()));
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(ds::label_muted(s.people));
                        ui.label(ds::value_strong(format_thousands(district.population)));
                    });
                    ui.add_space(ds::SPACE_MD);
                    ui.vertical(|ui| {
                        ui.label(ds::label_muted(s.jobs));
                        ui.label(ds::value_strong(format_thousands(district.jobs)));
                    });
                });
            }

            ui.add_space(ds::SPACE_SM);
            ui.separator();
            ui.add_space(ds::SPACE_XXS);

            ui.label(ds::label_muted(s.routes_serving_station));
            let serving = routes_serving_station(&state.routes, station.id);
            if serving.is_empty() {
                ui.label(ds::label_small(s.no_routes_reach_station));
            } else {
                for idx in serving {
                    let route = &state.routes[idx];
                    ui.horizontal(|ui| {
                        let (swatch, _) =
                            ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                        ui.painter()
                            .rect_filled(swatch, ds::CORNER_RADIUS, vivid_route_color(idx));
                        ui.label(ds::label_body(route_display_name(route, idx)));
                    });
                }
            }

            ui.add_space(ds::SPACE_SM);
            ui.separator();
            ui.add_space(ds::SPACE_XXS);
            station_upgrade_button(ui, station, link.as_deref(), &mut bus);
        });

    if !open {
        *selected = SelectedTarget::None;
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Finance panel
// ---------------------------------------------------------------------

/// `F` toggles the finance panel. Owned here (rather than in `hud.rs` or
/// `tools.rs`, which each already claim their own keybinds - `hud.rs`'s
/// speed/subway buttons and the pause toggle in `input.rs`, `tools.rs`'s
/// `1`/`2`/`3`/Enter) so this wave's new keybind can land as one self-
/// contained addition without editing either of those files, per this
/// mission's scope boundary. Flagged here for whoever wires
/// `MfPanelsPlugin` in at `v03/integration` time: if another parallel v0.3
/// branch (e.g. `v03/demand-overlay`) also binds `KeyCode::KeyF`, that's a
/// real conflict to resolve at that merge, not something this file can
/// detect on its own from an isolated worktree.
fn finance_keybind_system(keys: Res<ButtonInput<KeyCode>>, mut panel: ResMut<FinancePanelState>) {
    if keys.just_pressed(KeyCode::KeyF) {
        panel.open = !panel.open;
    }
}

/// Cash/loan, yesterday's ledger breakdown, a 7-day net sparkline, three
/// headline stats, and the sim's own plain-language insights - the
/// station panel is one particular THING you selected, this one is the
/// whole city's finances at a glance.
fn finance_panel_system(
    mut contexts: EguiContexts,
    ui_state: Res<LatestUi>,
    panel: Res<FinancePanelState>,
) -> Result {
    if !panel.open {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let Some(state) = &ui_state.0 else {
        return Ok(());
    };

    let s = crate::strings::current();
    egui::Window::new(s.finance)
        .id(egui::Id::new("mf_finance_panel"))
        .collapsible(false)
        .resizable(false)
        .default_width(FINANCE_PANEL_WIDTH)
        // Left side, `hud_top`'s cash readout is already up there so a
        // player's eye is already trained on this corner for money; the
        // station panel anchors RIGHT, so the two never start stacked on
        // top of each other even if both are open at once.
        .anchor(
            egui::Align2::LEFT_TOP,
            egui::vec2(ds::SPACE_MD, PANEL_TOP_OFFSET_PX),
        )
        .frame(
            egui::Frame::default()
                .fill(ds::panel_bg())
                .inner_margin(egui::Margin::symmetric(
                    ds::SPACE_SM as i8,
                    ds::SPACE_SM as i8,
                )),
        )
        .show(ctx, |ui| {
            if let Some(banner) = fail_banner_text(state) {
                egui::Frame::default()
                    .fill(ds::BAD)
                    .inner_margin(egui::Margin::symmetric(
                        ds::SPACE_SM as i8,
                        ds::SPACE_XS as i8,
                    ))
                    .corner_radius(ds::CORNER_RADIUS)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(banner)
                                .color(egui::Color32::WHITE)
                                .strong(),
                        );
                    });
                ui.add_space(ds::SPACE_SM);
            }

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(ds::label_muted(s.cash));
                    ui.label(ds::value_strong(format_cash(state.cash)));
                });
                ui.add_space(ds::SPACE_MD);
                ui.vertical(|ui| {
                    ui.label(ds::label_muted(s.loan_balance));
                    ui.label(ds::value_strong(format_cash(state.loan_balance)));
                });
            });

            ui.add_space(ds::SPACE_SM);
            ui.separator();
            ui.add_space(ds::SPACE_XXS);

            ui.label(ds::label_muted(s.yesterday));
            let ld = &state.last_day;
            money_row(ui, s.fares, ld.fares);
            money_row(ui, s.subsidy, ld.subsidy);
            money_row(ui, s.operations, -ld.operations);
            money_row(ui, s.maintenance, -ld.maintenance);
            money_row(ui, s.interest, -ld.interest);
            ui.add_space(ds::SPACE_XXS);
            ui.separator();
            money_row(ui, s.net, day_ledger_net(ld));

            ui.add_space(ds::SPACE_SM);
            ui.label(ds::label_muted(s.net_last_7_days));
            ds::sparkline(
                ui,
                &state.net_history,
                egui::vec2(FINANCE_PANEL_WIDTH - 24.0, 42.0),
            );

            ui.add_space(ds::SPACE_SM);
            ui.separator();
            ui.add_space(ds::SPACE_XXS);
            stat_row(
                ui,
                s.transit_share,
                format!("{:.0}%", state.transit_share * 100.0),
            );
            stat_row(ui, s.coverage, format!("{:.0}%", state.coverage * 100.0));
            stat_row(
                ui,
                s.daily_transit_trips,
                format_thousands(state.daily_transit_trips),
            );

            // Sim-depth (PR #31): farebox recovery + lifetime earnings, shown
            // only when the sidecar actually sends them (old sidecars omit
            // both, so these rows simply don't appear rather than reading 0).
            if let Some(recovery) = state.farebox_recovery {
                stat_row(ui, s.farebox_recovery, format!("{:.0}%", recovery * 100.0));
            }
            if let Some(lifetime) = state.lifetime {
                stat_row(ui, s.lifetime_earnings, format_cash(lifetime));
            }

            if !state.insights.is_empty() {
                ui.add_space(ds::SPACE_SM);
                ui.separator();
                ui.add_space(ds::SPACE_XXS);
                ui.label(ds::label_muted(s.insights));
                for insight in &state.insights {
                    insight_row(ui, insight);
                }
            }
        });

    Ok(())
}

// ---------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------

pub struct MfPanelsPlugin;

impl Plugin for MfPanelsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FinancePanelState>()
            .add_systems(
                Update,
                finance_keybind_system.run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                EguiPrimaryContextPass,
                (station_panel_system, finance_panel_system)
                    .run_if(in_state(AppState::InGame))
                    .run_if(|| !crate::design_system::hud_hidden()),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(id: i64, name: &str, station_ids: Vec<i64>) -> UiRoute {
        UiRoute {
            id,
            name: name.to_string(),
            color: "#000000".to_string(),
            mode: TransitMode::Bus,
            station_ids,
            headway_seconds: 300.0,
            fare: 2.0,
            vehicle_count: 2,
            daily_ridership: 0.0,
            daily_revenue: 0.0,
            length_meters: 0.0,
            capacity: 0.0,
            load: 0.0,
            crowding: 0.0,
            segment_loads: Vec::new(),
            live_crowding: None,
            operating_cost: None,
            farebox: None,
        }
    }

    fn ledger(
        fares: f64,
        subsidy: f64,
        operations: f64,
        maintenance: f64,
        interest: f64,
    ) -> DayLedger {
        DayLedger {
            fares,
            subsidy,
            operations,
            maintenance,
            interest,
        }
    }

    // --- routes_serving_station ---------------------------------------

    #[test]
    fn routes_serving_station_finds_every_match_in_order() {
        let routes = vec![
            route(1, "A", vec![10, 20]),
            route(2, "B", vec![30, 40]),
            route(3, "C", vec![20, 40]),
        ];
        assert_eq!(routes_serving_station(&routes, 20), vec![0, 2]);
        assert_eq!(routes_serving_station(&routes, 40), vec![1, 2]);
        assert_eq!(routes_serving_station(&routes, 999), Vec::<usize>::new());
    }

    #[test]
    fn routes_serving_station_empty_routes_is_empty() {
        assert_eq!(routes_serving_station(&[], 1), Vec::<usize>::new());
    }

    // --- nearest_district ------------------------------------------------

    fn district(id: i64, name: &str, x: f64, y: f64) -> UiDistrict {
        UiDistrict {
            id,
            name: name.to_string(),
            x,
            y,
            population: 1000.0,
            jobs: 500.0,
        }
    }

    #[test]
    fn nearest_district_picks_the_closest_centroid() {
        let districts = vec![
            district(1, "Downtown", 0.0, 0.0),
            district(2, "Riverside", 100.0, 0.0),
        ];
        assert_eq!(nearest_district(&districts, 10.0, 5.0).unwrap().id, 1);
        assert_eq!(nearest_district(&districts, 90.0, -5.0).unwrap().id, 2);
    }

    #[test]
    fn nearest_district_is_none_without_districts() {
        assert!(nearest_district(&[], 0.0, 0.0).is_none());
    }

    // --- day_ledger_net --------------------------------------------------

    #[test]
    fn day_ledger_net_matches_the_sim_formula() {
        let ld = ledger(100.0, 50.0, 30.0, 10.0, 5.0);
        assert!((day_ledger_net(&ld) - 105.0).abs() < 0.001);
    }

    #[test]
    fn day_ledger_net_can_go_negative() {
        let ld = ledger(10.0, 0.0, 40.0, 5.0, 1.0);
        assert!(day_ledger_net(&ld) < 0.0);
    }

    // --- fail_banner_text --------------------------------------------------

    fn base_ui_state() -> UiState {
        UiState {
            tick: 0,
            insights: Vec::new(),
            day: 1,
            speed: 1.0,
            cash: 0.0,
            loan_balance: 0.0,
            last_day: ledger(0.0, 0.0, 0.0, 0.0, 0.0),
            net_history: Vec::new(),
            population: 0.0,
            approval: 50.0,
            transit_share: 0.0,
            coverage: 0.0,
            daily_transit_trips: 0.0,
            unlocked_modes: Vec::new(),
            stations: Vec::new(),
            tracks: Vec::new(),
            routes: Vec::new(),
            active_events: Vec::new(),
            fields_version: 1,
            bankrupt: false,
            failed: None,
            max_day: None,
            era_label: None,
            command_count: 0,
            hour_of_day: None,
            demand_factor: None,
            farebox_recovery: None,
            lifetime: None,
            districts: Vec::new(),
            overcrowded_routes: Vec::new(),
        }
    }

    #[test]
    fn fail_banner_none_for_a_live_run() {
        assert_eq!(fail_banner_text(&base_ui_state()), None);
    }

    #[test]
    fn fail_banner_prefers_failed_reason_over_the_bankrupt_bool() {
        let mut ui = base_ui_state();
        ui.bankrupt = true;
        ui.failed = Some(FailReason::Approval);
        assert_eq!(
            fail_banner_text(&ui),
            Some("Approval collapsed. Your network has been shut down.")
        );
    }

    #[test]
    fn fail_banner_falls_back_to_the_bankrupt_bool_alone() {
        let mut ui = base_ui_state();
        ui.bankrupt = true;
        assert!(fail_banner_text(&ui).is_some());
    }

    // --- station_title / route_display_name -------------------------------

    fn station(id: i64, name: &str) -> UiStation {
        UiStation {
            id,
            name: name.to_string(),
            x: 0.0,
            y: 0.0,
            mode: TransitMode::Bus,
            level: 1,
            ridership: 0.0,
            alightings: 0.0,
        }
    }

    #[test]
    fn station_title_uses_the_name_when_present() {
        assert_eq!(station_title(&station(1, "Elm Street")), "Elm Street");
    }

    #[test]
    fn station_title_falls_back_to_the_id_when_unnamed() {
        assert_eq!(station_title(&station(7, "")), "Station 7");
        assert_eq!(station_title(&station(7, "   ")), "Station 7");
    }

    #[test]
    fn route_display_name_falls_back_to_line_number() {
        assert_eq!(route_display_name(&route(1, "", vec![]), 2), "Line 3");
        assert_eq!(
            route_display_name(&route(1, "Red Line", vec![]), 2),
            "Red Line"
        );
    }

    // --- station_icon_kind --------------------------------------------

    #[test]
    fn station_icon_kind_uses_dedicated_glyphs_for_bus_and_tram() {
        assert_eq!(station_icon_kind(TransitMode::Bus), ds::IconKind::Bus);
        assert_eq!(station_icon_kind(TransitMode::Tram), ds::IconKind::Tram);
    }

    #[test]
    fn station_icon_kind_falls_back_to_station_pin_for_metro_and_rail() {
        assert_eq!(
            station_icon_kind(TransitMode::Metro),
            ds::IconKind::StationPin
        );
        assert_eq!(
            station_icon_kind(TransitMode::Rail),
            ds::IconKind::StationPin
        );
    }

    // --- format_thousands / format_cash -------------------------------------

    #[test]
    fn format_thousands_groups_by_three() {
        assert_eq!(format_thousands(146015.0), "146,015");
        assert_eq!(format_thousands(42.0), "42");
        assert_eq!(format_thousands(0.0), "0");
    }

    #[test]
    fn format_cash_prefixes_a_dollar_sign() {
        assert_eq!(format_cash(1234.0), "$1,234");
    }
}
