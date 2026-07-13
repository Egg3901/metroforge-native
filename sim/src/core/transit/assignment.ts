/**
 * Transit assignment — the hybrid model's economic core.
 *
 * Demand: gravity-model OD matrix over districts.
 * Assignment: Dijkstra over a (station × route) node graph with walk access,
 * wait costs (headway/2), and transfer penalties. Logit mode split vs car.
 * Output: FlowResult[] — every derived stat (ridership, revenue, crowding)
 * comes from these flows, never from visual agents.
 */
import { CROWD_KNEE, CROWD_PENALTY_MIN, MODES, TRANSFER_PENALTY_MIN, WALK_SPEED } from '../constants';
import { dist } from '../geometry';
import { eventDemandMult } from '../events';
import { weatherCarPenaltyMin, weatherDemandMult, weatherWalkMult } from '../weatherEffects';
import { stationDepthAccessPenaltySec } from '../geologyCost';
import { segmentAssignmentSpeedMps, segmentDensity01 } from './gradeEffects';
import { attractorAt, MAX_POI_SURGE, poiSurge } from './cohorts';
import type { District, FlowResult, GameState, PoiAnchor, RouteDef, Station, TrackSegment } from '../types';

const CAR_SPEED = 8.3; // m/s effective urban driving
const CAR_OVERHEAD_MIN = 8; // parking, access
const LOGIT_THETA = 9; // minutes; mode-choice sensitivity
const TRIP_RATE = 0.9; // transit-relevant trips per resident per day
const DEST_KERNEL = 3600; // meters, destination-choice distance decay
const MAX_DESTS_PER_ORIGIN = 14;
const MAX_TRANSIT_COST_MIN = 90; // beyond this nobody rides
/** pairs served worse than this transit share are "unserved" (overlay/gaps). */
export const UNSERVED_SHARE_MAX = 0.35;
/** ignore trickles so the overlay shows real gaps. */
export const MIN_UNSERVED_TRIPS = 40;
/** keep the overlay legible. */
export const MAX_UNSERVED_LINES = 60;

interface NodeEdge {
  to: number;
  cost: number; // minutes
  routeId: number; // -1 for walk/alight edges
}

interface AssignmentGraph {
  /** node 0..S-1 = "street" node per station; then (station,route) nodes */
  edges: NodeEdge[][];
  streetNodeOf: Map<number, number>; // stationId -> street node index
  nodeStation: number[]; // node index -> stationId
  nodeRoute: number[]; // node index -> routeId (-1 for street)
  nodeCount: number;
}

function buildGraph(
  stations: Station[],
  routes: RouteDef[],
  tracks: TrackSegment[],
  fields: GameState['fields'],
): AssignmentGraph {
  const streetNodeOf = new Map<number, number>();
  const nodeStation: number[] = [];
  const nodeRoute: number[] = [];
  stations.forEach((s, i) => {
    streetNodeOf.set(s.id, i);
    nodeStation.push(s.id);
    nodeRoute.push(-1);
  });
  const stationById = new Map(stations.map((s) => [s.id, s]));
  const trackById = new Map(tracks.map((t) => [t.id, t]));

  // (station, route) nodes
  const routeNode = new Map<string, number>();
  let n = stations.length;
  for (const r of routes) {
    if (r.vehicleCount === 0) continue;
    for (const sid of r.stationIds) {
      const key = `${sid}:${r.id}`;
      if (!routeNode.has(key)) {
        routeNode.set(key, n++);
        nodeStation.push(sid);
        nodeRoute.push(r.id);
      }
    }
  }

  const edges: NodeEdge[][] = Array.from({ length: n }, () => []);

  for (const r of routes) {
    if (r.vehicleCount === 0) continue;
    const cfg = MODES[r.mode];
    const waitMin = r.headwaySeconds / 2 / 60;
    // crowding discomfort from the PREVIOUS assignment's load (lagged, stable):
    // an over-capacity line is less attractive, so riders divert to alternates
    // or the car until it settles.
    const crowdMin = Math.max(0, (r.crowding || 0) - CROWD_KNEE) * CROWD_PENALTY_MIN;
    // board / alight
    for (const sid of r.stationIds) {
      const street = streetNodeOf.get(sid);
      const rn = routeNode.get(`${sid}:${r.id}`);
      if (street === undefined || rn === undefined) continue;
      // deep underground stations add access time (stairs/escalators/lifts):
      // +30 s per 10 m below 10 m (see geologyCost.ts). Surface stops pay 0.
      const depthAccessMin = stationDepthAccessPenaltySec(stationById.get(sid)?.depth) / 60;
      // boarding cost carries the transfer penalty + crowding discomfort; one
      // transfer penalty is refunded at the end (first boarding isn't a transfer)
      edges[street]!.push({ to: rn, cost: waitMin + TRANSFER_PENALTY_MIN + crowdMin + depthAccessMin, routeId: r.id });
      edges[rn]!.push({ to: street, cost: 0.1, routeId: -1 });
    }
    // ride edges (both directions — vehicles run out-and-back)
    for (let i = 0; i + 1 < r.stationIds.length; i++) {
      const a = stationById.get(r.stationIds[i] as number);
      const b = stationById.get(r.stationIds[i + 1] as number);
      const na = routeNode.get(`${r.stationIds[i]}:${r.id}`);
      const nb = routeNode.get(`${r.stationIds[i + 1]}:${r.id}`);
      if (!a || !b || na === undefined || nb === undefined) continue;
      // Grade-aware ride time: a surface segment in a dense corridor rides slower
      // at the peak (when demand/crowding is measured), so grade separation shows
      // up in the mode split. Elevated/tunnel keep full mode cruise.
      const seg = trackById.get(r.segmentIds[i] as number);
      const len = seg?.polyline.length ?? dist(a.pos, b.pos);
      const dens = seg ? segmentDensity01(fields, seg) : 0.5;
      const grade = seg?.grade ?? 'surface';
      const spd = segmentAssignmentSpeedMps(r.mode, grade, dens);
      const rideMin = (len / spd + cfg.dwellSeconds) / 60;
      edges[na]!.push({ to: nb, cost: rideMin, routeId: r.id });
      edges[nb]!.push({ to: na, cost: rideMin, routeId: r.id });
    }
  }

  return { edges, streetNodeOf, nodeStation, nodeRoute, nodeCount: n };
}

/** Binary min-heap keyed on cost. */
class MinHeap {
  private nodes: number[] = [];
  private costs: number[] = [];
  get size(): number {
    return this.nodes.length;
  }
  push(node: number, cost: number): void {
    this.nodes.push(node);
    this.costs.push(cost);
    let i = this.nodes.length - 1;
    while (i > 0) {
      const p = (i - 1) >> 1;
      if ((this.costs[p] as number) <= (this.costs[i] as number)) break;
      this.swap(i, p);
      i = p;
    }
  }
  pop(): { node: number; cost: number } {
    const node = this.nodes[0] as number;
    const cost = this.costs[0] as number;
    const lastN = this.nodes.pop() as number;
    const lastC = this.costs.pop() as number;
    if (this.nodes.length > 0) {
      this.nodes[0] = lastN;
      this.costs[0] = lastC;
      let i = 0;
      for (;;) {
        const l = 2 * i + 1;
        const r = l + 1;
        let m = i;
        if (l < this.nodes.length && (this.costs[l] as number) < (this.costs[m] as number)) m = l;
        if (r < this.nodes.length && (this.costs[r] as number) < (this.costs[m] as number)) m = r;
        if (m === i) break;
        this.swap(i, m);
        i = m;
      }
    }
    return { node, cost };
  }
  private swap(a: number, b: number): void {
    const tn = this.nodes[a] as number;
    this.nodes[a] = this.nodes[b] as number;
    this.nodes[b] = tn;
    const tc = this.costs[a] as number;
    this.costs[a] = this.costs[b] as number;
    this.costs[b] = tc;
  }
}

/** Car demand per OD pair — every trip that drives, so the congestion model
 *  sees the whole road load, not just corridors that happen to carry transit. */
export interface CarFlow {
  originDistrict: number;
  destDistrict: number;
  carTrips: number;
}

/** An origin→destination pair whose trips overwhelmingly drive because transit
 *  serves them poorly (no path, or the path is far slower than driving). The
 *  weight is the daily car trips on the pair; `share` is the transit mode share
 *  achieved (low = badly served). Drives the unserved-demand overlay. */
export interface UnservedDesire {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  weight: number;
  share: number;
}

export interface AssignmentOutput {
  flows: FlowResult[];
  carFlows: CarFlow[];
  routeRidership: Map<number, number>;
  routeRevenue: Map<number, number>;
  stationBoardings: Map<number, number>;
  stationAlightings: Map<number, number>;
  /** per-segment load keyed `${routeId}:${minStationId}:${maxStationId}` */
  segmentLoad: Map<string, number>;
  unserved: UnservedDesire[];
  dailyTransitTrips: number;
  dailyCarTrips: number;
}

/** Fraction of its HOST district's own job attraction that a POI anchor adds at
 *  full surge. Kept proportional (not an absolute jobs count) so a game-day
 *  spike is a real, visible bump on that district without dominating the whole
 *  city or starving distant lines. At baseline surge (=1) the bump is 0, so POIs
 *  are invisible to the assignment except inside their event windows. */
const POI_SURGE_FRACTION: Record<PoiAnchor['kind'], number> = {
  stadium: 0.6,
  airport: 0.8,
  university: 0.4,
  hospital: 0.2,
  museum: 0.15,
};

/** anchor → nearest district id, cached per game instance (static geometry). */
const anchorDistrictCache = new Map<number, Map<string, number>>();

function anchorDistrictMap(state: GameState): Map<string, number> {
  const cached = anchorDistrictCache.get(state.instanceId);
  if (cached) return cached;
  const map = new Map<string, number>();
  for (const a of state.poiAnchors ?? []) {
    let best = -1;
    let bestD = Infinity;
    const ax = a.centroid[0];
    const ay = a.centroid[1];
    for (const d of state.districts) {
      const dd = (d.centroid.x - ax) ** 2 + (d.centroid.y - ay) ** 2;
      if (dd < bestD) {
        bestD = dd;
        best = d.id;
      }
    }
    if (best >= 0) map.set(a.id, best);
  }
  anchorDistrictCache.set(state.instanceId, map);
  return map;
}

/** Additive attractor bump per district id from POI surges active at the current
 *  tick. Empty when no anchor is surging (baseline is untouched). */
function poiBumpByDistrict(state: GameState): Map<number, number> {
  const out = new Map<number, number>();
  const anchors = state.poiAnchors;
  if (!anchors || anchors.length === 0) return out;
  const districtOf = anchorDistrictMap(state);
  const jobsById = new Map(state.districts.map((d) => [d.id, d.jobs]));
  for (const a of anchors) {
    const did = districtOf.get(a.id);
    if (did === undefined) continue;
    const surge = poiSurge(a, state.seed, state.tick);
    if (surge <= 1) continue;
    // normalize surge into [0,1] of its range, then take that fraction of the
    // host district's own jobs — a bounded, proportional event spike.
    const s01 = Math.min(1, (surge - 1) / (MAX_POI_SURGE - 1));
    const bump = s01 * POI_SURGE_FRACTION[a.kind] * (jobsById.get(did) ?? 0);
    out.set(did, (out.get(did) ?? 0) + bump);
  }
  return out;
}

export function runAssignment(state: GameState): AssignmentOutput {
  const { districts, stations, routes, tracks, fields } = state;
  // Cohort+hour destination-pull mix (AM→jobs/CBD, PM→home, weekend→leisure) and
  // any POI event surges active right now. Origin trip *magnitude* is unchanged
  // (daily scale), so only the OD *direction/shape* moves by hour — the daily
  // economy stays stable while lines swing between the AM and PM commute.
  // Reshape strength: blend the cohort hour mix toward the legacy jobs-only
  // gravity. <1 keeps job-directed demand dominant so tight scenarios stay
  // winnable, while still giving a visible AM→CBD / PM→home swing.
  const RS = 0.5;
  const raw = attractorAt(state.tick);
  const attractor = {
    job: 1 - RS * (1 - raw.job),
    home: RS * raw.home,
    leisure: RS * raw.leisure,
  };
  const poiBump = poiBumpByDistrict(state);
  // Home/leisure attractors are built from population, which runs ~10× jobs; the
  // blended pull weights (attractor.*) sum to 1, so to keep TOTAL destination
  // attraction — and thus overall served ridership + the daily economy — on the
  // same scale as the old jobs-only gravity, rescale population into
  // jobs-equivalent units. This makes the hour blend shift demand's DIRECTION
  // (AM→jobs, PM→home) without inflating or gutting its magnitude.
  let totJobs = 0;
  let totPop = 0;
  for (const d of districts) {
    totJobs += d.jobs;
    totPop += d.population;
  }
  const homeScale = totPop > 0 ? totJobs / totPop : 1;
  const graph = buildGraph(stations, routes, tracks, fields);
  const flows: FlowResult[] = [];
  const carFlows: CarFlow[] = [];
  const routeRidership = new Map<number, number>();
  const routeRevenue = new Map<number, number>();
  const stationBoardings = new Map<number, number>();
  const stationAlightings = new Map<number, number>();
  const segmentLoad = new Map<string, number>();
  const unserved: UnservedDesire[] = [];
  const segKey = (rid: number, a: number, b: number): string => `${rid}:${Math.min(a, b)}:${Math.max(a, b)}`;
  let dailyTransitTrips = 0;
  let dailyCarTrips = 0;

  const recordUnserved = (origin: District, dest: District, pairTrips: number, share: number): void => {
    if (pairTrips < MIN_UNSERVED_TRIPS || share >= UNSERVED_SHARE_MAX) return;
    unserved.push({
      x1: origin.centroid.x, y1: origin.centroid.y,
      x2: dest.centroid.x, y2: dest.centroid.y,
      weight: pairTrips * (1 - share), share,
    });
  };

  // weather shrinks how far people will walk to a stop (rain ~-15%, snow more)
  const walkMult = weatherWalkMult(state.weather);
  // access lists: district -> [(stationId, walkMinutes)]
  const access = new Map<number, { stationId: number; walkMin: number }[]>();
  for (const d of districts) {
    const list: { stationId: number; walkMin: number }[] = [];
    for (const s of stations) {
      const walkR = MODES[s.mode].walkRadius * walkMult;
      const dd = dist(d.centroid, s.pos);
      if (dd <= walkR) list.push({ stationId: s.id, walkMin: dd / WALK_SPEED / 60 });
    }
    list.sort((a, b) => a.walkMin - b.walkMin);
    access.set(d.id, list.slice(0, 6));
  }

  const routeById = new Map(routes.map((r) => [r.id, r]));
  const fareOf = (rid: number): number => routeById.get(rid)?.fare ?? 0;

  // citywide demand multiplier from active events (festivals, fuel spikes, …)
  // plus optional scenario global / per-district multipliers (data-driven beats)
  // plus the weather (fewer trips in rain/snow/storm).
  const demandMult =
    eventDemandMult(state.activeEvents) * (state.globalDemandMult ?? 1) * weatherDemandMult(state.weather);
  // weather makes driving worse → generalized-cost minutes added to every car
  // trip, which nudges the logit mode split toward transit.
  const carWeatherPenalty = weatherCarPenaltyMin(state.weather);
  // PERF: the cohort-blended destination attractor depends only on the hour +
  // the destination (not the origin), so precompute it ONCE per assignment
  // rather than inside the O(origins×dests) loop. The per-origin work then
  // stays at baseline cost (one distance + exp per pair), and the hour reshape
  // adds no per-pair arithmetic. Indexed by destination array position.
  const nD = districts.length;
  const destAttr = new Float64Array(nD);
  const destOk = new Uint8Array(nD);
  const hasPoi = poiBump.size > 0;
  for (let j = 0; j < nD; j++) {
    const dest = districts[j] as District;
    // jobs pull in the AM, home (population) pulls in the PM reversal, leisure
    // pull rises evenings/weekends. POI surges add a bounded bump to their host
    // district during event windows only.
    const homeAttr = dest.population * homeScale;
    const leisureAttr = homeAttr * (0.4 + 0.5 * Math.min(1, dest.landValue));
    let a = attractor.job * dest.jobs + attractor.home * homeAttr + attractor.leisure * leisureAttr;
    if (hasPoi) a += poiBump.get(dest.id) ?? 0;
    destAttr[j] = a;
    destOk[j] = a >= 20 ? 1 : 0; // negligible-destination gate (replaces jobs<20)
  }

  // destination choice weights per origin (gravity)
  for (const origin of districts) {
    if (origin.population < 50) continue;
    const districtMult = state.districtDemandMult?.[origin.id] ?? 1;
    const originTrips = origin.population * TRIP_RATE * demandMult * districtMult;

    const destWeights: { d: District; w: number }[] = [];
    for (let j = 0; j < nD; j++) {
      if (destOk[j] === 0) continue;
      const dest = districts[j] as District;
      if (dest.id === origin.id) continue;
      const dd = dist(origin.centroid, dest.centroid);
      destWeights.push({ d: dest, w: (destAttr[j] as number) * Math.exp(-dd / DEST_KERNEL) });
    }
    destWeights.sort((a, b) => b.w - a.w);
    const top = destWeights.slice(0, MAX_DESTS_PER_ORIGIN);
    let wSum = 0;
    for (const t of top) wSum += t.w;
    if (wSum <= 0) continue;

    // Dijkstra from origin's access stations
    const originAccess = access.get(origin.id) ?? [];
    const distArr = new Float64Array(graph.nodeCount).fill(Infinity);
    const prevNode = new Int32Array(graph.nodeCount).fill(-1);
    const prevRoute = new Int32Array(graph.nodeCount).fill(-1);
    if (originAccess.length > 0) {
      const heap = new MinHeap();
      for (const a of originAccess) {
        const node = graph.streetNodeOf.get(a.stationId);
        if (node === undefined) continue;
        if (a.walkMin < (distArr[node] as number)) {
          distArr[node] = a.walkMin;
          heap.push(node, a.walkMin);
        }
      }
      while (heap.size > 0) {
        const { node, cost } = heap.pop();
        if (cost > (distArr[node] as number)) continue;
        if (cost > MAX_TRANSIT_COST_MIN) continue;
        for (const e of graph.edges[node] as NodeEdge[]) {
          const nc = cost + e.cost;
          if (nc < (distArr[e.to] as number)) {
            distArr[e.to] = nc;
            prevNode[e.to] = node;
            prevRoute[e.to] = e.routeId;
            heap.push(e.to, nc);
          }
        }
      }
    }

    for (const { d: dest, w } of top) {
      const pairTrips = (originTrips * w) / wSum;
      const carMin = dist(origin.centroid, dest.centroid) / CAR_SPEED / 60 + CAR_OVERHEAD_MIN + carWeatherPenalty;

      // best egress over dest access stations
      let bestCost = Infinity;
      let bestStreet = -1;
      const destAccess = access.get(dest.id) ?? [];
      for (const a of destAccess) {
        const node = graph.streetNodeOf.get(a.stationId);
        if (node === undefined) continue;
        const c = (distArr[node] as number) + a.walkMin;
        if (c < bestCost) {
          bestCost = c;
          bestStreet = node;
        }
      }
      // refund one transfer penalty (the first boarding isn't a transfer)
      const transitCost = bestCost - TRANSFER_PENALTY_MIN;

      if (bestStreet < 0 || !isFinite(transitCost) || transitCost > MAX_TRANSIT_COST_MIN) {
        dailyCarTrips += pairTrips;
        carFlows.push({ originDistrict: origin.id, destDistrict: dest.id, carTrips: pairTrips });
        recordUnserved(origin, dest, pairTrips, 0);
        continue;
      }

      // logit split
      const share = 1 / (1 + Math.exp((transitCost - carMin) / LOGIT_THETA));
      recordUnserved(origin, dest, pairTrips, share);
      const transitTrips = pairTrips * share;
      const carTrips = pairTrips - transitTrips;
      if (transitTrips < 1) {
        dailyCarTrips += pairTrips;
        carFlows.push({ originDistrict: origin.id, destDistrict: dest.id, carTrips: pairTrips });
        continue;
      }
      if (carTrips >= 1) carFlows.push({ originDistrict: origin.id, destDistrict: dest.id, carTrips });

      // path recovery: walk back through prev pointers, collect route boardings
      const routeIds: number[] = [];
      const stationIds: number[] = [];
      let node = bestStreet;
      let guard = 0;
      while (node >= 0 && guard++ < 512) {
        if ((graph.nodeRoute[node] as number) === -1) stationIds.push(graph.nodeStation[node] as number);
        // ride edge: node and its predecessor are route nodes on the same route
        const pn = prevNode[node] as number;
        if (pn >= 0) {
          const nr = graph.nodeRoute[node] as number;
          if (nr >= 0 && nr === (graph.nodeRoute[pn] as number)) {
            const k = segKey(nr, graph.nodeStation[node] as number, graph.nodeStation[pn] as number);
            segmentLoad.set(k, (segmentLoad.get(k) ?? 0) + transitTrips);
          }
        }
        const viaRoute = prevRoute[node] as number;
        if (viaRoute >= 0 && routeIds[routeIds.length - 1] !== viaRoute) {
          // record on boarding transitions only (street -> route node)
          const pn = prevNode[node] as number;
          if (pn >= 0 && (graph.nodeRoute[pn] as number) === -1) routeIds.push(viaRoute);
          else if (routeIds.length === 0) routeIds.push(viaRoute);
        }
        node = prevNode[node] as number;
      }
      stationIds.reverse();
      routeIds.reverse();

      flows.push({
        originDistrict: origin.id,
        destDistrict: dest.id,
        transitTrips,
        carTrips,
        transitCost,
        routeIds,
        stationIds,
      });

      dailyTransitTrips += transitTrips;
      dailyCarTrips += carTrips;
      for (const rid of routeIds) {
        routeRidership.set(rid, (routeRidership.get(rid) ?? 0) + transitTrips);
        routeRevenue.set(rid, (routeRevenue.get(rid) ?? 0) + transitTrips * fareOf(rid));
      }
      const firstStation = stationIds[0];
      if (firstStation !== undefined) {
        stationBoardings.set(firstStation, (stationBoardings.get(firstStation) ?? 0) + transitTrips);
      }
      const lastStation = stationIds[stationIds.length - 1];
      if (lastStation !== undefined && lastStation !== firstStation) {
        stationAlightings.set(lastStation, (stationAlightings.get(lastStation) ?? 0) + transitTrips);
      }
    }
  }

  // keep only the biggest gaps so the overlay stays readable
  unserved.sort((a, b) => b.weight - a.weight);
  unserved.length = Math.min(unserved.length, MAX_UNSERVED_LINES);

  return { flows, carFlows, routeRidership, routeRevenue, stationBoardings, stationAlightings, segmentLoad, unserved, dailyTransitTrips, dailyCarTrips };
}

/** One origin→destination pair of the station-independent baseline demand field. */
export interface BaselineDemandPair {
  originDistrict: number;
  destDistrict: number;
  /** full gravity daily trip potential for the pair (jobs × distance decay). */
  trips: number;
}

/**
 * Station-independent baseline gravity demand over ALL qualifying district
 * pairs. Pure/read-only: it writes nothing to `state` and is never fed back
 * into the sim (analytics/overlay use only), so it is safe outside the
 * determinism hash.
 *
 * Unlike {@link runAssignment}, this does NOT cap destinations at
 * MAX_DESTS_PER_ORIGIN and does NOT require a transit path to exist — it is the
 * raw demand potential from population × jobs / distance decay, so demand shows
 * everywhere it exists, not just near stations the router enumerated.
 */
export function computeBaselineDemandOd(state: GameState): BaselineDemandPair[] {
  const { districts } = state;
  const demandMult =
    eventDemandMult(state.activeEvents) * (state.globalDemandMult ?? 1) * weatherDemandMult(state.weather);
  const out: BaselineDemandPair[] = [];
  for (const origin of districts) {
    if (origin.population < 50) continue;
    const districtMult = state.districtDemandMult?.[origin.id] ?? 1;
    const originTrips = origin.population * TRIP_RATE * demandMult * districtMult;

    const destWeights: { d: District; w: number }[] = [];
    let wSum = 0;
    for (const dest of districts) {
      if (dest.id === origin.id || dest.jobs < 20) continue;
      const dd = dist(origin.centroid, dest.centroid);
      const w = dest.jobs * Math.exp(-dd / DEST_KERNEL);
      destWeights.push({ d: dest, w });
      wSum += w;
    }
    if (wSum <= 0) continue;
    for (const { d: dest, w } of destWeights) {
      const pairTrips = (originTrips * w) / wSum;
      if (pairTrips <= 0) continue;
      out.push({ originDistrict: origin.id, destDistrict: dest.id, trips: pairTrips });
    }
  }
  return out;
}
