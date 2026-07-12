/**
 * Road graph over the generated street network. Used so road-running modes
 * (bus, tram) snap stations onto streets and route their tracks along the
 * actual street path instead of cutting across blocks. Derived data — cached
 * per road array, never saved.
 */
import { dist } from '../geometry';
import type { Vec2 } from '../geometry';
import type { RoadEdge } from '../types';

const NODE_SPACING = 70; // meters between graph nodes along a road
const JUNCTION_RADIUS = 40; // nodes of different roads this close are connected

interface RoadGraph {
  nodes: Vec2[];
  /** adjacency: node index -> [neighbor index, cost meters][] */
  adj: [number, number][][];
  cellOf: Map<number, number[]>; // spatial hash cell -> node indices
  cellSize: number;
}

const cache = new WeakMap<RoadEdge[], RoadGraph>();

function hashKey(x: number, y: number, cell: number): number {
  return Math.floor(x / cell) * 73856093 + Math.floor(y / cell) * 19349663;
}

export function getRoadGraph(roads: RoadEdge[]): RoadGraph {
  const hit = cache.get(roads);
  if (hit) return hit;

  const nodes: Vec2[] = [];
  const adj: [number, number][][] = [];
  const cellSize = JUNCTION_RADIUS * 2;
  const cellOf = new Map<number, number[]>();

  const addNode = (p: Vec2): number => {
    const idx = nodes.length;
    nodes.push(p);
    adj.push([]);
    const k = hashKey(p.x, p.y, cellSize);
    const arr = cellOf.get(k);
    if (arr) arr.push(idx);
    else cellOf.set(k, [idx]);
    return idx;
  };
  const link = (a: number, b: number): void => {
    const c = dist(nodes[a] as Vec2, nodes[b] as Vec2);
    (adj[a] as [number, number][]).push([b, c]);
    (adj[b] as [number, number][]).push([a, c]);
  };

  // sample every road polyline at ~NODE_SPACING and chain the samples
  for (const road of roads) {
    const pl = road.polyline;
    if (pl.length < 20) continue;
    let prevIdx = -1;
    const steps = Math.max(1, Math.round(pl.length / NODE_SPACING));
    for (let s = 0; s <= steps; s++) {
      const d = (s / steps) * pl.length;
      // walk cumulative
      let i = 1;
      while (i < pl.cumulative.length - 1 && (pl.cumulative[i] as number) < d) i++;
      const a = pl.points[i - 1] as Vec2;
      const b = pl.points[i] as Vec2;
      const segStart = pl.cumulative[i - 1] as number;
      const segLen = (pl.cumulative[i] as number) - segStart || 1;
      const t = (d - segStart) / segLen;
      const idx = addNode({ x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t });
      if (prevIdx >= 0) link(prevIdx, idx);
      prevIdx = idx;
    }
  }

  // junction links: connect close nodes (covers crossings + snapped joins)
  for (let i = 0; i < nodes.length; i++) {
    const p = nodes[i] as Vec2;
    const cx = Math.floor(p.x / cellSize);
    const cy = Math.floor(p.y / cellSize);
    for (let oy = -1; oy <= 1; oy++) {
      for (let ox = -1; ox <= 1; ox++) {
        const arr = cellOf.get((cx + ox) * 73856093 + (cy + oy) * 19349663);
        if (!arr) continue;
        for (const j of arr) {
          if (j <= i) continue;
          if (dist(p, nodes[j] as Vec2) <= JUNCTION_RADIUS) {
            // avoid duplicating along-road links
            if (!(adj[i] as [number, number][]).some(([n]) => n === j)) link(i, j);
          }
        }
      }
    }
  }

  const graph: RoadGraph = { nodes, adj, cellOf, cellSize };
  cache.set(roads, graph);
  return graph;
}

export function nearestRoadPoint(roads: RoadEdge[], p: Vec2, maxDist: number): Vec2 | null {
  const g = getRoadGraph(roads);
  let best = maxDist * maxDist;
  let bestP: Vec2 | null = null;
  const r = Math.ceil(maxDist / g.cellSize);
  const cx = Math.floor(p.x / g.cellSize);
  const cy = Math.floor(p.y / g.cellSize);
  for (let oy = -r; oy <= r; oy++) {
    for (let ox = -r; ox <= r; ox++) {
      const arr = g.cellOf.get((cx + ox) * 73856093 + (cy + oy) * 19349663);
      if (!arr) continue;
      for (const i of arr) {
        const q = g.nodes[i] as Vec2;
        const d = (q.x - p.x) * (q.x - p.x) + (q.y - p.y) * (q.y - p.y);
        if (d < best) {
          best = d;
          bestP = q;
        }
      }
    }
  }
  return bestP ? { ...bestP } : null;
}

function nearestNode(g: RoadGraph, p: Vec2, maxDist: number): number {
  let best = maxDist * maxDist;
  let bestI = -1;
  const r = Math.ceil(maxDist / g.cellSize);
  const cx = Math.floor(p.x / g.cellSize);
  const cy = Math.floor(p.y / g.cellSize);
  for (let oy = -r; oy <= r; oy++) {
    for (let ox = -r; ox <= r; ox++) {
      const arr = g.cellOf.get((cx + ox) * 73856093 + (cy + oy) * 19349663);
      if (!arr) continue;
      for (const i of arr) {
        const q = g.nodes[i] as Vec2;
        const d = (q.x - p.x) * (q.x - p.x) + (q.y - p.y) * (q.y - p.y);
        if (d < best) {
          best = d;
          bestI = i;
        }
      }
    }
  }
  return bestI;
}

/** A* street path between two points; null if either end is off-network or unreachable. */
export function findRoadPath(roads: RoadEdge[], from: Vec2, to: Vec2): Vec2[] | null {
  const g = getRoadGraph(roads);
  const start = nearestNode(g, from, 300);
  const goal = nearestNode(g, to, 300);
  if (start < 0 || goal < 0) return null;
  const goalP = g.nodes[goal] as Vec2;

  const dScore = new Map<number, number>();
  const prev = new Map<number, number>();
  const open: { i: number; f: number }[] = [{ i: start, f: 0 }];
  dScore.set(start, 0);
  const closed = new Set<number>();
  let guard = 0;
  while (open.length > 0 && guard++ < 60000) {
    // linear extract-min is fine at this scale
    let mi = 0;
    for (let k = 1; k < open.length; k++) if ((open[k] as { f: number }).f < (open[mi] as { f: number }).f) mi = k;
    const { i } = open.splice(mi, 1)[0] as { i: number };
    if (i === goal) break;
    if (closed.has(i)) continue;
    closed.add(i);
    const di = dScore.get(i) as number;
    for (const [j, c] of g.adj[i] as [number, number][]) {
      const nd = di + c;
      if (nd < (dScore.get(j) ?? Infinity)) {
        dScore.set(j, nd);
        prev.set(j, i);
        open.push({ i: j, f: nd + dist(g.nodes[j] as Vec2, goalP) });
      }
    }
  }
  if (!dScore.has(goal)) return null;
  const path: Vec2[] = [];
  let cur = goal;
  let guard2 = 0;
  while (cur !== start && guard2++ < 100000) {
    path.push({ ...(g.nodes[cur] as Vec2) });
    cur = prev.get(cur) as number;
    if (cur === undefined) return null;
  }
  path.push({ ...(g.nodes[start] as Vec2) });
  path.reverse();
  return path;
}
