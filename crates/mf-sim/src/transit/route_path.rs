//! Derived route geometry: the out-and-back polyline a route's vehicles follow.
//! Port of `sim/src/core/transit/routePath.ts`.
//!
//! The TS side memoizes per `instanceId:routeId`; the Rust port computes fresh
//! (the callers are cold paths / lane-B movement setup, not a hashed loop).

use crate::geometry::{make_polyline, Polyline, Vec2};
use crate::types::{GameState, RouteDef};

/// Out-and-back polyline for a route, or `None` if a segment is missing or the
/// path degenerates. Mirrors `getRoutePath`.
pub fn get_route_path(state: &GameState, route: &RouteDef) -> Option<Polyline> {
    let mut outbound: Vec<Vec2> = Vec::new();
    for i in 0..route.segment_ids.len() {
        let seg = state.tracks.iter().find(|t| t.id == route.segment_ids[i])?;
        let from_id = route.station_ids[i];
        let mut pts: Vec<Vec2> = seg.polyline.points.clone();
        if seg.to_station_id == from_id {
            pts.reverse();
        }
        let start_at = if outbound.is_empty() { 0 } else { 1 };
        for p in pts.into_iter().skip(start_at) {
            outbound.push(p);
        }
    }
    if outbound.len() < 2 {
        return None;
    }
    let back: Vec<Vec2> = outbound.iter().rev().skip(1).copied().collect();
    let mut all = outbound;
    all.extend(back);
    Some(make_polyline(all))
}
