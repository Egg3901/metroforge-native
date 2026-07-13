//! Simulation analytics layer -- presentation-only, derived, read-mostly.
//!
//! Port of `sim/src/core/analytics.ts`. Accumulates a per-cell ridership
//! heatmap (station boardings + alightings) and a district<->district OD matrix
//! from assignment output, then derives insight metrics. Never feeds back into
//! the economy or assignment, so it stays OUT of the determinism hash
//! (transient on `GameState`, like traffic). Deterministic given the same
//! assignment outputs and day cadence.
//!
//! NOTE: `build_demand_overlay` (TS) depends on `transit/assignment`
//! (`computeBaselineDemandOd`, `CarFlow`, `UnservedDesire`) owned by the transit
//! lane and is intentionally NOT ported here; wire it once that module lands.
//! A local [`CarFlow`] mirrors the assignment shape so the OD builders stand
//! alone. See the P3-ENV report note on the `types::AnalyticsState` placeholder.

use std::collections::BTreeMap;

use crate::constants::{modes, TICKS_PER_DAY};
use crate::fields::cell_index_at;
use crate::types::{District, FieldGrid, FlowResult, GameState, RouteDef, Station};

/// Rolling temporal window for heatmap + OD smoothing (sim-days).
pub const ANALYTICS_WINDOW_DAYS: usize = 7;
/// Emit a quantized heatmap payload every N completed sim-days.
pub const HEATMAP_EMIT_INTERVAL_DAYS: i64 = 7;
/// Catchment radius for the coverage insight (meters).
pub const CATCHMENT_RADIUS_M: f64 = 400.0;
/// Hard size budget for the encoded heatmap frame.
pub const ANALYTICS_PAYLOAD_BUDGET_BYTES: usize = 50 * 1024;

/// Heatmap wire message type.
pub const HEATMAP_MSG_TYPE: u8 = 6;
/// Heatmap wire version.
pub const HEATMAP_VERSION: u8 = 1;
/// Heatmap header size in bytes.
pub const HEATMAP_HEADER_BYTES: usize = 32;

/// A car-only residual flow (mirrors `transit/assignment.ts::CarFlow`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarFlow {
    /// Origin district id.
    pub origin_district: u32,
    /// Destination district id.
    pub dest_district: u32,
    /// Car trips per day.
    pub car_trips: f64,
}

/// Highest-load route segment (corridor). Mirrors the TS inline shape.
#[derive(Clone, Debug, PartialEq)]
pub struct OverloadedCorridor {
    /// Route id.
    pub route_id: u32,
    /// Route name.
    pub route_name: String,
    /// Boarding-side station id.
    pub from_station_id: u32,
    /// Alighting-side station id.
    pub to_station_id: u32,
    /// Segment daily load.
    pub load: f64,
}

/// Derived insight metrics. Mirrors `AnalyticsInsights`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AnalyticsInsights {
    /// District with highest demand * (1 - transit service); `None` if none.
    pub underserved_district_id: Option<u32>,
    /// Its display name.
    pub underserved_district_name: Option<String>,
    /// Daily trip potential used in the underserved score.
    pub underserved_demand: f64,
    /// Transit mode share for that district's originating trips, 0..1.
    pub underserved_service: f64,
    /// Highest-load corridor.
    pub overloaded_corridor: Option<OverloadedCorridor>,
    /// Daily transit riders per vehicle-kilometre.
    pub network_efficiency: f64,
    /// Fraction of population within [`CATCHMENT_RADIUS_M`] of a served station.
    pub catchment_coverage: f64,
}

/// Empty insights.
pub fn empty_insights() -> AnalyticsInsights {
    AnalyticsInsights::default()
}

/// Rolling analytics accumulator. Mirrors `AnalyticsState` (the transient one).
#[derive(Clone, Debug, Default)]
pub struct AnalyticsState {
    /// Ring buffer of daily per-cell activity grids.
    pub day_heat: Vec<Vec<f32>>,
    /// Ring buffer of daily OD totals keyed `(origin, dest)`.
    pub day_od: Vec<BTreeMap<(u32, u32), f64>>,
    /// Days pushed into the rolling windows.
    pub days_recorded: u64,
    /// Last sim-day a heatmap was emitted.
    pub last_heatmap_day: i64,
    /// Latest derived insights.
    pub insights: AnalyticsInsights,
    /// Pending boardings awaiting day close.
    pub pending_boardings: BTreeMap<u32, f64>,
    /// Pending alightings awaiting day close.
    pub pending_alightings: BTreeMap<u32, f64>,
    /// Pending transit flows.
    pub pending_flows: Vec<FlowResult>,
    /// Pending car residual flows.
    pub pending_car_flows: Vec<CarFlow>,
}

/// Structured heatmap payload. Mirrors `HeatmapPayload` / the binary body.
#[derive(Clone, Debug, PartialEq)]
pub struct HeatmapPayload {
    /// Grid width in cells.
    pub w: u32,
    /// Grid height in cells.
    pub h: u32,
    /// Meters per cell.
    pub cell_size: f64,
    /// World-space X origin.
    pub origin_x: f64,
    /// World-space Y origin.
    pub origin_y: f64,
    /// Raw smoothed activity corresponding to quantized value 255.
    pub max_value: f32,
    /// Sim-day (1-based) when built.
    pub day: i64,
    /// Quantized 0..255, length w*h, row-major.
    pub cells: Vec<u8>,
}

/// Splat station boardings + alightings into a zeroed grid (exact cell deposit).
pub fn splat_station_activity(
    grid: &FieldGrid,
    stations: &[Station],
    boardings: &BTreeMap<u32, f64>,
    alightings: &BTreeMap<u32, f64>,
    out: &mut [f32],
) {
    let n = (grid.w * grid.h) as usize;
    assert_eq!(out.len(), n, "heatmap buffer length {} != {}", out.len(), n);
    out.iter_mut().for_each(|v| *v = 0.0);
    for s in stations {
        let activity = boardings.get(&s.id).copied().unwrap_or(0.0)
            + alightings.get(&s.id).copied().unwrap_or(0.0);
        if activity <= 0.0 {
            continue;
        }
        let idx = cell_index_at(grid, s.pos);
        out[idx] += activity as f32;
    }
}

/// Build a dense OD total-trips map from transit flows + car residuals.
pub fn build_od_totals(flows: &[FlowResult], car_flows: &[CarFlow]) -> BTreeMap<(u32, u32), f64> {
    let mut od: BTreeMap<(u32, u32), f64> = BTreeMap::new();
    for f in flows {
        let trips = f.transit_trips + f.car_trips;
        if trips > 0.0 {
            *od.entry((f.origin_district, f.dest_district))
                .or_insert(0.0) += trips;
        }
    }
    for c in car_flows {
        let k = (c.origin_district, c.dest_district);
        if od.contains_key(&k) {
            continue;
        }
        if c.car_trips > 0.0 {
            *od.entry(k).or_insert(0.0) += c.car_trips;
        }
    }
    od
}

/// Mean of the rolling day grids (empty if none).
pub fn smoothed_heatmap(day_heat: &[Vec<f32>]) -> Vec<f32> {
    if day_heat.is_empty() {
        return Vec::new();
    }
    let n = day_heat[0].len();
    let mut out = vec![0.0f32; n];
    for day in day_heat {
        for (o, d) in out.iter_mut().zip(day.iter()) {
            *o += *d;
        }
    }
    let inv = 1.0 / day_heat.len() as f32;
    out.iter_mut().for_each(|v| *v *= inv);
    out
}

/// Mean OD over the rolling window.
pub fn smoothed_od(day_od: &[BTreeMap<(u32, u32), f64>]) -> BTreeMap<(u32, u32), f64> {
    let mut acc: BTreeMap<(u32, u32), f64> = BTreeMap::new();
    if day_od.is_empty() {
        return acc;
    }
    for day in day_od {
        for (k, v) in day {
            *acc.entry(*k).or_insert(0.0) += *v;
        }
    }
    let inv = 1.0 / day_od.len() as f64;
    acc.values_mut().for_each(|v| *v *= inv);
    acc
}

/// Quantize a smoothed grid to `u8` and report the value mapped to 255.
pub fn quantize_heatmap(smoothed: &[f32]) -> (Vec<u8>, f32) {
    let mut max_value = 0.0f32;
    for &v in smoothed {
        if v > max_value {
            max_value = v;
        }
    }
    let mut cells = vec![0u8; smoothed.len()];
    if max_value <= 0.0 {
        return (cells, 0.0);
    }
    for (c, &v) in cells.iter_mut().zip(smoothed.iter()) {
        *c = ((v / max_value * 255.0).round() as i32).min(255) as u8;
    }
    (cells, max_value)
}

/// Encode the compact quantized heatmap (little-endian; see TS file header).
pub fn encode_heatmap_payload(p: &HeatmapPayload) -> Vec<u8> {
    let cell_count = (p.w * p.h) as usize;
    assert_eq!(
        p.cells.len(),
        cell_count,
        "heatmap cells length {} != {}",
        p.cells.len(),
        cell_count
    );
    let total = HEATMAP_HEADER_BYTES + cell_count;
    assert!(
        total <= ANALYTICS_PAYLOAD_BUDGET_BYTES,
        "heatmap payload {total} B exceeds {ANALYTICS_PAYLOAD_BUDGET_BYTES} B budget"
    );
    let mut buf = Vec::with_capacity(total);
    buf.push(HEATMAP_MSG_TYPE);
    buf.push(HEATMAP_VERSION);
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&p.w.to_le_bytes());
    buf.extend_from_slice(&p.h.to_le_bytes());
    buf.extend_from_slice(&(p.cell_size as f32).to_le_bytes());
    buf.extend_from_slice(&(p.origin_x as f32).to_le_bytes());
    buf.extend_from_slice(&(p.origin_y as f32).to_le_bytes());
    buf.extend_from_slice(&p.max_value.to_le_bytes());
    buf.extend_from_slice(&(p.day as u32).to_le_bytes());
    buf.extend_from_slice(&p.cells);
    buf
}

/// Decode a msgType=6 heatmap frame (for tests / native parity).
pub fn decode_heatmap_payload(buf: &[u8]) -> HeatmapPayload {
    assert_eq!(buf[0], HEATMAP_MSG_TYPE, "not a heatmap payload");
    assert_eq!(
        buf[1], HEATMAP_VERSION,
        "unsupported heatmap version {}",
        buf[1]
    );
    let rd_u32 = |o: usize| u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    let rd_f32 = |o: usize| f32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    let w = rd_u32(4);
    let h = rd_u32(8);
    let cell_count = (w * h) as usize;
    HeatmapPayload {
        w,
        h,
        cell_size: rd_f32(12) as f64,
        origin_x: rd_f32(16) as f64,
        origin_y: rd_f32(20) as f64,
        max_value: rd_f32(24),
        day: rd_u32(28) as i64,
        cells: buf[HEATMAP_HEADER_BYTES..HEATMAP_HEADER_BYTES + cell_count].to_vec(),
    }
}

/// Build a heatmap payload from the current rolling window.
pub fn build_heatmap_payload(state: &GameState, day: i64) -> HeatmapPayload {
    let g = &state.fields;
    let a = state.analytics_ext();
    let smoothed = smoothed_heatmap(&a.day_heat);
    let n = (g.w * g.h) as usize;
    let values = if smoothed.len() == n {
        smoothed
    } else {
        vec![0.0f32; n]
    };
    let (cells, max_value) = quantize_heatmap(&values);
    HeatmapPayload {
        w: g.w,
        h: g.h,
        cell_size: g.cell_size,
        origin_x: g.origin_x,
        origin_y: g.origin_y,
        max_value,
        day,
        cells,
    }
}

/// Daily vehicle-kilometres: each vehicle covers the out-and-back path once per
/// cycle.
pub fn daily_vehicle_km(state: &GameState) -> f64 {
    let mut vkm = 0.0;
    for r in &state.routes {
        if r.vehicle_count == 0 {
            continue;
        }
        let mut one_way = 0.0;
        for seg_id in &r.segment_ids {
            if let Some(seg) = state.tracks.iter().find(|t| t.id == *seg_id) {
                one_way += seg.polyline.length;
            }
        }
        let path_m = one_way * 2.0;
        if path_m <= 0.0 {
            continue;
        }
        let cfg = modes(r.mode);
        let dwell_stops = 2.0 * (r.station_ids.len() as f64 - 1.0).max(1.0);
        let cycle = path_m / cfg.speed + dwell_stops * cfg.dwell_seconds;
        if cycle <= 0.0 {
            continue;
        }
        vkm += f64::from(r.vehicle_count) * (path_m / 1000.0) * (f64::from(TICKS_PER_DAY) / cycle);
    }
    vkm
}

/// Worst underserved district: high originating demand, low transit share.
/// Score = demand * (1 - service); ties break toward lower district id.
pub fn find_underserved_district(
    districts: &[District],
    od: &BTreeMap<(u32, u32), f64>,
    flows: &[FlowResult],
) -> Option<(u32, String, f64, f64)> {
    let mut transit_out: BTreeMap<u32, f64> = BTreeMap::new();
    for f in flows {
        *transit_out.entry(f.origin_district).or_insert(0.0) += f.transit_trips;
    }
    let mut demand_out: BTreeMap<u32, f64> = BTreeMap::new();
    for ((o, _d), v) in od {
        *demand_out.entry(*o).or_insert(0.0) += *v;
    }

    let mut best: Option<(u32, String, f64, f64, f64)> = None;
    for d in districts {
        let demand = demand_out.get(&d.id).copied().unwrap_or(0.0);
        if demand < 1.0 {
            continue;
        }
        let transit = transit_out.get(&d.id).copied().unwrap_or(0.0);
        let service = (transit / demand).min(1.0);
        let score = demand * (1.0 - service);
        let replace = match &best {
            None => true,
            Some((bid, _, _, _, bscore)) => score > *bscore || (score == *bscore && d.id < *bid),
        };
        if replace {
            best = Some((d.id, d.name.clone(), demand, service, score));
        }
    }
    best.map(|(id, name, demand, service, _)| (id, name, demand, service))
}

/// Highest `segment_loads` entry across routes.
pub fn find_overloaded_corridor(routes: &[RouteDef]) -> Option<OverloadedCorridor> {
    let mut best: Option<OverloadedCorridor> = None;
    for r in routes {
        for (i, &load) in r.segment_loads.iter().enumerate() {
            if load <= 0.0 {
                continue;
            }
            let replace = match &best {
                None => true,
                Some(b) => load > b.load || (load == b.load && r.id < b.route_id),
            };
            if replace {
                let (Some(&a), Some(&b2)) = (r.station_ids.get(i), r.station_ids.get(i + 1)) else {
                    continue;
                };
                best = Some(OverloadedCorridor {
                    route_id: r.id,
                    route_name: r.name.clone(),
                    from_station_id: a,
                    to_station_id: b2,
                    load,
                });
            }
        }
    }
    best
}

/// Population share within `radius_m` of any station on an active route.
pub fn catchment_coverage(
    grid: &FieldGrid,
    stations: &[Station],
    routes: &[RouteDef],
    radius_m: f64,
) -> f64 {
    let mut served: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    for r in routes {
        if r.vehicle_count == 0 {
            continue;
        }
        for sid in &r.station_ids {
            served.insert(*sid);
        }
    }
    let active: Vec<&Station> = stations.iter().filter(|s| served.contains(&s.id)).collect();
    let mut covered = 0.0;
    let mut total = 0.0;
    let r2 = radius_m * radius_m;
    for (i, &pop_f) in grid.population.iter().enumerate() {
        let pop = pop_f as f64;
        if pop <= 0.0 {
            continue;
        }
        total += pop;
        let cx = grid.origin_x + ((i as u32 % grid.w) as f64 + 0.5) * grid.cell_size;
        let cy = grid.origin_y + ((i as u32 / grid.w) as f64 + 0.5) * grid.cell_size;
        for s in &active {
            let dx = s.pos.x - cx;
            let dy = s.pos.y - cy;
            if dx * dx + dy * dy <= r2 {
                covered += pop;
                break;
            }
        }
    }
    if total > 0.0 {
        covered / total
    } else {
        0.0
    }
}

/// Compute insight metrics from state + a smoothed OD matrix.
pub fn compute_insights(state: &GameState, od: &BTreeMap<(u32, u32), f64>) -> AnalyticsInsights {
    let under = find_underserved_district(&state.districts, od, &state.flows);
    let vkm = daily_vehicle_km(state);
    let riders = state.stats.daily_transit_trips;
    let (uid, uname, udemand, uservice) = match under {
        Some((id, name, demand, service)) => (Some(id), Some(name), demand, service),
        None => (None, None, 0.0, 0.0),
    };
    AnalyticsInsights {
        underserved_district_id: uid,
        underserved_district_name: uname,
        underserved_demand: udemand,
        underserved_service: uservice,
        overloaded_corridor: find_overloaded_corridor(&state.routes),
        network_efficiency: if vkm > 0.0 { riders / vkm } else { 0.0 },
        catchment_coverage: catchment_coverage(
            &state.fields,
            &state.stations,
            &state.routes,
            CATCHMENT_RADIUS_M,
        ),
    }
}

/// Read-mostly analytics entry point: derive insights from the current state,
/// building the OD matrix from `state.flows` directly. This is the standalone
/// derived-analytics surface the orchestrator calls each analytics day; the
/// rolling accumulator (`AnalyticsState`) is threaded separately once the
/// transient `types::AnalyticsState` slot carries real fields (see report).
pub fn compute(state: &GameState) -> AnalyticsInsights {
    let od = build_od_totals(&state.flows, &[]);
    compute_insights(state, &od)
}

/// Plain-language cues derived from analytics insights.
pub fn analytics_insight_lines(insights: &AnalyticsInsights, limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(name) = &insights.underserved_district_name {
        out.push(format!(
            "{name} has demand but weak service ({}% transit share).",
            (insights.underserved_service * 100.0).round()
        ));
    }
    if let Some(c) = &insights.overloaded_corridor {
        out.push(format!(
            "{} corridor is overloaded ({} daily trips on one segment).",
            c.route_name,
            c.load.round()
        ));
    }
    if insights.network_efficiency > 0.0 {
        out.push(format!(
            "Network efficiency: {:.1} riders per vehicle-km.",
            insights.network_efficiency
        ));
    }
    if insights.catchment_coverage < 0.35 && insights.catchment_coverage > 0.0 {
        out.push(format!(
            "Only {}% of residents live within {}m of a served stop.",
            (insights.catchment_coverage * 100.0).round(),
            CATCHMENT_RADIUS_M as i64
        ));
    }
    out.truncate(limit);
    out
}

// -- Helper trait to reach a caller-owned AnalyticsState ----------------------
// The transient `types::AnalyticsState` slot on `GameState` is a P1 placeholder
// (empty struct). Until the orchestrator swaps it for this module's richer
// `AnalyticsState`, `build_heatmap_payload` reads an empty accumulator so it
// stays callable in isolation. See the P3-ENV report note.
trait AnalyticsExt {
    fn analytics_ext(&self) -> AnalyticsState;
}
impl AnalyticsExt for GameState {
    fn analytics_ext(&self) -> AnalyticsState {
        AnalyticsState::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::vec;
    use crate::types::{FieldGrid, TransitMode};

    fn tiny_grid() -> FieldGrid {
        // 4x4 grid, 100 m cells, origin at 0.
        let n = 16;
        FieldGrid {
            w: 4,
            h: 4,
            cell_size: 100.0,
            origin_x: 0.0,
            origin_y: 0.0,
            terrain: vec![0.0; n],
            water: vec![0; n],
            parks: vec![0; n],
            population: {
                let mut p = vec![0.0f32; n];
                p[0] = 500.0; // cell (0,0) center ~ (50,50)
                p[15] = 300.0; // far corner
                p
            },
            jobs: vec![0.0; n],
            land_value: vec![0.0; n],
            nimby: vec![0.0; n],
        }
    }

    fn station(id: u32, x: f64, y: f64) -> Station {
        Station {
            id,
            name: format!("S{id}"),
            pos: vec(x, y),
            mode: TransitMode::Bus,
            level: 1,
            ridership: 0.0,
            alightings: 0.0,
            build_tick: 0,
            depth: None,
        }
    }

    #[test]
    fn splat_deposits_exact_cells() {
        let g = tiny_grid();
        let stations = vec![station(1, 50.0, 50.0)];
        let mut b = BTreeMap::new();
        b.insert(1u32, 10.0);
        let mut a = BTreeMap::new();
        a.insert(1u32, 5.0);
        let mut out = vec![0.0f32; 16];
        splat_station_activity(&g, &stations, &b, &a, &mut out);
        assert_eq!(out[0], 15.0);
        assert_eq!(out.iter().sum::<f32>(), 15.0);
    }

    #[test]
    fn heatmap_roundtrips() {
        let p = HeatmapPayload {
            w: 4,
            h: 4,
            cell_size: 100.0,
            origin_x: -5.0,
            origin_y: 7.0,
            max_value: 42.0,
            day: 7,
            cells: (0..16).map(|i| i as u8).collect(),
        };
        let enc = encode_heatmap_payload(&p);
        assert_eq!(enc.len(), HEATMAP_HEADER_BYTES + 16);
        let dec = decode_heatmap_payload(&enc);
        assert_eq!(dec, p);
    }

    #[test]
    fn od_totals_no_double_count() {
        let flows = vec![FlowResult {
            origin_district: 1,
            dest_district: 2,
            transit_trips: 10.0,
            car_trips: 5.0,
            transit_cost: 0.0,
            route_ids: vec![],
            station_ids: vec![],
        }];
        let car = vec![
            CarFlow {
                origin_district: 1,
                dest_district: 2,
                car_trips: 99.0,
            }, // dup -> skipped
            CarFlow {
                origin_district: 3,
                dest_district: 4,
                car_trips: 7.0,
            }, // new
        ];
        let od = build_od_totals(&flows, &car);
        assert_eq!(od.get(&(1, 2)), Some(&15.0));
        assert_eq!(od.get(&(3, 4)), Some(&7.0));
    }

    #[test]
    fn quantize_scales_to_255() {
        let (cells, max) = quantize_heatmap(&[0.0, 5.0, 10.0]);
        assert_eq!(max, 10.0);
        assert_eq!(cells, vec![0, 128, 255]);
    }
}
