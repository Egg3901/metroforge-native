/**
 * Fixed-timestep simulation. 1 tick = 1 game-second. Everything here is
 * deterministic given (seed, command stream).
 */
import {
  ASSIGNMENT_INTERVAL_TICKS,
  BANKRUPTCY_FLOOR,
  BANKRUPTCY_GRACE_DAYS,
  BASE_DAILY_SUBSIDY,
  CROWD_APPROVAL_THRESHOLD,
  GRADE_MAINT_MULT,
  GROWTH_INTERVAL_DAYS,
  MODES,
  PEAK_HOUR_FRACTION,
  TICKS_PER_DAY,
} from './constants';
import { cellCenter } from './fields';
import { dist } from './geometry';
import { Rng } from './rng';
import { commitAnalyticsDay, captureAssignmentAnalytics, type HeatmapPayload } from './analytics';
import { runAssignment } from './transit/assignment';
import { computeTraffic } from './transit/traffic';
import { EVENT_DEFS, eventApprovalDelta, eventFareMult, rollEvent } from './events';
import { APPROVAL_GRACE_DAYS, modeUnlockReady } from './scenarioRules';
import { evaluateScenarioDay } from './scenario';
import { getRoutePath } from './transit/routePath';
import { routeOperatingCost } from './economy';
import { diurnalDemand, diurnalFactor } from './timeOfDay';
import { climateTable, weatherAt, type WeatherEvent } from './weather';
import { weatherSpeedMult } from './weatherEffects';
import { segmentDayAverageSpeedMps, segmentDensity01 } from './transit/gradeEffects';
import type { Vec2 } from './geometry';
import type { GameState, RouteDef, Station, TrackSegment } from './types';

/** Ticks per game-hour: weather is refreshed at most this often (it is a cheap
 *  pure function of the tick, so an hourly cadence keeps its cost negligible). */
const TICKS_PER_HOUR = TICKS_PER_DAY / 24;

/** Player copy for weather-event toasts. No em/en dashes, no filler. */
const WEATHER_EVENT_COPY: Record<WeatherEvent, { start: string; end: string; tone: 'warn' | 'info' }> = {
  blizzard: {
    start: 'Blizzard warning. Surface lines are crawling, but the underground keeps moving.',
    end: 'The blizzard has passed. Surface service is recovering.',
    tone: 'warn',
  },
  heatwave: {
    start: 'Heat wave. Riders are staying home and rail speeds are restricted.',
    end: 'The heat wave has broken. Rail speed limits are lifted.',
    tone: 'warn',
  },
};

/** Refresh the cached sky (pure fn of seed+tick+city) and emit begin/end toasts
 *  when a headline weather event starts or clears. */
function updateWeather(state: GameState, events: TickEvents): void {
  const table = climateTable(state.cityKey);
  const next = weatherAt(state.seed, state.tick, table);
  const prevEvent = state.lastWeatherEvent ?? null;
  const nextEvent = next.event ?? null;
  state.weather = next;
  if (nextEvent !== prevEvent) {
    const toasts = events.toasts ?? (events.toasts = []);
    if (prevEvent) toasts.push({ message: WEATHER_EVENT_COPY[prevEvent].end, tone: 'info' });
    if (nextEvent) {
      const c = WEATHER_EVENT_COPY[nextEvent];
      toasts.push({ message: c.start, tone: c.tone });
    }
    state.lastWeatherEvent = nextEvent;
  }
}

/**
 * Coarse spatial bucket over stations, sized so a 3×3 neighborhood covers the
 * largest coverage/growth query radius (max walkRadius × 1.5 = 1500 m). Replaces
 * the O(cells × stations) double loops in coverage + growth with O(cells) using
 * only nearby stations. Deterministic: `candidates` returns ascending station
 * indices so the growth `access` sum keeps the exact same float addition order
 * as the original full-array scan (stations outside the radius contributed 0).
 */
class StationGrid {
  private readonly cell = 1500;
  private readonly map = new Map<number, number[]>();
  constructor(stations: Station[]) {
    for (let i = 0; i < stations.length; i++) {
      const s = stations[i]!;
      const k = this.key(s.pos.x, s.pos.y);
      const arr = this.map.get(k);
      if (arr) arr.push(i);
      else this.map.set(k, [i]);
    }
  }
  private key(x: number, y: number): number {
    return Math.floor(x / this.cell) * 73856093 + Math.floor(y / this.cell) * 19349663;
  }
  /** ascending station indices in the 3×3 neighborhood of p (superset of those within 1500 m). */
  candidates(p: Vec2): number[] {
    const cx = Math.floor(p.x / this.cell);
    const cy = Math.floor(p.y / this.cell);
    const out: number[] = [];
    for (let oy = -1; oy <= 1; oy++) {
      for (let ox = -1; ox <= 1; ox++) {
        const arr = this.map.get((cx + ox) * 73856093 + (cy + oy) * 19349663);
        if (arr) for (const i of arr) out.push(i);
      }
    }
    out.sort((a, b) => a - b);
    return out;
  }
  /** true if any station in the neighborhood satisfies dist(p, station) <= radiusFor(station). */
  anyWithin(p: Vec2, stations: Station[], radiusFor: (s: Station) => number): boolean {
    const cx = Math.floor(p.x / this.cell);
    const cy = Math.floor(p.y / this.cell);
    for (let oy = -1; oy <= 1; oy++) {
      for (let ox = -1; ox <= 1; ox++) {
        const arr = this.map.get((cx + ox) * 73856093 + (cy + oy) * 19349663);
        if (!arr) continue;
        for (const i of arr) {
          const s = stations[i]!;
          if (dist(p, s.pos) <= radiusFor(s)) return true;
        }
      }
    }
    return false;
  }
}

export interface TickEvents {
  dayCompleted?: number;
  bankrupt?: boolean;
  /** approval-floor, time-limit, or scenario-condition failure */
  failed?: 'approval' | 'time' | 'condition';
  /** scenario win tree satisfied */
  won?: boolean;
  modeUnlocked?: string;
  messages: string[];
  /** themed toasts (city events) with a tone */
  toasts?: { message: string; tone: 'good' | 'warn' | 'info' }[];
  /** optional ridership heatmap (every HEATMAP_EMIT_INTERVAL_DAYS); clients may ignore */
  heatmap?: HeatmapPayload;
}

/**
 * @deprecated bankruptDays is now instance-scoped state on `GameState`
 * (`state.bankruptDays`), removing a module-level global that leaked across
 * games in a warm process — the same bug class as the #23 warm-process fix.
 * These shims remain only so existing callers/tests keep compiling; they are
 * no-ops (state.bankruptDays is authoritative and starts at 0 for a new game).
 */
export function getBankruptDays(): number {
  return 0;
}
export function setBankruptDays(_d: number): void {
  /* no-op: bankruptDays lives on GameState now */
}

export function simTick(state: GameState): TickEvents {
  const events: TickEvents = { messages: [] };
  if (state.failed || state.scenarioWon) return events;
  state.tick += 1;

  // refresh the sky once per game-hour (and on the very first tick)
  if (state.tick % TICKS_PER_HOUR === 0 || !state.weather) updateWeather(state, events);

  moveVehicles(state);

  // demand assignment: on dirty flag or periodic refresh
  if (state.demandDirty || state.tick % ASSIGNMENT_INTERVAL_TICKS === 0) {
    refreshAssignment(state);
    state.demandDirty = false;
  }

  if (state.tick % TICKS_PER_DAY === 0) {
    const day = state.tick / TICKS_PER_DAY;
    events.dayCompleted = day;
    updateEvents(state, day, events);
    runDailyEconomy(state, day, events);
    updateApproval(state);
    checkUnlocks(state, events);
    if (day % GROWTH_INTERVAL_DAYS === 0) runGrowth(state);
    // analytics day-close: rolling heatmap/OD + optional quantized payload
    const ar = commitAnalyticsDay(state, day);
    if (ar.emitHeatmap && ar.payload) events.heatmap = ar.payload;
    if (state.scenario) {
      const sr = evaluateScenarioDay(state, state.scenario, day);
      events.messages.push(...sr.messages);
      if (sr.toasts.length) {
        const toasts = events.toasts ?? (events.toasts = []);
        toasts.push(...sr.toasts);
      }
      if (sr.won) events.won = true;
      if (sr.lostCondition) events.failed = 'condition';
    }
    checkFailure(state, day, events);
  }

  return events;
}

function checkFailure(state: GameState, day: number, events: TickEvents): void {
  const rules = state.scenarioRules;
  if (state.budget.cash < BANKRUPTCY_FLOOR) {
    state.bankruptDays += 1;
    if (state.bankruptDays >= BANKRUPTCY_GRACE_DAYS) {
      state.failed = 'bankrupt';
      events.bankrupt = true;
    } else {
      events.messages.push(`Deep in the red: ${BANKRUPTCY_GRACE_DAYS - state.bankruptDays} days until the city takes over`);
    }
  } else {
    state.bankruptDays = 0;
  }
  if (state.failed) return;

  if (rules?.approvalFloor !== undefined) {
    if (state.stats.approval <= rules.approvalFloor) {
      state.lowApprovalDays += 1;
      if (state.lowApprovalDays >= APPROVAL_GRACE_DAYS) {
        state.failed = 'approval';
        events.failed = 'approval';
        events.messages.push('Approval collapsed — the board has fired you');
      } else {
        events.messages.push(
          `Approval critical (${Math.round(state.stats.approval)}%): ${APPROVAL_GRACE_DAYS - state.lowApprovalDays} days to turn it around`,
        );
      }
    } else {
      state.lowApprovalDays = 0;
    }
  }
  if (state.failed) return;

  // scenario win short-circuits the calendar deadline
  if (state.scenarioWon) return;

  if (rules?.maxDay !== undefined && day > rules.maxDay) {
    state.failed = 'time';
    events.failed = 'time';
    events.messages.push(`Time is up — day ${rules.maxDay} has passed without meeting the objective`);
  }
}

function moveVehicles(state: GameState): void {
  // one time-of-day factor per tick, shared by every vehicle's occupancy.
  const tod = diurnalFactor(state.tick);
  // Per-tick id indexes: turns the per-vehicle O(routes)/O(stations) Array.find
  // hash lookups into O(1) Map lookups (agents.ts:46-47 pattern), rebuilt each
  // tick so mutations can never leave a stale index. Same values, same order.
  const routeById = new Map<number, RouteDef>();
  for (const r of state.routes) routeById.set(r.id, r);
  const stationById = new Map<number, Station>();
  for (const s of state.stations) stationById.set(s.id, s);
  // memoize stop distances per route within the tick: every vehicle on a route
  // shares the same (route, path) stop list, so compute it once.
  const stopMemo = new Map<number, number[]>();
  for (const v of state.vehicles) {
    const route = routeById.get(v.routeId);
    if (!route) continue;
    const path = getRoutePath(state, route);
    if (!path) continue;
    v.pathLength = path.length;
    if (v.dwellRemaining > 0) {
      v.dwellRemaining -= 1;
      // still refresh occupancy while dwelling so bars stay live
      v.occupancy = occupancyAt(route, v.along, path.length, tod);
      continue;
    }
    const cfg = MODES[route.mode];
    let stops = stopMemo.get(route.id);
    if (!stops) {
      stops = allStopDistances(path, route, stationById, state.instanceId);
      stopMemo.set(route.id, stops);
    }
    // Advance segment-by-segment toward the next stop so we never overshoot a
    // dwell point on long ticks / high speeds. Two orthogonal speed factors
    // compose MULTIPLICATIVELY here (documented in gradeEffects.ts):
    //  1. grade congestion — surface running is slower in dense corridors; the
    //     route's day-average grade speed is cached each assignment
    //     (route.moveGradeSpeed), so this is a single read (elevated/tunnel keep
    //     mode cruise). The diurnal sharpness lives in assignment ridership.
    //  2. weather — rain/snow/blizzard slow surface running, scaled by
    //     surfaceExposure so a fully-underground line shrugs it off.
    const gradeSpeed = route.moveGradeSpeed ?? cfg.speed;
    const weatherMult = weatherSpeedMult(state.weather, route.mode, route.surfaceExposure ?? 1);
    let remaining = gradeSpeed * weatherMult;
    let guard = 0;
    while (remaining > 1e-6 && guard++ < 8) {
      const nextStop = nextStopAhead(stops, v.along, path.length);
      const gap =
        nextStop === null
          ? remaining
          : nextStop >= v.along
            ? nextStop - v.along
            : path.length - v.along + nextStop;
      if (nextStop !== null && gap <= remaining + 1e-6) {
        v.along = nextStop % path.length;
        v.dwellRemaining = cfg.dwellSeconds;
        remaining = 0;
        break;
      }
      const step = Math.min(remaining, gap);
      v.along = (v.along + step) % path.length;
      remaining -= step;
    }
    v.occupancy = occupancyAt(route, v.along, path.length, tod);
  }
}

/** Per-vehicle load from the segment the vehicle is currently on (falls back to route crowding). */
function occupancyAt(
  route: { crowding: number; segmentLoads: number[]; capacity: number; stationIds: number[]; vehicleCount: number },
  along: number,
  pathLen: number,
  todFactor: number,
): number {
  if (route.vehicleCount <= 0) return 0;
  const segs = route.segmentLoads;
  const n = Math.max(1, route.stationIds.length - 1);
  if (!segs.length || pathLen <= 0 || route.capacity <= 0) {
    return Math.min(1.5, (route.crowding || 0) * todFactor);
  }
  // Out-and-back: first half outbound segments, second half reverse.
  const half = pathLen / 2;
  let segIdx: number;
  if (along <= half) {
    segIdx = Math.min(n - 1, Math.floor((along / half) * n));
  } else {
    const t = (along - half) / half;
    segIdx = Math.min(n - 1, n - 1 - Math.floor(t * n));
  }
  const load = segs[segIdx] ?? 0;
  // segmentLoads are daily link trips; convert roughly to peak load / capacity,
  // then scale by the live time-of-day factor so a vehicle at rush hour rides
  // full while the same vehicle overnight rides near-empty.
  const peak = load * 0.14;
  return Math.min(1.5, (peak / route.capacity) * todFactor);
}

function allStopDistances(
  path: { points: { x: number; y: number }[]; cumulative: number[]; length: number },
  route: { stationIds: number[] },
  stationById: Map<number, Station>,
  instanceId: number,
): number[] {
  const out: number[] = [];
  for (const sid of route.stationIds) {
    const s = stationById.get(sid);
    if (!s) continue;
    for (const d of nearestAlong(path, s, instanceId)) out.push(d);
  }
  out.sort((a, b) => a - b);
  // de-dupe near-identical stops (out-and-back joints)
  const uniq: number[] = [];
  for (const d of out) {
    if (uniq.length === 0 || Math.abs(d - (uniq[uniq.length - 1] as number)) > 5) uniq.push(d);
  }
  return uniq;
}

function nextStopAhead(stops: number[], along: number, pathLen: number): number | null {
  if (stops.length === 0 || pathLen <= 0) return null;
  for (const d of stops) {
    if (d > along + 0.5) return d;
  }
  // wrap to first stop on the loop
  return stops[0] ?? null;
}

/** Distances along an out-and-back path where the path passes near a station. */
const stopDistCache = new Map<string, number[]>();
function nearestAlong(path: { points: { x: number; y: number }[]; cumulative: number[]; length: number }, s: Station, instanceId: number): number[] {
  // key is scoped to the game instance: entity ids reset per newGame, so a bare
  // `id:length` key would collide across games sharing this process (replay/sidecar).
  const key = `${instanceId}:${s.id}:${path.length.toFixed(1)}`;
  const hit = stopDistCache.get(key);
  if (hit) return hit;
  const out: number[] = [];
  for (let i = 0; i < path.points.length; i++) {
    const p = path.points[i]!;
    if (dist(p, s.pos) < 30) out.push(path.cumulative[i] as number);
  }
  stopDistCache.set(key, out);
  return out;
}

/** Re-exported so existing importers of `@core/sim`'s diurnal curve keep
 *  working; the implementation now lives in `./timeOfDay`. */
export { diurnalDemand };

export function refreshAssignment(state: GameState): void {
  const result = runAssignment(state);
  state.flows = result.flows;
  state.stats.dailyTransitTrips = result.dailyTransitTrips;
  state.stats.dailyCarTrips = result.dailyCarTrips;
  const total = result.dailyTransitTrips + result.dailyCarTrips;
  state.stats.transitShare = total > 0 ? result.dailyTransitTrips / total : 0;
  // per-route surface exposure (fraction of segments not in tunnel), for the
  // weather speed model: underground routes shrug off snow/blizzards.
  const gradeById = new Map<number, string>();
  const trackById = new Map<number, TrackSegment>();
  for (const t of state.tracks) {
    gradeById.set(t.id, t.grade);
    trackById.set(t.id, t);
    // refresh the grade-congestion density cache (growth shifts land value)
    if (t.grade === 'surface') t.congestionDensity = segmentDensity01(state.fields, t);
  }
  for (const r of state.routes) {
    r.dailyRidership = result.routeRidership.get(r.id) ?? 0;
    r.dailyRevenue = result.routeRevenue.get(r.id) ?? 0;
    // peak-hour capacity vs load → crowding (feeds next assignment's penalty)
    const cfg = MODES[r.mode];
    r.capacity = r.vehicleCount > 0 ? (cfg.vehicleCapacity * 3600) / r.headwaySeconds : 0;
    r.load = r.dailyRidership * PEAK_HOUR_FRACTION;
    r.crowding = r.capacity > 0 ? r.load / r.capacity : r.load > 0 ? 2 : 0;
    // per-segment load, aligned to segmentIds (segment i joins stop i and i+1)
    r.segmentLoads = r.segmentIds.map((_, i) => {
      const a = r.stationIds[i] as number;
      const b = r.stationIds[i + 1] as number;
      return result.segmentLoad.get(`${r.id}:${Math.min(a, b)}:${Math.max(a, b)}`) ?? 0;
    });
    if (r.segmentIds.length > 0) {
      let exposed = 0;
      for (const sid of r.segmentIds) if (gradeById.get(sid) !== 'tunnel') exposed += 1;
      r.surfaceExposure = exposed / r.segmentIds.length;
    } else {
      r.surfaceExposure = 1;
    }
    // grade-congestion movement speed: length-weighted DAY-AVERAGE grade speed
    // over the route's segments, cached so the per-tick vehicle loop is a single
    // property read. Day-average matches the headway model; the rush sharpness
    // of the tradeoff lives in the peak-biased assignment ride edges.
    let totalLen = 0;
    let speedLen = 0;
    for (const sid of r.segmentIds) {
      const seg = trackById.get(sid);
      if (!seg) continue;
      const len = seg.polyline.length;
      if (len <= 0) continue;
      const dens = seg.grade === 'surface' ? seg.congestionDensity ?? segmentDensity01(state.fields, seg) : 0;
      speedLen += segmentDayAverageSpeedMps(r.mode, seg.grade, dens) * len;
      totalLen += len;
    }
    r.moveGradeSpeed = totalLen > 0 ? speedLen / totalLen : MODES[r.mode].speed;
  }
  for (const s of state.stations) {
    // rolling blend so numbers move smoothly
    const target = result.stationBoardings.get(s.id) ?? 0;
    s.ridership = s.ridership * 0.5 + target * 0.5;
    const alight = result.stationAlightings.get(s.id) ?? 0;
    s.alightings = (s.alightings ?? 0) * 0.5 + alight * 0.5;
  }
  state.unserved = result.unserved;
  captureAssignmentAnalytics(
    state,
    result.stationBoardings,
    result.stationAlightings,
    result.flows,
    result.carFlows,
  );
  // coverage: fraction of population within walk radius of any station
  let covered = 0;
  let totalPop = 0;
  const g = state.fields;
  const covGrid = new StationGrid(state.stations);
  for (let i = 0; i < g.population.length; i++) {
    const pop = g.population[i] as number;
    if (pop <= 0) continue;
    totalPop += pop;
    const c = cellCenter(g, i);
    if (covGrid.anyWithin(c, state.stations, (s) => MODES[s.mode].walkRadius)) covered += pop;
  }
  state.stats.coverage = totalPop > 0 ? covered / totalPop : 0;

  // congestion overlay: scaled by a diurnal demand curve so traffic surges at
  // the AM/PM rush and eases overnight
  state.traffic = computeTraffic(state, result.carFlows, diurnalDemand(state.tick));
}

function runDailyEconomy(state: GameState, _day: number, events: TickEvents): void {
  const b = state.budget;
  let fares = 0;
  let operations = 0;
  let maintenance = 0;
  for (const r of state.routes) {
    fares += r.dailyRevenue;
    operations += routeOperatingCost(r.mode, r.vehicleCount);
  }
  fares *= eventFareMult(state.activeEvents); // fare-free events waive the farebox
  for (const t of state.tracks) {
    maintenance += (t.polyline.length / 1000) * MODES[t.mode].maintPerKmPerDay * GRADE_MAINT_MULT[t.grade];
  }
  for (const s of state.stations) {
    maintenance += MODES[s.mode].stationCost * 0.0002 * s.level;
  }
  // subsidy: base scaled by approval (0.5×..1.5×), declining 2%/year
  const year = Math.floor(state.tick / TICKS_PER_DAY / 365);
  const baseSub = state.scenarioRules?.dailySubsidy ?? BASE_DAILY_SUBSIDY[state.difficulty];
  const base = baseSub * Math.pow(0.98, year);
  const subsidy = base * (0.5 + state.stats.approval / 100);
  const interest = (b.loanBalance * b.loanRate) / 365;

  b.cash += fares + subsidy - operations - maintenance - interest;
  b.lastDay = { fares, subsidy, operations, maintenance, interest };
  const net = fares + subsidy - operations - maintenance - interest;
  if (!b.netHistory) b.netHistory = [];
  b.netHistory.push(net);
  if (b.netHistory.length > 7) b.netHistory.shift();

  // cumulative lifetime ledger (optional; drives the economy summary UI). Built
  // forward from the first day that closes; legacy saves start it fresh here.
  const life = b.lifetime ?? (b.lifetime = { fares: 0, subsidy: 0, operations: 0, maintenance: 0, interest: 0, days: 0 });
  life.fares += fares;
  life.subsidy += subsidy;
  life.operations += operations;
  life.maintenance += maintenance;
  life.interest += interest;
  life.days += 1;

  if (fares > 0 && fares > operations + maintenance) {
    events.messages.push('Farebox recovery above 100% — the network pays for itself');
  }
}

function updateApproval(state: GameState): void {
  const s = state.stats;
  // ridership-weighted overcrowding drag: packed lines annoy the riders who use
  // them most, so the hit scales with how many people ride an over-capacity line
  let crowdRiders = 0;
  let totalRiders = 0;
  for (const r of state.routes) {
    totalRiders += r.dailyRidership;
    if (r.crowding > CROWD_APPROVAL_THRESHOLD) crowdRiders += r.dailyRidership * (r.crowding - CROWD_APPROVAL_THRESHOLD);
  }
  const crowdDrag = totalRiders > 0 ? Math.min(20, (crowdRiders / totalRiders) * 40) : 0;
  // drift toward a target driven by coverage + transit share, plus event mood
  const target = Math.min(
    100,
    Math.max(0, 25 + s.coverage * 90 + s.transitShare * 60 + eventApprovalDelta(state.activeEvents) * 2 - crowdDrag),
  );
  s.approval += (target - s.approval) * 0.08;
  s.approval = Math.max(0, Math.min(100, s.approval));
}

/** Tick down active city events and occasionally start a new one (seeded). */
function updateEvents(state: GameState, day: number, events: TickEvents): void {
  const toasts = events.toasts ?? (events.toasts = []);
  const still: GameState['activeEvents'] = [];
  for (const a of state.activeEvents) {
    a.daysLeft -= 1;
    if (a.daysLeft > 0) still.push(a);
    else {
      const d = EVENT_DEFS.find((e) => e.id === a.id);
      if (d) toasts.push({ message: `${d.name} has ended.`, tone: 'info' });
    }
  }
  state.activeEvents = still;
  // one event at a time, spaced out by a cooldown, so each feels like an occasion
  const rng = new Rng(state.rngState);
  if (state.activeEvents.length === 0 && day >= state.nextEventDay && rng.chance(0.2)) {
    const def = rollEvent(rng.next());
    state.activeEvents.push({ id: def.id, daysLeft: def.days });
    state.demandDirty = true; // reflect the demand change on the next assignment
    toasts.push({ message: `${def.name} — ${def.desc}`, tone: def.tone });
    state.nextEventDay = day + def.days + 12 + rng.int(0, 10); // ~12–22 day gap after it ends
  }
  state.rngState = rng.state();
}

function checkUnlocks(state: GameState, events: TickEvents): void {
  // era / challenge scenarios can freeze the toolkit to startingModes
  if (state.scenarioRules?.lockModes) return;
  for (const mode of ['tram', 'metro', 'rail'] as const) {
    if (state.unlockedModes.includes(mode)) continue;
    if (!modeUnlockReady(mode, state.stats)) continue;
    state.unlockedModes.push(mode);
    events.modeUnlocked = MODES[mode].label;
    events.messages.push(`${MODES[mode].label} unlocked — your network earned it`);
  }
}

/** Weekly growth pass: transit access densifies nearby cells; neglect decays. */
function runGrowth(state: GameState): void {
  const g = state.fields;
  const rng = new Rng(state.rngState);
  const growthGrid = new StationGrid(state.stations);
  let totalPop = 0;
  for (let i = 0; i < g.population.length; i++) {
    const pop = g.population[i] as number;
    if ((g.water[i] as number) === 1) continue;
    const c = cellCenter(g, i);
    let access = 0;
    // ascending station indices preserve the original full-scan summation order,
    // so the compounding growth is bit-identical to the O(cells × stations) loop.
    for (const si of growthGrid.candidates(c)) {
      const s = state.stations[si]!;
      const d = dist(c, s.pos);
      const walkR = MODES[s.mode].walkRadius;
      if (d < walkR * 1.5) access += (s.level * Math.min(1, walkR / Math.max(d, 50))) * (1 + s.ridership / 5000);
    }
    if (access > 0.5 && pop > 5) {
      const growth = Math.min(0.03, 0.004 * access) * (0.8 + rng.next() * 0.4);
      g.population[i] = pop * (1 + growth);
      g.landValue[i] = Math.min(3, (g.landValue[i] as number) * (1 + growth * 0.5));
      g.jobs[i] = (g.jobs[i] as number) * (1 + growth * 0.6);
    } else if (access === 0 && pop > 5) {
      g.population[i] = pop * 0.9995;
    }
    totalPop += g.population[i] as number;
  }
  state.rngState = rng.state();

  // refresh district aggregates
  for (const d of state.districts) {
    let pop = 0;
    let jobs = 0;
    for (const i of d.cellIndices) {
      pop += g.population[i] as number;
      jobs += g.jobs[i] as number;
    }
    d.population = pop;
    d.jobs = jobs;
  }
  state.stats.population = totalPop;
  state.stats.jobs = state.districts.reduce((a, d) => a + d.jobs, 0);
  state.demandDirty = true;
}
