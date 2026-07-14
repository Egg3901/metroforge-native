//! City presets + map sizes. Port of `sim/src/core/city/presets.ts`.
//!
//! Presets do NOT import real GIS data; they retune the tensor-field generator
//! so each city reads like its real counterpart (grid regularity, downtown
//! pull, coastline, sprawl). Everything stays procedural + seed-deterministic;
//! a preset just picks the knobs.

/// Map size class. Mirrors `MapSize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapSize {
    /// 8 km world.
    Small,
    /// 12 km world (the default).
    Medium,
    /// 18 km world.
    Large,
}

impl MapSize {
    /// World edge length in meters. Mirrors `MAP_SIZE_METERS`.
    pub fn meters(self) -> f64 {
        match self {
            MapSize::Small => 8000.0,
            MapSize::Medium => 12000.0,
            MapSize::Large => 18000.0,
        }
    }
}

/// Water configuration for a preset. Mirrors `WaterConfig`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterConfig {
    /// A straight coastline (ocean / great lake) along one edge.
    pub coast: bool,
    /// Fixed coast bearing in degrees, or `None` for a seed-random bearing.
    pub coast_angle_deg: Option<f64>,
    /// 0..1 how far inland the coast sits (higher = more land).
    pub coast_inset: f64,
    /// Carve a meandering river.
    pub river: bool,
}

/// Street-grid regularity knobs. Mirrors `CityPreset.grid`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridConfig {
    /// Tensor grid patch weight (higher = streets snap harder to the grid).
    pub weight: f64,
    /// Base grid bearing in degrees; grids all align to it when rigid.
    pub angle_deg: f64,
    /// `true` = rigid rectilinear (NYC/Chicago); `false` = organic (Boston).
    pub rigid: bool,
    /// Field noise weight (the wobble in street direction).
    pub noise_weight: f64,
}

/// A city preset. Mirrors `CityPreset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CityPreset {
    /// Stable key.
    pub key: &'static str,
    /// Display label.
    pub label: &'static str,
    /// Backed by a real OpenStreetMap import (real roads + coastline).
    pub real: bool,
    /// Street-grid regularity.
    pub grid: GridConfig,
    /// Downtown radial convergence.
    pub radial_weight: f64,
    /// Water configuration.
    pub water: WaterConfig,
    /// >1 spreads density out (sprawl); <1 concentrates it.
    pub sprawl: f64,
}

const GENERIC: CityPreset = CityPreset {
    key: "generic",
    label: "Random City",
    real: false,
    grid: GridConfig {
        weight: 1.0,
        angle_deg: 0.0,
        rigid: false,
        noise_weight: 0.22,
    },
    radial_weight: 2.2,
    water: WaterConfig {
        coast: true,
        coast_angle_deg: None,
        coast_inset: 0.7,
        river: true,
    },
    sprawl: 1.0,
};

/// All city presets, in the same order as `CITY_PRESETS`.
pub const CITY_PRESETS: &[CityPreset] = &[
    GENERIC,
    CityPreset {
        key: "nyc",
        label: "New York",
        real: true,
        grid: GridConfig {
            weight: 1.5,
            angle_deg: 29.0,
            rigid: true,
            noise_weight: 0.06,
        },
        radial_weight: 1.4,
        water: WaterConfig {
            coast: true,
            coast_angle_deg: Some(120.0),
            coast_inset: 0.78,
            river: true,
        },
        sprawl: 0.72,
    },
    CityPreset {
        key: "chicago",
        label: "Chicago",
        real: true,
        grid: GridConfig {
            weight: 1.6,
            angle_deg: 0.0,
            rigid: true,
            noise_weight: 0.05,
        },
        radial_weight: 1.6,
        water: WaterConfig {
            coast: true,
            coast_angle_deg: Some(0.0),
            coast_inset: 0.82,
            river: true,
        },
        sprawl: 0.95,
    },
    CityPreset {
        key: "la",
        label: "Los Angeles",
        real: true,
        grid: GridConfig {
            weight: 1.2,
            angle_deg: 12.0,
            rigid: true,
            noise_weight: 0.12,
        },
        radial_weight: 0.9,
        water: WaterConfig {
            coast: true,
            coast_angle_deg: Some(210.0),
            coast_inset: 0.85,
            river: false,
        },
        sprawl: 1.7,
    },
    CityPreset {
        key: "boston",
        label: "Boston",
        real: true,
        grid: GridConfig {
            weight: 0.7,
            angle_deg: 40.0,
            rigid: false,
            noise_weight: 0.5,
        },
        radial_weight: 2.6,
        water: WaterConfig {
            coast: true,
            coast_angle_deg: Some(75.0),
            coast_inset: 0.62,
            river: true,
        },
        sprawl: 0.85,
    },
    CityPreset {
        key: "atlanta",
        label: "Atlanta",
        real: true,
        grid: GridConfig {
            weight: 0.9,
            angle_deg: 20.0,
            rigid: false,
            noise_weight: 0.3,
        },
        radial_weight: 3.2,
        water: WaterConfig {
            coast: false,
            coast_angle_deg: None,
            coast_inset: 1.0,
            river: false,
        },
        sprawl: 1.8,
    },
    CityPreset {
        key: "cleveland",
        label: "Cleveland",
        real: true,
        grid: GridConfig {
            weight: 1.3,
            angle_deg: 8.0,
            rigid: true,
            noise_weight: 0.1,
        },
        radial_weight: 1.8,
        water: WaterConfig {
            coast: true,
            coast_angle_deg: Some(0.0),
            coast_inset: 0.8,
            river: true,
        },
        sprawl: 1.1,
    },
    CityPreset {
        key: "philly",
        label: "Philadelphia",
        real: true,
        grid: GridConfig {
            weight: 1.55,
            angle_deg: 0.0,
            rigid: true,
            noise_weight: 0.06,
        },
        radial_weight: 1.5,
        water: WaterConfig {
            coast: false,
            coast_angle_deg: None,
            coast_inset: 1.0,
            river: true,
        },
        sprawl: 0.9,
    },
    CityPreset {
        key: "sf",
        label: "San Francisco",
        real: true,
        grid: GridConfig {
            weight: 1.35,
            angle_deg: 0.0,
            rigid: true,
            noise_weight: 0.18,
        },
        radial_weight: 2.0,
        water: WaterConfig {
            coast: true,
            coast_angle_deg: Some(45.0),
            coast_inset: 0.7,
            river: false,
        },
        sprawl: 0.8,
    },
    CityPreset {
        key: "dc",
        label: "Washington",
        real: true,
        grid: GridConfig {
            weight: 1.1,
            angle_deg: 0.0,
            rigid: false,
            noise_weight: 0.2,
        },
        radial_weight: 2.8,
        water: WaterConfig {
            coast: false,
            coast_angle_deg: None,
            coast_inset: 1.0,
            river: true,
        },
        sprawl: 1.05,
    },
    CityPreset {
        key: "seattle",
        label: "Seattle",
        real: true,
        grid: GridConfig {
            weight: 1.25,
            angle_deg: 0.0,
            rigid: true,
            noise_weight: 0.14,
        },
        radial_weight: 2.2,
        water: WaterConfig {
            coast: true,
            coast_angle_deg: Some(270.0),
            coast_inset: 0.72,
            river: false,
        },
        sprawl: 1.15,
    },
];

/// The `generic` preset. Mirrors the default `GENERIC`.
pub fn generic() -> CityPreset {
    GENERIC
}

/// Look up a preset by key, falling back to `generic`. Mirrors `presetByKey`.
pub fn preset_by_key(key: Option<&str>) -> CityPreset {
    match key {
        Some(k) => CITY_PRESETS
            .iter()
            .find(|p| p.key == k)
            .copied()
            .unwrap_or(GENERIC),
        None => GENERIC,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_hits_and_falls_back() {
        assert_eq!(preset_by_key(Some("nyc")).label, "New York");
        assert_eq!(preset_by_key(Some("nope")).key, "generic");
        assert_eq!(preset_by_key(None).key, "generic");
    }

    #[test]
    fn map_sizes() {
        assert_eq!(MapSize::Medium.meters(), 12000.0);
    }
}
