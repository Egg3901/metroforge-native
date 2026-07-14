//! Scenario engine. Port of `sim/src/core/scenario/*`.

pub mod catalog;
pub mod evaluate;
pub mod events;
pub mod progression;

use crate::types::{FailReason, GameState};

/// Daily scenario evaluation output.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScenarioDayResult {
    pub won: bool,
    pub lost_condition: bool,
    pub messages: Vec<String>,
    pub toasts: Vec<events::ScenarioToast>,
}

/// Run the scenario engine for one completed calendar day.
///
/// This evaluates:
/// - temporary demand-multiplier expiry,
/// - scheduled scripted events,
/// - win tree,
/// - optional lose tree.
///
/// Bankruptcy/approval/time failures remain in `sim::check_failure`.
pub fn evaluate_scenario_day(
    state: &mut GameState,
    def: &evaluate::ScenarioDef,
    day: i64,
) -> ScenarioDayResult {
    let mut out = ScenarioDayResult::default();
    if state.scenario_won == Some(true) || state.failed.is_some() {
        return out;
    }

    events::tick_global_demand_mult(state);
    let fired = events::apply_scenario_events(state, def, day);
    out.messages.extend(fired.messages);
    out.toasts.extend(fired.toasts);

    let mut m = evaluate::read_metrics(state);
    m.day = day as f64;
    if evaluate::eval_condition(&def.win, &m) {
        state.scenario_won = Some(true);
        out.won = true;
        out.messages
            .push(format!("Objective met - {} complete", def.label));
        out.toasts.push(events::ScenarioToast {
            message: format!("Victory - {}", def.label),
            tone: events::ScenarioTone::Good,
        });
        return out;
    }

    if let Some(lose) = &def.lose {
        if evaluate::eval_condition(lose, &m) {
            state.failed = Some(FailReason::Condition);
            out.lost_condition = true;
            out.messages
                .push("Scenario lose condition triggered".to_string());
            out.toasts.push(events::ScenarioToast {
                message: "Scenario failed".to_string(),
                tone: events::ScenarioTone::Warn,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_game::{new_game, NewGameOptions};
    use crate::types::Difficulty;

    #[test]
    fn evaluate_day_sets_won() {
        let def = catalog::playable_scenario("cleveland-first-riders").expect("scenario");
        let mut s = new_game(
            1,
            Difficulty::Easy,
            NewGameOptions {
                scenario: Some(crate::types::ScenarioDef { id: def.id.clone() }),
                ..NewGameOptions::default()
            },
        );
        s.stats.daily_transit_trips = 400.0;
        let r = evaluate_scenario_day(&mut s, def, 1);
        assert!(r.won);
        assert_eq!(s.scenario_won, Some(true));
    }
}
