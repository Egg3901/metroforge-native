/**
 * v0.9 System A (Operations) balance constants.
 *
 * BALANCE: owner-tuned. Every value in this file is a PLACEHOLDER chosen for
 * plausible feel, NOT a finalized economy balance. The coordinator tunes these
 * (fleet capex, maintenance opex, depot cost, breakdown rates, reliability
 * elasticity). Keep the SHAPE of the model in the ops modules; keep the NUMBERS
 * here so a tuning pass never has to touch logic.
 *
 * Difficulty is a scenario-rules axis: FORGIVING is the default (Mini-Metro
 * gentle: rare, quickly-cleared incidents); HARD is the punishing option
 * (higher breakdown rate, longer blocks, harsher reliability penalties).
 * `opsTunables(difficulty)` returns the active set.
 */
import type { Difficulty } from '../types';
import type { Period } from './periods';

/** Per-period default target headway (seconds) for a freshly created route.
 *  Peaks run tighter (more service) than nights. Players can override per route
 *  via the setRouteFrequency command; this is the starting profile. */
export const DEFAULT_PERIOD_HEADWAY: Record<Period, number> = {
  // BALANCE: owner-tuned starting service profile.
  amPeak: 300,
  midday: 600,
  pmPeak: 300,
  evening: 720,
  night: 1200,
};

export interface OpsTunables {
  // ── Rolling stock condition ────────────────────────────────────────────────
  /** Condition lost per meter run by an in-service unit (before weather).
   *  BALANCE: owner-tuned. A bus at ~8.3 m/s over a 20-min day (~1200 ticks)
   *  runs ~10 km/day → ~0.01 condition/day at 1e-6. */
  conditionDecayPerMeter: number;
  /** Extra condition decay multiplier from full weather surface exposure
   *  (scaled by the unit's route surfaceExposure and weather intensity).
   *  BALANCE: owner-tuned. */
  weatherExposureDecayMult: number;
  /** Below this condition a unit is eligible for a maintenance window (if a
   *  depot of its mode exists). BALANCE: owner-tuned. */
  maintenanceConditionThreshold: number;
  /** Condition a unit is restored to after a completed maintenance window.
   *  BALANCE: owner-tuned. */
  maintenanceRestoreTo: number;
  /** Ticks a maintenance window occupies a unit (it is out of service, cutting
   *  the route's in-service count while active). BALANCE: owner-tuned. */
  maintenanceTicks: number;

  // ── Breakdowns ──────────────────────────────────────────────────────────────
  /** Base per-unit per-tick breakdown probability at full condition, clear
   *  weather, no crowding. BALANCE: owner-tuned. Forgiving keeps this low. */
  breakdownBasePerTick: number;
  /** Multiplier on breakdown risk as condition falls to 0 (deferred maintenance
   *  compounds): risk *= 1 + conditionRiskMult * (1 - condition). BALANCE. */
  conditionRiskMult: number;
  /** Multiplier on breakdown risk from weather (uses the shared
   *  WEATHER_BREAKDOWN_CHANCE surface, normalized). BALANCE: owner-tuned. */
  weatherRiskMult: number;
  /** Multiplier on breakdown risk from crowding above capacity: risk *= 1 +
   *  crowdRiskMult * max(0, crowding - 1). BALANCE: owner-tuned. */
  crowdRiskMult: number;
  /** Ticks a breakdown blocks its segment (and disables the unit). Forgiving is
   *  quick to clear; hard blocks longer. BALANCE: owner-tuned. */
  breakdownBlockTicks: number;
  /** Condition a unit drops TO when it breaks down (needs attention after).
   *  BALANCE: owner-tuned. */
  breakdownConditionAfter: number;

  // ── Reliability feedback (the keystone) ─────────────────────────────────────
  /** On-time% at or above this counts as fully reliable (no demand/approval
   *  penalty). BALANCE: owner-tuned. */
  onTimeTarget: number;
  /** Ridership demand multiplier at 0% on-time (fully unreliable). At/above the
   *  target the multiplier is 1.0; it falls linearly to this floor as on-time%
   *  drops to 0. Reliable service keeps all its riders; chronic delays shed
   *  them. BALANCE: owner-tuned. */
  demandMultAtZeroOnTime: number;
  /** Approval points added at full reliability and subtracted at zero on-time,
   *  folded into the daily approval target. BALANCE: owner-tuned. */
  approvalReliabilitySwing: number;

  // ── Economy hooks (NUMBERS exposed; BALANCE owner-tuned) ─────────────────────
  /** Daily maintenance opex per active fleet unit (crews + parts), on TOP of the
   *  existing per-vehicle running cost. BALANCE: owner-tuned placeholder. */
  fleetMaintenancePerUnitPerDay: number;
  /** Daily standing cost per depot (staff + facility). BALANCE: owner-tuned. */
  depotDailyCost: number;
  /** One-off capex to build a depot. BALANCE: owner-tuned. */
  depotBuildCost: number;
}

/** FORGIVING default: recoverable incidents, slow bankruptcy — the widest
 *  audience. HARD: for the sim-depth crowd. Difficulty rides the scenario-rules
 *  axis, so `easy`/`normal` map to forgiving and `hard` maps to the punishing
 *  set. All values BALANCE: owner-tuned. */
const FORGIVING: OpsTunables = {
  conditionDecayPerMeter: 1e-6,
  weatherExposureDecayMult: 1.5,
  maintenanceConditionThreshold: 0.4,
  maintenanceRestoreTo: 1,
  maintenanceTicks: 240,
  // BALANCE: owner-tuned. Forgiving keeps incidents rare and quickly cleared so
  // a well-run network stays comfortably above onTimeTarget (reliability demand
  // multiplier ~1.0 = balance-neutral by default). The bite lives in HARD mode
  // and under real stress (worn stock, crowding, storms).
  breakdownBasePerTick: 1e-6,
  conditionRiskMult: 4,
  weatherRiskMult: 2,
  crowdRiskMult: 0.8,
  breakdownBlockTicks: 30,
  breakdownConditionAfter: 0.3,
  onTimeTarget: 0.9,
  demandMultAtZeroOnTime: 0.6,
  approvalReliabilitySwing: 8,
  fleetMaintenancePerUnitPerDay: 0,
  depotDailyCost: 2000,
  depotBuildCost: 750000,
};

const HARD: OpsTunables = {
  ...FORGIVING,
  breakdownBasePerTick: 9e-6,
  conditionRiskMult: 6,
  weatherRiskMult: 3,
  crowdRiskMult: 2.5,
  breakdownBlockTicks: 90,
  demandMultAtZeroOnTime: 0.4,
  approvalReliabilitySwing: 14,
  depotDailyCost: 3500,
};

/** Active ops tunables for a difficulty. `hard` is punishing; everything else
 *  (the forgiving default) shares the gentle set. */
export function opsTunables(difficulty: Difficulty): OpsTunables {
  return difficulty === 'hard' ? HARD : FORGIVING;
}
