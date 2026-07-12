/**
 * Continuous 2D geometry. World units are METERS. No grid geometry —
 * scalar fields live in fields.ts; everything spatial here is vectors
 * and polylines.
 */

export interface Vec2 {
  x: number;
  y: number;
}

export const vec = (x: number, y: number): Vec2 => ({ x, y });

export function dist(a: Vec2, b: Vec2): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  return Math.sqrt(dx * dx + dy * dy);
}

export function distSq(a: Vec2, b: Vec2): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  return dx * dx + dy * dy;
}

export function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

export function lerpVec(a: Vec2, b: Vec2, t: number): Vec2 {
  return { x: lerp(a.x, b.x, t), y: lerp(a.y, b.y, t) };
}

export function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

/** Polyline with precomputed cumulative lengths (stored, not recomputed — determinism policy). */
export interface Polyline {
  points: Vec2[];
  /** cumulative[i] = distance from points[0] to points[i]; cumulative[0] = 0 */
  cumulative: number[];
  length: number;
}

export function makePolyline(points: Vec2[]): Polyline {
  const cumulative: number[] = [0];
  let total = 0;
  for (let i = 1; i < points.length; i++) {
    total += dist(points[i - 1] as Vec2, points[i] as Vec2);
    cumulative.push(total);
  }
  return { points, cumulative, length: total };
}

/** Position + heading at distance d along the polyline (clamped). */
export function pointAlong(pl: Polyline, d: number): { pos: Vec2; heading: number } {
  const pts = pl.points;
  if (pts.length === 0) return { pos: vec(0, 0), heading: 0 };
  if (pts.length === 1 || d <= 0) {
    const p0 = pts[0] as Vec2;
    const p1 = pts[Math.min(1, pts.length - 1)] as Vec2;
    return { pos: { ...p0 }, heading: Math.atan2(p1.y - p0.y, p1.x - p0.x) };
  }
  if (d >= pl.length) {
    const pA = pts[pts.length - 2] as Vec2;
    const pB = pts[pts.length - 1] as Vec2;
    return { pos: { ...pB }, heading: Math.atan2(pB.y - pA.y, pB.x - pA.x) };
  }
  // binary search the segment
  let lo = 0;
  let hi = pl.cumulative.length - 1;
  while (lo < hi - 1) {
    const mid = (lo + hi) >> 1;
    if ((pl.cumulative[mid] as number) <= d) lo = mid;
    else hi = mid;
  }
  const segStart = pl.cumulative[lo] as number;
  const segLen = (pl.cumulative[hi] as number) - segStart;
  const t = segLen > 0 ? (d - segStart) / segLen : 0;
  const a = pts[lo] as Vec2;
  const b = pts[hi] as Vec2;
  return { pos: lerpVec(a, b, t), heading: Math.atan2(b.y - a.y, b.x - a.x) };
}

/** Closest point on segment ab to p; returns t in [0,1] and squared distance. */
export function closestOnSegment(p: Vec2, a: Vec2, b: Vec2): { t: number; dsq: number; pos: Vec2 } {
  const abx = b.x - a.x;
  const aby = b.y - a.y;
  const lenSq = abx * abx + aby * aby;
  const t = lenSq > 0 ? clamp(((p.x - a.x) * abx + (p.y - a.y) * aby) / lenSq, 0, 1) : 0;
  const pos = { x: a.x + abx * t, y: a.y + aby * t };
  return { t, dsq: distSq(p, pos), pos };
}

/** Closest point on a polyline; returns distance-along and squared distance. */
export function closestOnPolyline(pl: Polyline, p: Vec2): { along: number; dsq: number; pos: Vec2 } {
  let best = { along: 0, dsq: Infinity, pos: pl.points[0] ?? vec(0, 0) };
  for (let i = 1; i < pl.points.length; i++) {
    const a = pl.points[i - 1] as Vec2;
    const b = pl.points[i] as Vec2;
    const c = closestOnSegment(p, a, b);
    if (c.dsq < best.dsq) {
      const segStart = pl.cumulative[i - 1] as number;
      const segLen = (pl.cumulative[i] as number) - segStart;
      best = { along: segStart + segLen * c.t, dsq: c.dsq, pos: c.pos };
    }
  }
  return best;
}

/** Uniform-bucket spatial hash for point lookups (stations, nodes). */
export class SpatialHash<T> {
  private buckets = new Map<number, T[]>();
  constructor(
    private cellSize: number,
    private getPos: (item: T) => Vec2,
  ) {}

  private key(x: number, y: number): number {
    const cx = Math.floor(x / this.cellSize);
    const cy = Math.floor(y / this.cellSize);
    return cx * 73856093 + cy * 19349663;
  }

  insert(item: T): void {
    const p = this.getPos(item);
    const k = this.key(p.x, p.y);
    const arr = this.buckets.get(k);
    if (arr) arr.push(item);
    else this.buckets.set(k, [item]);
  }

  remove(item: T): void {
    const p = this.getPos(item);
    const k = this.key(p.x, p.y);
    const arr = this.buckets.get(k);
    if (!arr) return;
    const i = arr.indexOf(item);
    if (i >= 0) arr.splice(i, 1);
  }

  /** All items within radius r of p (exact, post-filtered). */
  queryRadius(p: Vec2, r: number): T[] {
    const out: T[] = [];
    const rSq = r * r;
    const minCx = Math.floor((p.x - r) / this.cellSize);
    const maxCx = Math.floor((p.x + r) / this.cellSize);
    const minCy = Math.floor((p.y - r) / this.cellSize);
    const maxCy = Math.floor((p.y + r) / this.cellSize);
    for (let cx = minCx; cx <= maxCx; cx++) {
      for (let cy = minCy; cy <= maxCy; cy++) {
        const arr = this.buckets.get(cx * 73856093 + cy * 19349663);
        if (!arr) continue;
        for (const item of arr) {
          if (distSq(this.getPos(item), p) <= rSq) out.push(item);
        }
      }
    }
    return out;
  }

  rebuild(items: readonly T[]): void {
    this.buckets.clear();
    for (const item of items) this.insert(item);
  }
}

/** Deterministic 2D value noise with fBm — used by the city generator. */
export class Noise2D {
  private perm: Uint8Array;

  constructor(rngNextUint: () => number) {
    const p = new Uint8Array(256);
    for (let i = 0; i < 256; i++) p[i] = i;
    for (let i = 255; i > 0; i--) {
      const j = rngNextUint() % (i + 1);
      const tmp = p[i] as number;
      p[i] = p[j] as number;
      p[j] = tmp;
    }
    this.perm = new Uint8Array(512);
    for (let i = 0; i < 512; i++) this.perm[i] = p[i & 255] as number;
  }

  private hash(x: number, y: number): number {
    return (this.perm[((this.perm[x & 255] as number) + y) & 255] as number) / 255;
  }

  /** Smooth value noise in [0,1]. */
  at(x: number, y: number): number {
    const xi = Math.floor(x);
    const yi = Math.floor(y);
    const xf = x - xi;
    const yf = y - yi;
    const u = xf * xf * (3 - 2 * xf);
    const v = yf * yf * (3 - 2 * yf);
    const n00 = this.hash(xi, yi);
    const n10 = this.hash(xi + 1, yi);
    const n01 = this.hash(xi, yi + 1);
    const n11 = this.hash(xi + 1, yi + 1);
    return lerp(lerp(n00, n10, u), lerp(n01, n11, u), v);
  }

  /** Fractal Brownian motion, output roughly [0,1]. */
  fbm(x: number, y: number, octaves: number, lacunarity = 2, gain = 0.5): number {
    let amp = 0.5;
    let freq = 1;
    let sum = 0;
    let norm = 0;
    for (let o = 0; o < octaves; o++) {
      sum += amp * this.at(x * freq, y * freq);
      norm += amp;
      amp *= gain;
      freq *= lacunarity;
    }
    return sum / norm;
  }
}
