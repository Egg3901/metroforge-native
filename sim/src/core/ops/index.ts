/**
 * v0.9 System A — Operations. Deterministic, save-serialized, on a dedicated
 * seeded RNG stream. Turns the map painter into a game: per-period frequency,
 * discrete aged/condition rolling stock, breakdowns that block segments and
 * cascade delay, depots + maintenance windows, and the KEYSTONE — reliability
 * (on-time% + delay) feeding BOTH approval and ridership demand.
 *
 * Boundary: this module never edits the demand pipeline the cohort lane owns
 * (transit/assignment.ts). It only READS route/vehicle state and applies the
 * reliability→ridership feedback AFTER assignment, in sim.refreshAssignment.
 *
 * Per-tick complexity budget: O(fleet units + active incidents). Fleet size is
 * bounded by purchased vehicles (~hundreds), so ops adds a small constant slice
 * on top of the existing vehicle loop. Reported via scripts/perf-harness.ts.
 */
import { MAX_HEADWAY, MODES } from '../constants';
import { Rng, type RngState } from '../rng';
import { routeCycleSeconds } from '../commands';
import { weatherBreakdownChance, WEATHER_BREAKDOWN_CHANCE } from '../weatherEffects';
import { DEFAULT_PERIOD_HEADWAY, opsTunables, type OpsTunables } from './tunables';
import { periodForTick, PERIODS, type Period } from './periods';
import type { BreakdownIncident, FleetUnit, GameState, RouteDef } from '../types';

/**
 * Ops cadence: the heavy per-unit work (condition decay, breakdown rolls,
 * timer/maintenance advance, reliability accrual) runs every OPS_INTERVAL ticks
 * with probabilities/decay/timers scaled by the interval, instead of every tick.
 * This keeps the whole subsystem's amortized cost a small slice of the tick
 * (perf budget) while staying fully deterministic — it just fires on a fixed
 * tick grid. Chosen well below the shortest breakdown block so incidents still
 * get multiple resolution steps.
 */
export const OPS_INTERVAL = 20;

/** Whether ops should run on this tick (fixed grid). */
export function opsDue(tick: number): boolean {
  return tick % OPS_INTERVAL === 0;
}

/** Lazily create ops sub-state on a fresh game or a legacy save that predates
 *  System A. Idempotent. */
export function initOps(state: GameState): void {
  if (!state.fleet) state.fleet = [];
  if (!state.depots) state.depots = [];
  if (!state.incidents) state.incidents = [];
  if (!state.opsDaily) state.opsDaily = {};
  if (!state.opsRngState) {
    // fork a stable, independent stream from the world seed so breakdown rolls
    // never reorder the events/growth stream.
    state.opsRngState = new Rng((state.seed ^ 0x09ab5eed) >>> 0).state();
  }
  if (!state.opsPeriod) state.opsPeriod = periodForTick(state.tick);
  // legacy saves carry routes with a fleet-less ledger: reconcile the discrete
  // units to each route's purchased vehicleCount so ops has something to run,
  // and freeze the neutral base headway ops restores at full availability.
  for (const r of state.routes) {
    if (r.scheduledHeadway === undefined) r.scheduledHeadway = r.headwaySeconds;
    syncFleetForRoute(state, r.id);
  }
}

/** Target headway (seconds) a route wants in a given period (player override or
 *  the default profile). */
export function periodTargetHeadway(route: RouteDef, period: Period): number {
  return route.frequency?.[period] ?? DEFAULT_PERIOD_HEADWAY[period];
}

/** Units a route needs to hit its target headway in a period, given its cycle
 *  time. Peak-period demand for units is what sizes the fleet a player must buy;
 *  off-peak needs fewer, so extra units idle (A1: more service = more vehicles). */
export function unitsForPeriod(state: GameState, route: RouteDef, period: Period): number {
  const cycle = routeCycleSeconds(state, route.id);
  const target = periodTargetHeadway(route, period);
  if (cycle <= 0 || target <= 0) return 0;
  return Math.max(1, Math.ceil(cycle / target));
}

/** The peak-period unit requirement across all periods — the fleet size a
 *  player should own to fully run their schedule. Exposed for UI ("needs N"). */
export function peakUnitsRequired(state: GameState, route: RouteDef): number {
  let peak = 0;
  for (const p of PERIODS) peak = Math.max(peak, unitsForPeriod(state, route, p));
  return peak;
}

/** Fleet units assigned to a route (any status). */
function assignedUnits(state: GameState, routeId: number): FleetUnit[] {
  return (state.fleet ?? []).filter((u) => u.routeId === routeId);
}

/**
 * Reconcile the discrete fleet ledger to a route's purchased `vehicleCount`.
 * Buying vehicles (vehicleCount up) mints new full-condition units; retiring
 * (down) removes the worst-condition units first. Kept in sync from the
 * command layer so `vehicleCount` stays the single purchase dial.
 */
export function syncFleetForRoute(state: GameState, routeId: number): void {
  if (!state.fleet) state.fleet = [];
  const route = state.routes.find((r) => r.id === routeId);
  if (!route) {
    // route gone: drop its units.
    state.fleet = state.fleet.filter((u) => u.routeId !== routeId);
    return;
  }
  const assigned = assignedUnits(state, routeId);
  const want = Math.max(0, Math.round(route.vehicleCount));
  if (assigned.length < want) {
    for (let i = assigned.length; i < want; i++) {
      state.fleet.push({
        id: state.nextId++,
        mode: route.mode,
        routeId,
        ageDays: 0,
        condition: 1,
        status: 'active',
        statusTicksLeft: 0,
      });
    }
  } else if (assigned.length > want) {
    // retire worst-condition first (stable id tiebreak → deterministic).
    const sorted = [...assigned].sort((a, b) => a.condition - b.condition || a.id - b.id);
    const drop = new Set(sorted.slice(0, assigned.length - want).map((u) => u.id));
    state.fleet = state.fleet.filter((u) => !drop.has(u.id));
  }
}

/** Units currently able to run service on a route (active, not down). */
function availableUnits(state: GameState, routeId: number): number {
  let n = 0;
  for (const u of state.fleet ?? []) if (u.routeId === routeId && u.status === 'active') n++;
  return n;
}

/** One fleet pass → available (active) unit count per route. Avoids the
 *  O(routes × fleet) cost of calling availableUnits per route. */
function availableCounts(state: GameState): Map<number, number> {
  const m = new Map<number, number>();
  for (const u of state.fleet ?? []) {
    if (u.status === 'active' && u.routeId !== null) m.set(u.routeId, (m.get(u.routeId) ?? 0) + 1);
  }
  return m;
}

/** In-service units for a route given a PRECOMPUTED cycle (the hot path). By
 *  default a route runs every healthy unit it owns (so a fresh route behaves
 *  exactly like the pre-v0.9 fleet→headway coupling, preserving balance); a set
 *  per-period target headway is an opt-in CAP that idles the rest. */
function inServiceWith(available: number, override: number | undefined, cycle: number): number {
  if (override === undefined || cycle <= 0 || override <= 0) return available;
  const cap = Math.max(1, Math.ceil(cycle / override));
  return Math.min(available, cap);
}

/** Effective headway (seconds) from a PRECOMPUTED cycle: cycle / in-service,
 *  floored at the mode minimum and capped at MAX_HEADWAY. */
function effectiveHeadwayWith(mode: RouteDef['mode'], cycle: number, inService: number, fallback: number): number {
  if (inService <= 0) return MAX_HEADWAY;
  if (cycle <= 0) return fallback;
  return Math.max(MODES[mode].minHeadway, Math.min(MAX_HEADWAY, cycle / inService));
}

/** In-service unit count for a route this period. Convenience wrapper (recomputes
 *  the cycle); the per-tick loop uses the cached-cycle path instead. */
export function inServiceFor(state: GameState, route: RouteDef, period: Period): number {
  return inServiceWith(availableUnits(state, route.id), route.frequency?.[period], routeCycleSeconds(state, route.id));
}

/** Refresh each route's in-service count + effective headway for `period`. Sets
 *  headwaySeconds (the value assignment reads) so per-period frequency flows
 *  straight into the existing headway/cycle-time path. Returns true if any
 *  route's headway moved enough to warrant a demand refresh.
 *
 *  Perf: the neutral path (full availability, no throttle) never touches
 *  routeCycleSeconds (the per-segment field-sampling hot spot) — it just
 *  restores the frozen base headway. Cycle is computed LAZILY only for routes
 *  that are actually degraded (breakdown/maintenance) or throttled. */
function refreshService(state: GameState, period: Period, avail: Map<number, number>): boolean {
  let changed = false;
  for (const r of state.routes) {
    const available = avail.get(r.id) ?? 0;
    const override = r.frequency?.[period];
    const base = r.scheduledHeadway ?? r.headwaySeconds ?? MAX_HEADWAY;
    let inService: number;
    let eff: number;
    if (override === undefined && available >= r.vehicleCount) {
      // full availability, no throttle → neutral: exactly the command-set
      // headway (identical to the pre-v0.9 fleet→headway coupling). Ops adds no
      // balance shift and no cycle recompute here.
      inService = available;
      eff = base;
    } else {
      const cycle = routeCycleSeconds(state, r.id); // lazy: degraded/throttled only
      inService = inServiceWith(available, override, cycle);
      eff = effectiveHeadwayWith(r.mode, cycle, inService, base);
    }
    if (r.inServiceVehicles !== inService || Math.abs((r.headwaySeconds ?? 0) - eff) > 1) changed = true;
    r.inServiceVehicles = inService;
    r.headwaySeconds = eff;
  }
  return changed;
}

/** Distance a running unit covers in one tick (meters), grade+weather aware.
 *  Uses the route's cached day-average grade speed (falls back to mode cruise). */
function metersPerTick(route: RouteDef): number {
  return route.moveGradeSpeed ?? MODES[route.mode].speed;
}

/** Decay condition of in-service units by distance run and weather exposure.
 *  Only units actually running service wear; parked/maintenance units don't. */
function decayCondition(state: GameState, t: OpsTunables): void {
  const intensity = state.weather?.intensity ?? 0;
  const routeById = new Map(state.routes.map((r) => [r.id, r]));
  for (const u of state.fleet ?? []) {
    if (u.status !== 'active' || u.routeId === null) continue;
    const route = routeById.get(u.routeId);
    if (!route) continue;
    // only units actually in service (not idling off-peak) accrue wear.
    const inService = route.inServiceVehicles ?? 0;
    if (inService <= 0) continue;
    const meters = metersPerTick(route) * OPS_INTERVAL; // interval's worth of running
    const exposure = route.surfaceExposure ?? 1;
    const weatherWear = 1 + t.weatherExposureDecayMult * exposure * intensity;
    u.condition = Math.max(0, u.condition - t.conditionDecayPerMeter * meters * weatherWear);
  }
}

/** Roll breakdowns for in-service units and advance active incidents. Returns a
 *  list of freshly broken-down routes for the host to toast (player copy). */
function rollBreakdowns(state: GameState, t: OpsTunables): { routeName: string }[] {
  const started: { routeName: string }[] = [];
  const rng = new Rng(state.opsRngState as RngState);
  const routeById = new Map(state.routes.map((r) => [r.id, r]));
  const clearBase = WEATHER_BREAKDOWN_CHANCE.clear;
  // weather ratio is mode-independent (the shared helper ignores mode) and the
  // same for every unit this tick, so compute it once.
  const weatherFactor = 1 + (weatherBreakdownChance(state.weather, 'bus') / clearBase - 1) * (t.weatherRiskMult / 10);
  // per-tick base scaled by the ops interval (rolls fire every OPS_INTERVAL ticks).
  const baseInterval = t.breakdownBasePerTick * OPS_INTERVAL;
  // one active incident per route blocks that route; skip rolling extra units on
  // an already-blocked route (the block IS the disruption).
  const blocked = new Set((state.incidents ?? []).map((i) => i.routeId));
  for (const u of state.fleet ?? []) {
    if (u.status !== 'active' || u.routeId === null) continue;
    if (blocked.has(u.routeId)) continue;
    const route = routeById.get(u.routeId);
    if (!route || (route.inServiceVehicles ?? 0) <= 0) continue;
    const crowdExcess = Math.max(0, (route.crowding ?? 0) - 1);
    const p =
      baseInterval *
      (1 + t.conditionRiskMult * (1 - u.condition)) *
      (1 + t.crowdRiskMult * crowdExcess) *
      weatherFactor;
    if (rng.chance(p)) {
      u.status = 'brokenDown';
      u.statusTicksLeft = t.breakdownBlockTicks;
      u.condition = Math.min(u.condition, t.breakdownConditionAfter);
      const segIdx = route.segmentIds.length > 0 ? rng.int(0, route.segmentIds.length - 1) : 0;
      (state.incidents ??= []).push({
        id: state.nextId++,
        routeId: route.id,
        unitId: u.id,
        segmentIndex: segIdx,
        ticksLeft: t.breakdownBlockTicks,
      });
      blocked.add(route.id);
      started.push({ routeName: route.name });
    }
  }
  state.opsRngState = rng.state();
  return started;
}

/** Advance timers on incidents + out-of-service units; clear when elapsed.
 *  Returns the number of units that returned to service (availability changed). */
function advanceTimers(state: GameState, t: OpsTunables): number {
  let recovered = 0;
  // incidents
  const stillActive: BreakdownIncident[] = [];
  for (const inc of state.incidents ?? []) {
    inc.ticksLeft -= OPS_INTERVAL;
    if (inc.ticksLeft > 0) stillActive.push(inc);
  }
  state.incidents = stillActive;
  const liveIncidentUnits = new Set(stillActive.map((i) => i.unitId));
  // units
  for (const u of state.fleet ?? []) {
    if (u.status === 'active') continue;
    u.statusTicksLeft -= OPS_INTERVAL;
    if (u.statusTicksLeft > 0) continue;
    if (u.status === 'maintenance') {
      u.condition = t.maintenanceRestoreTo;
      u.status = 'active';
      u.statusTicksLeft = 0;
      recovered++;
    } else if (u.status === 'brokenDown' && !liveIncidentUnits.has(u.id)) {
      // block cleared: unit limps back into service at its (low) condition.
      u.status = 'active';
      u.statusTicksLeft = 0;
      recovered++;
    }
  }
  return recovered;
}

/** Accumulate reliability for the day: fractional departures per tick and, while
 *  a route is blocked, the delayed share + delay seconds. Pure arithmetic →
 *  smooth, monotone, deterministic on-time% and average delay. */
function accrueReliability(state: GameState, t: OpsTunables): void {
  const daily = (state.opsDaily ??= {});
  // per route: number of active incidents + the worst remaining block. One
  // broken-down unit disables ONE unit and delays the vehicles behind it — it
  // does NOT take the whole line down. So the delayed share of departures scales
  // with the FRACTION of the route's fleet that is down, which keeps reliability
  // fleet-size-neutral (a bigger fleet has more redundancy, not more fragility).
  const incCount = new Map<number, number>();
  const worstBlk = new Map<number, number>();
  for (const inc of state.incidents ?? []) {
    incCount.set(inc.routeId, (incCount.get(inc.routeId) ?? 0) + 1);
    worstBlk.set(inc.routeId, Math.max(worstBlk.get(inc.routeId) ?? 0, inc.ticksLeft));
  }
  for (const r of state.routes) {
    const eff = r.headwaySeconds || MAX_HEADWAY;
    const dep = OPS_INTERVAL / eff; // fractional departures over this ops interval
    const d = (daily[r.id] ??= { departures: 0, delayedDepartures: 0, delaySec: 0 });
    d.departures += dep;
    const down = incCount.get(r.id);
    if (down !== undefined) {
      const inService = r.inServiceVehicles ?? 0;
      // fraction of service disrupted = down units / (running + down units).
      const frac = Math.min(1, down / Math.max(1, inService + down));
      d.delayedDepartures += dep * frac;
      // a delayed departure waits ~half the worst remaining block on average.
      d.delaySec += dep * frac * ((worstBlk.get(r.id) ?? 0) / 2);
    }
  }
  void t;
}

/**
 * Per-tick ops step. Called from simTick after moveVehicles. Cheap: refreshes
 * service only on a period change, then decays condition, rolls breakdowns,
 * advances timers, and accrues reliability.
 */
export function opsTick(state: GameState, events?: OpsTickSink): { serviceChanged: boolean } {
  if (!state.fleet) initOps(state);
  const t = opsTunables(state.difficulty);
  const period = periodForTick(state.tick);
  // refresh service (O(routes), cycle computed only for degraded routes); it
  // picks up fleet changes from breakdowns/maintenance so headway/capacity track
  // availability. `serviceMoved` is true only when a route's effective headway
  // ACTUALLY changed — a bare period change with no throttle leaves the neutral
  // base headway untouched, so it must NOT force an (expensive) reassignment.
  let serviceMoved = refreshService(state, period, availableCounts(state));
  state.opsPeriod = period;
  decayCondition(state, t);
  const started = rollBreakdowns(state, t);
  const recovered = advanceTimers(state, t);
  // re-refresh ONLY when availability actually changed this tick (a breakdown
  // started or a unit returned) so reliability sees the new service level.
  if (started.length > 0 || recovered > 0) serviceMoved = refreshService(state, period, availableCounts(state)) || serviceMoved;
  accrueReliability(state, t);
  if (events && started.length) {
    const toasts = events.toasts ?? (events.toasts = []);
    for (const b of started) {
      // player copy: no em/en dashes, no filler.
      toasts.push({ message: `A vehicle broke down on ${b.routeName}. Following services are delayed.`, tone: 'warn' });
    }
  }
  return { serviceChanged: serviceMoved };
}

/** Minimal sink for opsTick to push incident toasts (matches TickEvents). */
export interface OpsTickSink {
  toasts?: { message: string; tone: 'good' | 'warn' | 'info' }[];
}

/** Reliability→ridership multiplier for an on-time fraction (monotone
 *  increasing): 1.0 at/above target, falling linearly to the floor at 0% on
 *  time. Reliable service keeps its riders; chronic delays shed them. */
export function reliabilityDemandMultFor(onTimePct: number, t: OpsTunables): number {
  const clamped = Math.max(0, Math.min(1, onTimePct));
  if (clamped >= t.onTimeTarget) return 1;
  const frac = t.onTimeTarget > 0 ? clamped / t.onTimeTarget : 0;
  return t.demandMultAtZeroOnTime + (1 - t.demandMultAtZeroOnTime) * frac;
}

/**
 * Day-close ops pass: compute per-route on-time% + avg delay, refresh the lagged
 * reliability→demand multiplier, dispatch worn units to maintenance (if a depot
 * of that mode exists), age the fleet, and reset the daily accumulators.
 */
export function opsDailyClose(state: GameState): void {
  if (!state.fleet) initOps(state);
  const t = opsTunables(state.difficulty);
  const daily = state.opsDaily ?? {};
  for (const r of state.routes) {
    const d = daily[r.id];
    if (d && d.departures > 0) {
      const onTime = Math.max(0, Math.min(1, 1 - d.delayedDepartures / d.departures));
      r.onTimePct = onTime;
      r.avgDelaySec = d.delaySec / d.departures;
    } else {
      // no service ran → treat as fully on time (nothing to be late).
      r.onTimePct = 1;
      r.avgDelaySec = 0;
    }
    r.reliabilityDemandMult = reliabilityDemandMultFor(r.onTimePct, t);
  }
  // maintenance dispatch: worn units of a mode with a depot go out of service to
  // be restored; this CUTS the route's in-service count while active (the
  // maintenance-window tradeoff). No depot for the mode → no dispatch → deferred
  // maintenance, and condition keeps falling (raising breakdown risk).
  const depotModes = new Set((state.depots ?? []).map((dp) => dp.mode));
  for (const u of state.fleet ?? []) {
    if (u.status !== 'active') continue;
    if (u.condition < t.maintenanceConditionThreshold && depotModes.has(u.mode)) {
      u.status = 'maintenance';
      u.statusTicksLeft = t.maintenanceTicks;
    }
  }
  for (const u of state.fleet ?? []) u.ageDays += 1;
  state.opsDaily = {};
}

/**
 * Apply the KEYSTONE reliability→ridership feedback AFTER assignment (called
 * from refreshAssignment). Scales each route's ridership/revenue by its lagged
 * reliability multiplier and returns the total transit trips shed to chronic
 * unreliability (the caller moves them to car so the mode-share stat stays
 * consistent). Assignment.ts is never touched.
 */
export function applyReliabilityDemand(
  state: GameState,
  routeRidership: Map<number, number>,
  routeRevenue: Map<number, number>,
): { transitLost: number } {
  let transitLost = 0;
  for (const r of state.routes) {
    const mult = r.reliabilityDemandMult ?? 1;
    const rawRidership = routeRidership.get(r.id) ?? 0;
    const rawRevenue = routeRevenue.get(r.id) ?? 0;
    r.dailyRidership = rawRidership * mult;
    r.dailyRevenue = rawRevenue * mult;
    transitLost += rawRidership * (1 - mult);
  }
  return { transitLost };
}

/**
 * Ridership-weighted reliability contribution to the daily approval target,
 * in [-swing, +swing]: reliable networks lift approval, chronically delayed
 * ones drag it. Folded into updateApproval.
 */
export function opsApprovalDelta(state: GameState): number {
  const t = opsTunables(state.difficulty);
  let riders = 0;
  let weighted = 0;
  for (const r of state.routes) {
    const w = r.dailyRidership || 0;
    riders += w;
    weighted += w * (r.onTimePct ?? 1);
  }
  if (riders <= 0) return 0;
  const meanOnTime = weighted / riders; // 0..1
  // center on the on-time target: at target → 0, at 100% → +swing, at 0 → -swing.
  const centered = (meanOnTime - t.onTimeTarget) / Math.max(1e-6, 1 - t.onTimeTarget);
  return Math.max(-1, Math.min(1, centered)) * t.approvalReliabilitySwing;
}

/** Daily ops opex: fleet maintenance per active unit + standing depot cost.
 *  NUMBERS only — the constants are owner-tuned placeholders in tunables. */
export function opsDailyOpex(state: GameState): number {
  const t = opsTunables(state.difficulty);
  let units = 0;
  for (const u of state.fleet ?? []) if (u.status !== 'brokenDown') units++;
  const depotCost = (state.depots ?? []).length * t.depotDailyCost;
  return units * t.fleetMaintenancePerUnitPerDay + depotCost;
}

/** Fleet-wide summary for the UI (additive). */
export function fleetSummary(state: GameState): {
  total: number;
  active: number;
  maintenance: number;
  brokenDown: number;
  avgCondition: number;
  avgAgeDays: number;
} {
  const fleet = state.fleet ?? [];
  let active = 0;
  let maintenance = 0;
  let brokenDown = 0;
  let cond = 0;
  let age = 0;
  for (const u of fleet) {
    if (u.status === 'active') active++;
    else if (u.status === 'maintenance') maintenance++;
    else brokenDown++;
    cond += u.condition;
    age += u.ageDays;
  }
  const n = fleet.length || 1;
  return {
    total: fleet.length,
    active,
    maintenance,
    brokenDown,
    avgCondition: cond / n,
    avgAgeDays: age / n,
  };
}
