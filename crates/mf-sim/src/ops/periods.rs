//! Service periods + time-of-day demand curve for v0.9 System A (Operations).
//!
//! Ports `sim/src/core/ops/periods.ts` and the subset of
//! `sim/src/core/timeOfDay.ts` the ops system needs. Every function here is a
//! pure, deterministic function of the sim tick (no wall-clock, no RNG) so it
//! reproduces bit-for-bit across runs and the native port.
//!
//! Five periods span the game day: AM peak, midday, PM peak, evening, night.

use crate::constants::TICKS_PER_DAY;
use crate::types::Period;
use std::sync::LazyLock;

// Time-of-day curve is CANONICAL in `crate::transit::time_of_day` (dedup at
// integration). Re-export the shared functions so `period_for_tick` and
// `MEAN_RUSH_EXCESS` below use the single implementation the rest of the sim
// (grade effects, vehicle movement, traffic) also uses.
pub use crate::transit::time_of_day::{diurnal_demand, diurnal_factor, hour_of_day};

/// All periods in day order (stable iteration for schedules / peak sizing).
/// Mirrors `PERIODS` (periods.ts).
pub const PERIODS: [Period; 5] = [
    Period::AmPeak,
    Period::Midday,
    Period::PmPeak,
    Period::Evening,
    Period::Night,
];

/// Which service period an absolute tick falls in, by hour of the game day.
/// Boundaries are fixed (not tunable) so they never enter economy balance:
///   night `[0,6)`   amPeak `[6,9.5)`   midday `[9.5,16)`
///   pmPeak `[16,19)` evening `[19,22)` night `[22,24)`.
/// Mirrors `periodForTick` (periods.ts).
pub fn period_for_tick(tick: u64) -> Period {
    let h = hour_of_day(tick);
    if h < 6.0 {
        Period::Night
    } else if h < 9.5 {
        Period::AmPeak
    } else if h < 16.0 {
        Period::Midday
    } else if h < 19.0 {
        Period::PmPeak
    } else if h < 22.0 {
        Period::Evening
    } else {
        Period::Night
    }
}

/// Human label for HUD / toasts (no em/en dashes). Mirrors `PERIOD_LABEL`.
pub fn period_label(period: Period) -> &'static str {
    match period {
        Period::AmPeak => "AM peak",
        Period::Midday => "Midday",
        Period::PmPeak => "PM peak",
        Period::Evening => "Evening",
        Period::Night => "Night",
    }
}

/// Mean of `max(0, diurnalFactor - 1)` over a game day. Precomputed once so the
/// headway / cycle-time derivation can apply a day-average surface slowdown
/// (vehicles run all day) without integrating the curve on every edge.
/// Mirrors `MEAN_RUSH_EXCESS` (transit/gradeEffects.ts).
pub static MEAN_RUSH_EXCESS: LazyLock<f64> = LazyLock::new(|| {
    let mut sum = 0.0;
    for t in 0..TICKS_PER_DAY as u64 {
        sum += (diurnal_factor(t) - 1.0).max(0.0);
    }
    sum / TICKS_PER_DAY as f64
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_boundaries_match_ts() {
        // tick 0 = midnight -> Night. TICKS_PER_DAY = 1200 -> 50 ticks/hour.
        assert_eq!(period_for_tick(0), Period::Night); // hour 0
        assert_eq!(period_for_tick(300), Period::AmPeak); // hour 6
        assert_eq!(period_for_tick(475), Period::Midday); // hour 9.5
        assert_eq!(period_for_tick(800), Period::PmPeak); // hour 16
        assert_eq!(period_for_tick(950), Period::Evening); // hour 19
        assert_eq!(period_for_tick(1100), Period::Night); // hour 22
                                                          // wraps across days.
        assert_eq!(period_for_tick(1200), Period::Night);
        assert_eq!(period_for_tick(1200 + 300), Period::AmPeak);
    }

    #[test]
    fn diurnal_mean_is_normalized() {
        // diurnal_factor has daily mean exactly 1.0 by construction.
        let mut sum = 0.0;
        for t in 0..TICKS_PER_DAY as u64 {
            sum += diurnal_factor(t);
        }
        let mean = sum / TICKS_PER_DAY as f64;
        assert!((mean - 1.0).abs() < 1e-9, "mean={mean}");
    }

    #[test]
    fn rush_excess_is_positive_and_small() {
        // peaks push some ticks above the mean; the day-average excess is modest.
        assert!(*MEAN_RUSH_EXCESS > 0.0 && *MEAN_RUSH_EXCESS < 1.0);
    }
}
