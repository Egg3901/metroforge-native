use bevy_ecs::prelude::*;
use mf_protocol::{MaskWhich, StaticCityJson, StaticMask};

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
}

impl CurrentCity {
    /// Replace the city and clear any masks left over from the previous one.
    pub fn set_static_city(&mut self, static_city: StaticCityJson) {
        self.static_city = Some(static_city);
        self.water_mask = None;
        self.park_mask = None;
        self.building_mask = None;
    }

    /// Store one incoming `StaticMask` frame into the right slot.
    pub fn apply_mask(&mut self, mask: StaticMask) {
        match mask.which {
            MaskWhich::Water => self.water_mask = Some(mask.mask),
            MaskWhich::Park => self.park_mask = Some(mask.mask),
            MaskWhich::Building => self.building_mask = Some(mask.mask),
        }
    }

    /// True once `static_city` is present and every mask it flagged as
    /// present (`has*Mask`) has actually arrived. `mf-game`'s `Loading`
    /// state uses this as one of its gate conditions.
    pub fn masks_complete(&self) -> bool {
        let Some(city) = &self.static_city else {
            return false;
        };
        (!city.has_water_mask || self.water_mask.is_some())
            && (!city.has_park_mask || self.park_mask.is_some())
            && (!city.has_building_mask || self.building_mask.is_some())
    }
}
