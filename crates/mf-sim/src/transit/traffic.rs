//! Road congestion model. Port of `sim/src/core/transit/traffic.ts`.
//!
//! Turns the gravity model's leftover CAR trips into a spatial congestion field
//! plus a short list of bottleneck hotspots. Presentation/analytics only, never
//! fed back into the economy, so it is a TRANSIENT output outside the
//! determinism hash.
//!
//! LANE NOTE (P3-TRANSIT): the TS side caches the road-capacity field and the
//! fixed congestion reference (`refCache`) per road array so the overlay shows
//! ABSOLUTE load across rush/night recomputes. The Rust port recomputes both per
//! call (no interior-mutability cache); the reference therefore reflects the
//! current call's flows. This is an overlay-only behavioural difference on a
//! non-hashed output. The integration owner may add a per-network cache later.

use crate::transit::assignment::CarFlow;
use crate::types::{GameState, RoadClass};
use std::collections::BTreeMap;

/// A congestion hotspot. Mirrors `TrafficHotspot`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrafficHotspot {
    /// World x.
    pub x: f64,
    /// World y.
    pub y: f64,
    /// 0..1 congestion at the peak.
    pub severity: f64,
}

/// The full congestion field. Mirrors `TrafficField` (transit/traffic.ts). This
/// is the real shape; `types::TrafficField` is a P1 placeholder the integration
/// owner replaces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrafficFieldOut {
    /// Grid width.
    pub w: u32,
    /// Grid height.
    pub h: u32,
    /// Meters per cell.
    pub cell_size: f64,
    /// World-space x origin.
    pub origin_x: f64,
    /// World-space y origin.
    pub origin_y: f64,
    /// Per-cell congestion 0..1.
    pub values: Vec<f32>,
    /// Bottleneck hotspots, worst first.
    pub hotspots: Vec<TrafficHotspot>,
}

/// Per-class capacity contribution. Mirrors `CLASS_CAPACITY`.
fn class_capacity(cls: RoadClass) -> f32 {
    match cls {
        RoadClass::Arterial => 6.0,
        RoadClass::Collector => 3.5,
        RoadClass::Local => 1.4,
    }
}

/// In-place 3x3 box blur. Mirrors `blur`.
fn blur(arr: &mut [f32], w: usize, h: usize) {
    let mut next = vec![0.0f32; arr.len()];
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.0f32;
            let mut cnt = 0.0f32;
            for oy in -1i64..=1 {
                for ox in -1i64..=1 {
                    let nx = x as i64 + ox;
                    let ny = y as i64 + oy;
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    sum += arr[ny as usize * w + nx as usize];
                    cnt += 1.0;
                }
            }
            next[y * w + x] = sum / cnt;
        }
    }
    arr.copy_from_slice(&next);
}

/// Road capacity field derived from the street network. Mirrors
/// `roadCapacityField` (recomputed each call; see module note).
fn road_capacity_field(state: &GameState) -> Vec<f32> {
    let g = &state.fields;
    let (w, h) = (g.w as usize, g.h as usize);
    let mut cap = vec![0.0f32; w * h];
    let cell_of = |x: f64, y: f64| -> i64 {
        let cx = ((x - g.origin_x) / g.cell_size).floor() as i64;
        let cy = ((y - g.origin_y) / g.cell_size).floor() as i64;
        if cx < 0 || cy < 0 || cx >= g.w as i64 || cy >= g.h as i64 {
            -1
        } else {
            cy * g.w as i64 + cx
        }
    };
    for road in &state.roads {
        let add = class_capacity(road.cls);
        let pl = &road.polyline;
        let step = g.cell_size * 0.5;
        let mut d = 0.0;
        while d <= pl.length {
            let mut i = 1usize;
            while i < pl.cumulative.len() - 1 && pl.cumulative[i] < d {
                i += 1;
            }
            let a = pl.points[i - 1];
            let b = pl.points[i];
            let seg_start = pl.cumulative[i - 1];
            let seg_len = {
                let l = pl.cumulative[i] - seg_start;
                if l == 0.0 {
                    1.0
                } else {
                    l
                }
            };
            let t = (d - seg_start) / seg_len;
            let ci = cell_of(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
            if ci >= 0 {
                let c = ci as usize;
                cap[c] = cap[c].max(add);
            }
            d += step;
        }
    }
    blur(&mut cap, w, h);
    cap
}

/// Compute the congestion field from car OD flows. Mirrors `computeTraffic`.
pub fn compute_traffic(
    state: &GameState,
    car_flows: &[CarFlow],
    demand_scale: f64,
) -> TrafficFieldOut {
    let g = &state.fields;
    let (w, h) = (g.w as usize, g.h as usize);
    let mut load = vec![0.0f32; w * h];
    let centroid: BTreeMap<u32, crate::geometry::Vec2> =
        state.districts.iter().map(|d| (d.id, d.centroid)).collect();

    for f in car_flows {
        if f.car_trips < 1.0 {
            continue;
        }
        let (Some(a), Some(b)) = (
            centroid.get(&f.origin_district),
            centroid.get(&f.dest_district),
        ) else {
            continue;
        };
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let len = {
            let l = dx.hypot(dy);
            if l == 0.0 {
                1.0
            } else {
                l
            }
        };
        let steps = ((len / (g.cell_size * 0.6)).ceil() as i64).max(1);
        let per = (f.car_trips / steps as f64) as f32;
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let x = a.x + dx * t;
            let y = a.y + dy * t;
            let cx = ((x - g.origin_x) / g.cell_size).floor() as i64;
            let cy = ((y - g.origin_y) / g.cell_size).floor() as i64;
            if cx < 0 || cy < 0 || cx >= w as i64 || cy >= h as i64 {
                continue;
            }
            let idx = cy as usize * w + cx as usize;
            if g.water.get(idx).copied() == Some(1) {
                continue;
            }
            load[idx] += per;
        }
    }
    blur(&mut load, w, h);

    let cap = road_capacity_field(state);
    let mut ratio = vec![0.0f32; w * h];
    let mut base_ratios: Vec<f32> = Vec::new();
    for i in 0..w * h {
        let l = load[i];
        if l <= 0.0 || g.water.get(i).copied() == Some(1) {
            continue;
        }
        let r = l / (cap[i] * 90.0 + 1.0);
        ratio[i] = r;
        base_ratios.push(r);
    }
    let mut values = vec![0.0f32; w * h];
    if base_ratios.is_empty() {
        return TrafficFieldOut {
            w: g.w,
            h: g.h,
            cell_size: g.cell_size,
            origin_x: g.origin_x,
            origin_y: g.origin_y,
            values,
            hotspots: Vec::new(),
        };
    }
    let mut sorted = base_ratios.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let idx = (sorted.len() as f64 * 0.92).floor() as usize;
    let idx = idx.min(sorted.len() - 1);
    let reference = (sorted[idx] as f64 * 1.25).max(1e-6);
    for i in 0..values.len() {
        values[i] = (((ratio[i] as f64) * demand_scale) / reference).min(1.0) as f32;
    }

    // Hotspots: local maxima above a threshold, spaced apart, worst first.
    const HOT: f32 = 0.55;
    let mut cand: Vec<TrafficHotspot> = Vec::new();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let v = values[y * w + x];
            if v < HOT {
                continue;
            }
            let mut is_max = true;
            'outer: for oy in -1i64..=1 {
                for ox in -1i64..=1 {
                    if values[(y as i64 + oy) as usize * w + (x as i64 + ox) as usize] > v {
                        is_max = false;
                        break 'outer;
                    }
                }
            }
            if !is_max {
                continue;
            }
            cand.push(TrafficHotspot {
                x: g.origin_x + (x as f64 + 0.5) * g.cell_size,
                y: g.origin_y + (y as f64 + 0.5) * g.cell_size,
                severity: v as f64,
            });
        }
    }
    cand.sort_by(|p, q| q.severity.total_cmp(&p.severity));
    let mut hotspots: Vec<TrafficHotspot> = Vec::new();
    let min_sep = g.cell_size * 3.0;
    for c in cand {
        if hotspots
            .iter()
            .any(|hh| (hh.x - c.x).hypot(hh.y - c.y) < min_sep)
        {
            continue;
        }
        hotspots.push(c);
        if hotspots.len() >= 12 {
            break;
        }
    }

    TrafficFieldOut {
        w: g.w,
        h: g.h,
        cell_size: g.cell_size,
        origin_x: g.origin_x,
        origin_y: g.origin_y,
        values,
        hotspots,
    }
}
