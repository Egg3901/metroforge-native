//! Grade as an operating tradeoff. Port of `sim/src/core/transit/gradeEffects.ts`.
//!
//! Surface alignments share the street and feel the diurnal congestion curve;
//! elevated / tunnel keep full mode cruise. Density amplifies the surface
//! slowdown. Pure + deterministic (no RNG / clock). See the TS module header for
//! the full design intent and the composition rule with weather.

use crate::constants::{modes, surface_congestion_weight, TICKS_PER_DAY};
use crate::fields::sample_field;
use crate::geometry::Vec2;
use crate::transit::time_of_day::{diurnal_factor, diurnal_mean, diurnal_peak};
use crate::types::{FieldGrid, TrackGrade, TrackSegment, TransitMode};

/// Mean of `max(0, diurnalFactor - 1)` over a game day. Mirrors
/// `MEAN_RUSH_EXCESS`.
pub fn mean_rush_excess() -> f64 {
    let mut sum = 0.0;
    for t in 0..TICKS_PER_DAY as u64 {
        sum += (diurnal_factor(t) - 1.0).max(0.0);
    }
    sum / TICKS_PER_DAY as f64
}

/// Peak diurnalFactor (`DIURNAL_PEAK / DIURNAL_MEAN`). Mirrors
/// `PEAK_DIURNAL_FACTOR`.
pub fn peak_diurnal_factor() -> f64 {
    diurnal_peak() / diurnal_mean()
}

/// Map land-value (~0..3) onto a `[0,1]` density weight. Mirrors
/// `density01FromLandValue`.
pub fn density01_from_land_value(lv: f64) -> f64 {
    (lv / 2.0).clamp(0.0, 1.0)
}

/// Sample corridor density (land value) at a world point. Falls back to 0.5 when
/// fields are unavailable. Mirrors `sampleDensity01`.
pub fn sample_density01(fields: Option<&FieldGrid>, pos: Vec2) -> f64 {
    match fields {
        None => 0.5,
        Some(g) => density01_from_land_value(sample_field(g, &g.land_value, pos)),
    }
}

/// Density along a track segment (midpoint of its polyline). Mirrors
/// `segmentDensity01`.
pub fn segment_density01(fields: Option<&FieldGrid>, seg: &TrackSegment) -> f64 {
    let pts = &seg.polyline.points;
    if pts.is_empty() {
        return 0.5;
    }
    let mid = pts[pts.len() / 2];
    sample_density01(fields, mid)
}

/// Congestion slowdown multiplier (>=1). Elevated/tunnel always 1. Mirrors
/// `surfaceCongestionSlowdown`.
pub fn surface_congestion_slowdown(mode: TransitMode, density01: f64, tod_factor: f64) -> f64 {
    let excess = (tod_factor - 1.0).max(0.0);
    if excess <= 0.0 {
        return 1.0;
    }
    let dens = 0.35 + 0.65 * density01.clamp(0.0, 1.0);
    1.0 + excess * surface_congestion_weight(mode) * dens
}

/// Day-average surface slowdown for cycle/headway. Mirrors
/// `dayAverageSurfaceSlowdown`.
pub fn day_average_surface_slowdown(mode: TransitMode, density01: f64) -> f64 {
    let dens = 0.35 + 0.65 * density01.clamp(0.0, 1.0);
    1.0 + mean_rush_excess() * surface_congestion_weight(mode) * dens
}

/// Peak surface slowdown for assignment trip times. Mirrors
/// `assignmentSurfaceSlowdown`.
pub fn assignment_surface_slowdown(mode: TransitMode, density01: f64) -> f64 {
    surface_congestion_slowdown(mode, density01, peak_diurnal_factor())
}

/// Grade-only effective speed (m/s) at `tod_factor`. Mirrors
/// `segmentEffectiveSpeedMps`.
pub fn segment_effective_speed_mps(
    mode: TransitMode,
    grade: TrackGrade,
    tod_factor: f64,
    density01: f64,
) -> f64 {
    let base = modes(mode).speed;
    if grade != TrackGrade::Surface {
        return base;
    }
    base / surface_congestion_slowdown(mode, density01, tod_factor)
}

/// Day-average effective speed used by cycle time / headway. Mirrors
/// `segmentDayAverageSpeedMps`.
pub fn segment_day_average_speed_mps(mode: TransitMode, grade: TrackGrade, density01: f64) -> f64 {
    let base = modes(mode).speed;
    if grade != TrackGrade::Surface {
        return base;
    }
    base / day_average_surface_slowdown(mode, density01)
}

/// Peak-biased effective speed used by assignment ride edges. Mirrors
/// `segmentAssignmentSpeedMps`.
pub fn segment_assignment_speed_mps(mode: TransitMode, grade: TrackGrade, density01: f64) -> f64 {
    let base = modes(mode).speed;
    if grade != TrackGrade::Surface {
        return base;
    }
    base / assignment_surface_slowdown(mode, density01)
}
