/**
 * Evenly-spaced streamline tracing over a tensor field (Jobard–Lefer seeding,
 * Chen et al. hyperstreamlines). Streamlines become streets; separation
 * distance controls road class density. Deterministic: all randomness comes
 * from the caller's Rng.
 */
import type { Vec2 } from '../geometry';
import type { Rng } from '../rng';
import { stepDirection, type TensorField } from './tensor';

const STEP = 40; // meters per integration step

export interface TraceOptions {
  /** minimum spacing between parallel streamlines of this class */
  separation: number | ((p: Vec2) => number);
  /** stop when leaving this predicate (e.g. populated land) */
  inDomain: (p: Vec2) => boolean;
  /** allow jumping short forbidden spans (bridges); 0 disables */
  bridgeMaxSteps: number;
  /** water/blocked test used for bridging */
  blocked: (p: Vec2) => boolean;
  maxLength: number;
  minLength: number;
  /** seeds to start from, in priority order */
  seeds: Vec2[];
  /** existing road sample points (e.g. arterials) that new lines may snap/join onto */
  snapTargets?: Vec2[];
  /** how many extra seeds to spawn along each accepted streamline */
  spawnSeeds: boolean;
  eigenDirs: (0 | 1)[];
}

/** Uniform grid of accepted sample points for separation tests. */
class SeparationGrid {
  private cell: number;
  private map = new Map<number, Vec2[]>();
  constructor(cell: number) {
    this.cell = cell;
  }
  private key(x: number, y: number): number {
    return Math.floor(x / this.cell) * 73856093 + Math.floor(y / this.cell) * 19349663;
  }
  add(p: Vec2): void {
    const k = this.key(p.x, p.y);
    const arr = this.map.get(k);
    if (arr) arr.push(p);
    else this.map.set(k, [p]);
  }
  nearestPoint(p: Vec2, radius: number): Vec2 | null {
    let best = radius * radius;
    let bestQ: Vec2 | null = null;
    const r = Math.ceil(radius / this.cell);
    const cx = Math.floor(p.x / this.cell);
    const cy = Math.floor(p.y / this.cell);
    for (let oy = -r; oy <= r; oy++) {
      for (let ox = -r; ox <= r; ox++) {
        const arr = this.map.get((cx + ox) * 73856093 + (cy + oy) * 19349663);
        if (!arr) continue;
        for (const q of arr) {
          const d = (q.x - p.x) * (q.x - p.x) + (q.y - p.y) * (q.y - p.y);
          if (d < best) {
            best = d;
            bestQ = q;
          }
        }
      }
    }
    return bestQ;
  }
  nearest(p: Vec2, radius: number): number {
    let best = Infinity;
    const r = Math.ceil(radius / this.cell);
    const cx = Math.floor(p.x / this.cell);
    const cy = Math.floor(p.y / this.cell);
    for (let oy = -r; oy <= r; oy++) {
      for (let ox = -r; ox <= r; ox++) {
        const arr = this.map.get((cx + ox) * 73856093 + (cy + oy) * 19349663);
        if (!arr) continue;
        for (const q of arr) {
          const d = (q.x - p.x) * (q.x - p.x) + (q.y - p.y) * (q.y - p.y);
          if (d < best) best = d;
        }
      }
    }
    return Math.sqrt(best);
  }
}

export function traceStreamlines(field: TensorField, rng: Rng, opts: TraceOptions): Vec2[][] {
  const sepAt = typeof opts.separation === 'function' ? opts.separation : () => opts.separation as number;
  let minSep = Infinity;
  if (typeof opts.separation === 'number') minSep = opts.separation;
  else {
    // probe a few seeds to size the grid
    for (const s of opts.seeds) minSep = Math.min(minSep, sepAt(s));
    if (!isFinite(minSep)) minSep = 100;
  }
  // independent separation grids per eigen family: perpendicular crossings
  // are intersections, not collisions (Chen et al.)
  const grids: Record<0 | 1, SeparationGrid> = {
    0: new SeparationGrid(Math.max(40, minSep / 2)),
    1: new SeparationGrid(Math.max(40, minSep / 2)),
  };
  const results: Vec2[][] = [];
  // every accepted sample from ANY family + provided targets: line ends snap
  // onto this so the network is connected instead of almost-touching
  const snapGrid = new SeparationGrid(Math.max(40, minSep / 2));
  for (const t of opts.snapTargets ?? []) snapGrid.add(t);
  const queue: Vec2[] = [...opts.seeds];

  const traceOne = (seed: Vec2, eigen: 0 | 1): Vec2[] | null => {
    const sep = sepAt(seed);
    const grid = grids[eigen];
    if (grid.nearest(seed, sep) < sep * 0.9) return null;
    if (!opts.inDomain(seed)) return null;

    // trace both ways from the seed and stitch
    const halves: Vec2[][] = [];
    for (const flip of [1, -1] as const) {
      const pts: Vec2[] = [];
      let p = { ...seed };
      let prev: Vec2 | null = null;
      let bridging = 0;
      for (let len = 0; len < opts.maxLength / 2; len += STEP) {
        let dir = stepDirection(field, p, eigen, prev);
        if (!prev) dir = { x: dir.x * flip, y: dir.y * flip };
        const next = { x: p.x + dir.x * STEP, y: p.y + dir.y * STEP };
        if (opts.blocked(next)) {
          if (opts.bridgeMaxSteps > 0 && bridging < opts.bridgeMaxSteps && prev) {
            // bridge: keep heading straight across
            bridging++;
            p = { x: p.x + prev.x * STEP, y: p.y + prev.y * STEP };
            pts.push({ ...p });
            continue;
          }
          break;
        }
        if (bridging > 0 && !opts.blocked(next)) bridging = 0;
        if (!opts.inDomain(next)) break;
        // stop when crowding an existing streamline of the SAME family
        const dNear = grid.nearest(next, sep);
        if (pts.length > 2 && dNear < sep * 0.55) {
          pts.push(next);
          break;
        }
        pts.push(next);
        prev = dir;
        p = next;
      }
      // trim a failed bridge: never end a street in the water
      while (pts.length > 0 && opts.blocked(pts[pts.length - 1] as Vec2)) pts.pop();
      halves.push(pts);
    }
    const back = halves[1] as Vec2[];
    const fwd = halves[0] as Vec2[];
    const line = [...back.reverse(), { ...seed }, ...fwd];
    if (line.length * STEP < opts.minLength) return null;
    // connect: snap both ends onto the nearest existing road point
    const sepEnd = sepAt(seed);
    for (const endIdx of [0, line.length - 1] as const) {
      const end = line[endIdx] as Vec2;
      const q = snapGrid.nearestPoint(end, sepEnd * 1.3);
      if (q && Math.hypot(q.x - end.x, q.y - end.y) > 8 && !opts.blocked(q)) {
        if (endIdx === 0) line.unshift({ ...q });
        else line.push({ ...q });
      }
    }
    return line;
  };

  let guard = 0;
  while (queue.length > 0 && guard++ < 4000) {
    // random-ish pop keeps growth spatially balanced
    const idx = queue.length > 4 ? rng.int(0, queue.length - 1) : 0;
    const seed = queue.splice(idx, 1)[0] as Vec2;
    for (const eigen of opts.eigenDirs) {
      const line = traceOne(seed, eigen);
      if (!line) continue;
      results.push(line);
      const sep = sepAt(seed);
      for (const p of line) {
        grids[eigen].add(p);
        snapGrid.add(p);
      }
      if (opts.spawnSeeds) {
        // Jobard–Lefer: candidate seeds offset perpendicular to the line
        for (let i = 4; i < line.length - 4; i += 4) {
          const a = line[i - 1] as Vec2;
          const b = line[i + 1] as Vec2;
          const tx = b.x - a.x;
          const ty = b.y - a.y;
          const tl = Math.hypot(tx, ty) || 1;
          for (const side of [1, -1]) {
            const cand = {
              x: (line[i] as Vec2).x + (-ty / tl) * sep * side,
              y: (line[i] as Vec2).y + (tx / tl) * sep * side,
            };
            if (opts.inDomain(cand) && !opts.blocked(cand)) queue.push(cand);
          }
        }
      }
    }
  }
  return results;
}
