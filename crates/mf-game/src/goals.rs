//! Lightweight alpha goals system: a handful of client-side objectives
//! derived purely from fields already on `UiState` (no protocol changes).
//! Goals are grouped into tiers; a tier's goals only show as active once
//! every goal in the tier before it is complete, so the panel reads as a
//! short guided path rather than a wall of six bars at once.
//!
//! Completion is persisted per city (keyed by `PendingInit::preset_key`,
//! the same key `saves.rs`/`config.rs` treat as the city identity) using
//! the same `directories::ProjectDirs("com","ahousedivided","MetroForge")`
//! pattern `config.rs` uses for `config.toml` — a sibling `goals.toml`
//! under the config dir.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use mf_protocol::UiState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::audio::{PlaySfx, Sfx};
use crate::design_system as ds;
use crate::hud::ToastLog;
use crate::state::{AppState, PendingInit};
use mf_protocol::ToastTone;
use mf_state::LatestUi;

/// Stable identifier for one goal. Persisted to disk (as the string form
/// via `serde`'s default enum representation), so variants must never be
/// renamed/removed once shipped — only appended to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GoalId {
    PlaceThreeStations,
    LayFirstTrack,
    LaunchARoute,
    ReachCoverage25,
    Reach500Riders,
    ApprovalAbove60,
}

/// One goal's static definition: tier and progress function. Display copy
/// is looked up at render time via `title()` / `description()` so the
/// strings table can be swapped for a different locale without rebuilding
/// the const table.
pub struct GoalDef {
    pub id: GoalId,
    pub tier: u8,
    /// Returns `(current, target)`; the goal is complete when
    /// `current >= target`. Kept as plain numbers (not a bool) so the HUD
    /// can draw a progress bar instead of just a checkmark.
    pub progress: fn(&UiState) -> (f64, f64),
}

impl GoalDef {
    pub fn title(&self) -> &'static str {
        let s = crate::strings::current();
        match self.id {
            GoalId::PlaceThreeStations => s.goal_place_3_stations_title,
            GoalId::LayFirstTrack => s.goal_lay_first_track_title,
            GoalId::LaunchARoute => s.goal_launch_route_title,
            GoalId::ReachCoverage25 => s.goal_coverage_25_title,
            GoalId::Reach500Riders => s.goal_500_riders_title,
            GoalId::ApprovalAbove60 => s.goal_approval_60_title,
        }
    }

    pub fn description(&self) -> &'static str {
        let s = crate::strings::current();
        match self.id {
            GoalId::PlaceThreeStations => s.goal_place_3_stations_desc,
            GoalId::LayFirstTrack => s.goal_lay_first_track_desc,
            GoalId::LaunchARoute => s.goal_launch_route_desc,
            GoalId::ReachCoverage25 => s.goal_coverage_25_desc,
            GoalId::Reach500Riders => s.goal_500_riders_desc,
            GoalId::ApprovalAbove60 => s.goal_approval_60_desc,
        }
    }
}

/// All goals, ordered by tier then by definition order within the tier.
/// Values chosen to be reachable from fields already on the wire
/// (`UiState::stations`/`tracks`/`routes`/`coverage`/`daily_transit_trips`/
/// `approval`) — no new protocol fields.
pub const GOAL_DEFS: &[GoalDef] = &[
    GoalDef {
        id: GoalId::PlaceThreeStations,
        tier: 1,
        progress: |s| (s.stations.len() as f64, 3.0),
    },
    GoalDef {
        id: GoalId::LayFirstTrack,
        tier: 1,
        progress: |s| (s.tracks.len().min(1) as f64, 1.0),
    },
    GoalDef {
        id: GoalId::LaunchARoute,
        tier: 2,
        progress: |s| (s.routes.len().min(1) as f64, 1.0),
    },
    GoalDef {
        id: GoalId::ReachCoverage25,
        tier: 2,
        progress: |s| ((s.coverage * 100.0).min(25.0), 25.0),
    },
    GoalDef {
        id: GoalId::Reach500Riders,
        tier: 3,
        progress: |s| (s.daily_transit_trips.min(500.0), 500.0),
    },
    GoalDef {
        id: GoalId::ApprovalAbove60,
        tier: 3,
        progress: |s| (s.approval.min(60.0), 60.0),
    },
];

fn goal_def(id: GoalId) -> &'static GoalDef {
    GOAL_DEFS
        .iter()
        .find(|g| g.id == id)
        .expect("GoalId must have a matching GOAL_DEFS entry")
}

/// Highest tier currently visible/active: tier 1 is always shown; tier N+1
/// unlocks once every goal in tier N is complete. Pure so it's unit
/// testable without touching disk or `UiState`.
pub fn unlocked_tier(defs: &[GoalDef], completed: &std::collections::HashSet<GoalId>) -> u8 {
    let max_tier = defs.iter().map(|g| g.tier).max().unwrap_or(1);
    let mut tier = 1;
    while tier < max_tier {
        let this_tier_done = defs
            .iter()
            .filter(|g| g.tier == tier)
            .all(|g| completed.contains(&g.id));
        if !this_tier_done {
            break;
        }
        tier += 1;
    }
    tier
}

/// Evaluate every goal definition against a `UiState`, returning the set of
/// `GoalId`s that are newly satisfied (progress reached target) and were
/// NOT already in `already_completed`. Pure — no resources, no I/O — so
/// this is the unit of behavior the tests below exercise directly.
pub fn newly_completed(
    defs: &[GoalDef],
    state: &UiState,
    already_completed: &std::collections::HashSet<GoalId>,
) -> Vec<GoalId> {
    // Locked-tier goals never complete early: only goals at or below the
    // currently unlocked tier are evaluated, so the tier sequence is real
    // progression rather than cosmetic grouping. Completing the last goal
    // of a tier unlocks the next tier on the following evaluation pass.
    let tier_cap = unlocked_tier(defs, already_completed);
    defs.iter()
        .filter(|g| g.tier <= tier_cap)
        .filter(|g| !already_completed.contains(&g.id))
        .filter(|g| {
            let (cur, target) = (g.progress)(state);
            cur >= target
        })
        .map(|g| g.id)
        .collect()
}

/// On-disk shape: completed goal ids per city key. `city -> [GoalId]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct GoalsFile {
    #[serde(default)]
    cities: HashMap<String, Vec<GoalId>>,
}

fn goals_path() -> Option<PathBuf> {
    crate::paths::goals_toml_path()
}

/// Per-city goal completion, persisted to `goals.toml` next to
/// `config.toml`. A `Resource` so `goals_eval_system`/`goals_panel_system`
/// can share it.
#[derive(Resource, Debug, Default)]
pub struct GoalsState {
    /// The city key completion is currently tracked against
    /// (`PendingInit::preset_key` at the time `InGame` was entered).
    current_city: String,
    completed: std::collections::HashSet<GoalId>,
    path: Option<PathBuf>,
}

impl GoalsState {
    fn load_for_city(city: &str) -> Self {
        let path = goals_path();
        let file: GoalsFile = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        let completed = file
            .cities
            .get(city)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        GoalsState {
            current_city: city.to_string(),
            completed,
            path,
        }
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let mut file: GoalsFile = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        file.cities.insert(
            self.current_city.clone(),
            self.completed.iter().copied().collect(),
        );
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(toml_str) = toml::to_string_pretty(&file) {
            let _ = std::fs::write(path, toml_str);
        }
    }

    pub fn is_complete(&self, id: GoalId) -> bool {
        self.completed.contains(&id)
    }

    pub fn unlocked_tier(&self) -> u8 {
        unlocked_tier(GOAL_DEFS, &self.completed)
    }
}

/// Whether the goals panel is expanded (collapsible per art-direction: the
/// HUD shouldn't force a permanent block of screen for a side objective).
#[derive(Resource)]
pub struct GoalsPanelOpen(pub bool);

impl Default for GoalsPanelOpen {
    fn default() -> Self {
        GoalsPanelOpen(true)
    }
}

pub struct MfGoalsPlugin;

impl Plugin for MfGoalsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GoalsPanelOpen>()
            .add_systems(OnEnter(AppState::InGame), load_goals_for_city_system)
            .add_systems(Update, goals_eval_system.run_if(in_state(AppState::InGame)))
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                goals_panel_system
                    .run_if(in_state(AppState::InGame))
                    .run_if(crate::egui_idle::egui_content_active)
                    .run_if(|| !crate::design_system::hud_hidden()),
            );
    }
}

/// Loads (or re-keys) `GoalsState` for whichever city `PendingInit` names
/// the moment we enter `InGame`. Reloading here (rather than only once at
/// boot) is what makes per-city persistence work when the player returns
/// to the main menu and starts a different city in the same session.
fn load_goals_for_city_system(mut commands: Commands, pending: Res<PendingInit>) {
    commands.insert_resource(GoalsState::load_for_city(&pending.preset_key));
}

/// Diffs the latest `UiState` against `GoalsState::completed`, marks any
/// newly-satisfied goal complete, persists, and drops a "Good" toast per
/// completed goal so the player notices without a modal interrupting play.
fn goals_eval_system(
    ui_state: Res<LatestUi>,
    mut goals: ResMut<GoalsState>,
    mut toasts: ResMut<ToastLog>,
    mut sfx: EventWriter<PlaySfx>,
) {
    let Some(state) = &ui_state.0 else { return };
    let newly = newly_completed(GOAL_DEFS, state, &goals.completed);
    if newly.is_empty() {
        return;
    }
    let s = crate::strings::current();
    for id in newly {
        goals.completed.insert(id);
        let title = goal_def(id).title();
        toasts.push(s.goal_complete(title), ToastTone::Good);
        sfx.write(PlaySfx(Sfx::GoalComplete));
    }
    goals.save();
}

/// Collapsible Goals panel: a floating `egui::Window` (art-direction
/// parallel to `panels.rs`' station/finance windows, so it doesn't fight
/// `build_ui.rs`'s right-hand `SidePanel` for screen space) listing every
/// unlocked-tier goal with a progress bar or checkmark, plus a locked
/// summary line for tiers not yet reached.
pub fn goals_panel_system(
    mut contexts: EguiContexts,
    goals: Option<Res<GoalsState>>,
    ui_state: Res<LatestUi>,
    mut open: ResMut<GoalsPanelOpen>,
) -> Result {
    let Some(goals) = goals else { return Ok(()) };
    let Some(state) = &ui_state.0 else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;

    let s = crate::strings::current();
    let unlocked = goals.unlocked_tier();
    let max_tier = GOAL_DEFS.iter().map(|g| g.tier).max().unwrap_or(1);

    ds::window(
        ctx,
        ds::WindowOpts {
            title: s.goals,
            id: egui::Id::new("goals_panel"),
            open: Some(&mut open.0),
            collapsible: true,
            resizable: false,
            default_pos: Some(egui::pos2(14.0, 70.0)),
            default_width: None,
            anchor: None,
        },
        |ui| {
            for tier in 1..=unlocked {
                ui.label(crate::design_system::label_muted(s.tier(tier)));
                for def in GOAL_DEFS.iter().filter(|g| g.tier == tier) {
                    let done = goals.is_complete(def.id);
                    let (cur, target) = (def.progress)(state);
                    ui.horizontal(|ui| {
                        if done {
                            ui.colored_label(crate::design_system::GOOD, "\u{2713}");
                        } else {
                            ui.label("  ");
                        }
                        ui.vertical(|ui| {
                            ui.label(crate::design_system::value_strong(def.title()));
                            ui.label(crate::design_system::label_muted(def.description()));
                            let frac = if target > 0.0 {
                                (cur / target).clamp(0.0, 1.0) as f32
                            } else {
                                1.0
                            };
                            let _ = crate::design_system::progress_bar(ui, frac, 200.0);
                        });
                    });
                }
                crate::design_system::thin_separator(ui);
            }
            if unlocked < max_tier {
                ui.label(crate::design_system::label_muted(
                    s.tier_locked(unlocked + 1, unlocked),
                ));
            }
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_protocol::{DayLedger, FailReason};

    fn base_state() -> UiState {
        UiState {
            tick: 0,
            insights: vec![],
            day: 1,
            speed: 1.0,
            cash: 1000.0,
            loan_balance: 0.0,
            last_day: DayLedger {
                fares: 0.0,
                subsidy: 0.0,
                operations: 0.0,
                maintenance: 0.0,
                interest: 0.0,
            },
            net_history: vec![],
            population: 1000.0,
            approval: 50.0,
            transit_share: 0.0,
            coverage: 0.0,
            daily_transit_trips: 0.0,
            unlocked_modes: vec![],
            stations: vec![],
            tracks: vec![],
            routes: vec![],
            active_events: vec![],
            fields_version: 1,
            bankrupt: false,
            failed: None::<FailReason>,
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

    fn station(id: i64) -> mf_protocol::UiStation {
        mf_protocol::UiStation {
            id,
            name: format!("S{id}"),
            x: 0.0,
            y: 0.0,
            mode: mf_protocol::TransitMode::Bus,
            level: 1,
            ridership: 0.0,
            alightings: 0.0,
        }
    }

    #[test]
    fn no_goals_complete_on_fresh_city() {
        let state = base_state();
        let completed = std::collections::HashSet::new();
        assert!(newly_completed(GOAL_DEFS, &state, &completed).is_empty());
    }

    #[test]
    fn three_stations_completes_place_three_stations_only() {
        let mut state = base_state();
        state.stations = vec![station(1), station(2), station(3)];
        let completed = std::collections::HashSet::new();
        let newly = newly_completed(GOAL_DEFS, &state, &completed);
        assert_eq!(newly, vec![GoalId::PlaceThreeStations]);
    }

    #[test]
    fn two_stations_does_not_complete_goal() {
        let mut state = base_state();
        state.stations = vec![station(1), station(2)];
        let completed = std::collections::HashSet::new();
        assert!(newly_completed(GOAL_DEFS, &state, &completed).is_empty());
    }

    #[test]
    fn already_completed_goals_are_not_reported_again() {
        let mut state = base_state();
        state.stations = vec![station(1), station(2), station(3)];
        let mut completed = std::collections::HashSet::new();
        completed.insert(GoalId::PlaceThreeStations);
        assert!(newly_completed(GOAL_DEFS, &state, &completed).is_empty());
    }

    #[test]
    fn coverage_goal_reads_percent_of_coverage_fraction() {
        let mut state = base_state();
        state.coverage = 0.25;
        // Tier 1 complete so the tier 2 coverage goal is unlocked.
        let completed: std::collections::HashSet<GoalId> =
            [GoalId::PlaceThreeStations, GoalId::LayFirstTrack]
                .into_iter()
                .collect();
        let newly = newly_completed(GOAL_DEFS, &state, &completed);
        assert!(newly.contains(&GoalId::ReachCoverage25));
    }

    #[test]
    fn riders_and_approval_goals_evaluate_independently() {
        let mut state = base_state();
        state.daily_transit_trips = 500.0;
        state.approval = 61.0;
        // Tiers 1 and 2 complete, so tier 3 is unlocked and evaluable.
        let completed: std::collections::HashSet<GoalId> = [
            GoalId::PlaceThreeStations,
            GoalId::LayFirstTrack,
            GoalId::LaunchARoute,
            GoalId::ReachCoverage25,
        ]
        .into_iter()
        .collect();
        let newly = newly_completed(GOAL_DEFS, &state, &completed);
        assert!(newly.contains(&GoalId::Reach500Riders));
        assert!(newly.contains(&GoalId::ApprovalAbove60));
    }

    #[test]
    fn locked_tier_goals_do_not_complete_early() {
        // Approval is already past 60 on a fresh city, but its goal sits in
        // tier 3 and must stay dormant until tiers 1 and 2 are done.
        let mut state = base_state();
        state.approval = 90.0;
        state.daily_transit_trips = 1000.0;
        let completed = std::collections::HashSet::new();
        let newly = newly_completed(GOAL_DEFS, &state, &completed);
        assert!(!newly.contains(&GoalId::ApprovalAbove60));
        assert!(!newly.contains(&GoalId::Reach500Riders));
    }

    #[test]
    fn tier_two_locked_until_tier_one_fully_complete() {
        let mut completed = std::collections::HashSet::new();
        assert_eq!(unlocked_tier(GOAL_DEFS, &completed), 1);
        completed.insert(GoalId::PlaceThreeStations);
        assert_eq!(unlocked_tier(GOAL_DEFS, &completed), 1);
        completed.insert(GoalId::LayFirstTrack);
        assert_eq!(unlocked_tier(GOAL_DEFS, &completed), 2);
    }

    #[test]
    fn tier_three_locked_until_tier_two_fully_complete() {
        let mut completed = std::collections::HashSet::new();
        completed.insert(GoalId::PlaceThreeStations);
        completed.insert(GoalId::LayFirstTrack);
        completed.insert(GoalId::LaunchARoute);
        assert_eq!(unlocked_tier(GOAL_DEFS, &completed), 2);
        completed.insert(GoalId::ReachCoverage25);
        assert_eq!(unlocked_tier(GOAL_DEFS, &completed), 3);
    }

    #[test]
    fn all_goals_complete_reaches_max_tier() {
        let mut completed = std::collections::HashSet::new();
        for def in GOAL_DEFS {
            completed.insert(def.id);
        }
        assert_eq!(unlocked_tier(GOAL_DEFS, &completed), 3);
    }
}
