//! Mid-run scenario event application. Port of
//! `sim/src/core/scenario/events.ts`.

use std::collections::BTreeMap;

use crate::types::GameState;

use super::evaluate::{ScenarioDef, ScenarioEvent};

/// Toast tone for scenario events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScenarioTone {
    Info,
    Warn,
    Good,
}

/// One scenario toast message.
#[derive(Clone, Debug, PartialEq)]
pub struct ScenarioToast {
    pub message: String,
    pub tone: ScenarioTone,
}

/// Result of applying day-matching scenario events.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScenarioEventResult {
    pub messages: Vec<String>,
    pub toasts: Vec<ScenarioToast>,
}

fn districts_by_density(state: &GameState) -> Vec<u32> {
    let mut ranked: Vec<(u32, f64)> = state
        .districts
        .iter()
        .map(|d| (d.id, d.population + d.jobs))
        .collect();
    ranked.sort_by(|(aid, ad), (bid, bd)| {
        bd.partial_cmp(ad)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| aid.cmp(bid))
    });
    ranked.into_iter().map(|(id, _)| id).collect()
}

/// Fire events scheduled for `day` that have not fired yet.
pub fn apply_scenario_events(
    state: &mut GameState,
    def: &ScenarioDef,
    day: i64,
) -> ScenarioEventResult {
    let mut out = ScenarioEventResult::default();
    if def.events.is_empty() {
        return out;
    }
    let mut to_fire: Vec<ScenarioEvent> = Vec::new();
    {
        let fired = state.fired_scenario_events.get_or_insert_with(Vec::new);
        for ev in &def.events {
            let (id, ev_day) = match ev {
                ScenarioEvent::DistrictDemandMult { id, day, .. } => (id, *day),
                ScenarioEvent::GlobalDemandMult { id, day, .. } => (id, *day),
                ScenarioEvent::CashDelta { id, day, .. } => (id, *day),
            };
            if ev_day != day || fired.iter().any(|f| f == id) {
                continue;
            }
            fired.push(id.clone());
            to_fire.push(ev.clone());
        }
    }
    for ev in &to_fire {
        fire_one(state, ev, &mut out);
    }
    out
}

fn fire_one(state: &mut GameState, ev: &ScenarioEvent, out: &mut ScenarioEventResult) {
    match ev {
        ScenarioEvent::DistrictDemandMult {
            id,
            density_rank,
            mult,
            message,
            ..
        } => {
            let ranked = districts_by_density(state);
            let Some(did) = ranked.get(*density_rank).copied() else {
                out.messages.push(format!(
                    "Scenario event {id}: no district at density rank {density_rank}"
                ));
                return;
            };
            let map = state
                .district_demand_mult
                .get_or_insert_with(BTreeMap::<u32, f64>::new);
            let prev = map.get(&did).copied().unwrap_or(1.0);
            map.insert(did, prev * *mult);
            state.demand_dirty = true;
            out.messages.push(message.clone());
            out.toasts.push(ScenarioToast {
                message: message.clone(),
                tone: ScenarioTone::Info,
            });
        }
        ScenarioEvent::GlobalDemandMult {
            mult,
            duration_days,
            message,
            ..
        } => {
            state.global_demand_mult = Some(*mult);
            state.global_demand_mult_days_left = Some(*duration_days);
            state.demand_dirty = true;
            out.messages.push(message.clone());
            out.toasts.push(ScenarioToast {
                message: message.clone(),
                tone: ScenarioTone::Info,
            });
        }
        ScenarioEvent::CashDelta {
            amount, message, ..
        } => {
            state.budget.cash += *amount;
            out.messages.push(message.clone());
            out.toasts.push(ScenarioToast {
                message: message.clone(),
                tone: if *amount >= 0.0 {
                    ScenarioTone::Good
                } else {
                    ScenarioTone::Warn
                },
            });
        }
    }
}

/// Tick down temporary global demand multipliers at day boundary.
pub fn tick_global_demand_mult(state: &mut GameState) {
    let Some(mut days_left) = state.global_demand_mult_days_left else {
        return;
    };
    if days_left == 0 {
        state.global_demand_mult = None;
        state.global_demand_mult_days_left = None;
        state.demand_dirty = true;
        return;
    }
    days_left -= 1;
    if days_left == 0 {
        state.global_demand_mult = None;
        state.global_demand_mult_days_left = None;
        state.demand_dirty = true;
    } else {
        state.global_demand_mult_days_left = Some(days_left);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::catalog::playable_scenario;
    use crate::types::GameState;

    #[test]
    fn event_day_application_is_deterministic() {
        let def = playable_scenario("cleveland-farebox-30").expect("scenario");
        let mut a = GameState::new(7);
        let mut b = GameState::new(7);
        // fake a few districts for ranking
        a.districts = vec![
            crate::types::District {
                id: 1,
                name: "a".to_string(),
                centroid: crate::geometry::Vec2 { x: 0.0, y: 0.0 },
                cell_indices: vec![],
                population: 10.0,
                jobs: 5.0,
                land_value: 0.0,
                last_growth_delta: None,
            },
            crate::types::District {
                id: 2,
                name: "b".to_string(),
                centroid: crate::geometry::Vec2 { x: 0.0, y: 0.0 },
                cell_indices: vec![],
                population: 100.0,
                jobs: 5.0,
                land_value: 0.0,
                last_growth_delta: None,
            },
        ];
        b.districts = a.districts.clone();
        let ra = apply_scenario_events(&mut a, def, 10);
        let rb = apply_scenario_events(&mut b, def, 10);
        assert_eq!(ra, rb);
        assert_eq!(a.district_demand_mult, b.district_demand_mult);
    }
}
