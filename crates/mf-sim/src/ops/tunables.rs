//! v0.9 System A (Operations) balance constants.
//!
//! Ports `sim/src/core/ops/tunables.ts`.
//!
//! BALANCE: owner-tuned. Every value here is a PLACEHOLDER chosen for plausible
//! feel, NOT a finalized economy balance. Keep the SHAPE of the model in the ops
//! modules; keep the NUMBERS here so a tuning pass never touches logic.
//!
//! Difficulty is a scenario-rules axis: FORGIVING is the default (gentle: rare,
//! quickly-cleared incidents); HARD is the punishing option (higher breakdown
//! rate, longer blocks, harsher reliability penalties). `ops_tunables` returns
//! the active set: `easy`/`normal` map to forgiving, `hard` maps to punishing.

use crate::types::{Difficulty, Period};

/// Per-period default target headway (seconds) for a freshly created route.
/// Peaks run tighter (more service) than nights. Players can override per route;
/// this is the starting profile. Mirrors `DEFAULT_PERIOD_HEADWAY`.
pub fn default_period_headway(period: Period) -> f64 {
    match period {
        Period::AmPeak => 300.0,
        Period::Midday => 600.0,
        Period::PmPeak => 300.0,
        Period::Evening => 720.0,
        Period::Night => 1200.0,
    }
}

/// Active ops tunables. Mirrors the `OpsTunables` interface (tunables.ts).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpsTunables {
    // Rolling stock condition.
    /// Condition lost per meter run by an in-service unit (before weather).
    pub condition_decay_per_meter: f64,
    /// Extra condition decay multiplier from full weather surface exposure
    /// (scaled by the unit's route surface exposure and weather intensity).
    pub weather_exposure_decay_mult: f64,
    /// Below this condition a unit is eligible for a maintenance window (if a
    /// depot of its mode exists).
    pub maintenance_condition_threshold: f64,
    /// Condition a unit is restored to after a completed maintenance window.
    pub maintenance_restore_to: f64,
    /// Ticks a maintenance window occupies a unit (out of service).
    pub maintenance_ticks: u32,

    // Breakdowns.
    /// Base per-unit per-tick breakdown probability at full condition, clear
    /// weather, no crowding.
    pub breakdown_base_per_tick: f64,
    /// Multiplier on breakdown risk as condition falls to 0.
    pub condition_risk_mult: f64,
    /// Multiplier on breakdown risk from weather.
    pub weather_risk_mult: f64,
    /// Multiplier on breakdown risk from crowding above capacity.
    pub crowd_risk_mult: f64,
    /// Ticks a breakdown blocks its segment (and disables the unit).
    pub breakdown_block_ticks: u32,
    /// Condition a unit drops TO when it breaks down.
    pub breakdown_condition_after: f64,

    // Reliability feedback (the keystone).
    /// On-time% at or above this counts as fully reliable (no penalty).
    pub on_time_target: f64,
    /// Ridership demand multiplier at 0% on-time (fully unreliable).
    pub demand_mult_at_zero_on_time: f64,
    /// Approval points added at full reliability / subtracted at zero on-time.
    pub approval_reliability_swing: f64,

    // Economy hooks (NUMBERS exposed; BALANCE owner-tuned).
    /// Daily maintenance opex per active fleet unit.
    pub fleet_maintenance_per_unit_per_day: f64,
    /// Daily standing cost per depot.
    pub depot_daily_cost: f64,
    /// One-off capex to build a depot.
    pub depot_build_cost: f64,
}

/// FORGIVING default: recoverable incidents, slow bankruptcy. Mirrors
/// `FORGIVING` (tunables.ts). All values BALANCE: owner-tuned.
pub const FORGIVING: OpsTunables = OpsTunables {
    condition_decay_per_meter: 1e-6,
    weather_exposure_decay_mult: 1.5,
    maintenance_condition_threshold: 0.4,
    maintenance_restore_to: 1.0,
    maintenance_ticks: 240,
    breakdown_base_per_tick: 1e-6,
    condition_risk_mult: 4.0,
    weather_risk_mult: 2.0,
    crowd_risk_mult: 0.8,
    breakdown_block_ticks: 30,
    breakdown_condition_after: 0.3,
    on_time_target: 0.9,
    demand_mult_at_zero_on_time: 0.6,
    approval_reliability_swing: 8.0,
    fleet_maintenance_per_unit_per_day: 0.0,
    depot_daily_cost: 2000.0,
    depot_build_cost: 750_000.0,
};

/// HARD: punishing set for the sim-depth crowd. Mirrors `HARD` (tunables.ts):
/// FORGIVING with a harsher breakdown / reliability / depot-cost overlay.
pub const HARD: OpsTunables = OpsTunables {
    breakdown_base_per_tick: 9e-6,
    condition_risk_mult: 6.0,
    weather_risk_mult: 3.0,
    crowd_risk_mult: 2.5,
    breakdown_block_ticks: 90,
    demand_mult_at_zero_on_time: 0.4,
    approval_reliability_swing: 14.0,
    depot_daily_cost: 3500.0,
    ..FORGIVING
};

/// Active ops tunables for a difficulty. `hard` is punishing; everything else
/// (the forgiving default) shares the gentle set. Mirrors `opsTunables`.
pub fn ops_tunables(difficulty: Difficulty) -> OpsTunables {
    match difficulty {
        Difficulty::Hard => HARD,
        _ => FORGIVING,
    }
}
