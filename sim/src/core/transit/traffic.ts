/**
 * Road congestion model. Turns the gravity model's leftover CAR trips into a
 * spatial congestion field + a short list of bottleneck hotspots, so the UI can
 * show where the road network chokes.
 *
 * Cheap and deterministic: car OD flows are rasterized as desire lines into the
 * field grid (a betweenness proxy — corridors that many trips share light up),
 * then divided by a road-capacity field derived from the street network. Where
 * lots of car demand converges on thin roads, congestion spikes. Building
 * transit that wins those trips thins the demand and the heat drops — live.
 *
 * Presentation/analytics only. Never feeds back into the economy.
 */
import type { GameState, RoadEdge } from '../types';
import type { CarFlow } from './assignment';

export interface TrafficHotspot {
  x: number;
  y: number;
  /** 0..1 congestion at the peak */
  severity: number;
}

export interface TrafficField {
  w: number;
  h: number;
  cellSize: number;
  originX: number;
  originY: number;
  /** per-cell congestion, 0..1 (volume / capacity, normalized) */
  values: Float32Array;
  hotspots: TrafficHotspot[];
}

// Road capacity is a pure function of the (immutable) road network — cache it.
const capacityCache = new WeakMap<RoadEdge[], Float32Array>();
// A stable congestion reference per network so the overlay reflects ABSOLUTE
// load — it surges at rush hour and eases at night instead of self-normalizing
// to look identical every recompute.
const refCache = new WeakMap<RoadEdge[], number>();

/** How much throughput each road class contributes to a cell it passes through. */
const CLASS_CAPACITY: Record<string, number> = { arterial: 6, collector: 3.5, local: 1.4 };

function roadCapacityField(state: GameState): Float32Array {
  const hit = capacityCache.get(state.roads);
  if (hit) return hit;
  const g = state.fields;
  const cap = new Float32Array(g.w * g.h);
  const cellOf = (x: number, y: number): number => {
    const cx = Math.floor((x - g.originX) / g.cellSize);
    const cy = Math.floor((y - g.originY) / g.cellSize);
    if (cx < 0 || cy < 0 || cx >= g.w || cy >= g.h) return -1;
    return cy * g.w + cx;
  };
  for (const road of state.roads) {
    const add = CLASS_CAPACITY[road.cls] ?? 1;
    const pl = road.polyline;
    // walk the polyline at ~half a cell so no cell is skipped
    const step = g.cellSize * 0.5;
    for (let d = 0; d <= pl.length; d += step) {
      // linear scan of cumulative — polylines are short
      let i = 1;
      while (i < pl.cumulative.length - 1 && (pl.cumulative[i] as number) < d) i++;
      const a = pl.points[i - 1]!;
      const b = pl.points[i]!;
      const segStart = pl.cumulative[i - 1] as number;
      const segLen = (pl.cumulative[i] as number) - segStart || 1;
      const t = (d - segStart) / segLen;
      const ci = cellOf(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
      if (ci >= 0) cap[ci] = Math.max(cap[ci] as number, add);
    }
  }
  // one blur pass so capacity bleeds to the parcels a road actually serves
  blur(cap, g.w, g.h);
  capacityCache.set(state.roads, cap);
  return cap;
}

/** In-place 3x3 box blur. */
function blur(arr: Float32Array, w: number, h: number): void {
  const next = new Float32Array(arr.length);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      let sum = 0;
      let cnt = 0;
      for (let oy = -1; oy <= 1; oy++) {
        for (let ox = -1; ox <= 1; ox++) {
          const nx = x + ox;
          const ny = y + oy;
          if (nx < 0 || ny < 0 || nx >= w || ny >= h) continue;
          sum += arr[ny * w + nx] as number;
          cnt++;
        }
      }
      next[y * w + x] = sum / cnt;
    }
  }
  arr.set(next);
}

export function computeTraffic(state: GameState, carFlows: CarFlow[], demandScale = 1): TrafficField {
  const g = state.fields;
  const W = g.w;
  const H = g.h;
  const load = new Float32Array(W * H);
  const centroid = new Map(state.districts.map((d) => [d.id, d.centroid]));

  // Rasterize each car OD flow as a straight desire line into the load grid.
  for (const f of carFlows) {
    if (f.carTrips < 1) continue;
    const a = centroid.get(f.originDistrict);
    const b = centroid.get(f.destDistrict);
    if (!a || !b) continue;
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const len = Math.hypot(dx, dy) || 1;
    const steps = Math.max(1, Math.ceil(len / (g.cellSize * 0.6)));
    const per = f.carTrips / steps; // conserve total vehicle-trips along the corridor
    for (let s = 0; s <= steps; s++) {
      const t = s / steps;
      const x = a.x + dx * t;
      const y = a.y + dy * t;
      const cx = Math.floor((x - g.originX) / g.cellSize);
      const cy = Math.floor((y - g.originY) / g.cellSize);
      if (cx < 0 || cy < 0 || cx >= W || cy >= H) continue;
      if ((g.water[cy * W + cx] as number) === 1) continue; // no congestion on water
      load[cy * W + cx] = (load[cy * W + cx] as number) + per;
    }
  }
  blur(load, W, H);

  // Congestion = demand / capacity. Cells with demand but no road choke hardest.
  const cap = roadCapacityField(state);
  const ratio = new Float32Array(W * H);
  const baseRatios: number[] = [];
  for (let i = 0; i < W * H; i++) {
    const l = load[i] as number;
    if (l <= 0 || (g.water[i] as number) === 1) continue;
    const r = l / ((cap[i] as number) * 90 + 1); // baseline (unscaled) demand/capacity
    ratio[i] = r;
    baseRatios.push(r);
  }
  const values = new Float32Array(W * H);
  if (baseRatios.length === 0) {
    return { w: W, h: H, cellSize: g.cellSize, originX: g.originX, originY: g.originY, values, hotspots: [] };
  }
  // Fixed reference (first-seen 95th percentile) so absolute load shows through:
  // rush-hour demandScale pushes more streets red; night eases them green.
  let ref = refCache.get(state.roads);
  if (ref === undefined) {
    const sorted = [...baseRatios].sort((p, q) => p - q);
    ref = Math.max(1e-6, (sorted[Math.floor(sorted.length * 0.92)] as number) * 1.25);
    refCache.set(state.roads, ref);
  }
  for (let i = 0; i < values.length; i++) values[i] = Math.min(1, ((ratio[i] as number) * demandScale) / ref);

  // Hotspots: local maxima above a threshold, spaced apart, worst first.
  const HOT = 0.55;
  const cand: TrafficHotspot[] = [];
  for (let y = 1; y < H - 1; y++) {
    for (let x = 1; x < W - 1; x++) {
      const v = values[y * W + x] as number;
      if (v < HOT) continue;
      let isMax = true;
      for (let oy = -1; oy <= 1 && isMax; oy++) {
        for (let ox = -1; ox <= 1; ox++) {
          if ((values[(y + oy) * W + (x + ox)] as number) > v) { isMax = false; break; }
        }
      }
      if (!isMax) continue;
      cand.push({ x: g.originX + (x + 0.5) * g.cellSize, y: g.originY + (y + 0.5) * g.cellSize, severity: v });
    }
  }
  cand.sort((p, q) => q.severity - p.severity);
  const hotspots: TrafficHotspot[] = [];
  const MIN_SEP = g.cellSize * 3;
  for (const c of cand) {
    if (hotspots.some((h) => Math.hypot(h.x - c.x, h.y - c.y) < MIN_SEP)) continue;
    hotspots.push(c);
    if (hotspots.length >= 12) break;
  }
  return { w: W, h: H, cellSize: g.cellSize, originX: g.originX, originY: g.originY, values, hotspots };
}
