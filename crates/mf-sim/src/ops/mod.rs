//! v0.9 System A - Operations. Deterministic, save-serialized, on a dedicated
//! seeded RNG stream (`ops_rng_state`) so breakdown rolls never reorder other
//! systems. Ports `sim/src/core/ops/index.ts`.
//!
//! Turns the map painter into a game: per-period frequency, discrete
//! aged/condition rolling stock, breakdowns that block segments and cascade
//! delay, depots + maintenance windows, and the KEYSTONE - reliability
//! (on-time% + delay) feeding BOTH approval and ridership demand.
//!
//! Boundary: this module never edits the demand pipeline (transit/assignment).
//! It only READS route/vehicle state and applies the reliability -> ridership
//! feedback AFTER assignment. The frozen `sim_tick` orchestrator wires the
//! per-tick `step` + daily `ops_daily_close`; this crate delivers standalone
//! fns.

pub mod depot;
pub mod periods;
pub mod tunables;

use crate::constants::{modes, surface_congestion_weight, MAX_HEADWAY};
use crate::rng::Rng;
use crate::types::{
    BreakdownIncident, FieldGrid, FleetStatus, FleetUnit, GameState, Period, RouteDef, TrackGrade,
    TrackSegment, TransitMode, WeatherSnapshot,
};
use periods::{period_for_tick, PERIODS};
use std::collections::BTreeMap;
use tunables::{default_period_headway, ops_tunables, OpsTunables};

/// Ops cadence: the heavy per-unit work (condition decay, breakdown rolls,
/// timer/maintenance advance, reliability accrual) runs every `OPS_INTERVAL`
/// ticks with probabilities/decay/timers scaled by the interval. Keeps the
/// subsystem a small amortized slice of the tick while staying fully
/// deterministic (it fires on a fixed tick grid). Mirrors `OPS_INTERVAL`.
pub const OPS_INTERVAL: u32 = 20;

/// Whether ops should run on this tick (fixed grid). Mirrors `opsDue`.
pub fn ops_due(tick: u64) -> bool {
    tick.is_multiple_of(OPS_INTERVAL as u64)
}

/// Toast tone for host UI. Mirrors the `TickEvents` toast tone union.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpsTone {
    /// Positive.
    Good,
    /// Warning.
    Warn,
    /// Informational.
    Info,
}

/// A player-facing toast produced by the ops step (no em/en dashes).
#[derive(Clone, Debug, PartialEq)]
pub struct OpsToast {
    /// Message copy.
    pub message: String,
    /// Tone.
    pub tone: OpsTone,
}

/// Result of one ops [`step`]: whether service changed enough to warrant a
/// demand refresh, plus any incident toasts for the host.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpsStepResult {
    /// True when a route's effective headway actually moved.
    pub service_changed: bool,
    /// Incident toasts to surface.
    pub toasts: Vec<OpsToast>,
}

// ── init / fleet reconcile ────────────────────────────────────────────────────

/// Lazily create ops sub-state on a fresh game or a legacy save that predates
/// System A. Idempotent. Mirrors `initOps`.
pub fn init_ops(state: &mut GameState) {
    state.fleet.get_or_insert_with(Vec::new);
    state.depots.get_or_insert_with(Vec::new);
    state.incidents.get_or_insert_with(Vec::new);
    state.ops_daily.get_or_insert_with(BTreeMap::new);
    if state.ops_rng_state.is_none() {
        // fork a stable, independent stream from the world seed so breakdown
        // rolls never reorder the events/growth stream.
        state.ops_rng_state = Some(Rng::from_seed(state.seed ^ 0x09ab_5eed).state());
    }
    if state.ops_period.is_none() {
        state.ops_period = Some(period_for_tick(state.tick));
    }
    // legacy saves carry routes with a fleet-less ledger: freeze the neutral
    // base headway ops restores at full availability, then reconcile discrete
    // units to each route's purchased vehicle_count.
    let route_ids: Vec<u32> = state.routes.iter().map(|r| r.id).collect();
    for r in state.routes.iter_mut() {
        if r.scheduled_headway.is_none() {
            r.scheduled_headway = Some(r.headway_seconds);
        }
    }
    for id in route_ids {
        sync_fleet_for_route(state, id);
    }
}

/// Reconcile the discrete fleet ledger to a route's purchased `vehicle_count`.
/// Buying vehicles mints new full-condition units; retiring removes the
/// worst-condition units first (stable id tiebreak -> deterministic). Mirrors
/// `syncFleetForRoute`.
pub fn sync_fleet_for_route(state: &mut GameState, route_id: u32) {
    let fleet = state.fleet.get_or_insert_with(Vec::new);
    let route = state.routes.iter().find(|r| r.id == route_id);
    let Some(route) = route else {
        // route gone: drop its units.
        fleet.retain(|u| u.route_id != Some(route_id));
        return;
    };
    let mode = route.mode;
    let want = route.vehicle_count;
    let assigned: usize = fleet
        .iter()
        .filter(|u| u.route_id == Some(route_id))
        .count();
    if (assigned as u32) < want {
        for _ in assigned as u32..want {
            let id = state.next_id;
            state.next_id += 1;
            state.fleet.as_mut().unwrap().push(FleetUnit {
                id,
                mode,
                route_id: Some(route_id),
                age_days: 0.0,
                condition: 1.0,
                status: FleetStatus::Active,
                status_ticks_left: 0,
            });
        }
    } else if (assigned as u32) > want {
        // retire worst-condition first, stable id tiebreak.
        let mut units: Vec<(f64, u32)> = fleet
            .iter()
            .filter(|u| u.route_id == Some(route_id))
            .map(|u| (u.condition, u.id))
            .collect();
        units.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
        let drop_n = assigned - want as usize;
        let drop: std::collections::BTreeSet<u32> =
            units.iter().take(drop_n).map(|(_, id)| *id).collect();
        fleet.retain(|u| !drop.contains(&u.id));
    }
}

// ── service sizing (per period) ───────────────────────────────────────────────

/// Target headway (seconds) a route wants in a given period (player override or
/// the default profile). Mirrors `periodTargetHeadway`.
pub fn period_target_headway(route: &RouteDef, period: Period) -> f64 {
    route
        .frequency
        .as_ref()
        .and_then(|f| f.get(&period).copied())
        .unwrap_or_else(|| default_period_headway(period))
}

/// Units a route needs to hit its target headway in a period, given its cycle
/// time. Mirrors `unitsForPeriod`.
pub fn units_for_period(state: &GameState, route: &RouteDef, period: Period) -> u32 {
    let cycle = route_cycle_seconds(&state.tracks, &state.fields, route);
    let target = period_target_headway(route, period);
    if cycle <= 0.0 || target <= 0.0 {
        return 0;
    }
    ((cycle / target).ceil() as u32).max(1)
}

/// The peak-period unit requirement across all periods - the fleet size a
/// player should own to fully run their schedule. Mirrors `peakUnitsRequired`.
pub fn peak_units_required(state: &GameState, route: &RouteDef) -> u32 {
    PERIODS
        .iter()
        .map(|&p| units_for_period(state, route, p))
        .max()
        .unwrap_or(0)
}

/// In-service units given a precomputed cycle. A route runs every healthy unit
/// it owns by default (so a fresh route behaves exactly like the pre-v0.9
/// fleet -> headway coupling); a set per-period target headway is an opt-in CAP.
/// Mirrors `inServiceWith`.
fn in_service_with(available: u32, override_hw: Option<f64>, cycle: f64) -> u32 {
    match override_hw {
        Some(o) if cycle > 0.0 && o > 0.0 => {
            let cap = ((cycle / o).ceil() as u32).max(1);
            available.min(cap)
        }
        _ => available,
    }
}

/// Effective headway (seconds) from a precomputed cycle: `cycle / in_service`,
/// floored at the mode minimum and capped at `MAX_HEADWAY`. Mirrors
/// `effectiveHeadwayWith`.
fn effective_headway_with(mode: TransitMode, cycle: f64, in_service: u32, fallback: f64) -> f64 {
    if in_service == 0 {
        return MAX_HEADWAY;
    }
    if cycle <= 0.0 {
        return fallback;
    }
    (modes(mode).min_headway).max((cycle / in_service as f64).min(MAX_HEADWAY))
}

/// In-service unit count for a route this period (recomputes the cycle).
/// Convenience wrapper; the per-tick loop uses the cached path. Mirrors
/// `inServiceFor`.
pub fn in_service_for(state: &GameState, route: &RouteDef, period: Period) -> u32 {
    let available = available_units(state, route.id);
    let override_hw = route
        .frequency
        .as_ref()
        .and_then(|f| f.get(&period).copied());
    in_service_with(
        available,
        override_hw,
        route_cycle_seconds(&state.tracks, &state.fields, route),
    )
}

/// Units currently able to run service on a route (active, not down).
fn available_units(state: &GameState, route_id: u32) -> u32 {
    state
        .fleet
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter(|u| u.route_id == Some(route_id) && u.status == FleetStatus::Active)
        .count() as u32
}

/// One fleet pass -> available (active) unit count per route. Avoids the
/// O(routes x fleet) cost of calling `available_units` per route. Mirrors
/// `availableCounts`.
fn available_counts(state: &GameState) -> BTreeMap<u32, u32> {
    let mut m: BTreeMap<u32, u32> = BTreeMap::new();
    for u in state.fleet.as_deref().unwrap_or(&[]) {
        if u.status == FleetStatus::Active {
            if let Some(rid) = u.route_id {
                *m.entry(rid).or_insert(0) += 1;
            }
        }
    }
    m
}

/// Refresh each route's in-service count + effective headway for `period`. Sets
/// `headway_seconds` (the value assignment reads). Returns true if any route's
/// headway moved enough to warrant a demand refresh. The neutral path (full
/// availability, no throttle) never touches `route_cycle_seconds` - it just
/// restores the frozen base headway. Mirrors `refreshService`.
fn refresh_service(state: &mut GameState, period: Period, avail: &BTreeMap<u32, u32>) -> bool {
    let mut changed = false;
    // disjoint field borrows: routes mut, tracks/fields immut.
    let tracks = &state.tracks;
    let fields = &state.fields;
    for r in state.routes.iter_mut() {
        let available = avail.get(&r.id).copied().unwrap_or(0);
        let override_hw = r.frequency.as_ref().and_then(|f| f.get(&period).copied());
        let base = r
            .scheduled_headway
            .or(Some(r.headway_seconds))
            .unwrap_or(MAX_HEADWAY);
        let (in_service, eff) = if override_hw.is_none() && available >= r.vehicle_count {
            // full availability, no throttle -> neutral (no cycle recompute).
            (available, base)
        } else {
            let cycle = route_cycle_seconds(tracks, fields, r); // lazy: degraded only
            let is = in_service_with(available, override_hw, cycle);
            (is, effective_headway_with(r.mode, cycle, is, base))
        };
        if r.in_service_vehicles != Some(in_service) || (r.headway_seconds - eff).abs() > 1.0 {
            changed = true;
        }
        r.in_service_vehicles = Some(in_service);
        r.headway_seconds = eff;
    }
    changed
}

// ── condition decay ───────────────────────────────────────────────────────────

/// Distance a running unit covers in one tick (meters). Uses the route's cached
/// day-average grade speed (falls back to mode cruise). Mirrors `metersPerTick`.
fn meters_per_tick(route: &RouteDef) -> f64 {
    route.move_grade_speed.unwrap_or(modes(route.mode).speed)
}

/// Decay condition of in-service units by distance run and weather exposure.
/// Only units actually running service wear. Mirrors `decayCondition`.
fn decay_condition(state: &mut GameState, t: &OpsTunables) {
    let intensity = weather_intensity(&state.weather);
    // precompute per-route (meters, exposure, in_service) so the fleet loop
    // borrows only state.fleet.
    let per_route: BTreeMap<u32, (f64, f64, u32)> = state
        .routes
        .iter()
        .map(|r| {
            (
                r.id,
                (
                    meters_per_tick(r) * OPS_INTERVAL as f64,
                    r.surface_exposure.unwrap_or(1.0),
                    r.in_service_vehicles.unwrap_or(0),
                ),
            )
        })
        .collect();
    for u in state
        .fleet
        .as_mut()
        .map(|v| v.iter_mut())
        .into_iter()
        .flatten()
    {
        if u.status != FleetStatus::Active {
            continue;
        }
        let Some(rid) = u.route_id else { continue };
        let Some(&(meters, exposure, in_service)) = per_route.get(&rid) else {
            continue;
        };
        if in_service == 0 {
            continue;
        }
        let weather_wear = 1.0 + t.weather_exposure_decay_mult * exposure * intensity;
        u.condition = (u.condition - t.condition_decay_per_meter * meters * weather_wear).max(0.0);
    }
}

// ── breakdowns + timers ───────────────────────────────────────────────────────

struct RouteRoll {
    name: String,
    in_service: u32,
    crowding: f64,
    seg_len: usize,
}

/// Roll breakdowns for in-service units and open incidents. Returns the names
/// of freshly broken-down routes for the host to toast. Mirrors
/// `rollBreakdowns`.
fn roll_breakdowns(state: &mut GameState, t: &OpsTunables) -> Vec<String> {
    let mut started: Vec<String> = Vec::new();
    let mut rng = Rng::from_state(state.ops_rng_state.expect("ops rng seeded by init_ops"));
    let per_route: BTreeMap<u32, RouteRoll> = state
        .routes
        .iter()
        .map(|r| {
            (
                r.id,
                RouteRoll {
                    name: r.name.clone(),
                    in_service: r.in_service_vehicles.unwrap_or(0),
                    crowding: r.crowding,
                    seg_len: r.segment_ids.len(),
                },
            )
        })
        .collect();
    // weather factor is mode-independent and identical for every unit this tick.
    let weather_factor =
        1.0 + (weather_breakdown_ratio(&state.weather) - 1.0) * (t.weather_risk_mult / 10.0);
    let base_interval = t.breakdown_base_per_tick * OPS_INTERVAL as f64;
    // one active incident per route blocks it; skip already-blocked routes.
    let mut blocked: std::collections::BTreeSet<u32> = state
        .incidents
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|i| i.route_id)
        .collect();

    let mut new_incidents: Vec<BreakdownIncident> = Vec::new();
    let mut next_id = state.next_id;

    // iterate fleet with an index so we can mutate units in place.
    if let Some(fleet) = state.fleet.as_mut() {
        for u in fleet.iter_mut() {
            if u.status != FleetStatus::Active {
                continue;
            }
            let Some(rid) = u.route_id else { continue };
            if blocked.contains(&rid) {
                continue;
            }
            let Some(route) = per_route.get(&rid) else {
                continue;
            };
            if route.in_service == 0 {
                continue;
            }
            let crowd_excess = (route.crowding - 1.0).max(0.0);
            let p = base_interval
                * (1.0 + t.condition_risk_mult * (1.0 - u.condition))
                * (1.0 + t.crowd_risk_mult * crowd_excess)
                * weather_factor;
            if rng.chance(p) {
                u.status = FleetStatus::BrokenDown;
                u.status_ticks_left = t.breakdown_block_ticks;
                u.condition = u.condition.min(t.breakdown_condition_after);
                let seg_idx = if route.seg_len > 0 {
                    rng.int(0, route.seg_len as i64 - 1) as u32
                } else {
                    0
                };
                new_incidents.push(BreakdownIncident {
                    id: next_id,
                    route_id: rid,
                    unit_id: u.id,
                    segment_index: seg_idx,
                    ticks_left: t.breakdown_block_ticks,
                });
                next_id += 1;
                blocked.insert(rid);
                started.push(route.name.clone());
            }
        }
    }
    state.next_id = next_id;
    if !new_incidents.is_empty() {
        state
            .incidents
            .get_or_insert_with(Vec::new)
            .extend(new_incidents);
    }
    state.ops_rng_state = Some(rng.state());
    started
}

/// Advance timers on incidents + out-of-service units; clear when elapsed.
/// Returns the number of units that returned to service. Mirrors
/// `advanceTimers`.
fn advance_timers(state: &mut GameState, t: &OpsTunables) -> u32 {
    let mut recovered = 0u32;
    // incidents: decrement, keep those with time left.
    let mut still_active: Vec<BreakdownIncident> = Vec::new();
    for inc in state.incidents.take().unwrap_or_default() {
        let mut inc = inc;
        inc.ticks_left = inc.ticks_left.saturating_sub(OPS_INTERVAL);
        if inc.ticks_left > 0 {
            still_active.push(inc);
        }
    }
    let live_incident_units: std::collections::BTreeSet<u32> =
        still_active.iter().map(|i| i.unit_id).collect();
    state.incidents = Some(still_active);
    // units.
    if let Some(fleet) = state.fleet.as_mut() {
        for u in fleet.iter_mut() {
            if u.status == FleetStatus::Active {
                continue;
            }
            u.status_ticks_left = u.status_ticks_left.saturating_sub(OPS_INTERVAL);
            if u.status_ticks_left > 0 {
                continue;
            }
            match u.status {
                FleetStatus::Maintenance => {
                    u.condition = t.maintenance_restore_to;
                    u.status = FleetStatus::Active;
                    u.status_ticks_left = 0;
                    recovered += 1;
                }
                FleetStatus::BrokenDown if !live_incident_units.contains(&u.id) => {
                    // block cleared: unit limps back into service at low condition.
                    u.status = FleetStatus::Active;
                    u.status_ticks_left = 0;
                    recovered += 1;
                }
                _ => {}
            }
        }
    }
    recovered
}

// ── reliability accrual ───────────────────────────────────────────────────────

/// Accumulate reliability for the day: fractional departures per tick and,
/// while a route is blocked, the delayed share + delay seconds. Mirrors
/// `accrueReliability`.
fn accrue_reliability(state: &mut GameState) {
    let daily = state.ops_daily.get_or_insert_with(BTreeMap::new);
    // per route: active incident count + worst remaining block.
    let mut inc_count: BTreeMap<u32, u32> = BTreeMap::new();
    let mut worst_blk: BTreeMap<u32, u32> = BTreeMap::new();
    for inc in state.incidents.as_deref().unwrap_or(&[]) {
        *inc_count.entry(inc.route_id).or_insert(0) += 1;
        let w = worst_blk.entry(inc.route_id).or_insert(0);
        *w = (*w).max(inc.ticks_left);
    }
    for r in &state.routes {
        let eff = if r.headway_seconds != 0.0 {
            r.headway_seconds
        } else {
            MAX_HEADWAY
        };
        let dep = OPS_INTERVAL as f64 / eff;
        let d = daily.entry(r.id).or_default();
        d.departures += dep;
        if let Some(&down) = inc_count.get(&r.id) {
            let in_service = r.in_service_vehicles.unwrap_or(0);
            // fraction disrupted = down / (running + down). Fleet-size-neutral:
            // one broken unit disables ONE unit, not the whole line.
            let frac = (down as f64 / (in_service + down).max(1) as f64).min(1.0);
            d.delayed_departures += dep * frac;
            // a delayed departure waits ~half the worst remaining block.
            d.delay_sec += dep * frac * (worst_blk.get(&r.id).copied().unwrap_or(0) as f64 / 2.0);
        }
    }
}

// ── the per-tick step ─────────────────────────────────────────────────────────

/// Per-tick ops step (the ops tick). Called from `sim_tick` after moveVehicles.
/// Refreshes service only on a period change, then decays condition, rolls
/// breakdowns, advances timers, and accrues reliability. Mirrors `opsTick`.
pub fn step(state: &mut GameState) -> OpsStepResult {
    if state.fleet.is_none() {
        init_ops(state);
    }
    let t = ops_tunables(state.difficulty);
    let period = period_for_tick(state.tick);
    let mut service_moved = refresh_service(state, period, &available_counts(state));
    state.ops_period = Some(period);
    decay_condition(state, &t);
    let started = roll_breakdowns(state, &t);
    let recovered = advance_timers(state, &t);
    // re-refresh ONLY when availability actually changed this tick.
    if !started.is_empty() || recovered > 0 {
        service_moved = refresh_service(state, period, &available_counts(state)) || service_moved;
    }
    accrue_reliability(state);
    let mut toasts = Vec::new();
    for name in &started {
        // player copy: no em/en dashes, no filler.
        toasts.push(OpsToast {
            message: format!("A vehicle broke down on {name}. Following services are delayed."),
            tone: OpsTone::Warn,
        });
    }
    OpsStepResult {
        service_changed: service_moved,
        toasts,
    }
}

// ── the keystone: reliability -> demand + approval ────────────────────────────

/// Reliability -> ridership multiplier for an on-time fraction (monotone
/// increasing): 1.0 at/above target, falling linearly to the floor at 0% on
/// time. Mirrors `reliabilityDemandMultFor`.
pub fn reliability_demand_mult_for(on_time_pct: f64, t: &OpsTunables) -> f64 {
    let clamped = on_time_pct.clamp(0.0, 1.0);
    if clamped >= t.on_time_target {
        return 1.0;
    }
    let frac = if t.on_time_target > 0.0 {
        clamped / t.on_time_target
    } else {
        0.0
    };
    t.demand_mult_at_zero_on_time + (1.0 - t.demand_mult_at_zero_on_time) * frac
}

/// Day-close ops pass: compute per-route on-time% + avg delay, refresh the
/// lagged reliability -> demand multiplier, dispatch worn units to maintenance
/// (if a depot of that mode exists), age the fleet, and reset accumulators.
/// Mirrors `opsDailyClose`.
pub fn ops_daily_close(state: &mut GameState) {
    if state.fleet.is_none() {
        init_ops(state);
    }
    let t = ops_tunables(state.difficulty);
    let daily = state.ops_daily.take().unwrap_or_default();
    for r in state.routes.iter_mut() {
        match daily.get(&r.id) {
            Some(d) if d.departures > 0.0 => {
                let on_time = (1.0 - d.delayed_departures / d.departures).clamp(0.0, 1.0);
                r.on_time_pct = Some(on_time);
                r.avg_delay_sec = Some(d.delay_sec / d.departures);
            }
            _ => {
                // no service ran -> treat as fully on time.
                r.on_time_pct = Some(1.0);
                r.avg_delay_sec = Some(0.0);
            }
        }
        r.reliability_demand_mult = Some(reliability_demand_mult_for(
            r.on_time_pct.unwrap_or(1.0),
            &t,
        ));
    }
    // maintenance dispatch: worn units of a mode with a depot go out of service
    // to be restored (cutting the route's in-service count while active). No
    // depot for the mode -> deferred maintenance, condition keeps falling.
    // membership only (not a hashed iteration path) -> a Vec is fine and avoids
    // requiring Ord/Hash on the frozen TransitMode enum.
    let depot_modes: Vec<TransitMode> = state
        .depots
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|d| d.mode)
        .collect();
    if let Some(fleet) = state.fleet.as_mut() {
        for u in fleet.iter_mut() {
            if u.status != FleetStatus::Active {
                continue;
            }
            if u.condition < t.maintenance_condition_threshold && depot_modes.contains(&u.mode) {
                u.status = FleetStatus::Maintenance;
                u.status_ticks_left = t.maintenance_ticks;
            }
        }
        for u in fleet.iter_mut() {
            u.age_days += 1.0;
        }
    }
    state.ops_daily = Some(BTreeMap::new());
}

/// Apply the KEYSTONE reliability -> ridership feedback AFTER assignment. Scales
/// each route's ridership/revenue by its lagged reliability multiplier and
/// returns total transit trips shed to chronic unreliability (the caller moves
/// them to car so the mode-share stat stays consistent). Mirrors
/// `applyReliabilityDemand`.
pub fn apply_reliability_demand(
    state: &mut GameState,
    route_ridership: &BTreeMap<u32, f64>,
    route_revenue: &BTreeMap<u32, f64>,
) -> f64 {
    let mut transit_lost = 0.0;
    for r in state.routes.iter_mut() {
        let mult = r.reliability_demand_mult.unwrap_or(1.0);
        let raw_ridership = route_ridership.get(&r.id).copied().unwrap_or(0.0);
        let raw_revenue = route_revenue.get(&r.id).copied().unwrap_or(0.0);
        r.daily_ridership = raw_ridership * mult;
        r.daily_revenue = raw_revenue * mult;
        transit_lost += raw_ridership * (1.0 - mult);
    }
    transit_lost
}

/// Ridership-weighted reliability contribution to the daily approval target, in
/// `[-swing, +swing]`. Mirrors `opsApprovalDelta`.
pub fn ops_approval_delta(state: &GameState) -> f64 {
    let t = ops_tunables(state.difficulty);
    let mut riders = 0.0;
    let mut weighted = 0.0;
    for r in &state.routes {
        let w = r.daily_ridership;
        riders += w;
        weighted += w * r.on_time_pct.unwrap_or(1.0);
    }
    if riders <= 0.0 {
        return 0.0;
    }
    let mean_on_time = weighted / riders; // 0..1
    let centered = (mean_on_time - t.on_time_target) / (1.0 - t.on_time_target).max(1e-6);
    centered.clamp(-1.0, 1.0) * t.approval_reliability_swing
}

/// Daily ops opex: fleet maintenance per active unit + standing depot cost.
/// Mirrors `opsDailyOpex`.
pub fn ops_daily_opex(state: &GameState) -> f64 {
    let t = ops_tunables(state.difficulty);
    let units = state
        .fleet
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter(|u| u.status != FleetStatus::BrokenDown)
        .count() as f64;
    let depot_cost = state.depots.as_deref().unwrap_or(&[]).len() as f64 * t.depot_daily_cost;
    units * t.fleet_maintenance_per_unit_per_day + depot_cost
}

/// Fleet-wide summary for the UI (additive). Mirrors `fleetSummary`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FleetSummary {
    /// Total units.
    pub total: usize,
    /// Active units.
    pub active: usize,
    /// Units in maintenance.
    pub maintenance: usize,
    /// Broken-down units.
    pub broken_down: usize,
    /// Mean condition.
    pub avg_condition: f64,
    /// Mean age in days.
    pub avg_age_days: f64,
}

/// Compute the [`FleetSummary`]. Mirrors `fleetSummary`.
pub fn fleet_summary(state: &GameState) -> FleetSummary {
    let fleet = state.fleet.as_deref().unwrap_or(&[]);
    let mut active = 0;
    let mut maintenance = 0;
    let mut broken_down = 0;
    let mut cond = 0.0;
    let mut age = 0.0;
    for u in fleet {
        match u.status {
            FleetStatus::Active => active += 1,
            FleetStatus::Maintenance => maintenance += 1,
            FleetStatus::BrokenDown => broken_down += 1,
        }
        cond += u.condition;
        age += u.age_days;
    }
    let n = fleet.len().max(1) as f64;
    FleetSummary {
        total: fleet.len(),
        active,
        maintenance,
        broken_down,
        avg_condition: cond / n,
        avg_age_days: age / n,
    }
}

// ── route cycle time (ported for ops sizing) ──────────────────────────────────
//
// Ports `routeCycleSeconds` (commands.ts) + the grade/density helpers it needs
// from `transit/gradeEffects.ts` (segmentDensity01, segmentDayAverageSpeedMps,
// day-average surface slowdown). Kept ops-local so this lane does not depend on
// the parallel transit lane's port. If transit lands a shared
// `route_cycle_seconds`, the coordinator can swap this for it.

/// Map land value (~0..3) onto a `[0,1]` density weight. Mirrors
/// `density01FromLandValue`.
fn density01_from_land_value(lv: f64) -> f64 {
    (lv / 2.0).clamp(0.0, 1.0)
}

/// Density along a track segment (midpoint of its polyline). Mirrors
/// `segmentDensity01`.
fn segment_density01(fields: &FieldGrid, seg: &TrackSegment) -> f64 {
    let pts = &seg.polyline.points;
    if pts.is_empty() {
        return 0.5;
    }
    let mid = pts[pts.len() / 2];
    if fields.land_value.is_empty() {
        return 0.5;
    }
    density01_from_land_value(crate::fields::sample_field(fields, &fields.land_value, mid))
}

/// Day-average surface slowdown for cycle/headway. Mirrors
/// `dayAverageSurfaceSlowdown`.
fn day_average_surface_slowdown(mode: TransitMode, density01: f64) -> f64 {
    let dens = 0.35 + 0.65 * density01.clamp(0.0, 1.0);
    1.0 + *periods::MEAN_RUSH_EXCESS * surface_congestion_weight(mode) * dens
}

/// Day-average effective speed (m/s). Elevated/tunnel keep full mode cruise;
/// surface is divided by the congestion slowdown. Mirrors
/// `segmentDayAverageSpeedMps`.
fn segment_day_average_speed_mps(mode: TransitMode, grade: TrackGrade, density01: f64) -> f64 {
    let base = modes(mode).speed;
    if grade != TrackGrade::Surface {
        return base;
    }
    base / day_average_surface_slowdown(mode, density01)
}

/// Seconds for one vehicle to complete a full out-and-back cycle: travel time
/// plus a dwell at every stop it passes (each intermediate stop twice). Travel
/// uses day-average grade-aware segment speeds. Mirrors `routeCycleSeconds`.
pub fn route_cycle_seconds(tracks: &[TrackSegment], fields: &FieldGrid, route: &RouteDef) -> f64 {
    let cfg = modes(route.mode);
    let mut one_way = 0.0;
    for seg_id in &route.segment_ids {
        let Some(seg) = tracks.iter().find(|t| t.id == *seg_id) else {
            continue;
        };
        let dens = segment_density01(fields, seg);
        let spd = segment_day_average_speed_mps(route.mode, seg.grade, dens);
        if spd > 0.0 {
            one_way += seg.polyline.length / spd;
        }
    }
    if one_way <= 0.0 {
        return 0.0;
    }
    let travel = one_way * 2.0; // out-and-back
    let dwell_stops = 2 * route.station_ids.len().saturating_sub(1).max(1);
    travel + dwell_stops as f64 * cfg.dwell_seconds
}

// ── weather hooks (P3 lane C wires WeatherSnapshot fields) ─────────────────────
//
// The `WeatherSnapshot` type is an empty P3 placeholder in this build and
// `state.weather` is `None` on every current path, so weather is neutral here:
// intensity 0 and a breakdown ratio of 1.0 (matching `weatherBreakdownChance`
// returning the clear base when weather is absent). When lane C lands the real
// snapshot fields (`state`, `intensity`), these two helpers wire them.

/// Weather intensity `[0,1]` (0 when absent). See module note. Mirrors the
/// `state.weather?.intensity ?? 0` read in `decayCondition`.
fn weather_intensity(weather: &Option<WeatherSnapshot>) -> f64 {
    // TODO(P3 lane C): return weather.intensity once WeatherSnapshot carries it.
    let _ = weather;
    0.0
}

/// Ratio of this-tick breakdown chance to the clear-weather base (1.0 when
/// absent). Mirrors `weatherBreakdownChance(weather,'bus') / WEATHER_BREAKDOWN_CHANCE.clear`.
fn weather_breakdown_ratio(weather: &Option<WeatherSnapshot>) -> f64 {
    // TODO(P3 lane C): compute base[state] * (0.5 + 0.5*intensity) / clear once
    // WeatherSnapshot carries `state` + `intensity`.
    let _ = weather;
    1.0
}

#[cfg(test)]
mod tests;
