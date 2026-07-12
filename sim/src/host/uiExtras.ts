/**
 * Optional, additive UiState/UiRoute enrichments. Kept in one shared pure module
 * so both hosts (the native sidecar and the web sim.worker) emit byte-identical
 * extra fields, and so the fields can be unit-tested without standing up a host.
 *
 * Everything here is derived, read-only, and OPTIONAL on the wire: a client that
 * does not know these fields simply ignores them (they ride in the existing JSON
 * `ui` envelope), so adding them does not bump the protocol version.
 */
import { fareboxRecovery, routeOperatingCost } from '@core/economy';
import {
  buildScenarioState,
  SCENARIO_PROGRESSION,
  type ScenarioProgressionManifest,
  type ScenarioState,
} from '@core/scenario';
import { diurnalFactor, hourOfDay } from '@core/timeOfDay';
import type { AnalyticsInsights } from '@core/analytics';
import type { GameState, LifetimeLedger, RouteDef } from '@core/types';

export interface UiRouteExtras {
  /** daily fleet running cost (operations + maintenance) for this route */
  operatingCost: number;
  /** dailyRevenue / operatingCost; >1 means the line covers its own running cost */
  farebox: number;
  /** crowding scaled by the current time-of-day factor: how full the line is
   *  right now (peaks at the AM/PM rush), vs. the daily-average `crowding`. */
  liveCrowding: number;
}

export function routeExtras(r: RouteDef, todFactor: number): UiRouteExtras {
  const operatingCost = routeOperatingCost(r.mode, r.vehicleCount);
  const crowding = r.crowding ?? 0;
  return {
    operatingCost,
    farebox: operatingCost > 0 ? r.dailyRevenue / operatingCost : 0,
    liveCrowding: crowding * todFactor,
  };
}

export interface UiDistrict {
  id: number;
  name: string;
  x: number;
  y: number;
  population: number;
  jobs: number;
}

export interface UiStateExtras {
  /** hour of the game day in [0,24) */
  hourOfDay: number;
  /** live time-of-day demand multiplier (daily mean = 1.0) */
  demandFactor: number;
  /** farebox recovery for yesterday's ledger */
  fareboxRecovery: number;
  /** per-district catchment population + jobs (building-derived), world coords */
  districts: UiDistrict[];
  /** count of routes over capacity (crowding > 1) */
  overcrowdedRoutes: number;
  /** cumulative lifetime ledger, if the run has closed at least one day */
  lifetime?: LifetimeLedger;
  /** data-driven scenario progress; omitted when no scenario is active */
  scenarioState?: ScenarioState;
  /**
   * Full scenario progression graph (completing X unlocks Y). Additive —
   * always emitted so pickers / native clients can gate content without a
   * separate catalog fetch. Older clients ignore the field.
   */
  scenarioProgression?: ScenarioProgressionManifest;
  /** spatial analytics insights; omitted until the first analytics day closes */
  analytics?: AnalyticsInsights;
}

/** The current time-of-day factor for `s`; hand to `routeExtras` per route so
 *  every route shares one factor value per UI frame. */
export function todFactorOf(s: GameState): number {
  return diurnalFactor(s.tick);
}

export function uiExtras(s: GameState): UiStateExtras {
  const extras: UiStateExtras = {
    hourOfDay: hourOfDay(s.tick),
    demandFactor: diurnalFactor(s.tick),
    fareboxRecovery: fareboxRecovery(s.budget.lastDay),
    districts: s.districts.map((d) => ({
      id: d.id,
      name: d.name,
      x: d.centroid.x,
      y: d.centroid.y,
      population: d.population,
      jobs: d.jobs,
    })),
    overcrowdedRoutes: s.routes.filter((r) => (r.crowding ?? 0) > 1).length,
    scenarioProgression: SCENARIO_PROGRESSION,
  };
  if (s.budget.lifetime) extras.lifetime = s.budget.lifetime;
  if (s.scenario) extras.scenarioState = buildScenarioState(s.scenario, s);
  if (s.analytics) extras.analytics = s.analytics.insights;
  return extras;
}
