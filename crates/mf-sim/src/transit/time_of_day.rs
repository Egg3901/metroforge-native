//! Time-of-day travel-demand model. Port of `sim/src/core/timeOfDay.ts`.
//!
//! LANE NOTE (P3-TRANSIT): `timeOfDay.ts` is a shared P3 module that other
//! lanes (economy, ops) will also want. To avoid a merge conflict over a
//! top-level `time_of_day.rs`, the transit lane keeps its own copy here under
//! `transit/`. The integration owner may promote this to a shared module and
//! re-point the `use` sites; the functions are a faithful, self-contained port.
//!
//! Every function is a deterministic pure function of the sim tick (no wall
//! clock, no RNG), reproducing the TS curve.

use crate::constants::TICKS_PER_DAY;

/// Hour of the game day in `[0,24)` for an absolute tick. Mirrors `hourOfDay`.
pub fn hour_of_day(tick: u64) -> f64 {
    let tpd = TICKS_PER_DAY as f64;
    let m = (tick % TICKS_PER_DAY as u64) as f64;
    (m / tpd) * 24.0
}

/// Raw diurnal travel-demand multiplier (two rush peaks, quiet night). Mirrors
/// `diurnalDemand`.
pub fn diurnal_demand(tick: u64) -> f64 {
    let hour = hour_of_day(tick);
    let am = (-((hour - 8.0).powi(2)) / 6.0).exp();
    let pm = (-((hour - 17.5).powi(2)) / 8.0).exp();
    let mut f = 0.55 + 1.35 * (am + pm);
    if hour < 5.5 {
        f *= 0.35;
    } else if hour > 22.0 {
        f *= 0.45;
    }
    f
}

/// Daily mean of [`diurnal_demand`] over a whole game day. Mirrors
/// `DIURNAL_MEAN`.
pub fn diurnal_mean() -> f64 {
    let mut sum = 0.0;
    for t in 0..TICKS_PER_DAY as u64 {
        sum += diurnal_demand(t);
    }
    sum / TICKS_PER_DAY as f64
}

/// The busiest single tick's raw demand. Mirrors `DIURNAL_PEAK`.
pub fn diurnal_peak() -> f64 {
    let mut peak = 0.0f64;
    for t in 0..TICKS_PER_DAY as u64 {
        let d = diurnal_demand(t);
        if d > peak {
            peak = d;
        }
    }
    peak
}

/// Live time-of-day multiplier normalized so its daily mean is exactly 1.0.
/// Mirrors `diurnalFactor`.
pub fn diurnal_factor(tick: u64) -> f64 {
    diurnal_demand(tick) / diurnal_mean()
}
