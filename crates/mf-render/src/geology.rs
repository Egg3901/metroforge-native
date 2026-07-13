//! Client-side mirror of the sim's subsurface strata model
//! (`sim/src/core/geology.ts`), used by [`crate::diorama`] to paint the
//! floating-slab cut sides and the cutaway cut-face with banded flat-color
//! strata WITHOUT any `strataProbe` network round-trips.
//!
//! ## Why mirror instead of probe (the strata-source design choice)
//! The sim reconstructs a subsurface column as a PURE, deterministic O(1)
//! function of `(seed ⊕ salts, per-city geology profile, surface elevation)`
//! — no stored 3D grid, no RNG-stream draw. Every input the client needs is
//! something it already holds: the seed and city key it chose at `init`
//! (published via `mf_state::GeologyContext`) and the real-elevation channel
//! (`CurrentCity::elevation`). Drawing the permanent diorama edge would
//! otherwise need ~64 probe points per side × 4 sides = 256 `strataProbe`
//! round-trips per city load (and the cutaway cut-face far more), all to
//! reproduce a function that is ~40 lines of value noise. So this module ports
//! that function verbatim; `strataProbe` stays on the wire only for any future
//! single-point inspection UI.
//!
//! Determinism: the ports below use the exact same `Math.imul`-style u32
//! wrapping hash, the same salts, the same lattice cell size and thickness
//! fraction, and the same profile tables as `geology.ts`, so a client that
//! knows the world seed reproduces the sim's bands bit-for-bit. When the seed
//! is not pinned (the autostart default randomizes it) the bands still read as
//! plausible geology — the look is seed-independent, only the ±35% thickness
//! wander shifts.

/// Top-down stratum kinds (mirrors `geology.ts` `Stratum`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stratum {
    /// Made ground / soil (warm tan).
    Fill,
    /// Clay / sand overburden (muted ochre).
    Clay,
    /// Competent rock (grey-brown).
    Rock,
    /// Deep basement rock (darkest).
    Bedrock,
}

/// One reconstructed band; depths are metres below the surface (`top < bottom`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrataBand {
    pub kind: Stratum,
    pub top: f64,
    pub bottom: f64,
}

/// A fully reconstructed subsurface column (mirrors `geology.ts` `StrataColumn`).
#[derive(Debug, Clone)]
pub struct StrataColumn {
    pub bands: [StrataBand; 4],
    /// Depth (m below surface) to the water table.
    pub water_table_depth: f64,
    /// Surface elevation (m above sea level) the column was built at.
    pub surface_elevation: f64,
}

/// Nominal bottom of the unbounded bedrock band, metres (`BEDROCK_NOMINAL_BOTTOM`).
pub const BEDROCK_NOMINAL_BOTTOM: f64 = 1000.0;

/// Per-city geology profile (mirrors `geology.ts` `GeologyProfile`).
#[derive(Debug, Clone, Copy)]
struct GeologyProfile {
    soil: f64,
    clay: f64,
    rock_thickness: f64,
    base_water_table: f64,
    wetness: f64,
    wt_elev_factor: f64,
}

const GENERIC_PROFILE: GeologyProfile = GeologyProfile {
    soil: 4.0,
    clay: 15.0,
    rock_thickness: 40.0,
    base_water_table: 8.0,
    wetness: 0.35,
    wt_elev_factor: 0.4,
};

/// Resolve a city key to its profile, falling back to the generic temperate
/// one (mirrors `geology.ts` `geologyProfile` + the `CITY_PROFILES` table).
fn profile_for(city_key: Option<&str>) -> GeologyProfile {
    match city_key {
        Some("nyc") => GeologyProfile {
            soil: 4.0,
            clay: 8.0,
            rock_thickness: 60.0,
            base_water_table: 8.0,
            wetness: 0.4,
            wt_elev_factor: 0.45,
        },
        Some("boston") => GeologyProfile {
            soil: 8.0,
            clay: 22.0,
            rock_thickness: 45.0,
            base_water_table: 4.0,
            wetness: 0.6,
            wt_elev_factor: 0.35,
        },
        Some("chicago") => GeologyProfile {
            soil: 3.0,
            clay: 28.0,
            rock_thickness: 40.0,
            base_water_table: 5.0,
            wetness: 0.45,
            wt_elev_factor: 0.3,
        },
        Some("sf") => GeologyProfile {
            soil: 5.0,
            clay: 14.0,
            rock_thickness: 50.0,
            base_water_table: 4.0,
            wetness: 0.55,
            wt_elev_factor: 0.5,
        },
        Some("seattle") => GeologyProfile {
            soil: 4.0,
            clay: 26.0,
            rock_thickness: 45.0,
            base_water_table: 6.0,
            wetness: 0.45,
            wt_elev_factor: 0.45,
        },
        Some("la") => GeologyProfile {
            soil: 6.0,
            clay: 42.0,
            rock_thickness: 35.0,
            base_water_table: 12.0,
            wetness: 0.2,
            wt_elev_factor: 0.35,
        },
        Some("dc") => GeologyProfile {
            soil: 5.0,
            clay: 24.0,
            rock_thickness: 40.0,
            base_water_table: 5.0,
            wetness: 0.5,
            wt_elev_factor: 0.4,
        },
        Some("philly") => GeologyProfile {
            soil: 5.0,
            clay: 15.0,
            rock_thickness: 50.0,
            base_water_table: 6.0,
            wetness: 0.4,
            wt_elev_factor: 0.45,
        },
        Some("atlanta") => GeologyProfile {
            soil: 6.0,
            clay: 15.0,
            rock_thickness: 55.0,
            base_water_table: 10.0,
            wetness: 0.25,
            wt_elev_factor: 0.5,
        },
        Some("cleveland") => GeologyProfile {
            soil: 4.0,
            clay: 20.0,
            rock_thickness: 40.0,
            base_water_table: 5.0,
            wetness: 0.45,
            wt_elev_factor: 0.35,
        },
        _ => GENERIC_PROFILE,
    }
}

// ── Seeded value noise (mirrors geology.ts exactly) ──────────────────────────
const BAND_SALT: u32 = 0x51ed_270b;
const TABLE_SALT: u32 = 0x2c1b_3c6d;

/// Cell size (m) of the coarse noise lattice.
pub const STRATA_NOISE_CELL: f64 = 250.0;
/// ±fraction a layer thickness can wander from nominal.
pub const STRATA_NOISE_FRAC: f64 = 0.35;

pub const MIN_WATER_TABLE: f64 = 1.5;
pub const MAX_WATER_TABLE: f64 = 45.0;

/// `Math.imul` — 32-bit wrapping signed multiply, low 32 bits (JS semantics).
#[inline]
fn imul(a: u32, b: u32) -> u32 {
    a.wrapping_mul(b)
}

/// Port of `geology.ts` `hash2` (returns [0,1)). `ix`/`iy` are lattice
/// integer coords, added to constants exactly as the JS does — the JS uses
/// `ix + 0x9e3779b9` in a number context then `^` coerces to int32, which for
/// these magnitudes matches u32 wrapping add.
fn hash2(seed: u32, salt: u32, ix: i32, iy: i32) -> f64 {
    let mut h = seed ^ salt;
    h = imul(h ^ (ix as u32).wrapping_add(0x9e37_79b9), 0x85eb_ca6b);
    h = imul(h ^ (iy as u32).wrapping_add(0x1656_67b1), 0xc2b2_ae35);
    h ^= h >> 15;
    h as f64 / 4_294_967_296.0
}

fn smooth(f: f64) -> f64 {
    f * f * (3.0 - 2.0 * f)
}

/// Port of `geology.ts` `valueNoise` (bilinear value noise in [0,1)).
fn value_noise(seed: u32, salt: u32, x: f64, y: f64) -> f64 {
    let gx = x / STRATA_NOISE_CELL;
    let gy = y / STRATA_NOISE_CELL;
    let ix = gx.floor() as i32;
    let iy = gy.floor() as i32;
    let fx = gx - ix as f64;
    let fy = gy - iy as f64;
    let sx = smooth(fx);
    let sy = smooth(fy);
    let v00 = hash2(seed, salt, ix, iy);
    let v10 = hash2(seed, salt, ix + 1, iy);
    let v01 = hash2(seed, salt, ix, iy + 1);
    let v11 = hash2(seed, salt, ix + 1, iy + 1);
    (v00 * (1.0 - sx) + v10 * sx) * (1.0 - sy) + (v01 * (1.0 - sx) + v11 * sx) * sy
}

fn thickness_jitter(seed: u32, salt: u32, x: f64, y: f64) -> f64 {
    1.0 + (value_noise(seed, salt, x, y) - 0.5) * 2.0 * STRATA_NOISE_FRAC
}

fn water_table_depth_at(
    p: &GeologyProfile,
    surface_elevation: f64,
    seed: u32,
    x: f64,
    y: f64,
) -> f64 {
    let noise = (value_noise(seed, TABLE_SALT, x, y) - 0.5) * 2.0 * 3.0;
    let wt = p.base_water_table * (1.0 - 0.4 * p.wetness)
        + surface_elevation.max(0.0) * p.wt_elev_factor
        + noise;
    wt.clamp(MIN_WATER_TABLE, MAX_WATER_TABLE)
}

/// Sample the surface elevation (m above sea level) at world `(x, y)` from the
/// real-elevation channel (mirrors `geology.ts` `sampleSurfaceElevation`).
/// Returns 0 when no elevation is present (procedural cities → flat sea level).
pub fn sample_surface_elevation(
    heights: Option<&[i16]>,
    res: u32,
    world_size: f64,
    x: f64,
    y: f64,
) -> f64 {
    let Some(h) = heights else {
        return 0.0;
    };
    if res == 0 || h.len() != (res * res) as usize {
        return 0.0;
    }
    let half = world_size / 2.0;
    let mut c = (((x + half) / world_size) * res as f64).floor() as i64;
    let mut r = (((y + half) / world_size) * res as f64).floor() as i64;
    c = c.clamp(0, res as i64 - 1);
    r = r.clamp(0, res as i64 - 1);
    h[(r * res as i64 + c) as usize] as f64
}

/// Reconstruct the full subsurface column at world `(x, y)` — the client-side
/// mirror of `geology.ts` `strataColumn` / `columnAt`. `seed` is the world
/// seed (`u64`, truncated to `u32` for the noise hash exactly like the JS,
/// which operates on 32-bit ints throughout).
pub fn strata_column(
    seed: u64,
    city_key: Option<&str>,
    heights: Option<&[i16]>,
    res: u32,
    world_size: f64,
    x: f64,
    y: f64,
) -> StrataColumn {
    let p = profile_for(city_key);
    let seed32 = seed as u32;
    let surface_elevation = sample_surface_elevation(heights, res, world_size, x, y);
    let soil_bottom = p.soil * thickness_jitter(seed32, BAND_SALT, x, y);
    let clay_bottom = soil_bottom + p.clay * thickness_jitter(seed32, BAND_SALT ^ 0x1111, x, y);
    let bedrock_top =
        clay_bottom + p.rock_thickness * thickness_jitter(seed32, BAND_SALT ^ 0x2222, x, y);
    StrataColumn {
        bands: [
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
        ],
        water_table_depth: water_table_depth_at(&p, surface_elevation, seed32, x, y),
        surface_elevation,
    }
}

/// Which stratum sits at `depth` metres below the surface in `col`.
pub fn stratum_at_depth(col: &StrataColumn, depth: f64) -> Stratum {
    for b in &col.bands {
        if depth < b.bottom {
            return b.kind;
        }
    }
    Stratum::Bedrock
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash2_is_in_unit_range() {
        for i in -50..50 {
            let v = hash2(42, BAND_SALT, i, i * 3);
            assert!((0.0..1.0).contains(&v), "hash2 out of range: {v}");
        }
    }

    #[test]
    fn column_bands_are_ordered_top_down() {
        let col = strata_column(1234, Some("nyc"), None, 0, 12000.0, 100.0, -200.0);
        let b = &col.bands;
        assert_eq!(b[0].top, 0.0);
        for w in b.windows(2) {
            assert!(w[0].bottom <= w[1].top + 1e-9, "bands must not overlap");
            assert!(w[1].top <= w[1].bottom, "band top above its bottom");
        }
        assert_eq!(b[3].bottom, BEDROCK_NOMINAL_BOTTOM);
    }

    #[test]
    fn thickness_jitter_stays_within_declared_fraction() {
        for i in 0..200 {
            let j = thickness_jitter(7, BAND_SALT, i as f64 * 13.0, i as f64 * -7.0);
            assert!(j >= 1.0 - STRATA_NOISE_FRAC - 1e-9);
            assert!(j <= 1.0 + STRATA_NOISE_FRAC + 1e-9);
        }
    }

    #[test]
    fn water_table_within_bounds_and_deeper_on_high_ground() {
        let p = profile_for(Some("nyc"));
        let low = water_table_depth_at(&p, 0.0, 5, 10.0, 10.0);
        let high = water_table_depth_at(&p, 60.0, 5, 10.0, 10.0);
        assert!((MIN_WATER_TABLE..=MAX_WATER_TABLE).contains(&low));
        assert!((MIN_WATER_TABLE..=MAX_WATER_TABLE).contains(&high));
        // Same column, higher surface → table no shallower (elevation factor).
        assert!(high >= low);
    }

    #[test]
    fn stratum_at_depth_walks_the_bands() {
        let col = strata_column(99, Some("boston"), None, 0, 12000.0, 0.0, 0.0);
        assert_eq!(stratum_at_depth(&col, 0.0), Stratum::Fill);
        assert_eq!(
            stratum_at_depth(&col, col.bands[3].top + 5.0),
            Stratum::Bedrock
        );
    }

    #[test]
    fn elevation_sample_clamps_out_of_range() {
        let heights = vec![10i16, 20, 30, 40];
        let e = sample_surface_elevation(Some(&heights), 2, 100.0, 1000.0, 1000.0);
        // Far out-of-range clamps to the last cell (40).
        assert_eq!(e, 40.0);
        let e0 = sample_surface_elevation(Some(&heights), 2, 100.0, -1000.0, -1000.0);
        assert_eq!(e0, 10.0);
    }
}
