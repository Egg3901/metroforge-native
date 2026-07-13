/**
 * Simulation analytics layer — presentation-only, pure TypeScript.
 *
 * Accumulates a per-cell ridership heatmap (station boardings + alightings) and
 * a district↔district OD matrix from gravity-model assignment output, then
 * derives insight metrics. Never feeds back into the economy or assignment.
 *
 * Transient on GameState (stripped from saves, like traffic). Deterministic
 * given the same assignment outputs and day cadence.
 *
 * ── Heatmap binary payload (msgType=6) byte layout ──────────────────────────
 *
 * All multi-byte fields are little-endian. Clients that do not understand
 * msgType=6 MUST ignore the frame.
 *
 * Header (32 bytes):
 *   offset  size  type   field
 *   0       1     u8     msgType = 6
 *   1       1     u8     version = 1
 *   2       2     u16    reserved = 0
 *   4       4     u32    w            (grid width in cells)
 *   8       4     u32    h            (grid height in cells)
 *   12      4     f32    cellSize     (meters)
 *   16      4     f32    originX      (world meters, cell (0,0) corner)
 *   20      4     f32    originY
 *   24      4     f32    maxValue     (raw smoothed activity at 255)
 *   28      4     u32    day          (sim-day when emitted, 1-based)
 *
 * Body:
 *   offset 32: u8[w*h]  quantized cells, row-major
 *     reconstructed ≈ cells[i] / 255 * maxValue
 *     (boardings+alightings / day, mean over the rolling window)
 *
 * Total size = 32 + w*h bytes. For the default 96×96 field grid: 9248 B.
 * Budget: ANALYTICS_PAYLOAD_BUDGET_BYTES (50 KiB).
 *
 * Web worker host also ships the same fields as a structured `heatmap`
 * message (`HeatmapPayload` in protocol.ts) with a transferable Uint8Array;
 * the binary layout above is the sidecar / native wire form.
 */
import { MODES, TICKS_PER_DAY } from './constants';
import { cellIndexAt } from './fields';
import {
  computeBaselineDemandOd,
  MAX_UNSERVED_LINES,
  MIN_UNSERVED_TRIPS,
  UNSERVED_SHARE_MAX,
  type CarFlow,
  type UnservedDesire,
} from './transit/assignment';
import type { District, FieldGrid, FlowResult, GameState, RouteDef, Station } from './types';

/** Rolling temporal window for heatmap + OD smoothing (sim-days). */
export const ANALYTICS_WINDOW_DAYS = 7;
/** Emit a quantized heatmap payload every N completed sim-days. */
export const HEATMAP_EMIT_INTERVAL_DAYS = 7;
/** Catchment radius for the coverage insight (meters). */
export const CATCHMENT_RADIUS_M = 400;
/** Hard size budget for the encoded heatmap frame. */
export const ANALYTICS_PAYLOAD_BUDGET_BYTES = 50 * 1024;

export const HEATMAP_MSG_TYPE = 6;
export const HEATMAP_VERSION = 1;
export const HEATMAP_HEADER_BYTES = 32;

export interface AnalyticsInsights {
  /** District with highest demand potential × (1 − transit service). */
  underservedDistrictId: number | null;
  underservedDistrictName: string | null;
  /** Daily trip potential used in the underserved score. */
  underservedDemand: number;
  /** Transit mode share for that district's originating trips, 0..1. */
  underservedService: number;
  /** Highest-load route segment (corridor). */
  overloadedCorridor: {
    routeId: number;
    routeName: string;
    fromStationId: number;
    toStationId: number;
    load: number;
  } | null;
  /** Daily transit riders per vehicle-kilometre. */
  networkEfficiency: number;
  /** Fraction of population within CATCHMENT_RADIUS_M of a served station. */
  catchmentCoverage: number;
}

export interface AnalyticsState {
  /** Ring buffer of daily per-cell activity grids (boardings + alightings). */
  dayHeat: Float32Array[];
  /** Ring buffer of daily OD totals keyed `${originId}:${destId}`. */
  dayOd: Map<string, number>[];
  /** Days pushed into the rolling windows. */
  daysRecorded: number;
  /** Last sim-day (1-based calendar day index) a heatmap was emitted. */
  lastHeatmapDay: number;
  /** Latest derived insights (recomputed each analytics day). */
  insights: AnalyticsInsights;
  /** Latest assignment snapshot waiting to be committed at day close. */
  pendingBoardings: Map<number, number>;
  pendingAlightings: Map<number, number>;
  pendingFlows: FlowResult[];
  pendingCarFlows: CarFlow[];
}

/** Structured heatmap for the web host (mirrors the binary body). */
export interface HeatmapPayload {
  w: number;
  h: number;
  cellSize: number;
  originX: number;
  originY: number;
  /** Raw smoothed activity corresponding to quantized value 255. */
  maxValue: number;
  /** Sim-day (1-based) when this payload was built. */
  day: number;
  /** Quantized 0..255, length w*h, row-major. */
  cells: Uint8Array;
}

export function emptyInsights(): AnalyticsInsights {
  return {
    underservedDistrictId: null,
    underservedDistrictName: null,
    underservedDemand: 0,
    underservedService: 0,
    overloadedCorridor: null,
    networkEfficiency: 0,
    catchmentCoverage: 0,
  };
}

export function createAnalyticsState(): AnalyticsState {
  return {
    dayHeat: [],
    dayOd: [],
    daysRecorded: 0,
    lastHeatmapDay: 0,
    insights: emptyInsights(),
    pendingBoardings: new Map(),
    pendingAlightings: new Map(),
    pendingFlows: [],
    pendingCarFlows: [],
  };
}

/** Ensure analytics exists on state (lazy; keeps newGame / saves lean). */
export function ensureAnalytics(state: GameState): AnalyticsState {
  if (!state.analytics) state.analytics = createAnalyticsState();
  return state.analytics;
}

/**
 * Capture the latest assignment outputs. Called from refreshAssignment so the
 * next day-close samples the same numbers the economy just used.
 */
export function captureAssignmentAnalytics(
  state: GameState,
  boardings: Map<number, number>,
  alightings: Map<number, number>,
  flows: FlowResult[],
  carFlows: CarFlow[],
): void {
  const a = ensureAnalytics(state);
  a.pendingBoardings = new Map(boardings);
  a.pendingAlightings = new Map(alightings);
  a.pendingFlows = flows;
  a.pendingCarFlows = carFlows;
}

/** Splat station boardings + alightings into a zeroed grid (exact cell deposit). */
export function splatStationActivity(
  grid: Pick<FieldGrid, 'w' | 'h' | 'cellSize' | 'originX' | 'originY'>,
  stations: readonly { id: number; pos: { x: number; y: number } }[],
  boardings: ReadonlyMap<number, number>,
  alightings: ReadonlyMap<number, number>,
  out: Float32Array,
): void {
  out.fill(0);
  const n = grid.w * grid.h;
  if (out.length !== n) throw new Error(`heatmap buffer length ${out.length} != ${n}`);
  for (const s of stations) {
    const activity = (boardings.get(s.id) ?? 0) + (alightings.get(s.id) ?? 0);
    if (activity <= 0) continue;
    const idx = cellIndexAt(grid as FieldGrid, s.pos);
    out[idx] = (out[idx] as number) + activity;
  }
}

/** Build a dense OD total-trips map from transit flows + car residuals. */
export function buildOdTotals(flows: readonly FlowResult[], carFlows: readonly CarFlow[]): Map<string, number> {
  const od = new Map<string, number>();
  const add = (o: number, d: number, trips: number): void => {
    if (trips <= 0) return;
    const k = `${o}:${d}`;
    od.set(k, (od.get(k) ?? 0) + trips);
  };
  for (const f of flows) add(f.originDistrict, f.destDistrict, f.transitTrips + f.carTrips);
  for (const c of carFlows) {
    // flows already counted pairs with transit; carFlows may duplicate those.
    // Prefer the flow entry when both exist: only add car-only pairs here by
    // skipping keys already present from flows.
    const k = `${c.originDistrict}:${c.destDistrict}`;
    if (od.has(k)) continue;
    add(c.originDistrict, c.destDistrict, c.carTrips);
  }
  return od;
}

/**
 * OD totals that attribute car residual onto pairs already in `flows` without
 * double-counting: for each flow, total = transit + car; car-only pairs from
 * carFlows fill the gaps. When both lists describe the same pair, the flow
 * row is authoritative (it already includes carTrips).
 */
export function buildOdMatrixExact(flows: readonly FlowResult[], carFlows: readonly CarFlow[]): Map<string, number> {
  return buildOdTotals(flows, carFlows);
}

/**
 * Build the demand/gaps overlay from the station-independent baseline gravity
 * field rather than from `state.unserved` (which only contains pairs the
 * assignment router enumerated near existing stations, making demand look like
 * it clusters at stations).
 *
 * For each qualifying district pair, gap weight = baselineDemand × (1 −
 * servedShare), where `servedShare` is the transit mode share the existing
 * assignment achieved on that pair (`transitTrips / baselineDemand`, capped at
 * 1). Pairs the router never enumerated simply have served = 0, so their full
 * demand surfaces as an unmet gap — demand shows everywhere it exists, gaps
 * show everywhere demand goes unmet, not just around stations.
 *
 * Pure/read-only: derives from `state.flows` + `state.districts` and writes
 * nothing back, so it stays out of the determinism hash. The top-N by weight
 * trim keeps the payload bounded (MAX_UNSERVED_LINES lines, well under budget).
 */
export function buildDemandOverlay(state: GameState): UnservedDesire[] {
  const baseline = computeBaselineDemandOd(state);
  const served = new Map<string, number>();
  for (const f of state.flows) {
    const k = `${f.originDistrict}:${f.destDistrict}`;
    served.set(k, (served.get(k) ?? 0) + f.transitTrips);
  }
  const districtById = new Map(state.districts.map((d) => [d.id, d]));
  const lines: UnservedDesire[] = [];
  for (const b of baseline) {
    if (b.trips < MIN_UNSERVED_TRIPS) continue;
    const transit = served.get(`${b.originDistrict}:${b.destDistrict}`) ?? 0;
    const share = Math.min(1, transit / b.trips);
    if (share >= UNSERVED_SHARE_MAX) continue;
    const o = districtById.get(b.originDistrict);
    const d = districtById.get(b.destDistrict);
    if (!o || !d) continue;
    lines.push({
      x1: o.centroid.x, y1: o.centroid.y,
      x2: d.centroid.x, y2: d.centroid.y,
      weight: b.trips * (1 - share), share,
    });
  }
  lines.sort((a, b) => b.weight - a.weight);
  lines.length = Math.min(lines.length, MAX_UNSERVED_LINES);
  return lines;
}

/** Mean of the rolling day grids (zeros if empty). */
export function smoothedHeatmap(dayHeat: readonly Float32Array[]): Float32Array {
  if (dayHeat.length === 0) return new Float32Array(0);
  const n = dayHeat[0]!.length;
  const out = new Float32Array(n);
  for (const day of dayHeat) {
    for (let i = 0; i < n; i++) out[i] = (out[i] as number) + (day[i] as number);
  }
  const inv = 1 / dayHeat.length;
  for (let i = 0; i < n; i++) out[i] = (out[i] as number) * inv;
  return out;
}

/** Mean OD over the rolling window. */
export function smoothedOd(dayOd: readonly Map<string, number>[]): Map<string, number> {
  const acc = new Map<string, number>();
  if (dayOd.length === 0) return acc;
  for (const day of dayOd) {
    for (const [k, v] of day) acc.set(k, (acc.get(k) ?? 0) + v);
  }
  const inv = 1 / dayOd.length;
  for (const [k, v] of acc) acc.set(k, v * inv);
  return acc;
}

export function quantizeHeatmap(smoothed: Float32Array): { cells: Uint8Array; maxValue: number } {
  let maxValue = 0;
  for (let i = 0; i < smoothed.length; i++) {
    const v = smoothed[i] as number;
    if (v > maxValue) maxValue = v;
  }
  const cells = new Uint8Array(smoothed.length);
  if (maxValue <= 0) return { cells, maxValue: 0 };
  for (let i = 0; i < smoothed.length; i++) {
    cells[i] = Math.min(255, Math.round(((smoothed[i] as number) / maxValue) * 255));
  }
  return { cells, maxValue };
}

/** Encode the compact quantized heatmap (see file header for byte layout). */
export function encodeHeatmapPayload(p: HeatmapPayload): ArrayBuffer {
  const cellCount = p.w * p.h;
  if (p.cells.length !== cellCount) {
    throw new Error(`heatmap cells length ${p.cells.length} != ${cellCount}`);
  }
  const total = HEATMAP_HEADER_BYTES + cellCount;
  if (total > ANALYTICS_PAYLOAD_BUDGET_BYTES) {
    throw new Error(`heatmap payload ${total} B exceeds ${ANALYTICS_PAYLOAD_BUDGET_BYTES} B budget`);
  }
  const buf = new ArrayBuffer(total);
  const dv = new DataView(buf);
  const bytes = new Uint8Array(buf);
  dv.setUint8(0, HEATMAP_MSG_TYPE);
  dv.setUint8(1, HEATMAP_VERSION);
  dv.setUint16(2, 0, true);
  dv.setUint32(4, p.w >>> 0, true);
  dv.setUint32(8, p.h >>> 0, true);
  dv.setFloat32(12, p.cellSize, true);
  dv.setFloat32(16, p.originX, true);
  dv.setFloat32(20, p.originY, true);
  dv.setFloat32(24, p.maxValue, true);
  dv.setUint32(28, p.day >>> 0, true);
  bytes.set(p.cells, HEATMAP_HEADER_BYTES);
  return buf;
}

/** Decode a msgType=6 heatmap frame (for tests / native parity). */
export function decodeHeatmapPayload(buf: ArrayBuffer): HeatmapPayload {
  const dv = new DataView(buf);
  if (dv.getUint8(0) !== HEATMAP_MSG_TYPE) throw new Error('not a heatmap payload');
  if (dv.getUint8(1) !== HEATMAP_VERSION) throw new Error(`unsupported heatmap version ${dv.getUint8(1)}`);
  const w = dv.getUint32(4, true);
  const h = dv.getUint32(8, true);
  const cellSize = dv.getFloat32(12, true);
  const originX = dv.getFloat32(16, true);
  const originY = dv.getFloat32(20, true);
  const maxValue = dv.getFloat32(24, true);
  const day = dv.getUint32(28, true);
  const cellCount = w * h;
  const cells = new Uint8Array(cellCount);
  cells.set(new Uint8Array(buf, HEATMAP_HEADER_BYTES, cellCount));
  return { w, h, cellSize, originX, originY, maxValue, day, cells };
}

export function buildHeatmapPayload(state: GameState, day: number): HeatmapPayload {
  const a = ensureAnalytics(state);
  const g = state.fields;
  const smoothed = smoothedHeatmap(a.dayHeat);
  const values = smoothed.length === g.w * g.h ? smoothed : new Float32Array(g.w * g.h);
  const { cells, maxValue } = quantizeHeatmap(values);
  return {
    w: g.w,
    h: g.h,
    cellSize: g.cellSize,
    originX: g.originX,
    originY: g.originY,
    maxValue,
    day,
    cells,
  };
}

/** Daily vehicle-kilometres: each vehicle covers the out-and-back path once per cycle. */
export function dailyVehicleKm(state: GameState): number {
  let vkm = 0;
  for (const r of state.routes) {
    if (r.vehicleCount <= 0) continue;
    let oneWay = 0;
    for (const segId of r.segmentIds) {
      const seg = state.tracks.find((t) => t.id === segId);
      if (seg) oneWay += seg.polyline.length;
    }
    const pathM = oneWay * 2;
    if (pathM <= 0) continue;
    const cfg = MODES[r.mode];
    const dwellStops = 2 * Math.max(1, r.stationIds.length - 1);
    const cycle = pathM / cfg.speed + dwellStops * cfg.dwellSeconds;
    if (cycle <= 0) continue;
    vkm += r.vehicleCount * (pathM / 1000) * (TICKS_PER_DAY / cycle);
  }
  return vkm;
}

/**
 * Worst underserved district: high originating demand, low transit share.
 * Score = demand × (1 − service); ties break toward lower district id.
 */
export function findUnderservedDistrict(
  districts: readonly District[],
  od: ReadonlyMap<string, number>,
  flows: readonly FlowResult[],
): { id: number; name: string; demand: number; service: number } | null {
  const transitOut = new Map<number, number>();
  for (const f of flows) {
    transitOut.set(f.originDistrict, (transitOut.get(f.originDistrict) ?? 0) + f.transitTrips);
  }
  const demandOut = new Map<number, number>();
  for (const [k, v] of od) {
    const o = Number(k.split(':')[0]);
    demandOut.set(o, (demandOut.get(o) ?? 0) + v);
  }

  let best: { id: number; name: string; demand: number; service: number; score: number } | null = null;
  for (const d of districts) {
    const demand = demandOut.get(d.id) ?? 0;
    if (demand < 1) continue;
    const transit = transitOut.get(d.id) ?? 0;
    const service = Math.min(1, transit / demand);
    const score = demand * (1 - service);
    if (
      !best ||
      score > best.score ||
      (score === best.score && d.id < best.id)
    ) {
      best = { id: d.id, name: d.name, demand, service, score };
    }
  }
  if (!best) return null;
  return { id: best.id, name: best.name, demand: best.demand, service: best.service };
}

/** Highest segmentLoads entry across routes. */
export function findOverloadedCorridor(
  routes: readonly RouteDef[],
): AnalyticsInsights['overloadedCorridor'] {
  let best: AnalyticsInsights['overloadedCorridor'] = null;
  for (const r of routes) {
    const loads = r.segmentLoads ?? [];
    for (let i = 0; i < loads.length; i++) {
      const load = loads[i] ?? 0;
      if (load <= 0) continue;
      if (!best || load > best.load || (load === best.load && r.id < best.routeId)) {
        const a = r.stationIds[i];
        const b = r.stationIds[i + 1];
        if (a === undefined || b === undefined) continue;
        best = {
          routeId: r.id,
          routeName: r.name,
          fromStationId: a,
          toStationId: b,
          load,
        };
      }
    }
  }
  return best;
}

/**
 * Population share within `radiusM` of any station that sits on an active
 * route (vehicleCount > 0). Distinct from stats.coverage (mode walk-radius).
 */
export function catchmentCoverage(
  grid: FieldGrid,
  stations: readonly Station[],
  routes: readonly RouteDef[],
  radiusM: number = CATCHMENT_RADIUS_M,
): number {
  const served = new Set<number>();
  for (const r of routes) {
    if (r.vehicleCount <= 0) continue;
    for (const sid of r.stationIds) served.add(sid);
  }
  const active = stations.filter((s) => served.has(s.id));
  let covered = 0;
  let total = 0;
  const r2 = radiusM * radiusM;
  for (let i = 0; i < grid.population.length; i++) {
    const pop = grid.population[i] as number;
    if (pop <= 0) continue;
    total += pop;
    const cx = grid.originX + ((i % grid.w) + 0.5) * grid.cellSize;
    const cy = grid.originY + (Math.floor(i / grid.w) + 0.5) * grid.cellSize;
    for (const s of active) {
      const dx = s.pos.x - cx;
      const dy = s.pos.y - cy;
      if (dx * dx + dy * dy <= r2) {
        covered += pop;
        break;
      }
    }
  }
  return total > 0 ? covered / total : 0;
}

export function computeInsights(state: GameState, od: ReadonlyMap<string, number>): AnalyticsInsights {
  const under = findUnderservedDistrict(state.districts, od, state.flows);
  const vkm = dailyVehicleKm(state);
  const riders = state.stats.dailyTransitTrips;
  return {
    underservedDistrictId: under?.id ?? null,
    underservedDistrictName: under?.name ?? null,
    underservedDemand: under?.demand ?? 0,
    underservedService: under?.service ?? 0,
    overloadedCorridor: findOverloadedCorridor(state.routes),
    networkEfficiency: vkm > 0 ? riders / vkm : 0,
    catchmentCoverage: catchmentCoverage(state.fields, state.stations, state.routes),
  };
}

export interface AnalyticsDayResult {
  /** True when a heatmap should be shipped this day. */
  emitHeatmap: boolean;
  payload: HeatmapPayload | null;
}

/**
 * Close one sim-day: push pending assignment samples into the rolling windows,
 * recompute insights, and optionally build a quantized heatmap payload.
 */
export function commitAnalyticsDay(state: GameState, day: number): AnalyticsDayResult {
  const a = ensureAnalytics(state);
  const g = state.fields;
  const heat = new Float32Array(g.w * g.h);
  splatStationActivity(g, state.stations, a.pendingBoardings, a.pendingAlightings, heat);
  a.dayHeat.push(heat);
  if (a.dayHeat.length > ANALYTICS_WINDOW_DAYS) a.dayHeat.shift();

  const od = buildOdMatrixExact(a.pendingFlows, a.pendingCarFlows);
  a.dayOd.push(od);
  if (a.dayOd.length > ANALYTICS_WINDOW_DAYS) a.dayOd.shift();

  a.daysRecorded += 1;
  a.insights = computeInsights(state, smoothedOd(a.dayOd));

  const shouldEmit =
    day > 0 && day % HEATMAP_EMIT_INTERVAL_DAYS === 0 && a.lastHeatmapDay !== day;
  if (!shouldEmit) return { emitHeatmap: false, payload: null };

  a.lastHeatmapDay = day;
  const payload = buildHeatmapPayload(state, day);
  // Touch encode path so size budget is enforced even for the structured form.
  encodeHeatmapPayload(payload);
  return { emitHeatmap: true, payload };
}

/** Plain-language cues derived from analytics insights (optional UI strings). */
export function analyticsInsightLines(insights: AnalyticsInsights, limit = 2): string[] {
  const out: string[] = [];
  if (insights.underservedDistrictName) {
    out.push(
      `${insights.underservedDistrictName} has demand but weak service (${Math.round(insights.underservedService * 100)}% transit share).`,
    );
  }
  if (insights.overloadedCorridor) {
    const c = insights.overloadedCorridor;
    out.push(`${c.routeName} corridor is overloaded (${Math.round(c.load)} daily trips on one segment).`);
  }
  if (insights.networkEfficiency > 0) {
    out.push(`Network efficiency: ${insights.networkEfficiency.toFixed(1)} riders per vehicle-km.`);
  }
  if (insights.catchmentCoverage < 0.35 && insights.catchmentCoverage > 0) {
    out.push(
      `Only ${Math.round(insights.catchmentCoverage * 100)}% of residents live within ${CATCHMENT_RADIUS_M}m of a served stop.`,
    );
  }
  return out.slice(0, limit);
}
