/**
 * The ONLY mutation API for the sim. UI, events, tutorial, and replays all
 * call applyCommand. Validation lives here so every client (web, future
 * native) enforces identical rules.
 */
import { MAX_HEADWAY, MODES, REFUND_FRACTION, ROUTE_COLORS, WATER_CROSSING_MULT, WORLD_SIZE } from './constants';
import { isWaterAt } from './fields';
import { findRoadPath, nearestRoadPoint } from './transit/roadGraph';
import { dist, makePolyline } from './geometry';
import { segmentDayAverageSpeedMps, segmentDensity01 } from './transit/gradeEffects';
import { weatherBuildCostMult } from './weatherEffects';
import { columnAt } from './geology';
import { DEFAULT_TUNNEL_DEPTH, RIVER_TUNNEL_DEPTH, stationDepthSurcharge, undergroundSegmentCost } from './geologyCost';
import type { Vec2 } from './geometry';
import type { Command, CommandResult, GameState, TrackCostBreakdown, TrackSegment, TransitMode } from './types';

const STATION_NAMES = [
  'Central', 'Riverside', 'Oakwood', 'Hillcrest', 'Harborview', 'Elmgate', 'Northfield',
  'Southbank', 'Westbrook', 'Eastvale', 'Maplewood', 'Kingsway', 'Queensport', 'Foxhall',
  'Ironbridge', 'Silverlake', 'Granton', 'Ashford', 'Birchmount', 'Cedarholm', 'Drayton',
  'Everly', 'Fairmont', 'Glenrose', 'Halston', 'Inverness', 'Juniper', 'Kestrel',
  'Larkspur', 'Milbourne', 'Norcross', 'Ottervale', 'Pinegate', 'Quarry', 'Redwing',
  'Stonebridge', 'Thornbury', 'Uplands', 'Vantage', 'Wexford', 'Yarrow', 'Zephyr',
];

function nextStationName(state: GameState): string {
  const used = new Set(state.stations.map((s) => s.name));
  for (const n of STATION_NAMES) if (!used.has(n)) return n;
  return `Station ${state.stations.length + 1}`;
}

/** Default tunnel depth (m) at a world point: deeper under water (dips below
 *  the river/bay bed), the land default otherwise. */
export function tunnelDepthAt(state: GameState, p: Vec2): number {
  return isWaterAt(state.fields, p) ? RIVER_TUNNEL_DEPTH : DEFAULT_TUNNEL_DEPTH;
}

/** Cost of a track polyline given mode/grade — plus a full v0.8 breakdown. For
 *  surface/elevated the geology model is inert (the historic per-metre × water
 *  crossing × weather formula is preserved bit-for-bit); tunnels are priced
 *  through the strata model in geologyCost.ts. */
export function trackCostDetailed(
  state: GameState,
  mode: TransitMode,
  grade: 'surface' | 'elevated' | 'tunnel',
  points: Vec2[],
): { cost: number; breakdown: TrackCostBreakdown } {
  const cfg = MODES[mode];
  const surfacePerM = cfg.trackCostPerMeter * cfg.gradeCostMult.surface;
  const elevatedPerM = cfg.trackCostPerMeter * cfg.gradeCostMult.elevated;
  const perMeter = cfg.trackCostPerMeter * cfg.gradeCostMult[grade];

  let cost = 0;
  let surfaceRef = 0;
  let elevatedRef = 0;
  let cutCover = 0;
  let bored = 0;
  let belowWaterTable = false;
  const strataSeen = new Set<string>();

  for (let i = 1; i < points.length; i++) {
    const a = points[i - 1] as Vec2;
    const b = points[i] as Vec2;
    const len = dist(a, b);
    const samples = Math.max(2, Math.ceil(len / 120));

    if (grade === 'tunnel') {
      // price each sub-sample through its own strata column, average per-metre
      let perMSum = 0;
      for (let s = 0; s < samples; s++) {
        const t = (s + 0.5) / samples;
        const p = { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t };
        const col = columnAt(state.cityKey, state.seed, WORLD_SIZE, state.osmElevation, state.osmElevRes, p);
        const seg = undergroundSegmentCost(surfacePerM, 1, col, tunnelDepthAt(state, p));
        perMSum += seg.costPerM;
        strataSeen.add(seg.stratum);
        if (seg.belowWaterTable) belowWaterTable = true;
        if (seg.method === 'cutCover') cutCover += seg.costPerM * (len / samples);
        else bored += seg.costPerM * (len / samples);
      }
      cost += (perMSum / samples) * len;
    } else {
      // surface / elevated: unchanged historic formula (water bridge premium)
      let waterFrac = 0;
      for (let s = 0; s <= samples; s++) {
        const t = s / samples;
        if (isWaterAt(state.fields, { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t })) waterFrac += 1 / (samples + 1);
      }
      const waterMult = 1 + waterFrac * (WATER_CROSSING_MULT - 1);
      cost += len * perMeter * waterMult;
    }
    surfaceRef += len * surfacePerM;
    elevatedRef += len * elevatedPerM;
  }

  // pouring track in rain or snow costs more (tunnels are sheltered → no surcharge)
  const weatherMult = grade === 'tunnel' ? 1 : weatherBuildCostMult(state.weather);
  cost = Math.round(cost * weatherMult);

  const breakdown: TrackCostBreakdown = {
    surface: Math.round(surfaceRef),
    elevated: Math.round(elevatedRef),
    cutCover: Math.round(cutCover),
    bored: Math.round(bored),
    strata: strataSeen.size ? [...strataSeen].join('/') : grade,
    belowWaterTable,
  };
  return { cost, breakdown };
}

/** Cost of a track polyline given mode/grade and water/strata. */
export function trackCost(state: GameState, mode: TransitMode, grade: 'surface' | 'elevated' | 'tunnel', points: Vec2[]): number {
  return trackCostDetailed(state, mode, grade, points).cost;
}

export function stationCost(mode: TransitMode): number {
  return MODES[mode].stationCost;
}

export function applyCommand(state: GameState, cmd: Command): CommandResult {
  if (state.failed) return { ok: false, error: 'This run is over' };
  const result = applyCommandInner(state, cmd);
  if (result.ok) {
    state.commandLog.push({ tick: state.tick, cmd });
  }
  return result;
}

function applyCommandInner(state: GameState, cmd: Command): CommandResult {
  switch (cmd.kind) {
    case 'buildStation': {
      if (!state.unlockedModes.includes(cmd.mode)) return { ok: false, error: `${MODES[cmd.mode].label} not yet unlocked` };
      // road-running modes snap the station onto the street network
      let pos = { ...cmd.pos };
      if (cmd.mode === 'bus' || cmd.mode === 'tram') {
        const snapped = nearestRoadPoint(state.roads, pos, 260);
        if (snapped) pos = snapped;
      }
      if (isWaterAt(state.fields, pos)) return { ok: false, error: 'Cannot build a station on water' };
      const cost = stationCost(cmd.mode);
      if (state.budget.cash < cost) return { ok: false, error: 'Insufficient funds' };
      for (const s of state.stations) {
        if (s.mode === cmd.mode && dist(s.pos, pos) < 200) return { ok: false, error: 'Too close to an existing station of this mode' };
      }
      const id = state.nextId++;
      state.stations.push({
        id,
        name: nextStationName(state),
        pos,
        mode: cmd.mode,
        level: 1,
        ridership: 0,
        alightings: 0,
        buildTick: state.tick,
      });
      state.budget.cash -= cost;
      state.demandDirty = true;
      state.stats.approval = Math.min(100, state.stats.approval + 2);
      return { ok: true, createdId: id };
    }

    case 'buildTrack': {
      const from = state.stations.find((s) => s.id === cmd.fromStationId);
      const to = state.stations.find((s) => s.id === cmd.toStationId);
      if (!from || !to) return { ok: false, error: 'Station not found' };
      if (from.id === to.id) return { ok: false, error: 'Track must connect two different stations' };
      if (from.mode !== cmd.mode || to.mode !== cmd.mode) return { ok: false, error: 'Both stations must match the track mode' };
      if (!MODES[cmd.mode].gradeOptions.includes(cmd.grade)) return { ok: false, error: `${MODES[cmd.mode].label} cannot be built ${cmd.grade}` };
      const exists = state.tracks.some(
        (t) => t.mode === cmd.mode &&
          ((t.fromStationId === from.id && t.toStationId === to.id) || (t.fromStationId === to.id && t.toStationId === from.id)),
      );
      if (exists) return { ok: false, error: 'Track already exists between these stations' };
      let points: Vec2[] = [from.pos, ...cmd.waypoints, to.pos];
      // bus/tram tracks follow the street network between stops
      if (cmd.mode === 'bus' || cmd.mode === 'tram') {
        const stops = [from.pos, ...cmd.waypoints, to.pos];
        const routed: Vec2[] = [];
        let allFound = true;
        for (let i = 0; i + 1 < stops.length; i++) {
          const leg = findRoadPath(state.roads, stops[i] as Vec2, stops[i + 1] as Vec2);
          if (!leg) {
            allFound = false;
            break;
          }
          for (let j = routed.length > 0 ? 1 : 0; j < leg.length; j++) routed.push(leg[j] as Vec2);
        }
        if (allFound && routed.length >= 2) points = routed;
      }
      let cost = trackCost(state, cmd.mode, cmd.grade, points);
      // Underground track sinks its end stations to the line's depth here and
      // charges the incremental station-deepening surcharge (deeper = pricier).
      if (cmd.grade === 'tunnel') {
        const base = MODES[cmd.mode].stationCost;
        for (const st of [from, to]) {
          const newDepth = tunnelDepthAt(state, st.pos);
          const prevDepth = st.depth ?? 0;
          if (newDepth > prevDepth) {
            cost += Math.round(stationDepthSurcharge(base, newDepth) - stationDepthSurcharge(base, prevDepth));
            st.depth = newDepth;
          }
        }
      }
      if (state.budget.cash < cost) return { ok: false, error: 'Insufficient funds' };
      // surface/elevated track cannot terminate mid-water; sampled cost already
      // prices crossings, so no hard block on crossing.
      const id = state.nextId++;
      const seg: TrackSegment = {
        id,
        mode: cmd.mode,
        grade: cmd.grade,
        fromStationId: from.id,
        toStationId: to.id,
        polyline: makePolyline(points.map((p) => ({ ...p }))),
        buildCost: cost,
      };
      // seed the grade-congestion density cache (refreshed each assignment)
      seg.congestionDensity = segmentDensity01(state.fields, seg);
      state.tracks.push(seg);
      state.budget.cash -= cost;
      state.demandDirty = true;
      return { ok: true, createdId: id };
    }

    case 'createRoute': {
      if (cmd.stationIds.length < 2) return { ok: false, error: 'A route needs at least 2 stops' };
      const segmentIds: number[] = [];
      for (let i = 0; i + 1 < cmd.stationIds.length; i++) {
        const a = cmd.stationIds[i] as number;
        const b = cmd.stationIds[i + 1] as number;
        const seg = state.tracks.find(
          (t) => t.mode === cmd.mode &&
            ((t.fromStationId === a && t.toStationId === b) || (t.fromStationId === b && t.toStationId === a)),
        );
        if (!seg) return { ok: false, error: `No ${MODES[cmd.mode].label} track between stops ${i + 1} and ${i + 2}` };
        segmentIds.push(seg.id);
      }
      const cfg = MODES[cmd.mode];
      const id = state.nextId++;
      const color = ROUTE_COLORS[state.routes.length % ROUTE_COLORS.length] as string;
      state.routes.push({
        id,
        name: `${cfg.label} ${state.routes.filter((r) => r.mode === cmd.mode).length + 1}`,
        color,
        mode: cmd.mode,
        stationIds: [...cmd.stationIds],
        segmentIds,
        headwaySeconds: cfg.defaultHeadway,
        fare: 2.5,
        vehicleCount: 0,
        dailyRidership: 0,
        dailyRevenue: 0,
        capacity: 0,
        load: 0,
        crowding: 0,
        segmentLoads: [],
      });
      // starter fleet: 2 vehicles if affordable, so new routes run immediately
      const starterCost = 2 * cfg.vehicleCost;
      if (state.budget.cash >= starterCost) {
        state.budget.cash -= starterCost;
        const route = state.routes[state.routes.length - 1]!;
        route.vehicleCount = 2;
        syncVehicles(state, id);
      }
      // headway follows the fleet (2 vehicles → a real frequency; 0 → MAX)
      state.routes[state.routes.length - 1]!.headwaySeconds = deriveHeadway(state, id);
      state.demandDirty = true;
      return { ok: true, createdId: id };
    }

    case 'editRoute': {
      const route = state.routes.find((r) => r.id === cmd.routeId);
      if (!route) return { ok: false, error: 'Route not found' };
      const cfg = MODES[route.mode];
      if (cmd.fare !== undefined) route.fare = Math.min(10, Math.max(0, cmd.fare));
      if (cmd.name !== undefined) route.name = cmd.name.slice(0, 40);
      if (cmd.color !== undefined) route.color = cmd.color;
      if (cmd.vehicleCount !== undefined) {
        const target = Math.max(0, Math.min(40, Math.round(cmd.vehicleCount)));
        const delta = target - route.vehicleCount;
        if (delta > 0) {
          const cost = delta * cfg.vehicleCost;
          if (state.budget.cash < cost) return { ok: false, error: 'Insufficient funds for vehicles' };
          state.budget.cash -= cost;
        } else if (delta < 0) {
          state.budget.cash += -delta * cfg.vehicleCost * 0.4; // resale
        }
        route.vehicleCount = target;
        syncVehicles(state, route.id);
      }
      // Frequency is derived from the fleet, never set directly — buying
      // vehicles is the only way to run more often. (cmd.headwaySeconds is
      // ignored; kept in the command type for back-compat.)
      route.headwaySeconds = deriveHeadway(state, route.id);
      state.demandDirty = true;
      return { ok: true };
    }

    case 'deleteRoute': {
      const idx = state.routes.findIndex((r) => r.id === cmd.routeId);
      if (idx < 0) return { ok: false, error: 'Route not found' };
      const route = state.routes[idx]!;
      state.budget.cash += route.vehicleCount * MODES[route.mode].vehicleCost * 0.4;
      state.routes.splice(idx, 1);
      state.vehicles = state.vehicles.filter((v) => v.routeId !== cmd.routeId);
      state.demandDirty = true;
      return { ok: true };
    }

    case 'demolishStation': {
      const idx = state.stations.findIndex((s) => s.id === cmd.stationId);
      if (idx < 0) return { ok: false, error: 'Station not found' };
      const usedByRoute = state.routes.some((r) => r.stationIds.includes(cmd.stationId));
      if (usedByRoute) return { ok: false, error: 'Remove routes serving this station first' };
      const usedByTrack = state.tracks.some((t) => t.fromStationId === cmd.stationId || t.toStationId === cmd.stationId);
      if (usedByTrack) return { ok: false, error: 'Demolish connected tracks first' };
      const station = state.stations[idx]!;
      state.budget.cash += MODES[station.mode].stationCost * REFUND_FRACTION;
      state.stations.splice(idx, 1);
      state.demandDirty = true;
      return { ok: true };
    }

    case 'demolishTrack': {
      const idx = state.tracks.findIndex((t) => t.id === cmd.trackId);
      if (idx < 0) return { ok: false, error: 'Track not found' };
      const track = state.tracks[idx] as TrackSegment;
      const usedByRoute = state.routes.some((r) => r.segmentIds.includes(cmd.trackId));
      if (usedByRoute) return { ok: false, error: 'Remove routes using this track first' };
      state.budget.cash += track.buildCost * REFUND_FRACTION;
      state.tracks.splice(idx, 1);
      state.demandDirty = true;
      return { ok: true };
    }

    case 'upgradeStation': {
      const station = state.stations.find((s) => s.id === cmd.stationId);
      if (!station) return { ok: false, error: 'Station not found' };
      if (station.level >= 5) return { ok: false, error: 'Station already at max level' };
      const cost = MODES[station.mode].stationCost * 0.5 * station.level;
      if (state.budget.cash < cost) return { ok: false, error: 'Insufficient funds' };
      state.budget.cash -= cost;
      station.level += 1;
      state.demandDirty = true;
      return { ok: true };
    }

    case 'takeLoan': {
      const amount = Math.max(0, cmd.amount);
      const maxLoan = 20_000_000;
      if (state.budget.loanBalance + amount > maxLoan) return { ok: false, error: 'Loan limit reached ($20M)' };
      state.budget.loanBalance += amount;
      state.budget.cash += amount;
      return { ok: true };
    }

    case 'repayLoan': {
      const amount = Math.min(cmd.amount, state.budget.loanBalance, state.budget.cash);
      if (amount <= 0) return { ok: false, error: 'Nothing to repay' };
      state.budget.loanBalance -= amount;
      state.budget.cash -= amount;
      return { ok: true };
    }

    case 'renameStation': {
      const station = state.stations.find((s) => s.id === cmd.stationId);
      if (!station) return { ok: false, error: 'Station not found' };
      station.name = cmd.name.slice(0, 40);
      return { ok: true };
    }
  }
  return { ok: false, error: 'Unknown command' };
}

/** Rebuild the vehicle pool for a route, spacing vehicles evenly. */
export function syncVehicles(state: GameState, routeId: number): void {
  const route = state.routes.find((r) => r.id === routeId);
  if (!route) return;
  state.vehicles = state.vehicles.filter((v) => v.routeId !== routeId);
  const pathLength = routePathLength(state, route.id);
  if (pathLength <= 0) return;
  for (let i = 0; i < route.vehicleCount; i++) {
    state.vehicles.push({
      id: state.nextId++,
      routeId,
      along: (i / route.vehicleCount) * pathLength,
      pathLength,
      dwellRemaining: 0,
      occupancy: 0,
    });
  }
}

/** Out-and-back path length for a route (vehicles loop A→B→A). */
export function routePathLength(state: GameState, routeId: number): number {
  const route = state.routes.find((r) => r.id === routeId);
  if (!route) return 0;
  let oneWay = 0;
  for (const segId of route.segmentIds) {
    const seg = state.tracks.find((t) => t.id === segId);
    if (seg) oneWay += seg.polyline.length;
  }
  return oneWay * 2;
}

/** Seconds for one vehicle to complete a full out-and-back cycle: travel time
 *  plus a dwell at every stop it passes (each intermediate stop twice).
 *  Travel time uses day-average grade-aware segment speeds (gradeEffects.ts), so
 *  surface lines that share the street get longer cycles — and thus worse
 *  headways — than their grade-separated twins. */
export function routeCycleSeconds(state: GameState, routeId: number): number {
  const route = state.routes.find((r) => r.id === routeId);
  if (!route) return 0;
  const cfg = MODES[route.mode];
  let oneWay = 0;
  for (const segId of route.segmentIds) {
    const seg = state.tracks.find((t) => t.id === segId);
    if (!seg) continue;
    const dens = segmentDensity01(state.fields, seg);
    const spd = segmentDayAverageSpeedMps(route.mode, seg.grade, dens);
    if (spd > 0) oneWay += seg.polyline.length / spd;
  }
  if (oneWay <= 0) return 0;
  const travel = oneWay * 2; // out-and-back
  const dwellStops = 2 * Math.max(1, route.stationIds.length - 1);
  return travel + dwellStops * cfg.dwellSeconds;
}

/** Headway is a CONSEQUENCE of fleet size: more vehicles on the same loop come
 *  more often. This is the coupling that makes buying vehicles matter. */
export function deriveHeadway(state: GameState, routeId: number): number {
  const route = state.routes.find((r) => r.id === routeId);
  if (!route) return MAX_HEADWAY;
  const cfg = MODES[route.mode];
  if (route.vehicleCount <= 0) return MAX_HEADWAY;
  const cycle = routeCycleSeconds(state, routeId);
  if (cycle <= 0) return cfg.defaultHeadway;
  return Math.max(cfg.minHeadway, Math.min(MAX_HEADWAY, cycle / route.vehicleCount));
}
