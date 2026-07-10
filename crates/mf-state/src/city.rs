use bevy_ecs::prelude::*;
use mf_protocol::{MaskWhich, StaticBuildings, StaticCityJson, StaticMask};

/// The currently-loaded city: the `StaticCityJson` sent with `ready`, plus
/// the (0-3) mask byte arrays that arrive right after it as binary
/// `StaticMask` frames (spec §1.2 msgType=4). `mf-render`'s `terrain.rs` /
/// `buildings.rs` read this to bake statics; `mf-game`'s `Loading` state
/// waits for `static_city` plus every mask flagged `has_*_mask` before
/// advancing to `InGame`.
#[derive(Resource, Default)]
pub struct CurrentCity {
    pub static_city: Option<StaticCityJson>,
    /// `res*res` bytes, row-major, present iff `static_city.has_water_mask`.
    pub water_mask: Option<Vec<u8>>,
    /// `res*res` bytes, row-major, present iff `static_city.has_park_mask`.
    pub park_mask: Option<Vec<u8>>,
    /// `res*res` bytes, row-major, present iff `static_city.has_building_mask`.
    pub building_mask: Option<Vec<u8>>,
    /// Exact building footprints (spec §1.2 msgType=5), sent once and
    /// independent of `has_building_mask`/`building_mask` above (those are a
    /// coarse rasterized silhouette used for the ground shader; this is
    /// per-building vector geometry used by `mf-render`'s `buildings.rs` to
    /// extrude real footprints instead of its procedural density formula).
    /// Stored as the wire type verbatim rather than a lighter mirror: it's
    /// already been unit-converted to meters at decode (see
    /// `BuildingFootprint`), and every other binary payload in this resource
    /// set (`Fields`, `FrameSnapshot`, `StaticMask`) is likewise stored
    /// as-is, so this keeps the pattern consistent. Deliberately absent from
    /// `masks_complete()`: it's optional data, not a loading-gate input — a
    /// city with no buildings message is valid, and the renderer's fallback
    /// makes waiting for it pointless.
    pub buildings: Option<StaticBuildings>,
}

impl CurrentCity {
    /// Replace the city and clear any masks left over from the previous one.
    pub fn set_static_city(&mut self, static_city: StaticCityJson) {
        self.static_city = Some(static_city);
        self.water_mask = None;
        self.park_mask = None;
        self.building_mask = None;
        self.buildings = None;
    }

    /// Store one incoming `StaticMask` frame into the right slot.
    pub fn apply_mask(&mut self, mask: StaticMask) {
        match mask.which {
            MaskWhich::Water => self.water_mask = Some(mask.mask),
            MaskWhich::Park => self.park_mask = Some(mask.mask),
            MaskWhich::Building => self.building_mask = Some(mask.mask),
        }
    }

    /// Store the (single, one-shot) incoming `StaticBuildings` frame.
    pub fn apply_buildings(&mut self, buildings: StaticBuildings) {
        self.buildings = Some(buildings);
    }

    /// True once `static_city` is present and every mask it flagged as
    /// present (`has*Mask`) has actually arrived. `mf-game`'s `Loading`
    /// state uses this as one of its gate conditions.
    ///
    /// Intentionally does NOT factor in `self.buildings`: `StaticBuildings`
    /// is optional additive data (see its field doc), not a loading-gate
    /// input, so its absence must never block the transition to `InGame`.
    pub fn masks_complete(&self) -> bool {
        let Some(city) = &self.static_city else {
            return false;
        };
        (!city.has_water_mask || self.water_mask.is_some())
            && (!city.has_park_mask || self.park_mask.is_some())
            && (!city.has_building_mask || self.building_mask.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_protocol::binary::BuildingFootprint;

    fn dummy_static_city() -> StaticCityJson {
        StaticCityJson {
            field_w: 4,
            field_h: 4,
            cell_size: 10.0,
            origin_x: 0.0,
            origin_y: 0.0,
            world_size: 40.0,
            road_scale: 1.0,
            mask_res: None,
            has_water_mask: false,
            has_park_mask: false,
            has_building_mask: false,
            labels: None,
            roads: Vec::new(),
        }
    }

    fn dummy_buildings() -> StaticBuildings {
        StaticBuildings {
            buildings: vec![BuildingFootprint {
                height_dm: 300,
                min_height_dm: 0,
                verts: vec![[-1.0, -1.0], [1.0, -1.0], [0.0, 1.0]],
            }],
        }
    }

    #[test]
    fn apply_buildings_then_read_back() {
        let mut city = CurrentCity::default();
        assert!(city.buildings.is_none());

        city.apply_buildings(dummy_buildings());

        let stored = city.buildings.as_ref().expect("buildings should be set");
        assert_eq!(stored.buildings.len(), 1);
        assert_eq!(stored.buildings[0].height_dm, 300);
        assert_eq!(
            stored.buildings[0].verts,
            vec![[-1.0, -1.0], [1.0, -1.0], [0.0, 1.0]]
        );
    }

    #[test]
    fn set_static_city_clears_buildings() {
        let mut city = CurrentCity::default();
        city.apply_buildings(dummy_buildings());
        assert!(city.buildings.is_some());

        city.set_static_city(dummy_static_city());

        assert!(
            city.buildings.is_none(),
            "buildings from the previous city must not leak into the new one"
        );
    }

    #[test]
    fn buildings_do_not_gate_masks_complete() {
        let mut city = CurrentCity::default();
        city.set_static_city(dummy_static_city());
        // No masks flagged as present, and no buildings ever applied.
        assert!(city.masks_complete());

        // Applying buildings (optional data) must not change the gate.
        city.apply_buildings(dummy_buildings());
        assert!(city.masks_complete());
    }
}
