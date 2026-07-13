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
import {
  cohortDemandFactor,
  cohortMix,
  hourBucket,
  hourlyDemandCurve,
  isWeekend,
  type CohortKind,
} from '@core/transit/cohorts';
import { segmentDensity01, segmentEffectiveSpeedMps } from '@core/transit/gradeEffects';
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
  /** length-weighted average grade-effective speed (m/s) at the current tick —
   *  surface lines drop under traffic (grade congestion); elevated/tunnel stay
   *  at mode cruise. 0 when no state/segments are available. */
  avgEffectiveSpeed: number;
}

/** Length-weighted mean of per-segment grade-effective speeds at `todFactor`. */
export function routeAvgEffectiveSpeed(state: GameState, r: RouteDef, todFactor: number): number {
  let lenSum = 0;
  let speedLen = 0;
  for (const segId of r.segmentIds) {
    const seg = state.tracks.find((t) => t.id === segId);
    if (!seg) continue;
    const len = seg.polyline.length;
    if (len <= 0) continue;
    const dens = segmentDensity01(state.fields, seg);
    const spd = segmentEffectiveSpeedMps(r.mode, seg.grade, todFactor, dens);
    lenSum += len;
    speedLen += spd * len;
  }
  return lenSum > 0 ? speedLen / lenSum : 0;
}

export function routeExtras(r: RouteDef, todFactor: number, state?: GameState): UiRouteExtras {
  const operatingCost = routeOperatingCost(r.mode, r.vehicleCount);
  const crowding = r.crowding ?? 0;
  return {
    operatingCost,
    farebox: operatingCost > 0 ? r.dailyRevenue / operatingCost : 0,
    liveCrowding: crowding * todFactor,
    avgEffectiveSpeed: state ? routeAvgEffectiveSpeed(state, r, todFactor) : 0,
  };
}

export interface UiDistrict {
  id: number;
  name: string;
  x: number;
  y: number;
  population: number;
  jobs: number;
  /** v0.9 zone response: fractional population change at the last growth period
   *  (>0 thickening near good transit, <0 shrinking). Additive; older clients
   *  ignore it. Absent until the first growth period has run. */
  growthDelta?: number;
}

/** v0.9 cohort demand-by-hour summary for the HUD (schedule-driven demand). All
 *  values are deterministic pure functions of the sim tick. Additive on the
 *  wire — older clients ignore it. */
export interface UiCohortDemand {
  /** current integer hour bucket [0,24) */
  hour: number;
  /** live total demand factor at this hour (daily mean = 1.0) */
  factor: number;
  /** relative per-cohort demand weight right now (commuter/student/…); not
   *  normalized — the HUD normalizes for a stacked bar. */
  mix: Record<CohortKind, number>;
  /** the full 24-entry normalized hourly demand curve for the current day type */
  curve: number[];
  /** true on weekend game-days (curve tilts to leisure) */
  weekend: boolean;
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
  /** v0.9 cohort demand-by-hour summary (schedule-driven demand shape) */
  cohortDemand: UiCohortDemand;
  /** current weather state (clear|overcast|rain|fog|snow|storm) */
  weatherState?: string;
  /** weather intensity 0..1 (rainfall/snowfall strength; heat for clear) */
  weatherIntensity?: number;
  /** season derived from the sim date (winter|spring|summer|autumn) */
  weatherSeason?: string;
  /** headline weather event, when active (blizzard|heatwave) */
  weatherEvent?: string;
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
    districts: s.districts.map((d) => {
      const ud: UiDistrict = {
        id: d.id,
        name: d.name,
        x: d.centroid.x,
        y: d.centroid.y,
        population: d.population,
        jobs: d.jobs,
      };
      if (d.lastGrowthDelta !== undefined) ud.growthDelta = d.lastGrowthDelta;
      return ud;
    }),
    overcrowdedRoutes: s.routes.filter((r) => (r.crowding ?? 0) > 1).length,
    cohortDemand: {
      hour: hourBucket(s.tick),
      factor: cohortDemandFactor(s.tick),
      mix: cohortMix(s.tick),
      curve: hourlyDemandCurve(isWeekend(s.tick)),
      weekend: isWeekend(s.tick),
    },
    scenarioProgression: SCENARIO_PROGRESSION,
  };
  if (s.weather) {
    extras.weatherState = s.weather.state;
    extras.weatherIntensity = s.weather.intensity;
    extras.weatherSeason = s.weather.season;
    if (s.weather.event) extras.weatherEvent = s.weather.event;
  }
  if (s.budget.lifetime) extras.lifetime = s.budget.lifetime;
  if (s.scenario) extras.scenarioState = buildScenarioState(s.scenario, s);
  if (s.analytics) extras.analytics = s.analytics.insights;
  return extras;
}
