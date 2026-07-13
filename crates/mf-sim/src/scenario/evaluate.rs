//! Deterministic scenario condition evaluation + UI snapshot builder.
//!
//! Port of `sim/src/core/scenario/evaluate.ts`. No wall-clock, no RNG: metrics
//! come only from `GameState`. The win/lose condition trees and scenario meta
//! are passed in ([`ScenarioDef`]) so this evaluator does not depend on the
//! content-lane catalog / progression manifest. `unlocks` / `requires`
//! (progression edges) are left to the caller and omitted here.

use crate::types::{DayLedger, FailReason, GameState};

/// Farebox recovery ratio: fares / (operations + maintenance). Port of
/// `economy.ts::fareboxRecovery` (inlined; economy lane owns the full module).
pub fn farebox_recovery(ledger: &DayLedger) -> f64 {
    let running = ledger.operations + ledger.maintenance;
    if running > 0.0 {
        ledger.fares / running
    } else {
        0.0
    }
}

/// Metrics readable by condition leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScenarioMetric {
    /// Daily transit trips.
    DailyTransitTrips,
    /// Farebox recovery ratio.
    FareboxRecovery,
    /// Coverage 0..1.
    Coverage,
    /// Transit mode share 0..1.
    TransitShare,
    /// Approval 0..100.
    Approval,
    /// Cash.
    Cash,
    /// Population.
    Population,
    /// Calendar day.
    Day,
    /// Count of routes with crowding > 1.
    OvercrowdedRoutes,
}

impl ScenarioMetric {
    /// UI label.
    pub fn label(self) -> &'static str {
        match self {
            ScenarioMetric::DailyTransitTrips => "Daily riders",
            ScenarioMetric::FareboxRecovery => "Farebox recovery",
            ScenarioMetric::Coverage => "Coverage",
            ScenarioMetric::TransitShare => "Transit share",
            ScenarioMetric::Approval => "Approval",
            ScenarioMetric::Cash => "Cash",
            ScenarioMetric::Population => "Population",
            ScenarioMetric::Day => "Day",
            ScenarioMetric::OvercrowdedRoutes => "Overcrowded routes",
        }
    }
}

/// Comparison operator for a condition leaf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareOp {
    /// `>=`
    Ge,
    /// `>`
    Gt,
    /// `<=`
    Le,
    /// `<`
    Lt,
    /// `==`
    Eq,
}

impl CompareOp {
    /// String form (mirrors the TS union member).
    pub fn as_str(self) -> &'static str {
        match self {
            CompareOp::Ge => ">=",
            CompareOp::Gt => ">",
            CompareOp::Le => "<=",
            CompareOp::Lt => "<",
            CompareOp::Eq => "==",
        }
    }
}

/// A single threshold check against a live metric.
#[derive(Clone, Debug, PartialEq)]
pub struct ConditionLeaf {
    /// Metric to read.
    pub metric: ScenarioMetric,
    /// Comparison operator.
    pub op: CompareOp,
    /// Target value.
    pub value: f64,
    /// Optional UI label; defaults to a generated readout.
    pub label: Option<String>,
}

/// Boolean tree over leaves -- AND / OR / NOT compose arbitrarily.
#[derive(Clone, Debug, PartialEq)]
pub enum ConditionNode {
    /// A single leaf check.
    Leaf(ConditionLeaf),
    /// All children must hold.
    And(Vec<ConditionNode>),
    /// Any child must hold.
    Or(Vec<ConditionNode>),
    /// Negation.
    Not(Box<ConditionNode>),
}

/// A live snapshot of the metrics a condition can read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MetricSnapshot {
    /// Daily transit trips.
    pub daily_transit_trips: f64,
    /// Farebox recovery ratio.
    pub farebox_recovery: f64,
    /// Coverage 0..1.
    pub coverage: f64,
    /// Transit mode share 0..1.
    pub transit_share: f64,
    /// Approval 0..100.
    pub approval: f64,
    /// Cash.
    pub cash: f64,
    /// Population.
    pub population: f64,
    /// Calendar day (0-based).
    pub day: f64,
    /// Count of routes with crowding > 1.
    pub overcrowded_routes: f64,
}

impl MetricSnapshot {
    /// Read a metric value.
    pub fn get(&self, m: ScenarioMetric) -> f64 {
        match m {
            ScenarioMetric::DailyTransitTrips => self.daily_transit_trips,
            ScenarioMetric::FareboxRecovery => self.farebox_recovery,
            ScenarioMetric::Coverage => self.coverage,
            ScenarioMetric::TransitShare => self.transit_share,
            ScenarioMetric::Approval => self.approval,
            ScenarioMetric::Cash => self.cash,
            ScenarioMetric::Population => self.population,
            ScenarioMetric::Day => self.day,
            ScenarioMetric::OvercrowdedRoutes => self.overcrowded_routes,
        }
    }
}

/// Read the live metrics off `GameState`.
pub fn read_metrics(state: &GameState) -> MetricSnapshot {
    MetricSnapshot {
        daily_transit_trips: state.stats.daily_transit_trips,
        farebox_recovery: farebox_recovery(&state.budget.last_day),
        coverage: state.stats.coverage,
        transit_share: state.stats.transit_share,
        approval: state.stats.approval,
        cash: state.budget.cash,
        population: state.stats.population,
        day: (state.tick / u64::from(crate::constants::TICKS_PER_DAY)) as f64,
        overcrowded_routes: state.routes.iter().filter(|r| r.crowding > 1.0).count() as f64,
    }
}

/// Compare two values with an operator.
pub fn compare(op: CompareOp, current: f64, target: f64) -> bool {
    match op {
        CompareOp::Ge => current >= target,
        CompareOp::Gt => current > target,
        CompareOp::Le => current <= target,
        CompareOp::Lt => current < target,
        CompareOp::Eq => current == target,
    }
}

/// Evaluate a condition tree against a metric snapshot.
pub fn eval_condition(node: &ConditionNode, m: &MetricSnapshot) -> bool {
    match node {
        ConditionNode::Leaf(l) => compare(l.op, m.get(l.metric), l.value),
        ConditionNode::And(cs) => cs.iter().all(|c| eval_condition(c, m)),
        ConditionNode::Or(cs) => cs.iter().any(|c| eval_condition(c, m)),
        ConditionNode::Not(c) => !eval_condition(c, m),
    }
}

/// Leaf progress 0..1 -- for `>=`/`>` toward a positive target; inverted for
/// `<=`/`<`.
pub fn leaf_progress(leaf: &ConditionLeaf, m: &MetricSnapshot) -> f64 {
    let cur = m.get(leaf.metric);
    let t = leaf.value;
    match leaf.op {
        CompareOp::Ge | CompareOp::Gt => {
            if t <= 0.0 {
                if cur >= t {
                    1.0
                } else {
                    0.0
                }
            } else {
                (cur / t).clamp(0.0, 1.0)
            }
        }
        CompareOp::Le | CompareOp::Lt => {
            if cur <= t {
                1.0
            } else if t <= 0.0 {
                0.0
            } else {
                (t / cur).clamp(0.0, 1.0)
            }
        }
        CompareOp::Eq => {
            if cur == t {
                1.0
            } else {
                0.0
            }
        }
    }
}

/// Aggregate progress over a condition tree.
pub fn tree_progress(node: &ConditionNode, m: &MetricSnapshot) -> f64 {
    match node {
        ConditionNode::Leaf(l) => leaf_progress(l, m),
        ConditionNode::And(cs) => {
            if cs.is_empty() {
                return 1.0;
            }
            cs.iter().fold(1.0, |min, c| min.min(tree_progress(c, m)))
        }
        ConditionNode::Or(cs) => {
            if cs.is_empty() {
                return 0.0;
            }
            cs.iter().fold(0.0, |max, c| max.max(tree_progress(c, m)))
        }
        ConditionNode::Not(_) => {
            if eval_condition(node, m) {
                1.0
            } else {
                0.0
            }
        }
    }
}

/// Format a metric value for display.
fn format_metric(metric: ScenarioMetric, value: f64) -> String {
    match metric {
        ScenarioMetric::FareboxRecovery
        | ScenarioMetric::Coverage
        | ScenarioMetric::TransitShare => {
            format!("{}%", (value * 100.0).round())
        }
        ScenarioMetric::Cash => format!("${}", value.round()),
        ScenarioMetric::Approval => format!("{}%", value.round()),
        _ => format!("{}", value.round()),
    }
}

/// Default UI label for a leaf.
pub fn default_leaf_label(leaf: &ConditionLeaf) -> String {
    format!(
        "{} {} {}",
        leaf.metric.label(),
        leaf.op.as_str(),
        format_metric(leaf.metric, leaf.value)
    )
}

/// One flattened objective row for the UI envelope.
#[derive(Clone, Debug, PartialEq)]
pub struct ScenarioObjectiveState {
    /// Stable id.
    pub id: String,
    /// UI label.
    pub label: String,
    /// Metric read.
    pub metric: ScenarioMetric,
    /// Current value.
    pub current: f64,
    /// Target value.
    pub target: f64,
    /// Comparison operator.
    pub op: CompareOp,
    /// Whether it is currently met.
    pub met: bool,
    /// 0..1 progress toward this leaf.
    pub progress: f64,
}

/// Flatten a win tree into UI objective rows (top-level AND leaves; compounds
/// otherwise).
pub fn flatten_objectives(node: &ConditionNode, m: &MetricSnapshot) -> Vec<ScenarioObjectiveState> {
    let nodes: Vec<&ConditionNode> = match node {
        ConditionNode::And(cs) => cs.iter().collect(),
        other => vec![other],
    };
    let mut out = Vec::new();
    for (i, n) in nodes.into_iter().enumerate() {
        match n {
            ConditionNode::Leaf(l) => out.push(ScenarioObjectiveState {
                id: format!("obj-{i}-{:?}", l.metric),
                label: l.label.clone().unwrap_or_else(|| default_leaf_label(l)),
                metric: l.metric,
                current: m.get(l.metric),
                target: l.value,
                op: l.op,
                met: compare(l.op, m.get(l.metric), l.value),
                progress: leaf_progress(l, m),
            }),
            _ => out.push(ScenarioObjectiveState {
                id: format!("obj-{i}-compound"),
                label: "Compound objective".to_string(),
                metric: ScenarioMetric::DailyTransitTrips,
                current: tree_progress(n, m),
                target: 1.0,
                op: CompareOp::Ge,
                met: eval_condition(n, m),
                progress: tree_progress(n, m),
            }),
        }
    }
    out
}

/// Run outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Still in progress.
    Playing,
    /// Won.
    Won,
    /// Lost.
    Lost,
}

/// Minimal scenario definition needed to build the UI snapshot. Placeholder for
/// `scenario/types.ts::ScenarioDef` (win/lose trees + meta).
#[derive(Clone, Debug, PartialEq)]
pub struct ScenarioDef {
    /// Scenario id.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Win condition tree.
    pub win: ConditionNode,
    /// Calendar deadline in sim-days; `None` if unlimited.
    pub deadline_days: Option<i64>,
}

/// UI envelope for the scenario state. Mirrors the additive `ScenarioState`.
#[derive(Clone, Debug, PartialEq)]
pub struct ScenarioState {
    /// Scenario id.
    pub scenario_id: String,
    /// Display label.
    pub label: String,
    /// Flattened objective rows.
    pub objectives: Vec<ScenarioObjectiveState>,
    /// 0..1 aggregate progress.
    pub progress: f64,
    /// Calendar deadline in sim-days; `None` if unlimited.
    pub deadline: Option<i64>,
    /// Current calendar day (1-based).
    pub day: i64,
    /// Won flag.
    pub won: bool,
    /// Lost flag.
    pub lost: bool,
    /// Outcome.
    pub outcome: Outcome,
    /// Reason for loss, if any.
    pub lose_reason: Option<FailReason>,
}

/// Build the UI scenario snapshot from a def + live state.
pub fn build_scenario_state(def: &ScenarioDef, state: &GameState) -> ScenarioState {
    let m = read_metrics(state);
    let won = state.scenario_won == Some(true);
    let lost = state.failed.is_some();
    let objectives = flatten_objectives(&def.win, &m);
    ScenarioState {
        scenario_id: def.id.clone(),
        label: def.label.clone(),
        objectives,
        progress: if won {
            1.0
        } else {
            tree_progress(&def.win, &m)
        },
        deadline: def.deadline_days,
        day: (state.tick / u64::from(crate::constants::TICKS_PER_DAY)) as i64 + 1,
        won,
        lost,
        outcome: if won {
            Outcome::Won
        } else if lost {
            Outcome::Lost
        } else {
            Outcome::Playing
        },
        lose_reason: if lost { state.failed } else { None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::GameState;

    fn leaf(metric: ScenarioMetric, op: CompareOp, value: f64) -> ConditionNode {
        ConditionNode::Leaf(ConditionLeaf {
            metric,
            op,
            value,
            label: None,
        })
    }

    #[test]
    fn compare_ops() {
        assert!(compare(CompareOp::Ge, 5.0, 5.0));
        assert!(!compare(CompareOp::Gt, 5.0, 5.0));
        assert!(compare(CompareOp::Lt, 4.0, 5.0));
    }

    #[test]
    fn leaf_progress_clamps() {
        let l = ConditionLeaf {
            metric: ScenarioMetric::Population,
            op: CompareOp::Ge,
            value: 100.0,
            label: None,
        };
        let mut m = zero_metrics();
        m.population = 50.0;
        assert!((leaf_progress(&l, &m) - 0.5).abs() < 1e-9);
        m.population = 200.0;
        assert_eq!(leaf_progress(&l, &m), 1.0);
    }

    #[test]
    fn and_or_trees() {
        let mut m = zero_metrics();
        m.population = 100.0;
        m.approval = 60.0;
        let win = ConditionNode::And(vec![
            leaf(ScenarioMetric::Population, CompareOp::Ge, 50.0),
            leaf(ScenarioMetric::Approval, CompareOp::Ge, 50.0),
        ]);
        assert!(eval_condition(&win, &m));
        assert_eq!(tree_progress(&win, &m), 1.0);
        let or = ConditionNode::Or(vec![
            leaf(ScenarioMetric::Population, CompareOp::Ge, 1000.0),
            leaf(ScenarioMetric::Approval, CompareOp::Ge, 50.0),
        ]);
        assert!(eval_condition(&or, &m));
    }

    #[test]
    fn build_snapshot_flattens_objectives() {
        let state = GameState::new(1);
        let def = ScenarioDef {
            id: "test".into(),
            label: "Test".into(),
            win: ConditionNode::And(vec![leaf(ScenarioMetric::Population, CompareOp::Ge, 100.0)]),
            deadline_days: Some(30),
        };
        let snap = build_scenario_state(&def, &state);
        assert_eq!(snap.objectives.len(), 1);
        assert_eq!(snap.outcome, Outcome::Playing);
        assert_eq!(snap.day, 1);
    }

    fn zero_metrics() -> MetricSnapshot {
        MetricSnapshot {
            daily_transit_trips: 0.0,
            farebox_recovery: 0.0,
            coverage: 0.0,
            transit_share: 0.0,
            approval: 0.0,
            cash: 0.0,
            population: 0.0,
            day: 0.0,
            overcrowded_routes: 0.0,
        }
    }
}
