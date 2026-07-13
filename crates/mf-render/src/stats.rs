//! Live render-cache / entity counters for the F11 debug overlay and the
//! `MF_SOAK` harness. Each layer writes its own fields; consumers read the
//! whole resource. Cheap `Copy` so the overlay can snapshot without
//! fighting the render systems for a `ResMut`.

use bevy::prelude::*;

/// Per-layer cache and entity counts. Updated in place by the owning
/// systems; never grows itself (fixed-size struct).
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct RenderCacheStats {
    pub vehicle_slots: usize,
    pub vehicle_material_cache: usize,
    pub vehicle_light_material_cache: usize,
    pub transit_station_entities: usize,
    pub transit_track_entities: usize,
    pub transit_route_entities: usize,
    pub road_entities: usize,
    pub building_chunks: usize,
    pub tree_chunks: usize,
    pub street_lamp_chunks: usize,
    pub agent_entities: usize,
    /// Live ambient street-traffic car instances (see `traffic.rs`).
    pub ambient_traffic_cars: usize,
}
