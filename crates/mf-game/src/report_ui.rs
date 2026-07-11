//! End-of-scenario report overlay (ship-plan #25, v0.4): a read-only,
//! full-screen egui overlay shown once a [`crate::campaign::ScenarioOutcome`]
//! transitions to `Failed`/`Finished`. Layering deliberately mirrors
//! `hud.rs`'s pause overlay (`egui::Area`s at `Order::Foreground`, a dim
//! scrim first, then a centered card) rather than reusing it directly:
//! `hud.rs` is off-limits this wave (parallel agent), so the handful of
//! lines that make that layering choice work are duplicated here rather
//! than shared.
//!
//! Not yet wired into `main.rs`'s `add_plugins` this wave — see
//! `campaign.rs`'s module doc for the same "lands unwired, `v04/integration`
//! registers it" convention.
#![allow(dead_code)]

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use mf_protocol::{FailReason, UiState};
use mf_state::LatestUi;

use crate::audio::{PlaySfx, Sfx};
use crate::campaign::{objectives_for, CampaignProgress, ScenarioOutcome};
use crate::design_system as ds;
use crate::state::{AppState, PendingInit};

/// Dash-free verdict heading for the four ways a scenario can end. `Finished`
/// (clean max-day out) reads as success copy; the three `Failed` reasons
/// each get their own plain-language line rather than echoing the wire
/// enum's name.
fn verdict_heading(outcome: ScenarioOutcome) -> &'static str {
    let s = crate::strings::current();
    match outcome {
        ScenarioOutcome::Failed(FailReason::Bankrupt) => s.verdict_bankrupt,
        ScenarioOutcome::Failed(FailReason::Approval) => s.verdict_lost_faith,
        ScenarioOutcome::Failed(FailReason::Time) => s.verdict_time_up,
        ScenarioOutcome::Finished => s.verdict_complete,
        // Not shown by `report_ui_system` (only Failed/Finished trigger the
        // overlay) - present so the match stays exhaustive rather than
        // needing a wildcard arm that could silently swallow a future
        // ScenarioOutcome variant.
        ScenarioOutcome::Playing | ScenarioOutcome::Completed => "",
    }
}

/// Net for the most-recently-completed day: prefers `net_history`'s last
/// entry (the sidecar's own rolling figure) and falls back to summing
/// `last_day`'s ledger fields if history is empty (e.g. day 1, before a
/// full 7-day window exists) rather than showing a misleading zero.
fn last_day_net(state: &UiState) -> f64 {
    state.net_history.last().copied().unwrap_or(
        state.last_day.fares + state.last_day.subsidy
            - state.last_day.operations
            - state.last_day.maintenance
            - state.last_day.interest,
    )
}

/// Comma-grouped integer — same shorthand `campaign.rs` uses for goal
/// descriptions (kept file-local rather than shared: neither file may
/// touch `hud.rs`, which has its own private copy of the same one-liner).
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

/// Signed cash string (`"$1,234"` / `"-$500"`) — `format_thousands` alone
/// clamps negatives to 0 (fine for population/trip counts, which are never
/// negative), but a day's net income is routinely negative and losing that
/// sign on the report would misreport an actual loss as "$0".
fn format_signed_cash(value: f64) -> String {
    if value < 0.0 {
        format!("-${}", format_thousands(value.abs()))
    } else {
        format!("${}", format_thousands(value))
    }
}

fn key_number_row(ui: &mut egui::Ui, label: &str, value: String) {
    ui.horizontal(|ui| {
        ui.set_width(240.0);
        ui.label(ds::label_muted(label));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(ds::value_strong(value));
        });
    });
}

/// Draws the star-rating row: [`CampaignProgress::stars`]'s best-ever count
/// for the active city, filled `GOOD` up to that count and `MUTED` after —
/// see `design_system.rs`'s `IconKind::Star` doc for why a filled glyph
/// (not an outline) carries the earned/not-earned distinction via color.
fn star_row(ui: &mut egui::Ui, earned: u8) {
    let (rect, _response) = ui.allocate_exact_size(egui::vec2(120.0, 36.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let star_size = 32.0;
    for i in 0..3u8 {
        let star_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + i as f32 * (star_size + 8.0), rect.top()),
            egui::vec2(star_size, star_size),
        );
        let color = if i < earned { ds::GOOD } else { ds::muted() };
        ds::icon(&painter, star_rect, ds::IconKind::Star, color, 1.5);
    }
}

/// True the first frame `outcome` differs from whatever was last dismissed
/// (or never dismissed at all) — the "shows once per outcome" latch. Split
/// out from the system itself so the transition logic is unit-testable
/// without an `egui::Context`/`Ui`.
fn should_show(outcome: ScenarioOutcome, dismissed_for: Option<ScenarioOutcome>) -> bool {
    matches!(
        outcome,
        ScenarioOutcome::Failed(_) | ScenarioOutcome::Finished
    ) && dismissed_for != Some(outcome)
}

#[allow(clippy::too_many_arguments)]
fn report_ui_system(
    mut contexts: EguiContexts,
    outcome: Res<ScenarioOutcome>,
    ui_state: Res<LatestUi>,
    progress: Res<CampaignProgress>,
    pending: Res<PendingInit>,
    mut next_state: ResMut<NextState<AppState>>,
    mut sfx: EventWriter<PlaySfx>,
    mut dismissed_for: Local<Option<ScenarioOutcome>>,
    mut hovered: Local<Option<egui::Id>>,
) -> Result {
    if !should_show(*outcome, *dismissed_for) {
        return Ok(());
    }
    let Some(state) = &ui_state.0 else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let fade = ds::animate(ctx, egui::Id::new("report_fade"), 1.0);

    ds::modal(ctx, egui::Id::new("report_modal"), fade, |ui| {
        ui.set_width(360.0);
        let s = crate::strings::current();
        ui.vertical_centered(|ui| {
            ui.label(ds::heading(verdict_heading(*outcome)));
            ui.add_space(ds::SPACE_MD);

            let earned = progress.stars(&pending.preset_key);
            star_row(ui, earned);
            if let Some(objectives) = objectives_for(&pending.preset_key) {
                for (i, goal) in objectives.stars.iter().enumerate() {
                    let text = crate::campaign::describe_goal(*goal);
                    let rich = if (i as u8) < earned {
                        ds::label_body(text)
                    } else {
                        ds::label_muted(text)
                    };
                    ui.label(rich);
                }
            }

            ui.add_space(ds::SPACE_MD);
            thin_separator(ui);
            ui.add_space(ds::SPACE_XS);

            key_number_row(ui, s.day, format!("{}", state.day));
            key_number_row(ui, s.population_served, format_thousands(state.population));
            key_number_row(
                ui,
                s.daily_transit_trips,
                format_thousands(state.daily_transit_trips),
            );
            key_number_row(ui, s.approval, format!("{:.0}%", state.approval));
            key_number_row(ui, s.coverage, format!("{:.0}%", state.coverage * 100.0));
            key_number_row(ui, s.net_last_day, format_signed_cash(last_day_net(state)));

            ui.add_space(ds::SPACE_LG);

            if matches!(*outcome, ScenarioOutcome::Finished) {
                let keep_playing = ds::button_sized(
                    ui,
                    s.keep_playing,
                    ds::ButtonKind::Primary,
                    Some(egui::vec2(220.0, 40.0)),
                );
                hover_tick(&keep_playing, &mut hovered, &mut sfx);
                if keep_playing.clicked() {
                    sfx.write(PlaySfx(Sfx::Confirm));
                    *dismissed_for = Some(*outcome);
                }
                ui.add_space(ds::SPACE_XS);
            }

            let back_to_menu = ds::button_sized(
                ui,
                s.back_to_menu,
                ds::ButtonKind::Ghost,
                Some(egui::vec2(220.0, 40.0)),
            );
            hover_tick(&back_to_menu, &mut hovered, &mut sfx);
            if back_to_menu.clicked() {
                sfx.write(PlaySfx(Sfx::Cancel));
                *dismissed_for = Some(*outcome);
                next_state.set(AppState::MainMenu);
            }
        });
    });

    Ok(())
}

fn thin_separator(ui: &mut egui::Ui) {
    ds::thin_separator(ui);
}

/// One hover tick the first frame the pointer lands on a widget — same
/// shorthand `hud.rs` uses for its own buttons, duplicated locally for the
/// same reason as `thin_separator` above.
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

pub struct MfReportUiPlugin;

impl Plugin for MfReportUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            EguiPrimaryContextPass,
            report_ui_system
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
    fn verdict_heading_is_dash_free_for_every_shown_outcome() {
        let outcomes = [
            ScenarioOutcome::Failed(FailReason::Bankrupt),
            ScenarioOutcome::Failed(FailReason::Approval),
            ScenarioOutcome::Failed(FailReason::Time),
            ScenarioOutcome::Finished,
        ];
        for outcome in outcomes {
            let text = verdict_heading(outcome);
            assert!(!text.is_empty(), "{outcome:?} produced an empty heading");
            assert!(!text.contains('-'), "{text:?} contains a dash");
            assert!(!text.contains('\u{2013}'), "{text:?} contains an en dash");
            assert!(!text.contains('\u{2014}'), "{text:?} contains an em dash");
        }
    }

    #[test]
    fn verdict_heading_maps_each_fail_reason_distinctly() {
        assert_eq!(
            verdict_heading(ScenarioOutcome::Failed(FailReason::Bankrupt)),
            "Bankrupt"
        );
        assert_eq!(
            verdict_heading(ScenarioOutcome::Failed(FailReason::Approval)),
            "The city lost faith"
        );
        assert_eq!(
            verdict_heading(ScenarioOutcome::Failed(FailReason::Time)),
            "Time is up"
        );
        assert_eq!(
            verdict_heading(ScenarioOutcome::Finished),
            "Scenario complete"
        );
    }

    fn base_ui() -> UiState {
        UiState {
            tick: 0,
            insights: Vec::new(),
            day: 5,
            speed: 1.0,
            cash: 500_000.0,
            loan_balance: 0.0,
            last_day: mf_protocol::DayLedger {
                fares: 1_000.0,
                subsidy: 200.0,
                operations: 500.0,
                maintenance: 100.0,
                interest: 50.0,
            },
            net_history: Vec::new(),
            population: 1_000_000.0,
            approval: 50.0,
            transit_share: 0.0,
            coverage: 0.0,
            daily_transit_trips: 0.0,
            unlocked_modes: Vec::new(),
            stations: Vec::new(),
            tracks: Vec::new(),
            routes: Vec::new(),
            active_events: Vec::new(),
            fields_version: 0,
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
            overcrowded_routes: None,
        }
    }

    #[test]
    fn last_day_net_prefers_net_history_tail() {
        let mut state = base_ui();
        state.net_history = vec![10.0, -5.0, 42.0];
        assert_eq!(last_day_net(&state), 42.0);
    }

    #[test]
    fn last_day_net_falls_back_to_ledger_sum_when_history_is_empty() {
        let state = base_ui();
        // 1000 + 200 - 500 - 100 - 50 = 550
        assert_eq!(last_day_net(&state), 550.0);
    }

    #[test]
    fn should_show_true_on_a_fresh_failed_outcome() {
        assert!(should_show(
            ScenarioOutcome::Failed(FailReason::Bankrupt),
            None
        ));
    }

    #[test]
    fn should_show_false_once_dismissed_for_the_same_outcome() {
        let outcome = ScenarioOutcome::Finished;
        assert!(!should_show(outcome, Some(outcome)));
    }

    #[test]
    fn should_show_true_again_when_the_outcome_changes_after_a_dismiss() {
        // Dismissed while Finished, then the scenario later goes bankrupt
        // (sandbox play continued past Finished) - a genuinely new outcome
        // must show again even though *something* was already dismissed.
        assert!(should_show(
            ScenarioOutcome::Failed(FailReason::Bankrupt),
            Some(ScenarioOutcome::Finished)
        ));
    }

    #[test]
    fn should_show_false_while_still_playing_or_merely_completed() {
        assert!(!should_show(ScenarioOutcome::Playing, None));
        assert!(!should_show(ScenarioOutcome::Completed, None));
    }

    #[test]
    fn format_thousands_groups_by_three() {
        assert_eq!(format_thousands(1_234_567.0), "1,234,567");
        assert_eq!(format_thousands(42.0), "42");
        assert_eq!(format_thousands(0.0), "0");
    }

    #[test]
    fn format_signed_cash_keeps_the_minus_sign_on_a_loss() {
        // This is the bug this helper exists to avoid: `format_thousands`
        // alone clamps negatives to 0, which would silently misreport a
        // losing day as "$0" instead of "-$500".
        assert_eq!(format_signed_cash(-500.0), "-$500");
        assert_eq!(format_signed_cash(1_500.0), "$1,500");
        assert_eq!(format_signed_cash(0.0), "$0");
    }
}
