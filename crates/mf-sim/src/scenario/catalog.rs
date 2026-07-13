//! Playable scenario catalog. Port of `sim/src/core/scenario/catalog.ts` +
//! `sim/src/content/scenarios/*.json`.

use std::sync::OnceLock;

use crate::types::{Difficulty, TransitMode};

use super::evaluate::{
    CompareOp, ConditionLeaf, ConditionNode, ScenarioDef, ScenarioEvent, ScenarioMetric,
};

fn leaf(metric: ScenarioMetric, op: CompareOp, value: f64, label: &str) -> ConditionNode {
    ConditionNode::Leaf(ConditionLeaf {
        metric,
        op,
        value,
        label: Some(label.to_string()),
    })
}

fn and(children: Vec<ConditionNode>) -> ConditionNode {
    ConditionNode::And(children)
}

fn district_event(
    id: &str,
    day: i64,
    density_rank: usize,
    mult: f64,
    message: &str,
) -> ScenarioEvent {
    ScenarioEvent::DistrictDemandMult {
        id: id.to_string(),
        day,
        density_rank,
        mult,
        message: message.to_string(),
    }
}

fn global_event(id: &str, day: i64, mult: f64, duration_days: u32, message: &str) -> ScenarioEvent {
    ScenarioEvent::GlobalDemandMult {
        id: id.to_string(),
        day,
        mult,
        duration_days,
        message: message.to_string(),
    }
}

fn cash_event(id: &str, day: i64, amount: f64, message: &str) -> ScenarioEvent {
    ScenarioEvent::CashDelta {
        id: id.to_string(),
        day,
        amount,
        message: message.to_string(),
    }
}

/// Catalog order mirrors TS: Cleveland chain, then NYC chain.
pub fn playable_scenarios() -> &'static [ScenarioDef] {
    static SCENARIOS: OnceLock<Vec<ScenarioDef>> = OnceLock::new();
    SCENARIOS.get_or_init(|| {
        vec![
            ScenarioDef {
                id: "cleveland-first-riders".to_string(),
                label: "First Riders".to_string(),
                description: "Cleveland, bus only. Carry 300 daily riders before day 45.".to_string(),
                city_key: "cleveland".to_string(),
                tier: 1,
                difficulty: Difficulty::Easy,
                starting_budget: 12_000_000.0,
                starting_modes: vec![TransitMode::Bus],
                lock_modes: Some(true),
                daily_subsidy: Some(35_000.0),
                deadline_days: 45,
                era_label: Some("Starter".to_string()),
                win: leaf(
                    ScenarioMetric::DailyTransitTrips,
                    CompareOp::Ge,
                    300.0,
                    "Carry 300 daily riders",
                ),
                lose: None,
                events: Vec::new(),
            },
            ScenarioDef {
                id: "cleveland-five-hundred".to_string(),
                label: "Five Hundred".to_string(),
                description: "Tutorial adjacent. Carry 500 daily riders on buses before day 50."
                    .to_string(),
                city_key: "cleveland".to_string(),
                tier: 1,
                difficulty: Difficulty::Easy,
                starting_budget: 12_000_000.0,
                starting_modes: vec![TransitMode::Bus],
                lock_modes: Some(true),
                daily_subsidy: Some(32_000.0),
                deadline_days: 50,
                era_label: Some("Lesson".to_string()),
                win: leaf(
                    ScenarioMetric::DailyTransitTrips,
                    CompareOp::Ge,
                    500.0,
                    "Carry 500 daily riders",
                ),
                lose: None,
                events: Vec::new(),
            },
            ScenarioDef {
                id: "cleveland-farebox-30".to_string(),
                label: "Pay the Bills".to_string(),
                description: "500 daily riders and farebox recovery above 60% within 30 days. Day 10 doubles demand in the densest district.".to_string(),
                city_key: "cleveland".to_string(),
                tier: 2,
                difficulty: Difficulty::Easy,
                starting_budget: 10_000_000.0,
                starting_modes: vec![TransitMode::Bus],
                lock_modes: Some(true),
                daily_subsidy: Some(30_000.0),
                deadline_days: 30,
                era_label: Some("1955".to_string()),
                win: and(vec![
                    leaf(
                        ScenarioMetric::DailyTransitTrips,
                        CompareOp::Ge,
                        500.0,
                        "Carry 500 daily riders",
                    ),
                    leaf(
                        ScenarioMetric::FareboxRecovery,
                        CompareOp::Gt,
                        0.6,
                        "Farebox recovery above 60%",
                    ),
                ]),
                lose: None,
                events: vec![district_event(
                    "cle-demand-surge",
                    10,
                    0,
                    2.0,
                    "West side boom. The densest district doubles its travel demand.",
                )],
            },
            ScenarioDef {
                id: "cleveland-farebox-80".to_string(),
                label: "Farebox Target".to_string(),
                description:
                    "Economic drill. Hit 80% farebox recovery with 400 daily riders before day 40."
                        .to_string(),
                city_key: "cleveland".to_string(),
                tier: 2,
                difficulty: Difficulty::Normal,
                starting_budget: 8_000_000.0,
                starting_modes: vec![TransitMode::Bus],
                lock_modes: Some(true),
                daily_subsidy: Some(15_000.0),
                deadline_days: 40,
                era_label: Some("Ledger".to_string()),
                win: and(vec![
                    leaf(
                        ScenarioMetric::DailyTransitTrips,
                        CompareOp::Ge,
                        400.0,
                        "Carry 400 daily riders",
                    ),
                    leaf(
                        ScenarioMetric::FareboxRecovery,
                        CompareOp::Ge,
                        0.8,
                        "Farebox recovery at least 80%",
                    ),
                ]),
                lose: None,
                events: Vec::new(),
            },
            ScenarioDef {
                id: "cleveland-reach".to_string(),
                label: "Within Reach".to_string(),
                description:
                    "800 daily riders and 7% coverage before day 40. Bus and tram from the start."
                        .to_string(),
                city_key: "cleveland".to_string(),
                tier: 3,
                difficulty: Difficulty::Normal,
                starting_budget: 11_000_000.0,
                starting_modes: vec![TransitMode::Bus, TransitMode::Tram],
                lock_modes: Some(true),
                daily_subsidy: Some(28_000.0),
                deadline_days: 40,
                era_label: Some("Reach".to_string()),
                win: and(vec![
                    leaf(
                        ScenarioMetric::DailyTransitTrips,
                        CompareOp::Ge,
                        800.0,
                        "Carry 800 daily riders",
                    ),
                    leaf(
                        ScenarioMetric::Coverage,
                        CompareOp::Ge,
                        0.07,
                        "Cover 7% of residents",
                    ),
                ]),
                lose: None,
                events: vec![global_event(
                    "cle-fuel-spike",
                    15,
                    1.25,
                    5,
                    "Fuel prices spike citywide. Transit demand jumps for five days.",
                )],
            },
            ScenarioDef {
                id: "cleveland-roadworks".to_string(),
                label: "Road Works".to_string(),
                description:
                    "Crisis. Survive the day 5 detour surge past day 22 with 700 riders and zero overcrowded routes.".to_string(),
                city_key: "cleveland".to_string(),
                tier: 3,
                difficulty: Difficulty::Normal,
                starting_budget: 11_000_000.0,
                starting_modes: vec![TransitMode::Bus],
                lock_modes: Some(true),
                daily_subsidy: Some(25_000.0),
                deadline_days: 35,
                era_label: Some("Detour".to_string()),
                win: and(vec![
                    leaf(ScenarioMetric::Day, CompareOp::Ge, 22.0, "Hold through day 22"),
                    leaf(
                        ScenarioMetric::DailyTransitTrips,
                        CompareOp::Ge,
                        700.0,
                        "Carry 700 daily riders",
                    ),
                    leaf(
                        ScenarioMetric::OvercrowdedRoutes,
                        CompareOp::Le,
                        0.0,
                        "Keep crowding under control",
                    ),
                ]),
                lose: Some(leaf(
                    ScenarioMetric::OvercrowdedRoutes,
                    CompareOp::Ge,
                    1.0,
                    "Any route over capacity",
                )),
                events: vec![
                    global_event(
                        "cle-roadworks-global",
                        5,
                        3.0,
                        16,
                        "Road works choke the arterials. Transit demand triples for sixteen days.",
                    ),
                    district_event(
                        "cle-roadworks-hot",
                        5,
                        0,
                        3.0,
                        "Detours dump onto the densest district.",
                    ),
                ],
            },
            ScenarioDef {
                id: "cleveland-tram-line".to_string(),
                label: "Tram Line".to_string(),
                description:
                    "Sandbox unlock. Bus and tram open. Reach 1000 riders and 7% coverage before day 45.".to_string(),
                city_key: "cleveland".to_string(),
                tier: 3,
                difficulty: Difficulty::Normal,
                starting_budget: 10_500_000.0,
                starting_modes: vec![TransitMode::Bus, TransitMode::Tram],
                lock_modes: Some(true),
                daily_subsidy: Some(26_000.0),
                deadline_days: 45,
                era_label: Some("Rails".to_string()),
                win: and(vec![
                    leaf(
                        ScenarioMetric::DailyTransitTrips,
                        CompareOp::Ge,
                        1000.0,
                        "Carry 1000 daily riders",
                    ),
                    leaf(
                        ScenarioMetric::Coverage,
                        CompareOp::Ge,
                        0.07,
                        "Cover 7% of residents",
                    ),
                ]),
                lose: None,
                events: vec![global_event(
                    "cle-tram-boost",
                    12,
                    1.2,
                    6,
                    "Streetcar curiosity week. Demand up for six days.",
                )],
            },
            ScenarioDef {
                id: "cleveland-austerity".to_string(),
                label: "Austerity".to_string(),
                description:
                    "Thin subsidy, thin cash. 600 riders and 90% farebox before day 35 without cratering."
                        .to_string(),
                city_key: "cleveland".to_string(),
                tier: 4,
                difficulty: Difficulty::Hard,
                starting_budget: 5_500_000.0,
                starting_modes: vec![TransitMode::Bus],
                lock_modes: Some(true),
                daily_subsidy: Some(8_000.0),
                deadline_days: 35,
                era_label: Some("Cuts".to_string()),
                win: and(vec![
                    leaf(
                        ScenarioMetric::DailyTransitTrips,
                        CompareOp::Ge,
                        600.0,
                        "Carry 600 daily riders",
                    ),
                    leaf(
                        ScenarioMetric::FareboxRecovery,
                        CompareOp::Ge,
                        0.9,
                        "Farebox recovery at least 90%",
                    ),
                ]),
                lose: Some(leaf(
                    ScenarioMetric::Cash,
                    CompareOp::Lt,
                    -100_000.0,
                    "Cash below -$100k",
                )),
                events: vec![cash_event(
                    "cle-austerity-cut",
                    15,
                    -500_000.0,
                    "Council clawback. $500k gone.",
                )],
            },
            ScenarioDef {
                id: "nyc-first-thousand".to_string(),
                label: "First Thousand".to_string(),
                description:
                    "Tutorial adjacent. Carry 1000 daily riders on New York buses before day 40."
                        .to_string(),
                city_key: "nyc".to_string(),
                tier: 1,
                difficulty: Difficulty::Easy,
                starting_budget: 11_000_000.0,
                starting_modes: vec![TransitMode::Bus],
                lock_modes: Some(true),
                daily_subsidy: Some(28_000.0),
                deadline_days: 40,
                era_label: Some("Lesson".to_string()),
                win: leaf(
                    ScenarioMetric::DailyTransitTrips,
                    CompareOp::Ge,
                    1000.0,
                    "Carry 1000 daily riders",
                ),
                lose: None,
                events: Vec::new(),
            },
            ScenarioDef {
                id: "nyc-farebox-80".to_string(),
                label: "Cover Fares".to_string(),
                description:
                    "Economic drill. 1200 riders and 80% farebox recovery before day 40."
                        .to_string(),
                city_key: "nyc".to_string(),
                tier: 2,
                difficulty: Difficulty::Normal,
                starting_budget: 9_500_000.0,
                starting_modes: vec![TransitMode::Bus],
                lock_modes: Some(true),
                daily_subsidy: Some(18_000.0),
                deadline_days: 40,
                era_label: Some("Ledger".to_string()),
                win: and(vec![
                    leaf(
                        ScenarioMetric::DailyTransitTrips,
                        CompareOp::Ge,
                        1200.0,
                        "Carry 1200 daily riders",
                    ),
                    leaf(
                        ScenarioMetric::FareboxRecovery,
                        CompareOp::Ge,
                        0.8,
                        "Farebox recovery at least 80%",
                    ),
                ]),
                lose: None,
                events: vec![district_event(
                    "nyc-fare-nudge",
                    10,
                    0,
                    1.5,
                    "Office return week. Densest district demand up 50%.",
                )],
            },
            ScenarioDef {
                id: "nyc-bus-spine".to_string(),
                label: "Bus Spine".to_string(),
                description:
                    "New York, bus only. Carry 1500 daily riders with farebox at least 80% before day 45.".to_string(),
                city_key: "nyc".to_string(),
                tier: 4,
                difficulty: Difficulty::Hard,
                starting_budget: 9_000_000.0,
                starting_modes: vec![TransitMode::Bus],
                lock_modes: Some(true),
                daily_subsidy: Some(22_000.0),
                deadline_days: 45,
                era_label: Some("NYC".to_string()),
                win: and(vec![
                    leaf(
                        ScenarioMetric::DailyTransitTrips,
                        CompareOp::Ge,
                        1500.0,
                        "Carry 1500 daily riders",
                    ),
                    leaf(
                        ScenarioMetric::FareboxRecovery,
                        CompareOp::Ge,
                        0.8,
                        "Farebox recovery at least 80%",
                    ),
                ]),
                lose: None,
                events: vec![
                    district_event(
                        "nyc-midtown-surge",
                        12,
                        0,
                        2.0,
                        "Midtown surge. The densest district doubles its demand.",
                    ),
                    cash_event(
                        "nyc-grant-cut",
                        20,
                        -400_000.0,
                        "State grant clawback. $400k leaves overnight.",
                    ),
                ],
            },
            ScenarioDef {
                id: "nyc-dig-season".to_string(),
                label: "Dig Season".to_string(),
                description:
                    "Crisis. Survive the day 8 dig surge past day 25 with 3000 riders and zero overcrowded routes.".to_string(),
                city_key: "nyc".to_string(),
                tier: 3,
                difficulty: Difficulty::Hard,
                starting_budget: 10_000_000.0,
                starting_modes: vec![TransitMode::Bus],
                lock_modes: Some(true),
                daily_subsidy: Some(20_000.0),
                deadline_days: 40,
                era_label: Some("Dig".to_string()),
                win: and(vec![
                    leaf(ScenarioMetric::Day, CompareOp::Ge, 25.0, "Hold through day 25"),
                    leaf(
                        ScenarioMetric::DailyTransitTrips,
                        CompareOp::Ge,
                        3000.0,
                        "Carry 3000 daily riders",
                    ),
                    leaf(
                        ScenarioMetric::OvercrowdedRoutes,
                        CompareOp::Le,
                        0.0,
                        "Keep crowding under control",
                    ),
                ]),
                lose: Some(leaf(
                    ScenarioMetric::OvercrowdedRoutes,
                    CompareOp::Ge,
                    1.0,
                    "Any route over capacity",
                )),
                events: vec![
                    global_event(
                        "nyc-dig-global",
                        8,
                        2.2,
                        16,
                        "Avenue digs begin. Citywide transit demand jumps for sixteen days.",
                    ),
                    district_event(
                        "nyc-dig-hot",
                        8,
                        0,
                        2.5,
                        "Detours crush the densest district.",
                    ),
                ],
            },
            ScenarioDef {
                id: "nyc-pressure".to_string(),
                label: "Pressure Cooker".to_string(),
                description:
                    "Tight cash, locked buses. Hit 2000 riders, 100% farebox, and 10% coverage before day 50.".to_string(),
                city_key: "nyc".to_string(),
                tier: 5,
                difficulty: Difficulty::Hard,
                starting_budget: 7_500_000.0,
                starting_modes: vec![TransitMode::Bus],
                lock_modes: Some(true),
                daily_subsidy: Some(18_000.0),
                deadline_days: 50,
                era_label: Some("Pressure".to_string()),
                win: and(vec![
                    leaf(
                        ScenarioMetric::DailyTransitTrips,
                        CompareOp::Ge,
                        2000.0,
                        "Carry 2000 daily riders",
                    ),
                    leaf(
                        ScenarioMetric::FareboxRecovery,
                        CompareOp::Ge,
                        1.0,
                        "Farebox recovery at least 100%",
                    ),
                    leaf(
                        ScenarioMetric::Coverage,
                        CompareOp::Ge,
                        0.1,
                        "Cover 10% of residents",
                    ),
                ]),
                lose: Some(leaf(
                    ScenarioMetric::Cash,
                    CompareOp::Lt,
                    -200_000.0,
                    "Cash below -$200k",
                )),
                events: vec![
                    district_event(
                        "nyc-pressure-boom",
                        8,
                        0,
                        2.0,
                        "A district doubles its demand. Serve it or drown in cars.",
                    ),
                    cash_event(
                        "nyc-pressure-austerity",
                        25,
                        -750_000.0,
                        "Austerity order. $750k yanked from the operating budget.",
                    ),
                ],
            },
            ScenarioDef {
                id: "nyc-express".to_string(),
                label: "Express Grid".to_string(),
                description:
                    "Sandbox unlock. Bus and tram. Hit 2500 riders and 12% coverage before day 45."
                        .to_string(),
                city_key: "nyc".to_string(),
                tier: 4,
                difficulty: Difficulty::Hard,
                starting_budget: 10_000_000.0,
                starting_modes: vec![TransitMode::Bus, TransitMode::Tram],
                lock_modes: Some(true),
                daily_subsidy: Some(20_000.0),
                deadline_days: 45,
                era_label: Some("Grid".to_string()),
                win: and(vec![
                    leaf(
                        ScenarioMetric::DailyTransitTrips,
                        CompareOp::Ge,
                        2500.0,
                        "Carry 2500 daily riders",
                    ),
                    leaf(
                        ScenarioMetric::Coverage,
                        CompareOp::Ge,
                        0.12,
                        "Cover 12% of residents",
                    ),
                ]),
                lose: None,
                events: vec![district_event(
                    "nyc-express-surge",
                    14,
                    0,
                    1.8,
                    "Express trial week. Densest district demand up.",
                )],
            },
            ScenarioDef {
                id: "nyc-last-stand".to_string(),
                label: "Last Stand".to_string(),
                description:
                    "Brutal endgame. From day 30 hold 4000 riders, 100% farebox, and 12% coverage on a starved bus budget.".to_string(),
                city_key: "nyc".to_string(),
                tier: 5,
                difficulty: Difficulty::Hard,
                starting_budget: 5_500_000.0,
                starting_modes: vec![TransitMode::Bus],
                lock_modes: Some(true),
                daily_subsidy: Some(10_000.0),
                deadline_days: 55,
                era_label: Some("Endgame".to_string()),
                win: and(vec![
                    leaf(ScenarioMetric::Day, CompareOp::Ge, 30.0, "Hold through day 30"),
                    leaf(
                        ScenarioMetric::DailyTransitTrips,
                        CompareOp::Ge,
                        4000.0,
                        "Carry 4000 daily riders",
                    ),
                    leaf(
                        ScenarioMetric::FareboxRecovery,
                        CompareOp::Ge,
                        1.0,
                        "Farebox recovery at least 100%",
                    ),
                    leaf(
                        ScenarioMetric::Coverage,
                        CompareOp::Ge,
                        0.12,
                        "Cover 12% of residents",
                    ),
                ]),
                lose: Some(leaf(
                    ScenarioMetric::Cash,
                    CompareOp::Lt,
                    -150_000.0,
                    "Cash below -$150k",
                )),
                events: vec![
                    district_event(
                        "nyc-end-boom",
                        10,
                        0,
                        2.2,
                        "Peak season. Densest district demand more than doubles.",
                    ),
                    cash_event(
                        "nyc-end-cut-a",
                        18,
                        -600_000.0,
                        "Emergency cut. $600k yanked.",
                    ),
                    cash_event(
                        "nyc-end-cut-b",
                        28,
                        -500_000.0,
                        "Second cut. Another $500k gone.",
                    ),
                    global_event(
                        "nyc-end-surge",
                        22,
                        1.4,
                        10,
                        "Storm week. Citywide demand up for ten days.",
                    ),
                ],
            },
        ]
    })
}

/// Lookup by stable scenario id.
pub fn playable_scenario(id: &str) -> Option<&'static ScenarioDef> {
    playable_scenarios().iter().find(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_fifteen_unique_ids() {
        let all = playable_scenarios();
        assert_eq!(all.len(), 15);
        let mut ids: Vec<&str> = all.iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 15);
    }

    #[test]
    fn no_em_or_en_dashes_in_player_copy() {
        for s in playable_scenarios() {
            assert!(!s.label.contains('\u{2013}') && !s.label.contains('\u{2014}'));
            assert!(!s.description.contains('\u{2013}') && !s.description.contains('\u{2014}'));
        }
    }
}
