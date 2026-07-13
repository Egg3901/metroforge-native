//! Transit assignment — the hybrid model's economic core. Port of
//! `sim/src/core/transit/assignment.ts`.
//!
//! * Demand: gravity-model OD matrix over districts, reshaped by the cohort
//!   hour mix ([`super::cohorts`]).
//! * Assignment: Dijkstra over a (station x route) node graph with walk access,
//!   wait costs (headway/2), transfer penalties. Logit mode split vs car.
//! * Output: every derived stat (ridership, revenue, crowding) comes from these
//!   flows, never from visual agents.
//!
//! LANE NOTE (P3-TRANSIT): the TS source reads several sibling P3 systems not
//! yet ported (`events`, `weatherEffects`, `geologyCost`). Those are provided
//! here as neutral local helpers (see [`deps`]) that reproduce the TS behaviour
//! for the current state (no active events, no weather, surface stations). The
//! integration owner re-points them at the real modules when those lanes land.

use crate::constants::{modes, CROWD_KNEE, CROWD_PENALTY_MIN, TRANSFER_PENALTY_MIN, WALK_SPEED};
use crate::geometry::dist;
use crate::transit::cohorts::{attractor_at, poi_surge, MAX_POI_SURGE};
use crate::transit::grade_effects::{segment_assignment_speed_mps, segment_density01};
use crate::types::{
    District, FieldGrid, FlowResult, GameState, PoiKind, RouteDef, Station, TrackSegment,
};
use std::collections::BTreeMap;

const CAR_SPEED: f64 = 8.3;
const CAR_OVERHEAD_MIN: f64 = 8.0;
const LOGIT_THETA: f64 = 9.0;
const TRIP_RATE: f64 = 0.9;
const DEST_KERNEL: f64 = 3600.0;
const MAX_DESTS_PER_ORIGIN: usize = 14;
const MAX_TRANSIT_COST_MIN: f64 = 90.0;

/// Transit share below which a pair is "unserved" (overlay/gaps). Mirrors
/// `UNSERVED_SHARE_MAX`.
pub const UNSERVED_SHARE_MAX: f64 = 0.35;
/// Ignore trickles so the overlay shows real gaps. Mirrors `MIN_UNSERVED_TRIPS`.
pub const MIN_UNSERVED_TRIPS: f64 = 40.0;
/// Keep the overlay legible. Mirrors `MAX_UNSERVED_LINES`.
pub const MAX_UNSERVED_LINES: usize = 60;

/// Neutral local ports of not-yet-landed sibling P3 systems. See module note.
mod deps {
    use crate::types::GameState;

    /// `events::eventDemandMult`. No active-event demand model exists in the
    /// ported state yet (the `ActiveEvent` placeholder carries no `demandMult`),
    /// so this returns the neutral 1.0. TODO(P3-events): read per-event mults.
    pub fn event_demand_mult(_state: &GameState) -> f64 {
        1.0
    }
    /// `weatherEffects::weatherDemandMult` for `weather = None` -> 1.0.
    pub fn weather_demand_mult(_state: &GameState) -> f64 {
        1.0
    }
    /// `weatherEffects::weatherWalkMult` for `weather = None` -> 1.0.
    pub fn weather_walk_mult(_state: &GameState) -> f64 {
        1.0
    }
    /// `weatherEffects::weatherCarPenaltyMin` for `weather = None` -> 0.0.
    pub fn weather_car_penalty_min(_state: &GameState) -> f64 {
        0.0
    }
    /// `geologyCost::stationDepthAccessPenaltySec`: +30 s per 10 m below 10 m.
    pub fn station_depth_access_penalty_sec(depth: Option<f64>) -> f64 {
        const FREE_M: f64 = 10.0;
        const SEC_PER_10M: f64 = 30.0;
        match depth {
            Some(d) if d > FREE_M => ((d - FREE_M) / 10.0) * SEC_PER_10M,
            _ => 0.0,
        }
    }
}

// ── Assignment graph ──────────────────────────────────────────────────────────

struct NodeEdge {
    to: usize,
    cost: f64,
    #[allow(dead_code)]
    route_id: i64,
}

struct AssignmentGraph {
    edges: Vec<Vec<NodeEdge>>,
    street_node_of: BTreeMap<u32, usize>,
    node_station: Vec<u32>,
    node_route: Vec<i64>,
    node_count: usize,
}

fn build_graph(
    stations: &[Station],
    routes: &[RouteDef],
    tracks: &[TrackSegment],
    fields: &FieldGrid,
) -> AssignmentGraph {
    let mut street_node_of: BTreeMap<u32, usize> = BTreeMap::new();
    let mut node_station: Vec<u32> = Vec::new();
    let mut node_route: Vec<i64> = Vec::new();
    for (i, s) in stations.iter().enumerate() {
        street_node_of.insert(s.id, i);
        node_station.push(s.id);
        node_route.push(-1);
    }
    let station_by_id: BTreeMap<u32, &Station> = stations.iter().map(|s| (s.id, s)).collect();
    let track_by_id: BTreeMap<u32, &TrackSegment> = tracks.iter().map(|t| (t.id, t)).collect();

    // (station, route) nodes. Key = (station_id, route_id).
    let mut route_node: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    let mut n = stations.len();
    for r in routes {
        if r.vehicle_count == 0 {
            continue;
        }
        for &sid in &r.station_ids {
            route_node.entry((sid, r.id)).or_insert_with(|| {
                let idx = n;
                n += 1;
                node_station.push(sid);
                node_route.push(r.id as i64);
                idx
            });
        }
    }

    let mut edges: Vec<Vec<NodeEdge>> = (0..n).map(|_| Vec::new()).collect();

    for r in routes {
        if r.vehicle_count == 0 {
            continue;
        }
        let cfg = modes(r.mode);
        let wait_min = r.headway_seconds / 2.0 / 60.0;
        let crowd_min = (r.crowding - CROWD_KNEE).max(0.0) * CROWD_PENALTY_MIN;
        // board / alight
        for &sid in &r.station_ids {
            let (Some(&street), Some(&rn)) =
                (street_node_of.get(&sid), route_node.get(&(sid, r.id)))
            else {
                continue;
            };
            let depth_access_min = deps::station_depth_access_penalty_sec(
                station_by_id.get(&sid).and_then(|s| s.depth),
            ) / 60.0;
            edges[street].push(NodeEdge {
                to: rn,
                cost: wait_min + TRANSFER_PENALTY_MIN + crowd_min + depth_access_min,
                route_id: r.id as i64,
            });
            edges[rn].push(NodeEdge {
                to: street,
                cost: 0.1,
                route_id: -1,
            });
        }
        // ride edges (both directions)
        for i in 0..r.station_ids.len().saturating_sub(1) {
            let (Some(&a), Some(&b)) = (
                station_by_id.get(&r.station_ids[i]),
                station_by_id.get(&r.station_ids[i + 1]),
            ) else {
                continue;
            };
            let (Some(&na), Some(&nb)) = (
                route_node.get(&(r.station_ids[i], r.id)),
                route_node.get(&(r.station_ids[i + 1], r.id)),
            ) else {
                continue;
            };
            let seg = r.segment_ids.get(i).and_then(|sid| track_by_id.get(sid));
            let len = seg
                .map(|s| s.polyline.length)
                .unwrap_or_else(|| dist(a.pos, b.pos));
            let dens = seg
                .map(|s| segment_density01(Some(fields), s))
                .unwrap_or(0.5);
            let grade = seg
                .map(|s| s.grade)
                .unwrap_or(crate::types::TrackGrade::Surface);
            let spd = segment_assignment_speed_mps(r.mode, grade, dens);
            let ride_min = (len / spd + cfg.dwell_seconds) / 60.0;
            edges[na].push(NodeEdge {
                to: nb,
                cost: ride_min,
                route_id: r.id as i64,
            });
            edges[nb].push(NodeEdge {
                to: na,
                cost: ride_min,
                route_id: r.id as i64,
            });
        }
    }

    AssignmentGraph {
        edges,
        street_node_of,
        node_station,
        node_route,
        node_count: n,
    }
}

// ── Dijkstra min-heap (cost-ordered, FIFO on ties via seq) ────────────────────

#[derive(Clone, Copy)]
struct HeapItem {
    node: usize,
    cost: f64,
    seq: u64,
}
impl PartialEq for HeapItem {
    fn eq(&self, o: &Self) -> bool {
        self.cost == o.cost && self.seq == o.seq
    }
}
impl Eq for HeapItem {}
impl Ord for HeapItem {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        // Reverse for a min-heap on (cost, seq).
        o.cost
            .total_cmp(&self.cost)
            .then_with(|| o.seq.cmp(&self.seq))
    }
}
impl PartialOrd for HeapItem {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

// ── Public output types ───────────────────────────────────────────────────────

/// Car demand per OD pair. Mirrors `CarFlow`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarFlow {
    /// Origin district id.
    pub origin_district: u32,
    /// Destination district id.
    pub dest_district: u32,
    /// Daily car trips on the pair.
    pub car_trips: f64,
}

/// An origin->destination pair that overwhelmingly drives. Mirrors
/// `UnservedDesire`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnservedDesire {
    /// Origin x.
    pub x1: f64,
    /// Origin y.
    pub y1: f64,
    /// Dest x.
    pub x2: f64,
    /// Dest y.
    pub y2: f64,
    /// Daily car trips weight.
    pub weight: f64,
    /// Transit mode share achieved (low = badly served).
    pub share: f64,
}

/// Result bundle of a full assignment pass. Mirrors `AssignmentOutput`. Maps use
/// `BTreeMap` for deterministic iteration.
#[derive(Clone, Debug, Default)]
pub struct AssignmentOutput {
    /// Every assigned OD flow.
    pub flows: Vec<FlowResult>,
    /// Car OD flows for the congestion model.
    pub car_flows: Vec<CarFlow>,
    /// route id -> daily ridership.
    pub route_ridership: BTreeMap<u32, f64>,
    /// route id -> daily revenue.
    pub route_revenue: BTreeMap<u32, f64>,
    /// station id -> boardings.
    pub station_boardings: BTreeMap<u32, f64>,
    /// station id -> alightings.
    pub station_alightings: BTreeMap<u32, f64>,
    /// per-segment load keyed `(routeId, minStationId, maxStationId)`.
    pub segment_load: BTreeMap<(u32, u32, u32), f64>,
    /// Unserved-demand overlay lines.
    pub unserved: Vec<UnservedDesire>,
    /// Total daily transit trips.
    pub daily_transit_trips: f64,
    /// Total daily car trips.
    pub daily_car_trips: f64,
}

/// Fraction of a host district's job attraction a POI adds at full surge.
/// Mirrors `POI_SURGE_FRACTION`.
fn poi_surge_fraction(kind: PoiKind) -> f64 {
    match kind {
        PoiKind::Stadium => 0.6,
        PoiKind::Airport => 0.8,
        PoiKind::University => 0.4,
        PoiKind::Hospital => 0.2,
        PoiKind::Museum => 0.15,
    }
}

/// anchor id -> nearest district id. Mirrors `anchorDistrictMap` (computed
/// fresh; the TS instance cache is an optimization only).
fn anchor_district_map(state: &GameState) -> BTreeMap<String, u32> {
    let mut map = BTreeMap::new();
    let Some(anchors) = &state.poi_anchors else {
        return map;
    };
    for a in anchors {
        let mut best: i64 = -1;
        let mut best_d = f64::INFINITY;
        let (ax, ay) = (a.centroid[0], a.centroid[1]);
        for d in &state.districts {
            let dd = (d.centroid.x - ax).powi(2) + (d.centroid.y - ay).powi(2);
            if dd < best_d {
                best_d = dd;
                best = d.id as i64;
            }
        }
        if best >= 0 {
            map.insert(a.id.clone(), best as u32);
        }
    }
    map
}

/// Additive attractor bump per district id from active POI surges. Mirrors
/// `poiBumpByDistrict`.
fn poi_bump_by_district(state: &GameState) -> BTreeMap<u32, f64> {
    let mut out: BTreeMap<u32, f64> = BTreeMap::new();
    let Some(anchors) = &state.poi_anchors else {
        return out;
    };
    if anchors.is_empty() {
        return out;
    }
    let district_of = anchor_district_map(state);
    let jobs_by_id: BTreeMap<u32, f64> = state.districts.iter().map(|d| (d.id, d.jobs)).collect();
    for a in anchors {
        let Some(&did) = district_of.get(&a.id) else {
            continue;
        };
        let surge = poi_surge(a, state.seed, state.tick);
        if surge <= 1.0 {
            continue;
        }
        let s01 = ((surge - 1.0) / (MAX_POI_SURGE - 1.0)).min(1.0);
        let bump = s01 * poi_surge_fraction(a.kind) * jobs_by_id.get(&did).copied().unwrap_or(0.0);
        *out.entry(did).or_insert(0.0) += bump;
    }
    out
}

/// Run the full transit assignment over `state`. Pure/read-only. Mirrors
/// `runAssignment`. The integration owner wires the resulting maps back into the
/// route/station/flow state and city stats.
pub fn run_assignment(state: &GameState) -> AssignmentOutput {
    let districts = &state.districts;
    let stations = &state.stations;
    let routes = &state.routes;
    let tracks = &state.tracks;
    let fields = &state.fields;

    // Cohort hour-mix reshape (blended toward legacy jobs-only gravity).
    const RS: f64 = 0.5;
    let raw = attractor_at(state.tick);
    let attr_job = 1.0 - RS * (1.0 - raw.job);
    let attr_home = RS * raw.home;
    let attr_leisure = RS * raw.leisure;
    let poi_bump = poi_bump_by_district(state);

    let mut tot_jobs = 0.0;
    let mut tot_pop = 0.0;
    for d in districts {
        tot_jobs += d.jobs;
        tot_pop += d.population;
    }
    let home_scale = if tot_pop > 0.0 {
        tot_jobs / tot_pop
    } else {
        1.0
    };

    let graph = build_graph(stations, routes, tracks, fields);
    let mut out = AssignmentOutput::default();
    let seg_key = |rid: u32, a: u32, b: u32| (rid, a.min(b), a.max(b));

    let record_unserved = |out: &mut AssignmentOutput,
                           origin: &District,
                           dest: &District,
                           pair_trips: f64,
                           share: f64| {
        if pair_trips < MIN_UNSERVED_TRIPS || share >= UNSERVED_SHARE_MAX {
            return;
        }
        out.unserved.push(UnservedDesire {
            x1: origin.centroid.x,
            y1: origin.centroid.y,
            x2: dest.centroid.x,
            y2: dest.centroid.y,
            weight: pair_trips * (1.0 - share),
            share,
        });
    };

    let walk_mult = deps::weather_walk_mult(state);
    // access lists: district id -> [(station id, walk minutes)]
    let mut access: BTreeMap<u32, Vec<(u32, f64)>> = BTreeMap::new();
    for d in districts {
        let mut list: Vec<(u32, f64)> = Vec::new();
        for s in stations {
            let walk_r = modes(s.mode).walk_radius * walk_mult;
            let dd = dist(d.centroid, s.pos);
            if dd <= walk_r {
                list.push((s.id, dd / WALK_SPEED / 60.0));
            }
        }
        list.sort_by(|a, b| a.1.total_cmp(&b.1));
        list.truncate(6);
        access.insert(d.id, list);
    }

    let route_by_id: BTreeMap<u32, &RouteDef> = routes.iter().map(|r| (r.id, r)).collect();
    let fare_of = |rid: u32| route_by_id.get(&rid).map(|r| r.fare).unwrap_or(0.0);

    let demand_mult = deps::event_demand_mult(state)
        * state.global_demand_mult.unwrap_or(1.0)
        * deps::weather_demand_mult(state);
    let car_weather_penalty = deps::weather_car_penalty_min(state);

    // Precompute per-destination attraction (hour reshape is origin-independent).
    let n_d = districts.len();
    let mut dest_attr = vec![0.0f64; n_d];
    let mut dest_ok = vec![false; n_d];
    let has_poi = !poi_bump.is_empty();
    for j in 0..n_d {
        let dest = &districts[j];
        let home_attr = dest.population * home_scale;
        let leisure_attr = home_attr * (0.4 + 0.5 * dest.land_value.min(1.0));
        let mut a = attr_job * dest.jobs + attr_home * home_attr + attr_leisure * leisure_attr;
        if has_poi {
            a += poi_bump.get(&dest.id).copied().unwrap_or(0.0);
        }
        dest_attr[j] = a;
        dest_ok[j] = a >= 20.0;
    }

    for origin in districts {
        if origin.population < 50.0 {
            continue;
        }
        let district_mult = state
            .district_demand_mult
            .as_ref()
            .and_then(|m| m.get(&origin.id).copied())
            .unwrap_or(1.0);
        let origin_trips = origin.population * TRIP_RATE * demand_mult * district_mult;

        let mut dest_weights: Vec<(usize, f64)> = Vec::new();
        for j in 0..n_d {
            if !dest_ok[j] {
                continue;
            }
            let dest = &districts[j];
            if dest.id == origin.id {
                continue;
            }
            let dd = dist(origin.centroid, dest.centroid);
            dest_weights.push((j, dest_attr[j] * (-dd / DEST_KERNEL).exp()));
        }
        // sort by weight desc; TS Array.sort is stable — preserve index order on ties.
        dest_weights.sort_by(|a, b| b.1.total_cmp(&a.1));
        dest_weights.truncate(MAX_DESTS_PER_ORIGIN);
        let w_sum: f64 = dest_weights.iter().map(|(_, w)| *w).sum();
        if w_sum <= 0.0 {
            continue;
        }

        // Dijkstra from origin access stations.
        let mut dist_arr = vec![f64::INFINITY; graph.node_count];
        let mut prev_node = vec![-1i64; graph.node_count];
        let mut prev_route = vec![-1i64; graph.node_count];
        let origin_access = access.get(&origin.id).cloned().unwrap_or_default();
        if !origin_access.is_empty() {
            let mut heap: std::collections::BinaryHeap<HeapItem> =
                std::collections::BinaryHeap::new();
            let mut seq = 0u64;
            for (sid, walk_min) in &origin_access {
                if let Some(&node) = graph.street_node_of.get(sid) {
                    if *walk_min < dist_arr[node] {
                        dist_arr[node] = *walk_min;
                        heap.push(HeapItem {
                            node,
                            cost: *walk_min,
                            seq,
                        });
                        seq += 1;
                    }
                }
            }
            while let Some(HeapItem { node, cost, .. }) = heap.pop() {
                if cost > dist_arr[node] {
                    continue;
                }
                if cost > MAX_TRANSIT_COST_MIN {
                    continue;
                }
                for e in &graph.edges[node] {
                    let nc = cost + e.cost;
                    if nc < dist_arr[e.to] {
                        dist_arr[e.to] = nc;
                        prev_node[e.to] = node as i64;
                        prev_route[e.to] = e.route_id;
                        heap.push(HeapItem {
                            node: e.to,
                            cost: nc,
                            seq,
                        });
                        seq += 1;
                    }
                }
            }
        }

        for &(j, w) in &dest_weights {
            let dest = &districts[j];
            let pair_trips = origin_trips * w / w_sum;
            let car_min = dist(origin.centroid, dest.centroid) / CAR_SPEED / 60.0
                + CAR_OVERHEAD_MIN
                + car_weather_penalty;

            // best egress over dest access stations
            let mut best_cost = f64::INFINITY;
            let mut best_street: i64 = -1;
            let dest_access = access.get(&dest.id).cloned().unwrap_or_default();
            for (sid, walk_min) in &dest_access {
                if let Some(&node) = graph.street_node_of.get(sid) {
                    let c = dist_arr[node] + walk_min;
                    if c < best_cost {
                        best_cost = c;
                        best_street = node as i64;
                    }
                }
            }
            let transit_cost = best_cost - TRANSFER_PENALTY_MIN;

            if best_street < 0 || !transit_cost.is_finite() || transit_cost > MAX_TRANSIT_COST_MIN {
                out.daily_car_trips += pair_trips;
                out.car_flows.push(CarFlow {
                    origin_district: origin.id,
                    dest_district: dest.id,
                    car_trips: pair_trips,
                });
                record_unserved(&mut out, origin, dest, pair_trips, 0.0);
                continue;
            }

            let share = 1.0 / (1.0 + ((transit_cost - car_min) / LOGIT_THETA).exp());
            record_unserved(&mut out, origin, dest, pair_trips, share);
            let transit_trips = pair_trips * share;
            let car_trips = pair_trips - transit_trips;
            if transit_trips < 1.0 {
                out.daily_car_trips += pair_trips;
                out.car_flows.push(CarFlow {
                    origin_district: origin.id,
                    dest_district: dest.id,
                    car_trips: pair_trips,
                });
                continue;
            }
            if car_trips >= 1.0 {
                out.car_flows.push(CarFlow {
                    origin_district: origin.id,
                    dest_district: dest.id,
                    car_trips,
                });
            }

            // path recovery
            let mut route_ids: Vec<u32> = Vec::new();
            let mut station_ids: Vec<u32> = Vec::new();
            let mut node = best_street;
            let mut guard = 0u32;
            while node >= 0 && guard < 512 {
                guard += 1;
                let un = node as usize;
                if graph.node_route[un] == -1 {
                    station_ids.push(graph.node_station[un]);
                }
                let pn = prev_node[un];
                if pn >= 0 {
                    let nr = graph.node_route[un];
                    if nr >= 0 && nr == graph.node_route[pn as usize] {
                        let k = seg_key(
                            nr as u32,
                            graph.node_station[un],
                            graph.node_station[pn as usize],
                        );
                        *out.segment_load.entry(k).or_insert(0.0) += transit_trips;
                    }
                }
                let via_route = prev_route[un];
                if via_route >= 0 && route_ids.last().copied() != Some(via_route as u32) {
                    // TS records on a boarding transition (street -> route node)
                    // or, degenerately, when no route has been recorded yet.
                    let pn = prev_node[un];
                    if (pn >= 0 && graph.node_route[pn as usize] == -1) || route_ids.is_empty() {
                        route_ids.push(via_route as u32);
                    }
                }
                node = prev_node[un];
            }
            station_ids.reverse();
            route_ids.reverse();

            out.flows.push(FlowResult {
                origin_district: origin.id,
                dest_district: dest.id,
                transit_trips,
                car_trips,
                transit_cost,
                route_ids: route_ids.clone(),
                station_ids: station_ids.clone(),
            });

            out.daily_transit_trips += transit_trips;
            out.daily_car_trips += car_trips;
            for &rid in &route_ids {
                *out.route_ridership.entry(rid).or_insert(0.0) += transit_trips;
                *out.route_revenue.entry(rid).or_insert(0.0) += transit_trips * fare_of(rid);
            }
            if let Some(&first_station) = station_ids.first() {
                *out.station_boardings.entry(first_station).or_insert(0.0) += transit_trips;
                if let Some(&last_station) = station_ids.last() {
                    if last_station != first_station {
                        *out.station_alightings.entry(last_station).or_insert(0.0) += transit_trips;
                    }
                }
            }
        }
    }

    out.unserved.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    out.unserved.truncate(MAX_UNSERVED_LINES);
    out
}

/// One OD pair of the station-independent baseline demand field. Mirrors
/// `BaselineDemandPair`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BaselineDemandPair {
    /// Origin district id.
    pub origin_district: u32,
    /// Destination district id.
    pub dest_district: u32,
    /// Full gravity daily trip potential for the pair.
    pub trips: f64,
}

/// Station-independent baseline gravity demand over all qualifying district
/// pairs. Pure/read-only. Mirrors `computeBaselineDemandOd`.
pub fn compute_baseline_demand_od(state: &GameState) -> Vec<BaselineDemandPair> {
    let districts = &state.districts;
    let demand_mult = deps::event_demand_mult(state)
        * state.global_demand_mult.unwrap_or(1.0)
        * deps::weather_demand_mult(state);
    let mut out = Vec::new();
    for origin in districts {
        if origin.population < 50.0 {
            continue;
        }
        let district_mult = state
            .district_demand_mult
            .as_ref()
            .and_then(|m| m.get(&origin.id).copied())
            .unwrap_or(1.0);
        let origin_trips = origin.population * TRIP_RATE * demand_mult * district_mult;

        let mut dest_weights: Vec<(usize, f64)> = Vec::new();
        let mut w_sum = 0.0;
        for (j, dest) in districts.iter().enumerate() {
            if dest.id == origin.id || dest.jobs < 20.0 {
                continue;
            }
            let dd = dist(origin.centroid, dest.centroid);
            let w = dest.jobs * (-dd / DEST_KERNEL).exp();
            dest_weights.push((j, w));
            w_sum += w;
        }
        if w_sum <= 0.0 {
            continue;
        }
        for (j, w) in dest_weights {
            let pair_trips = origin_trips * w / w_sum;
            if pair_trips <= 0.0 {
                continue;
            }
            out.push(BaselineDemandPair {
                origin_district: origin.id,
                dest_district: districts[j].id,
                trips: pair_trips,
            });
        }
    }
    out
}
