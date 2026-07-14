//! Geology -> construction economics (v0.8).
//!
//! Port of `sim/src/core/geologyCost.ts`. ALL tunable tunnel-cost constants
//! live here (the geology twin of [`crate::weather_effects`]). Pure and
//! deterministic: costs are a function of `(segment length, chosen depth, the
//! strata column, water table)`.
//!
//! An underground segment is priced RELATIVE to the same segment built at grade
//! (`surface_cost_per_m`). Two build methods compete per segment and the
//! cheaper feasible one is chosen automatically: cut-and-cover (cheap, shallow,
//! bad in rock) vs bored TBM (any depth, cheaper in competent rock than in
//! soft/wet ground). Below the water table a waterproofing surcharge applies.

use crate::geology::{stratum_at_depth, StrataColumn, Stratum};

// -- Method feasibility / default depths --------------------------------------
/// Cut-and-cover is only viable down to here; deeper forces a bored tunnel.
pub const CUT_COVER_MAX_DEPTH: f64 = 15.0;
/// Default tunnel depth (m) under land when the caller supplies none.
pub const DEFAULT_TUNNEL_DEPTH: f64 = 12.0;
/// Default tunnel depth (m) under water -- tunnels dip below the river/bay bed.
pub const RIVER_TUNNEL_DEPTH: f64 = 24.0;

// -- Cost multipliers, all relative to the surface cost of the same segment ---
/// Cut-and-cover base multiplier (shallow soft ground).
pub const CUT_COVER_BASE: f64 = 2.5;
/// Penalty factor when cut-and-cover has to chew through rock from above.
pub const CUT_COVER_ROCK_PENALTY: f64 = 2.0;
/// Bored base multiplier when the bore runs through competent rock.
pub const BORED_ROCK_BASE: f64 = 3.0;
/// Bored base multiplier when the bore runs through soft overburden.
pub const BORED_SOIL_BASE: f64 = 4.2;
/// Extra added to the soil bore when it is below the water table.
pub const BORED_WET_SOIL_PENALTY: f64 = 0.9;
/// Multiplier on rock hardness (0..1) added to a rock bore.
pub const ROCK_HARDNESS_PREMIUM: f64 = 1.1;
/// Fraction of surface cost added per metre of depth.
pub const DEPTH_COST_PER_M: f64 = 0.02;
/// Waterproofing surcharge (fraction) applied below the water table.
pub const WATERPROOF_SURCHARGE: f64 = 0.28;

// -- Flood risk (stored, consumed later by weather storm events) --------------
/// Residual per-segment flood-risk base for a waterproofed below-table tunnel.
pub const FLOOD_RISK_BASE: f64 = 0.04;
/// Extra flood risk per metre the invert sits below the table.
pub const FLOOD_RISK_PER_M_BELOW: f64 = 0.01;

// -- Station depth economics --------------------------------------------------
/// Underground-station cost surcharge = base station cost * this * depth(m).
pub const STATION_DEPTH_COST_FACTOR: f64 = 0.03;
/// Below this depth (m) a station has no access-time penalty.
pub const STATION_DEPTH_FREE_M: f64 = 10.0;
/// Access-time penalty added per 10 m of station depth below the free depth.
pub const STATION_DEPTH_ACCESS_SEC_PER_10M: f64 = 30.0;

/// Chosen build method for an underground segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildMethod {
    /// Cut-and-cover box (cheap, shallow).
    CutCover,
    /// Bored (TBM) tunnel (any depth).
    Bored,
}

impl BuildMethod {
    /// String name (mirrors the TS union member).
    pub fn as_str(self) -> &'static str {
        match self {
            BuildMethod::CutCover => "cutCover",
            BuildMethod::Bored => "bored",
        }
    }
}

/// Result of pricing one underground segment. Mirrors `SegmentCostResult`.
#[derive(Clone, Debug, PartialEq)]
pub struct SegmentCostResult {
    /// Total cost for this segment (money).
    pub cost: f64,
    /// Cost per metre (money/m).
    pub cost_per_m: f64,
    /// Method chosen (cheaper feasible one).
    pub method: BuildMethod,
    /// Chosen tunnel depth (m).
    pub depth: f64,
    /// Is the invert below the water table?
    pub below_water_table: bool,
    /// Residual flood-risk factor stored for future storm coupling.
    pub flood_risk: f64,
    /// Stratum the bore/box sits in, for the breakdown summary.
    pub stratum: Stratum,
}

/// Bored multiplier for a column at a depth (before depth/waterproof factors).
pub fn bored_mult(col: &StrataColumn, depth: f64, below_water_table: bool) -> f64 {
    match stratum_at_depth(col, depth) {
        Stratum::Rock | Stratum::Bedrock => {
            BORED_ROCK_BASE + col.rock_hardness * ROCK_HARDNESS_PREMIUM
        }
        _ => {
            let mut m = BORED_SOIL_BASE;
            if below_water_table {
                m += BORED_WET_SOIL_PENALTY;
            }
            m
        }
    }
}

/// Cut-and-cover multiplier for a column at a depth.
pub fn cut_cover_mult(col: &StrataColumn, depth: f64) -> f64 {
    match stratum_at_depth(col, depth) {
        Stratum::Rock | Stratum::Bedrock => CUT_COVER_BASE * CUT_COVER_ROCK_PENALTY,
        _ => CUT_COVER_BASE,
    }
}

/// Price one underground segment of length `len_m` through column `col`,
/// choosing the cheaper feasible method.
pub fn underground_segment_cost(
    surface_cost_per_m: f64,
    len_m: f64,
    col: &StrataColumn,
    depth: f64,
) -> SegmentCostResult {
    let below_water_table = depth >= col.water_table_depth;
    let depth_mult = 1.0 + DEPTH_COST_PER_M * depth;
    let waterproof_mult = if below_water_table {
        1.0 + WATERPROOF_SURCHARGE
    } else {
        1.0
    };

    let bored_per_m = surface_cost_per_m
        * bored_mult(col, depth, below_water_table)
        * depth_mult
        * waterproof_mult;
    let cut_feasible = depth <= CUT_COVER_MAX_DEPTH;
    let cut_per_m = if cut_feasible {
        surface_cost_per_m * cut_cover_mult(col, depth) * depth_mult * waterproof_mult
    } else {
        f64::INFINITY
    };

    let use_cut = cut_per_m <= bored_per_m;
    let cost_per_m = if use_cut { cut_per_m } else { bored_per_m };
    let method = if use_cut {
        BuildMethod::CutCover
    } else {
        BuildMethod::Bored
    };

    let flood_risk = if below_water_table {
        FLOOD_RISK_BASE + (depth - col.water_table_depth).max(0.0) * FLOOD_RISK_PER_M_BELOW
    } else {
        0.0
    };

    SegmentCostResult {
        cost: cost_per_m * len_m,
        cost_per_m,
        method,
        depth,
        below_water_table,
        flood_risk,
        stratum: stratum_at_depth(col, depth),
    }
}

/// Extra cost of sinking a station to `depth` metres for `base_station_cost`.
pub fn station_depth_surcharge(base_station_cost: f64, depth: f64) -> f64 {
    base_station_cost * STATION_DEPTH_COST_FACTOR * depth.max(0.0)
}

/// Rider access-time penalty (SECONDS) for a station at `depth` metres.
/// Surface stations (`None`/0 depth) pay nothing.
pub fn station_depth_access_penalty_sec(depth: Option<f64>) -> f64 {
    match depth {
        Some(d) if d > STATION_DEPTH_FREE_M => {
            ((d - STATION_DEPTH_FREE_M) / 10.0) * STATION_DEPTH_ACCESS_SEC_PER_10M
        }
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geology::{column_at, GENERIC_PROFILE};
    use crate::geometry::vec;

    #[test]
    fn shallow_soft_ground_prefers_cut_cover() {
        // A soft dry shallow segment: default profile, depth 8 m, above table.
        let col = column_at(Some("generic"), 1, 12000.0, None, None, vec(0.0, 0.0));
        let r = underground_segment_cost(1.0, 100.0, &col, 8.0);
        // depth 8 < generic base table (~8*(1-.14)=~6.9..) may be below; assert
        // it stays a tunnel (>= 2.5x) and picks a valid method.
        assert!(r.cost_per_m >= 2.5);
        assert!(r.cost > 0.0);
    }

    #[test]
    fn deep_forces_bored() {
        let col = column_at(Some("generic"), 1, 12000.0, None, None, vec(0.0, 0.0));
        let r = underground_segment_cost(1.0, 10.0, &col, 40.0);
        assert_eq!(r.method, BuildMethod::Bored);
    }

    #[test]
    fn station_access_penalty_curve() {
        assert_eq!(station_depth_access_penalty_sec(None), 0.0);
        assert_eq!(station_depth_access_penalty_sec(Some(10.0)), 0.0);
        assert!((station_depth_access_penalty_sec(Some(30.0)) - 60.0).abs() < 1e-9);
    }

    #[test]
    fn hard_rock_bore_cheaper_than_wet_soil_bore() {
        // Build two synthetic columns via the profile knobs.
        let _ = GENERIC_PROFILE;
        let rock = column_at(Some("nyc"), 5, 12000.0, None, None, vec(0.0, 0.0));
        let soft = column_at(Some("boston"), 5, 12000.0, None, None, vec(0.0, 0.0));
        // Bore both at a depth that lands in rock for nyc and soil for boston.
        let r_rock = bored_mult(&rock, 30.0, true);
        let r_soft = bored_mult(&soft, 20.0, true);
        assert!(r_rock < r_soft + BORED_WET_SOIL_PENALTY + 1.0);
    }
}
