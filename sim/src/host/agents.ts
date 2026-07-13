/**
 * Visual passenger agents — presentation only (never feed economics or the
 * save/determinism contract). Each agent follows a real journey: WALK legs are
 * routed along the street network to/from stations, RIDE legs follow the actual
 * track between stations. No more floating through buildings.
 *
 * Paths are built once per OD flow and shared by all agents on that flow (with a
 * per-agent offset), so the A* routing cost is bounded regardless of pool size.
 */
import { WALK_SPEED } from '@core/constants';
import { dist } from '@core/geometry';
import type { Vec2 } from '@core/geometry';
import { Rng } from '@core/rng';
import { cohortDemandFactor } from '@core/transit/cohorts';
import { findRoadPath } from '@core/transit/roadGraph';
import type { FlowResult, GameState } from '@core/types';

const MAX_AGENTS = 1600;
const RIDE_SPEED = 16; // m/s visual
const PATH_BUDGET = 160; // max A* route builds per resample

interface FlowPath {
  pts: Vec2[];
  cum: number[]; // arc length to each point
  segPhase: number[]; // phase per segment (0 walk, 1 ride)
  total: number;
}
interface Agent {
  path: FlowPath;
  d: number; // distance along the path
}

export class AgentPool {
  private agents: Agent[] = [];
  private rng = new Rng(0xa9e17);
  buffer = new Float32Array(MAX_AGENTS * 3);
  count = 0;

  resample(state: GameState): void {
    this.agents.length = 0;
    const flows = state.flows;
    if (flows.length === 0) return;
    let totalTrips = 0;
    for (const f of flows) totalTrips += f.transitTrips;
    if (totalTrips <= 0) return;

    const stationById = new Map(state.stations.map((s) => [s.id, s]));
    const districtById = new Map(state.districts.map((d) => [d.id, d]));
    // track geometry by station pair (both directions) for ride legs
    const trackByPair = new Map<string, Vec2[]>();
    for (const t of state.tracks) {
      const p = t.polyline.points;
      trackByPair.set(`${t.fromStationId}:${t.toStationId}`, p);
      trackByPair.set(`${t.toStationId}:${t.fromStationId}`, [...p].reverse());
    }

    const cache = new Map<FlowResult, FlowPath | null>();
    let budget = PATH_BUDGET;
    const buildPath = (f: FlowResult): FlowPath | null => {
      if (cache.has(f)) return cache.get(f) ?? null;
      const origin = districtById.get(f.originDistrict);
      const destD = districtById.get(f.destDistrict);
      const stops = f.stationIds.map((id) => stationById.get(id)).filter((s): s is NonNullable<typeof s> => !!s);
      if (!origin || !destD || stops.length === 0) { cache.set(f, null); return null; }

      // assemble legs: walk o→s0, ride s0→s1→…, walk sN→dest
      const legs: { poly: Vec2[]; phase: number }[] = [];
      const walk = (a: Vec2, b: Vec2): Vec2[] => (budget-- > 0 ? findRoadPath(state.roads, a, b) : null) ?? [a, b];
      legs.push({ poly: walk(origin.centroid, stops[0]!.pos), phase: 0 });
      for (let i = 0; i + 1 < stops.length; i++) {
        const seg = trackByPair.get(`${stops[i]!.id}:${stops[i + 1]!.id}`);
        legs.push({ poly: seg && seg.length >= 2 ? seg : [stops[i]!.pos, stops[i + 1]!.pos], phase: 1 });
      }
      legs.push({ poly: walk(stops[stops.length - 1]!.pos, destD.centroid), phase: 0 });

      const pts: Vec2[] = [];
      const segPhase: number[] = [];
      for (const leg of legs) {
        for (let k = 0; k < leg.poly.length; k++) {
          const pt = leg.poly[k] as Vec2;
          if (pts.length === 0) { pts.push(pt); continue; }
          if (dist(pts[pts.length - 1] as Vec2, pt) < 1) continue; // dedupe join points
          pts.push(pt);
          segPhase.push(leg.phase);
        }
      }
      if (pts.length < 2) { cache.set(f, null); return null; }
      const cum = [0];
      let total = 0;
      for (let i = 1; i < pts.length; i++) { total += dist(pts[i - 1] as Vec2, pts[i] as Vec2); cum.push(total); }
      const fp: FlowPath = { pts, cum, segPhase, total };
      cache.set(f, fp);
      return fp;
    };

    const weights = flows.map((x) => x.transitTrips);
    // Schedule-driven crowd size: scale the sampled population by the cohort
    // time-of-day factor so station crowds swell at the 8am peak and thin at
    // 2am, while the assignment flows (economics) are untouched. Still sampled/
    // instanced and MAX_AGENTS-capped; the render tier caps counts further.
    const todScale = Math.max(0.15, Math.min(1.8, cohortDemandFactor(state.tick)));
    const target = Math.min(MAX_AGENTS, Math.round((totalTrips / 35) * todScale));
    for (let i = 0; i < target; i++) {
      const f = flows[this.rng.weighted(weights)];
      if (!f) continue;
      const path = buildPath(f);
      if (!path) continue;
      this.agents.push({ path, d: this.rng.next() * path.total });
    }
  }

  update(dtGameSeconds: number): void {
    let idx = 0;
    for (const a of this.agents) {
      const fp = a.path;
      // segment at current distance
      let seg = 1;
      while (seg < fp.cum.length - 1 && (fp.cum[seg] as number) < a.d) seg++;
      const phase = fp.segPhase[seg - 1] ?? 0;
      const speed = phase === 1 ? RIDE_SPEED : WALK_SPEED * 2.4;
      a.d += speed * dtGameSeconds;
      if (a.d >= fp.total) { a.d -= fp.total; seg = 1; }
      while (seg < fp.cum.length - 1 && (fp.cum[seg] as number) < a.d) seg++;
      const d0 = fp.cum[seg - 1] as number;
      const segLen = (fp.cum[seg] as number) - d0 || 1;
      const t = Math.max(0, Math.min(1, (a.d - d0) / segLen));
      const p0 = fp.pts[seg - 1] as Vec2;
      const p1 = fp.pts[seg] as Vec2;
      this.buffer[idx * 3] = p0.x + (p1.x - p0.x) * t;
      this.buffer[idx * 3 + 1] = p0.y + (p1.y - p0.y) * t;
      this.buffer[idx * 3 + 2] = fp.segPhase[seg - 1] ?? 0;
      idx++;
    }
    this.count = idx;
  }
}
