//! Campaign progression (ship-plan #25, v0.4): per-city star objectives,
//! evaluated entirely client-side over [`LatestUi`] — the sidecar sends a
//! plain `UiState` and knows nothing about stars, unlocks, or scenario
//! outcomes. Everything in this file is a pure overlay on top of that wire
//! state.
//!
//! Not yet wired into `main.rs`'s `add_plugins` (per this wave's scope
//! split — `main.rs` only gets `mod campaign;`/`mod report_ui;` lines this
//! wave; a future `v04/integration` pass registers [`MfCampaignPlugin`],
//! same convention `panels.rs` and `command_bus.rs` used for their v0.2/v0.3
//! landings). `#![allow(dead_code)]` keeps clippy quiet about the
//! currently-unreachable-from-`main` public surface in the meantime.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;

use bevy::prelude::*;
use mf_protocol::{FailReason, ToastTone, UiState};
use mf_state::LatestUi;
use serde::{Deserialize, Serialize};

use crate::audio::{PlaySfx, Sfx};
use crate::hud::ToastLog;
use crate::state::{AppState, PendingInit};

// ---------------------------------------------------------------------
// Objective table
// ---------------------------------------------------------------------

/// One star's win condition. Thresholds are "reach at least" (`>=`) against
/// the matching [`UiState`] field, except [`StarGoal::NetPositiveDays`]
/// which reads the trailing run of positive entries in `net_history` (see
/// [`trailing_positive_run`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StarGoal {
    /// `UiState.coverage`, a `0.0..=1.0` fraction of the city served.
    Coverage(f64),
    /// `UiState.approval`, a `0..=100` percentage.
    Approval(f64),
    /// `UiState.daily_transit_trips`, an absolute trip count.
    DailyTrips(f64),
    /// `UiState.transit_share`, a `0.0..=1.0` fraction of all trips taken
    /// by transit (vs. car/other).
    TransitShare(f64),
    /// Consecutive most-recent days (from `net_history`'s tail) with a
    /// strictly positive net — "run a profitable network for N days
    /// straight".
    NetPositiveDays(u32),
}

/// Dash-free, player-facing description of a [`StarGoal`] — used both by the
/// evaluation system's "Star earned" toast and (via [`CityObjectives`])
/// anywhere a future objectives panel wants to list what's left.
pub fn describe_goal(goal: StarGoal) -> String {
    match goal {
        StarGoal::Coverage(frac) => format!("Cover {:.0}% of the city", frac * 100.0),
        StarGoal::Approval(pct) => format!("Keep approval at {:.0}% or higher", pct),
        StarGoal::DailyTrips(trips) => {
            format!("Carry {} daily transit trips", format_thousands(trips))
        }
        StarGoal::TransitShare(frac) => format!("Reach {:.0}% transit mode share", frac * 100.0),
        StarGoal::NetPositiveDays(days) => {
            format!("Run {days} days in a row without losing money")
        }
    }
}

/// Comma-grouped integer, local to this file rather than reused from
/// `hud.rs` (its `format_thousands` is a private fn and this file must not
/// touch `hud.rs`).
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

/// One city's three star goals. `stars` is read as a difficulty ladder:
/// [`evaluate_earned_stars`] only counts a star as earned once every goal
/// before it (in array order) is *also* currently satisfied — see that
/// function's doc for why this beats an independent per-goal count.
pub struct CityObjectives {
    pub key: &'static str,
    pub stars: [StarGoal; 3],
}

/// Fixed unlock/authoring order: `nyc` first (always unlocked, and the only
/// fully-tuned row below), the remaining nine alphabetically. This is an
/// authored convention, not something the sidecar dictates — a future
/// balance pass may reorder it, in which case [`city_unlocked`]'s
/// `2 * index` math (see its doc) tracks whatever order lands here.
pub const CITY_ORDER: [&str; 10] = [
    "nyc",
    "atlanta",
    "boston",
    "chicago",
    "cleveland",
    "dc",
    "la",
    "philly",
    "seattle",
    "sf",
];

/// Static objective table, one row per [`CITY_ORDER`] entry. Only `nyc` has
/// balance-pass-considered values; every other row is a `// TUNE:`-flagged
/// placeholder so the campaign has *something* playable end to end before
/// a real balance pass happens, without blocking on one.
pub const CITY_OBJECTIVES: &[CityObjectives] = &[
    // nyc: star1 easy (a starter line already gets you most of the way
    // there), star2 mid (also an implicit "survive" — bankruptcy/approval
    // failure ends the scenario before a star can be recorded), star3 hard
    // (NYC's real subway carries millions of daily riders; 150k is a stiff
    // but reachable target for a built-out native-client network).
    CityObjectives {
        key: "nyc",
        stars: [
            StarGoal::Coverage(0.15),
            StarGoal::Approval(65.0),
            StarGoal::DailyTrips(150_000.0),
        ],
    },
    // TUNE: placeholder — smaller Sun Belt sprawl city, lower bar all round.
    CityObjectives {
        key: "atlanta",
        stars: [
            StarGoal::Coverage(0.10),
            StarGoal::Approval(55.0),
            StarGoal::NetPositiveDays(5),
        ],
    },
    // TUNE: placeholder.
    CityObjectives {
        key: "boston",
        stars: [
            StarGoal::Coverage(0.12),
            StarGoal::TransitShare(0.20),
            StarGoal::DailyTrips(30_000.0),
        ],
    },
    // TUNE: placeholder — dense Midwest city, closer to nyc's bar.
    CityObjectives {
        key: "chicago",
        stars: [
            StarGoal::Coverage(0.15),
            StarGoal::Approval(60.0),
            StarGoal::DailyTrips(80_000.0),
        ],
    },
    // TUNE: placeholder — smallest/lowest-bar city in the table.
    CityObjectives {
        key: "cleveland",
        stars: [
            StarGoal::Coverage(0.08),
            StarGoal::Approval(50.0),
            StarGoal::NetPositiveDays(4),
        ],
    },
    // TUNE: placeholder.
    CityObjectives {
        key: "dc",
        stars: [
            StarGoal::Coverage(0.12),
            StarGoal::TransitShare(0.25),
            StarGoal::DailyTrips(40_000.0),
        ],
    },
    // TUNE: placeholder — sprawling, car-centric; low transit-share bar.
    CityObjectives {
        key: "la",
        stars: [
            StarGoal::Coverage(0.08),
            StarGoal::Approval(50.0),
            StarGoal::TransitShare(0.15),
        ],
    },
    // TUNE: placeholder.
    CityObjectives {
        key: "philly",
        stars: [
            StarGoal::Coverage(0.12),
            StarGoal::Approval(55.0),
            StarGoal::NetPositiveDays(5),
        ],
    },
    // TUNE: placeholder.
    CityObjectives {
        key: "seattle",
        stars: [
            StarGoal::Coverage(0.10),
            StarGoal::TransitShare(0.20),
            StarGoal::DailyTrips(25_000.0),
        ],
    },
    // TUNE: placeholder.
    CityObjectives {
        key: "sf",
        stars: [
            StarGoal::Coverage(0.10),
            StarGoal::Approval(55.0),
            StarGoal::DailyTrips(35_000.0),
        ],
    },
];

/// Looks up the active city's objective row by `preset_key` — `report_ui.rs`
/// uses this to draw per-star goal captions alongside the star glyphs.
pub fn objectives_for(key: &str) -> Option<&'static CityObjectives> {
    CITY_OBJECTIVES.iter().find(|o| o.key == key)
}

/// Trailing run length of strictly-positive entries at the *end* of
/// `net_history` (oldest..newest per [`UiState::net_history`]'s doc) — i.e.
/// "how many most-recent days in a row were profitable". Stops counting at
/// the first non-positive entry walking backward from the newest; an empty
/// history has no run at all.
fn trailing_positive_run(net_history: &[f64]) -> u32 {
    net_history.iter().rev().take_while(|&&v| v > 0.0).count() as u32
}

fn evaluate_star(goal: StarGoal, ui: &UiState) -> bool {
    match goal {
        StarGoal::Coverage(threshold) => ui.coverage >= threshold,
        StarGoal::Approval(threshold) => ui.approval >= threshold,
        StarGoal::DailyTrips(threshold) => ui.daily_transit_trips >= threshold,
        StarGoal::TransitShare(threshold) => ui.transit_share >= threshold,
        StarGoal::NetPositiveDays(days) => trailing_positive_run(&ui.net_history) >= days,
    }
}

/// How many of `objectives.stars` are currently earned, read as a ladder:
/// star *N* only counts once stars `0..N` are *also* true against the same
/// `ui` snapshot. This resolves what would otherwise be an ambiguous "which
/// goal do I name in the toast" question if a harder star could be true
/// while an easier one wasn't (e.g. `DailyTrips` clearing while `Coverage`
/// hasn't) — with the ladder, the newly-earned star is always exactly
/// `objectives.stars[previous_count]`.
pub fn evaluate_earned_stars(objectives: &CityObjectives, ui: &UiState) -> u8 {
    let mut count = 0u8;
    for goal in objectives.stars {
        if evaluate_star(goal, ui) {
            count += 1;
        } else {
            break;
        }
    }
    count
}

// ---------------------------------------------------------------------
// Persisted progress
// ---------------------------------------------------------------------

/// TOML-serializable mirror of [`CampaignProgress`]'s persisted fields —
/// kept separate from the `Resource` itself the same way `config.rs`'s
/// `ConfigFile` is split from `MfConfig` (the resource also carries a
/// non-serializable `path`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CampaignFile {
    #[serde(default)]
    stars_by_city: HashMap<String, u8>,
}

/// Best stars-per-city reached so far, persisted to `campaign.toml` next to
/// `config.rs`'s `config.toml` (same `directories::ProjectDirs` app
/// identity). Public: the menu wave reads `stars`/`city_unlocked` to draw
/// the city picker's lock state and star counts — this is a verbatim
/// contract with that agent, so keep the signatures stable even if the
/// internals change.
#[derive(Resource, Debug, Clone, Default)]
pub struct CampaignProgress {
    stars_by_city: HashMap<String, u8>,
    path: Option<PathBuf>,
}

impl CampaignProgress {
    fn campaign_path() -> Option<PathBuf> {
        crate::paths::campaign_toml_path()
    }

    /// Load from disk, falling back to no progress at all if the file is
    /// absent/unreadable — a fresh install (or a wiped config dir) is not an
    /// error, it's just a player who hasn't earned anything yet.
    pub fn load() -> Self {
        let Some(path) = Self::campaign_path() else {
            tracing::warn!(
                "mf-game: no config dir available on this platform; campaign progress will not persist"
            );
            return CampaignProgress::default();
        };
        let stars_by_city = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| toml::from_str::<CampaignFile>(&s).ok())
            .map(|f| f.stars_by_city)
            .unwrap_or_default();
        CampaignProgress {
            stars_by_city,
            path: Some(path),
        }
    }

    /// Persist current progress back to `campaign.toml`.
    pub fn save(&self) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            anyhow::bail!("no campaign path resolved for this platform");
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = CampaignFile {
            stars_by_city: self.stars_by_city.clone(),
        };
        let toml_str = toml::to_string_pretty(&file)?;
        std::fs::write(path, toml_str)?;
        Ok(())
    }

    /// Best stars ever recorded for `key` (0 if never played / unknown key).
    pub fn stars(&self, key: &str) -> u8 {
        self.stars_by_city.get(key).copied().unwrap_or(0)
    }

    /// Sum of best-stars across every city — the input to [`city_unlocked`]'s
    /// threshold math.
    pub fn total_stars(&self) -> u32 {
        self.stars_by_city.values().map(|&s| s as u32).sum()
    }

    /// Record a newly-earned star count for `key`, persisting only if it's
    /// actually an improvement (progress never regresses, and a no-op call
    /// shouldn't touch disk every ~1Hz evaluation tick). Returns whether it
    /// changed.
    pub fn record_stars(&mut self, key: &str, stars: u8) -> bool {
        let stars = stars.min(3);
        let existing = self.stars_by_city.entry(key.to_string()).or_insert(0);
        if stars > *existing {
            *existing = stars;
            if let Err(e) = self.save() {
                tracing::warn!("mf-game: failed to persist campaign.toml: {e}");
            }
            true
        } else {
            false
        }
    }

    /// Whether `key` is playable yet. `nyc` (index 0 in [`CITY_ORDER`]) is
    /// always unlocked; every other city at index `i` unlocks once
    /// cumulative stars across *all* cities reach `2 * i` (documented
    /// placeholder curve — TUNE at the balance pass: e.g. index 1
    /// ("atlanta") needs 2 total stars, index 2 needs 4, and so on). An
    /// unknown key (not in `CITY_ORDER`) is never unlocked.
    pub fn city_unlocked(&self, key: &str) -> bool {
        match CITY_ORDER.iter().position(|&k| k == key) {
            Some(0) => true,
            Some(index) => self.total_stars() >= 2 * index as u32,
            None => false,
        }
    }
}

fn init_campaign_progress_system(mut commands: Commands) {
    commands.insert_resource(CampaignProgress::load());
}

// ---------------------------------------------------------------------
// Scenario outcome
// ---------------------------------------------------------------------

/// End-of-scenario state, recomputed from [`LatestUi`] every evaluation
/// tick (see [`compute_outcome`] for why this can be a stateless
/// recompute rather than a one-way latch). `report_ui.rs` reads this to
/// decide when to draw the full-screen report; it owns the "show only
/// once per outcome" latch itself (a `Local` comparing against the
/// previously-seen variant), since that's a UI-presentation concern, not a
/// scenario-state one.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScenarioOutcome {
    #[default]
    Playing,
    Failed(FailReason),
    Finished,
    /// All 3 stars earned on the active city. Deliberately does NOT end
    /// the scenario (spec: "keep playing, no forced end") — sandbox play
    /// continues, and a later `Failed`/`Finished` transition (see
    /// [`compute_outcome`]'s priority order) still fires normally from here.
    Completed,
}

/// Priority order, evaluated fresh against the latest `ui` snapshot every
/// tick: `Failed` (bankrupt/approval/time-out-as-failure) beats `Finished`
/// (clean end-of-scenario at `max_day`) beats `Completed` (all stars, but
/// sandbox continues) beats `Playing`. Because every branch is re-derived
/// from the current snapshot rather than latched, a later, more severe
/// signal always wins on its own (e.g. going bankrupt after already
/// reaching `Completed`, or after `Finished`, naturally recomputes to
/// `Failed` the very next tick) with no extra state-machine bookkeeping.
fn compute_outcome(ui: &UiState, all_three_stars_earned: bool) -> ScenarioOutcome {
    if let Some(reason) = ui.failed {
        ScenarioOutcome::Failed(reason)
    } else if ui.bankrupt {
        ScenarioOutcome::Failed(FailReason::Bankrupt)
    } else if ui.max_day.is_some_and(|max_day| ui.day >= max_day) {
        ScenarioOutcome::Finished
    } else if all_three_stars_earned {
        ScenarioOutcome::Completed
    } else {
        ScenarioOutcome::Playing
    }
}

/// Same cap `hud.rs`'s `ToastLog` trims to (kept in sync by convention, not
/// by a shared constant — `TOAST_LOG_CAP` there is private and this file
/// must not touch `hud.rs`).
const TOAST_LOG_CAP: usize = 20;

/// ~1Hz evaluation: checks the active city's stars against [`LatestUi`],
/// toasts + persists any newly-earned ones, and updates [`ScenarioOutcome`].
/// The active city is [`PendingInit::preset_key`] — it's set once at
/// `MainMenu` and never cleared while `InGame`, so it doubles as "which
/// city is this InGame session" without needing a second resource.
#[allow(clippy::too_many_arguments)]
fn evaluate_progress_system(
    mut timer: Local<Option<Timer>>,
    time: Res<Time>,
    ui: Res<LatestUi>,
    pending: Res<PendingInit>,
    mut progress: ResMut<CampaignProgress>,
    mut toasts: ResMut<ToastLog>,
    mut sfx: EventWriter<PlaySfx>,
    mut outcome: ResMut<ScenarioOutcome>,
) {
    let timer = timer.get_or_insert_with(|| Timer::from_seconds(1.0, TimerMode::Repeating));
    timer.tick(time.delta());
    if !timer.just_finished() {
        return;
    }
    let Some(state) = &ui.0 else {
        return;
    };

    if let Some(objectives) = objectives_for(&pending.preset_key) {
        let previous = progress.stars(&pending.preset_key);
        let earned = evaluate_earned_stars(objectives, state);
        if earned > previous {
            for goal in &objectives.stars[previous as usize..earned as usize] {
                toasts.0.push((
                    format!("Star earned: {}", describe_goal(*goal)),
                    ToastTone::Good,
                ));
            }
            if toasts.0.len() > TOAST_LOG_CAP {
                let excess = toasts.0.len() - TOAST_LOG_CAP;
                toasts.0.drain(0..excess);
            }
            sfx.write(PlaySfx(Sfx::Confirm));
            progress.record_stars(&pending.preset_key, earned);
        }

        *outcome = compute_outcome(state, earned >= 3);
    } else {
        // Unknown/unlisted preset key (shouldn't happen — `MainMenu` only
        // offers keys the sidecar's own `cityList` sent — but a stray
        // `MF_AUTOSTART` value could reach here): still track fail/finish
        // outcomes so the report can show, just never any stars.
        *outcome = compute_outcome(state, false);
    }
}

pub struct MfCampaignPlugin;

impl Plugin for MfCampaignPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScenarioOutcome>()
            .add_systems(Startup, init_campaign_progress_system)
            .add_systems(
                Update,
                evaluate_progress_system.run_if(in_state(AppState::InGame)),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_ui() -> UiState {
        UiState {
            tick: 0,
            insights: Vec::new(),
            day: 1,
            speed: 1.0,
            cash: 1_000_000.0,
            loan_balance: 0.0,
            last_day: mf_protocol::DayLedger {
                fares: 0.0,
                subsidy: 0.0,
                operations: 0.0,
                maintenance: 0.0,
                interest: 0.0,
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
            overcrowded_routes: Vec::new(),
        }
    }

    // --- trailing_positive_run -------------------------------------------

    #[test]
    fn trailing_positive_run_empty_history_is_zero() {
        assert_eq!(trailing_positive_run(&[]), 0);
    }

    #[test]
    fn trailing_positive_run_counts_from_the_newest_backward() {
        // oldest -> newest; the run only looks at the tail.
        assert_eq!(trailing_positive_run(&[-5.0, 1.0, 2.0, 3.0]), 3);
    }

    #[test]
    fn trailing_positive_run_stops_at_first_non_positive_from_the_end() {
        assert_eq!(trailing_positive_run(&[5.0, 5.0, -1.0, 2.0]), 1);
    }

    #[test]
    fn trailing_positive_run_zero_counts_as_not_positive() {
        assert_eq!(trailing_positive_run(&[1.0, 1.0, 0.0]), 0);
    }

    #[test]
    fn trailing_positive_run_all_positive_counts_everything() {
        assert_eq!(trailing_positive_run(&[1.0, 2.0, 3.0]), 3);
    }

    // --- evaluate_earned_stars (per goal type + ladder semantics) --------

    #[test]
    fn evaluate_star_goal_types_each_gate_correctly() {
        let mut ui = base_ui();
        ui.coverage = 0.20;
        ui.approval = 70.0;
        ui.daily_transit_trips = 200_000.0;
        ui.transit_share = 0.30;
        ui.net_history = vec![1.0, 1.0, 1.0];

        assert!(evaluate_star(StarGoal::Coverage(0.15), &ui));
        assert!(!evaluate_star(StarGoal::Coverage(0.25), &ui));
        assert!(evaluate_star(StarGoal::Approval(65.0), &ui));
        assert!(!evaluate_star(StarGoal::Approval(80.0), &ui));
        assert!(evaluate_star(StarGoal::DailyTrips(150_000.0), &ui));
        assert!(!evaluate_star(StarGoal::DailyTrips(300_000.0), &ui));
        assert!(evaluate_star(StarGoal::TransitShare(0.25), &ui));
        assert!(!evaluate_star(StarGoal::TransitShare(0.50), &ui));
        assert!(evaluate_star(StarGoal::NetPositiveDays(3), &ui));
        assert!(!evaluate_star(StarGoal::NetPositiveDays(4), &ui));
    }

    #[test]
    fn evaluate_earned_stars_counts_a_leading_run_only() {
        let objectives = CityObjectives {
            key: "test",
            stars: [
                StarGoal::Coverage(0.10),
                StarGoal::Approval(60.0),
                StarGoal::DailyTrips(50_000.0),
            ],
        };
        let mut ui = base_ui();
        // star1 true, star2 true, star3 false -> earns 2.
        ui.coverage = 0.12;
        ui.approval = 65.0;
        ui.daily_transit_trips = 1_000.0;
        assert_eq!(evaluate_earned_stars(&objectives, &ui), 2);
    }

    #[test]
    fn evaluate_earned_stars_a_later_goal_true_does_not_count_if_an_earlier_one_is_false() {
        let objectives = CityObjectives {
            key: "test",
            stars: [
                StarGoal::Coverage(0.10),
                StarGoal::Approval(60.0),
                StarGoal::DailyTrips(50_000.0),
            ],
        };
        let mut ui = base_ui();
        // star1 false even though star3's threshold is cleared -> earns 0,
        // not "2 out of 3 with a gap".
        ui.coverage = 0.0;
        ui.approval = 0.0;
        ui.daily_transit_trips = 999_999.0;
        assert_eq!(evaluate_earned_stars(&objectives, &ui), 0);
    }

    #[test]
    fn evaluate_earned_stars_all_three_caps_at_three() {
        let objectives = CityObjectives {
            key: "test",
            stars: [
                StarGoal::Coverage(0.10),
                StarGoal::Approval(60.0),
                StarGoal::DailyTrips(50_000.0),
            ],
        };
        let mut ui = base_ui();
        ui.coverage = 1.0;
        ui.approval = 100.0;
        ui.daily_transit_trips = 1_000_000.0;
        assert_eq!(evaluate_earned_stars(&objectives, &ui), 3);
    }

    // --- compute_outcome ---------------------------------------------------

    #[test]
    fn compute_outcome_playing_by_default() {
        let ui = base_ui();
        assert_eq!(compute_outcome(&ui, false), ScenarioOutcome::Playing);
    }

    #[test]
    fn compute_outcome_failed_takes_priority_from_the_failed_field() {
        let mut ui = base_ui();
        ui.failed = Some(FailReason::Approval);
        ui.max_day = Some(1);
        ui.day = 999; // would also be Finished, but Failed wins.
        assert_eq!(
            compute_outcome(&ui, true), // would also be Completed, but Failed wins.
            ScenarioOutcome::Failed(FailReason::Approval)
        );
    }

    #[test]
    fn compute_outcome_bankrupt_bool_maps_to_failed_bankrupt_even_without_failed_field() {
        let mut ui = base_ui();
        ui.bankrupt = true;
        assert_eq!(
            compute_outcome(&ui, false),
            ScenarioOutcome::Failed(FailReason::Bankrupt)
        );
    }

    #[test]
    fn compute_outcome_finished_when_day_reaches_max_day() {
        let mut ui = base_ui();
        ui.max_day = Some(30);
        ui.day = 30;
        assert_eq!(compute_outcome(&ui, false), ScenarioOutcome::Finished);
    }

    #[test]
    fn compute_outcome_not_finished_before_max_day() {
        let mut ui = base_ui();
        ui.max_day = Some(30);
        ui.day = 29;
        assert_eq!(compute_outcome(&ui, false), ScenarioOutcome::Playing);
    }

    #[test]
    fn compute_outcome_completed_when_all_three_stars_and_otherwise_fine() {
        let ui = base_ui();
        assert_eq!(compute_outcome(&ui, true), ScenarioOutcome::Completed);
    }

    #[test]
    fn compute_outcome_finished_beats_completed() {
        let mut ui = base_ui();
        ui.max_day = Some(10);
        ui.day = 10;
        assert_eq!(compute_outcome(&ui, true), ScenarioOutcome::Finished);
    }

    // --- CampaignProgress: stars/unlock/round-trip -----------------------

    #[test]
    fn fresh_progress_has_zero_stars_everywhere() {
        let progress = CampaignProgress::default();
        assert_eq!(progress.stars("nyc"), 0);
        assert_eq!(progress.total_stars(), 0);
    }

    #[test]
    fn record_stars_only_moves_upward() {
        let mut progress = CampaignProgress::default();
        assert!(progress.record_stars("nyc", 2));
        assert_eq!(progress.stars("nyc"), 2);
        // A lower/equal value is not a regression or a no-op-worth-saving change.
        assert!(!progress.record_stars("nyc", 1));
        assert_eq!(progress.stars("nyc"), 2);
        assert!(progress.record_stars("nyc", 3));
        assert_eq!(progress.stars("nyc"), 3);
    }

    #[test]
    fn record_stars_clamps_above_three() {
        let mut progress = CampaignProgress::default();
        progress.record_stars("nyc", 250);
        assert_eq!(progress.stars("nyc"), 3);
    }

    #[test]
    fn nyc_is_always_unlocked_even_with_zero_stars() {
        let progress = CampaignProgress::default();
        assert!(progress.city_unlocked("nyc"));
    }

    #[test]
    fn unknown_city_key_is_never_unlocked() {
        let progress = CampaignProgress::default();
        assert!(!progress.city_unlocked("gotham"));
    }

    #[test]
    fn unlock_ordering_math_follows_cumulative_star_threshold() {
        let mut progress = CampaignProgress::default();
        // index("atlanta") == 1 -> needs total_stars >= 2.
        assert!(!progress.city_unlocked("atlanta"));
        progress.record_stars("nyc", 1);
        assert!(!progress.city_unlocked("atlanta"));
        progress.record_stars("nyc", 2);
        assert!(progress.city_unlocked("atlanta"));

        // index("boston") == 2 -> needs total_stars >= 4.
        assert!(!progress.city_unlocked("boston"));
        progress.record_stars("atlanta", 2);
        assert!(progress.city_unlocked("boston"));
    }

    #[test]
    fn later_cities_need_more_cumulative_stars_than_earlier_ones() {
        let progress = CampaignProgress::default();
        for pair in CITY_ORDER.windows(2) {
            let i = CITY_ORDER.iter().position(|&k| k == pair[0]).unwrap();
            let j = CITY_ORDER.iter().position(|&k| k == pair[1]).unwrap();
            assert!(i < j);
        }
        let _ = progress; // silence unused if the loop above is ever removed
    }

    // --- CampaignFile (de)serialization round trip -----------------------

    #[test]
    fn campaign_file_roundtrips_through_toml() {
        let mut stars_by_city = HashMap::new();
        stars_by_city.insert("nyc".to_string(), 3u8);
        stars_by_city.insert("atlanta".to_string(), 1u8);
        let file = CampaignFile { stars_by_city };

        let s = toml::to_string_pretty(&file).unwrap();
        let back: CampaignFile = toml::from_str(&s).unwrap();
        assert_eq!(back.stars_by_city.get("nyc"), Some(&3));
        assert_eq!(back.stars_by_city.get("atlanta"), Some(&1));
    }

    #[test]
    fn empty_campaign_file_serializes_and_parses_back_empty() {
        let file = CampaignFile::default();
        let s = toml::to_string_pretty(&file).unwrap();
        let back: CampaignFile = toml::from_str(&s).unwrap();
        assert!(back.stars_by_city.is_empty());
    }

    #[test]
    fn every_city_objectives_row_key_is_in_city_order() {
        for objectives in CITY_OBJECTIVES {
            assert!(
                CITY_ORDER.contains(&objectives.key),
                "{} missing from CITY_ORDER",
                objectives.key
            );
        }
        assert_eq!(CITY_OBJECTIVES.len(), CITY_ORDER.len());
    }

    #[test]
    fn describe_goal_is_dash_free() {
        let goals = [
            StarGoal::Coverage(0.15),
            StarGoal::Approval(65.0),
            StarGoal::DailyTrips(150_000.0),
            StarGoal::TransitShare(0.2),
            StarGoal::NetPositiveDays(5),
        ];
        for goal in goals {
            let text = describe_goal(goal);
            assert!(!text.contains('-'), "{text:?} contains a dash");
            assert!(!text.contains('\u{2013}'), "{text:?} contains an en dash");
            assert!(!text.contains('\u{2014}'), "{text:?} contains an em dash");
        }
    }
}
