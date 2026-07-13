//! Strata / geology model (v0.8 Underground -- sim side).
//!
//! Port of `sim/src/core/geology.ts`. The subsurface at any column `(x, y)` is
//! a pure function of `(seed, x, y, city geology profile)`: there is NO stored
//! 3D grid, band depths are reconstructed on demand from seeded value-noise, so
//! the model costs O(1) memory and reproduces bit-for-bit. It draws only from a
//! DERIVED salted noise field (never `state.rng_state`), so enabling geology
//! cannot perturb the existing city-event / growth RNG stream.
//!
//! The tunable *economics* live in the sibling module [`crate::geology_cost`].

use crate::geometry::Vec2;

/// Top-down stratum kinds. `Fill` = made ground / soil, `Clay` = clay/sand
/// mixed overburden, `Rock` = competent rock, `Bedrock` = deep basement rock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stratum {
    /// Made ground / soil.
    Fill,
    /// Clay / sand mixed overburden.
    Clay,
    /// Competent rock.
    Rock,
    /// Deep basement rock.
    Bedrock,
}

impl Stratum {
    /// Lowercase string name (mirrors the TS union member).
    pub fn as_str(self) -> &'static str {
        match self {
            Stratum::Fill => "fill",
            Stratum::Clay => "clay",
            Stratum::Rock => "rock",
            Stratum::Bedrock => "bedrock",
        }
    }
}

/// Canonical top-down order.
pub const STRATA: [Stratum; 4] = [
    Stratum::Fill,
    Stratum::Clay,
    Stratum::Rock,
    Stratum::Bedrock,
];

/// A single band in a reconstructed column. Depths are metres BELOW the surface
/// (`top < bottom`); the bedrock band uses a large nominal bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrataBand {
    /// Stratum kind of this band.
    pub kind: Stratum,
    /// Depth (m) to the top of the band.
    pub top: f64,
    /// Depth (m) to the bottom of the band.
    pub bottom: f64,
}

/// A fully reconstructed subsurface column at one `(x, y)`.
#[derive(Clone, Debug, PartialEq)]
pub struct StrataColumn {
    /// Top-down bands.
    pub bands: [StrataBand; 4],
    /// Depth (m below surface) to the top of the water table.
    pub water_table_depth: f64,
    /// 0..1 -- how hard the competent rock is to cut (schist/granite high).
    pub rock_hardness: f64,
    /// Surface elevation (m above sea level) used to build this column.
    pub surface_elevation: f64,
}

/// Nominal bottom of the (unbounded) bedrock band, metres.
pub const BEDROCK_NOMINAL_BOTTOM: f64 = 1000.0;

/// Per-city geology profile (content, like the climate tables).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeologyProfile {
    /// Nominal fill/soil thickness (m).
    pub soil: f64,
    /// Nominal clay/sand overburden thickness (m).
    pub clay: f64,
    /// Nominal competent-rock band thickness before bedrock (m).
    pub rock_thickness: f64,
    /// 0..1 hardness of the competent rock (drives bored-TBM cost).
    pub rock_hardness: f64,
    /// Nominal water-table depth (m) on flat ground at sea level.
    pub base_water_table: f64,
    /// 0..1 wetness: higher pulls the table up (low-lying, near water).
    pub wetness: f64,
    /// Metres the water table drops per metre of surface elevation.
    pub wt_elev_factor: f64,
}

/// Temperate baseline; every unknown city falls back to this.
pub const GENERIC_PROFILE: GeologyProfile = GeologyProfile {
    soil: 4.0,
    clay: 15.0,
    rock_thickness: 40.0,
    rock_hardness: 0.55,
    base_water_table: 8.0,
    wetness: 0.35,
    wt_elev_factor: 0.4,
};

/// Resolve a city's geology profile, falling back to the generic temperate one.
pub fn geology_profile(city_key: Option<&str>) -> GeologyProfile {
    match city_key {
        Some("generic") => GENERIC_PROFILE,
        Some("nyc") => GeologyProfile {
            soil: 4.0,
            clay: 8.0,
            rock_thickness: 60.0,
            rock_hardness: 0.88,
            base_water_table: 8.0,
            wetness: 0.4,
            wt_elev_factor: 0.45,
        },
        Some("boston") => GeologyProfile {
            soil: 8.0,
            clay: 22.0,
            rock_thickness: 45.0,
            rock_hardness: 0.5,
            base_water_table: 4.0,
            wetness: 0.6,
            wt_elev_factor: 0.35,
        },
        Some("chicago") => GeologyProfile {
            soil: 3.0,
            clay: 28.0,
            rock_thickness: 40.0,
            rock_hardness: 0.45,
            base_water_table: 5.0,
            wetness: 0.45,
            wt_elev_factor: 0.3,
        },
        Some("sf") => GeologyProfile {
            soil: 5.0,
            clay: 14.0,
            rock_thickness: 50.0,
            rock_hardness: 0.62,
            base_water_table: 4.0,
            wetness: 0.55,
            wt_elev_factor: 0.5,
        },
        Some("seattle") => GeologyProfile {
            soil: 4.0,
            clay: 26.0,
            rock_thickness: 45.0,
            rock_hardness: 0.55,
            base_water_table: 6.0,
            wetness: 0.45,
            wt_elev_factor: 0.45,
        },
        Some("la") => GeologyProfile {
            soil: 6.0,
            clay: 42.0,
            rock_thickness: 35.0,
            rock_hardness: 0.3,
            base_water_table: 12.0,
            wetness: 0.2,
            wt_elev_factor: 0.35,
        },
        Some("dc") => GeologyProfile {
            soil: 5.0,
            clay: 24.0,
            rock_thickness: 40.0,
            rock_hardness: 0.42,
            base_water_table: 5.0,
            wetness: 0.5,
            wt_elev_factor: 0.4,
        },
        Some("philly") => GeologyProfile {
            soil: 5.0,
            clay: 15.0,
            rock_thickness: 50.0,
            rock_hardness: 0.62,
            base_water_table: 6.0,
            wetness: 0.4,
            wt_elev_factor: 0.45,
        },
        Some("atlanta") => GeologyProfile {
            soil: 6.0,
            clay: 15.0,
            rock_thickness: 55.0,
            rock_hardness: 0.78,
            base_water_table: 10.0,
            wetness: 0.25,
            wt_elev_factor: 0.5,
        },
        Some("cleveland") => GeologyProfile {
            soil: 4.0,
            clay: 20.0,
            rock_thickness: 40.0,
            rock_hardness: 0.5,
            base_water_table: 5.0,
            wetness: 0.45,
            wt_elev_factor: 0.35,
        },
        _ => GENERIC_PROFILE,
    }
}

// -- Seeded value-noise (deterministic, no RNG-stream draw) -------------------
const BAND_SALT: u32 = 0x51ed_270b;
const TABLE_SALT: u32 = 0x2c1b_3c6d;

/// Cell size (m) of the coarse noise lattice; band depths vary smoothly on this.
pub const STRATA_NOISE_CELL: f64 = 250.0;
/// +/- fraction a layer thickness can wander from nominal.
pub const STRATA_NOISE_FRAC: f64 = 0.35;

/// Integer hash to `[0,1)`. Mirrors the JS `hash2` bit-for-bit: every op is a
/// low-32-bit integer operation, identical whether read signed or unsigned.
fn hash2(seed: u32, salt: u32, ix: i32, iy: i32) -> f64 {
    let mut h = seed ^ salt;
    let t1 = (ix as u32).wrapping_add(0x9e37_79b9);
    h = (h ^ t1).wrapping_mul(0x85eb_ca6b);
    let t2 = (iy as u32).wrapping_add(0x1656_67b1);
    h = (h ^ t2).wrapping_mul(0xc2b2_ae35);
    h ^= h >> 15;
    f64::from(h) / 4_294_967_296.0
}

/// Smooth bilinear value noise in `[0,1)` at world `(x,y)` on the coarse lattice.
fn value_noise(seed: u32, salt: u32, x: f64, y: f64) -> f64 {
    let gx = x / STRATA_NOISE_CELL;
    let gy = y / STRATA_NOISE_CELL;
    let ix = gx.floor() as i32;
    let iy = gy.floor() as i32;
    let fx = gx - f64::from(ix);
    let fy = gy - f64::from(iy);
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let v00 = hash2(seed, salt, ix, iy);
    let v10 = hash2(seed, salt, ix + 1, iy);
    let v01 = hash2(seed, salt, ix, iy + 1);
    let v11 = hash2(seed, salt, ix + 1, iy + 1);
    (v00 * (1.0 - sx) + v10 * sx) * (1.0 - sy) + (v01 * (1.0 - sx) + v11 * sx) * sy
}

/// Signed +/-`STRATA_NOISE_FRAC` multiplier for a layer.
fn thickness_jitter(seed: u32, salt: u32, x: f64, y: f64) -> f64 {
    1.0 + (value_noise(seed, salt, x, y) - 0.5) * 2.0 * STRATA_NOISE_FRAC
}

/// Sample the surface elevation (m above sea level) at world `(x,y)`. Returns 0
/// (sea level, flat) when a city has no baked elevation (procedural cities).
pub fn sample_surface_elevation(
    elev: Option<&[i16]>,
    res: Option<u32>,
    world_size: f64,
    x: f64,
    y: f64,
) -> f64 {
    let (elev, res) = match (elev, res) {
        (Some(e), Some(r)) if r > 0 => (e, r as i64),
        _ => return 0.0,
    };
    let half = world_size / 2.0;
    let mut c = (((x + half) / world_size) * res as f64).floor() as i64;
    let mut r = (((y + half) / world_size) * res as f64).floor() as i64;
    if c < 0 {
        c = 0;
    } else if c >= res {
        c = res - 1;
    }
    if r < 0 {
        r = 0;
    } else if r >= res {
        r = res - 1;
    }
    f64::from(elev[(r * res + c) as usize])
}

// -- Water table --------------------------------------------------------------
/// Minimum modelled water-table depth (m).
pub const MIN_WATER_TABLE: f64 = 1.5;
/// Maximum modelled water-table depth (m).
pub const MAX_WATER_TABLE: f64 = 45.0;

/// Depth (m) to the water table at a column.
pub fn water_table_depth_at(
    profile: &GeologyProfile,
    surface_elevation: f64,
    seed: u32,
    x: f64,
    y: f64,
) -> f64 {
    let noise = (value_noise(seed, TABLE_SALT, x, y) - 0.5) * 2.0 * 3.0; // +/-3 m
    let wt = profile.base_water_table * (1.0 - 0.4 * profile.wetness)
        + surface_elevation.max(0.0) * profile.wt_elev_factor
        + noise;
    wt.clamp(MIN_WATER_TABLE, MAX_WATER_TABLE)
}

// -- Column reconstruction ----------------------------------------------------
/// Reconstruct the full subsurface column at world `p` for a city. Pure
/// function of `(seed, profile, elevation)` -- no RNG-stream draw, O(1).
pub fn strata_column(
    profile: &GeologyProfile,
    seed: u32,
    world_size: f64,
    elev: Option<&[i16]>,
    elev_res: Option<u32>,
    p: Vec2,
) -> StrataColumn {
    let surface_elevation = sample_surface_elevation(elev, elev_res, world_size, p.x, p.y);
    let soil_bottom = profile.soil * thickness_jitter(seed, BAND_SALT, p.x, p.y);
    let clay_bottom =
        soil_bottom + profile.clay * thickness_jitter(seed, BAND_SALT ^ 0x1111, p.x, p.y);
    let bedrock_top =
        clay_bottom + profile.rock_thickness * thickness_jitter(seed, BAND_SALT ^ 0x2222, p.x, p.y);
    let bands = [
        StrataBand {
            kind: Stratum::Fill,
            top: 0.0,
            bottom: soil_bottom,
        },
        StrataBand {
            kind: Stratum::Clay,
            top: soil_bottom,
            bottom: clay_bottom,
        },
        StrataBand {
            kind: Stratum::Rock,
            top: clay_bottom,
            bottom: bedrock_top,
        },
        StrataBand {
            kind: Stratum::Bedrock,
            top: bedrock_top,
            bottom: BEDROCK_NOMINAL_BOTTOM,
        },
    ];
    StrataColumn {
        bands,
        water_table_depth: water_table_depth_at(profile, surface_elevation, seed, p.x, p.y),
        rock_hardness: profile.rock_hardness,
        surface_elevation,
    }
}

/// Which stratum sits at a given depth (m below surface) in a column.
pub fn stratum_at_depth(col: &StrataColumn, depth: f64) -> Stratum {
    for b in &col.bands {
        if depth < b.bottom {
            return b.kind;
        }
    }
    Stratum::Bedrock
}

/// Convenience: build a column straight from GameState-shaped inputs.
#[allow(clippy::too_many_arguments)]
pub fn column_at(
    city_key: Option<&str>,
    seed: u32,
    world_size: f64,
    elev: Option<&[i16]>,
    elev_res: Option<u32>,
    p: Vec2,
) -> StrataColumn {
    strata_column(
        &geology_profile(city_key),
        seed,
        world_size,
        elev,
        elev_res,
        p,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::vec;

    #[test]
    fn column_is_deterministic_run_twice() {
        let p = vec(1234.5, -678.9);
        let a = column_at(Some("nyc"), 12345, 12000.0, None, None, p);
        let b = column_at(Some("nyc"), 12345, 12000.0, None, None, p);
        assert_eq!(a, b);
    }

    #[test]
    fn bands_are_ordered_and_contiguous() {
        let col = column_at(Some("boston"), 999, 12000.0, None, None, vec(500.0, 500.0));
        assert_eq!(col.bands[0].top, 0.0);
        for w in col.bands.windows(2) {
            assert!((w[0].bottom - w[1].top).abs() < 1e-9);
            assert!(w[0].bottom <= w[1].bottom);
        }
        assert_eq!(col.bands[3].bottom, BEDROCK_NOMINAL_BOTTOM);
    }

    #[test]
    fn water_table_within_bounds() {
        for city in ["nyc", "boston", "chicago", "sf", "la", "generic"] {
            let col = column_at(Some(city), 7, 12000.0, None, None, vec(-2000.0, 3000.0));
            assert!(col.water_table_depth >= MIN_WATER_TABLE);
            assert!(col.water_table_depth <= MAX_WATER_TABLE);
        }
    }

    #[test]
    fn stratum_at_depth_walks_bands() {
        let col = column_at(Some("generic"), 42, 12000.0, None, None, vec(0.0, 0.0));
        assert_eq!(stratum_at_depth(&col, 0.0), Stratum::Fill);
        assert_eq!(stratum_at_depth(&col, 2000.0), Stratum::Bedrock);
    }
}
