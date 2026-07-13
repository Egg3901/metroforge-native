//! Cohort demand (v0.9 System B "Living City"). Port of
//! `sim/src/core/transit/cohorts.ts`.
//!
//! Residents split into four behavioural cohorts, each with an hourly departure
//! rhythm and a per-hour destination-pull mix (job / home / leisure). This
//! reshapes the OD generation in [`super::assignment`] so demand becomes
//! schedule-driven. Everything is a deterministic pure function of the tick +
//! the game seed (no RNG stream, no wall clock); POI surges are seeded off the
//! game seed so they reproduce bit-for-bit.

use crate::constants::TICKS_PER_DAY;
use crate::transit::time_of_day::hour_of_day;
use crate::types::{PoiAnchor, PoiKind};

/// A cohort's per-hour destination-pull mix. The three pulls sum to 1 for every
/// hour. Mirrors `DirBias`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DirBias {
    /// Pull toward job districts.
    pub job: f64,
    /// Pull back toward home.
    pub home: f64,
    /// Pull out to leisure / POI.
    pub leisure: f64,
}

struct CohortModel {
    base_share: f64,
    hourly: [f64; 24],
    dir: [DirBias; 24],
    weekend_tilt: f64,
}

/// Normalize a 24-length weight vector into a propensity row (sum = 1). Mirrors
/// `row`.
fn row(weights: [f64; 24]) -> [f64; 24] {
    let s: f64 = weights.iter().sum();
    if s <= 0.0 {
        return [1.0 / 24.0; 24];
    }
    let mut out = [0.0; 24];
    for i in 0..24 {
        out[i] = weights[i] / s;
    }
    out
}

/// A gaussian bump centred on `mu` hours, wrapping midnight. Mirrors `bump`.
fn bump(h: f64, mu: f64, sigma: f64, amp: f64) -> f64 {
    let mut d = (h - mu).abs();
    if d > 12.0 {
        d = 24.0 - d;
    }
    amp * (-(d * d) / (2.0 * sigma * sigma)).exp()
}

/// Build a per-hour direction-bias curve. Mirrors `dirCurve`.
fn dir_curve(leisure_floor: f64, outbound_is_leisure: bool) -> [DirBias; 24] {
    let mut out = [DirBias::default(); 24];
    for (h, slot) in out.iter_mut().enumerate() {
        let hf = ((h as f64 - 6.0) / 13.0).clamp(0.0, 1.0);
        let leisure = leisure_floor * (0.5 + 0.5 * bump(h as f64, 19.0, 4.0, 1.0));
        let remain = (1.0 - leisure).max(0.0);
        let home = remain * hf;
        let outbound = remain * (1.0 - hf);
        *slot = if outbound_is_leisure {
            DirBias {
                job: outbound * 0.35,
                home,
                leisure: leisure + outbound * 0.65,
            }
        } else {
            DirBias {
                job: outbound,
                home,
                leisure,
            }
        };
    }
    out
}

fn hourly_from<F: Fn(f64) -> f64>(f: F) -> [f64; 24] {
    let mut w = [0.0; 24];
    for (h, slot) in w.iter_mut().enumerate() {
        *slot = f(h as f64);
    }
    row(w)
}

fn cohort_models() -> [CohortModel; 4] {
    [
        // commuter
        CohortModel {
            base_share: 0.55,
            hourly: hourly_from(|h| 0.05 + bump(h, 8.0, 1.1, 1.0) + bump(h, 17.5, 1.4, 0.95)),
            dir: dir_curve(0.08, false),
            weekend_tilt: 0.45,
        },
        // student
        CohortModel {
            base_share: 0.15,
            hourly: hourly_from(|h| {
                0.04 + bump(h, 8.0, 1.0, 1.0) + bump(h, 15.0, 1.6, 0.8) + bump(h, 19.0, 1.5, 0.3)
            }),
            dir: dir_curve(0.15, false),
            weekend_tilt: 0.35,
        },
        // leisure
        CohortModel {
            base_share: 0.2,
            hourly: hourly_from(|h| 0.06 + bump(h, 13.0, 3.0, 0.7) + bump(h, 20.0, 3.2, 1.0)),
            dir: dir_curve(0.55, true),
            weekend_tilt: 1.7,
        },
        // nightShift
        CohortModel {
            base_share: 0.1,
            hourly: hourly_from(|h| 0.03 + bump(h, 22.5, 1.6, 1.0) + bump(h, 5.5, 1.6, 0.9)),
            dir: dir_curve(0.05, false),
            weekend_tilt: 0.8,
        },
    ]
}

/// Collapsed destination-pull mix per hour across all cohorts. Mirrors
/// `HOUR_ATTRACTOR`.
fn hour_attractor() -> [DirBias; 24] {
    let models = cohort_models();
    let mut table = [DirBias::default(); 24];
    for (h, slot) in table.iter_mut().enumerate() {
        let (mut job, mut home, mut leisure, mut w) = (0.0, 0.0, 0.0, 0.0);
        for c in &models {
            let cw = c.base_share * c.hourly[h];
            let d = c.dir[h];
            job += cw * d.job;
            home += cw * d.home;
            leisure += cw * d.leisure;
            w += cw;
        }
        if w > 0.0 {
            job /= w;
            home /= w;
            leisure /= w;
        } else {
            job = 1.0;
        }
        *slot = DirBias { job, home, leisure };
    }
    table
}

/// Un-normalized total demand weight per hour. Mirrors `HOURLY_RAW`.
fn hourly_raw() -> [f64; 24] {
    let models = cohort_models();
    let mut out = [0.0; 24];
    for (h, slot) in out.iter_mut().enumerate() {
        let mut s = 0.0;
        for c in &models {
            s += c.base_share * c.hourly[h];
        }
        *slot = s;
    }
    out
}

/// Total demand factor per hour, normalized so the 24-hour mean is 1.0. Mirrors
/// `HOURLY_DEMAND`.
fn hourly_demand() -> [f64; 24] {
    let raw = hourly_raw();
    let mean: f64 = raw.iter().sum::<f64>() / 24.0;
    let mut out = [0.0; 24];
    for i in 0..24 {
        out[i] = if mean > 0.0 { raw[i] / mean } else { 1.0 };
    }
    out
}

/// Integer hour bucket `[0,24)` for a sim tick. Mirrors `hourBucket`.
pub fn hour_bucket(tick: u64) -> usize {
    let h = hour_of_day(tick).floor() as i64 % 24;
    (if h < 0 { h + 24 } else { h }) as usize
}

/// True on weekend game-days (day-of-week 5,6). Mirrors `isWeekend`.
pub fn is_weekend(tick: u64) -> bool {
    let day = (tick / TICKS_PER_DAY as u64) as i64;
    let dow = ((day % 7) + 7) % 7;
    dow >= 5
}

/// The destination-pull mix the assignment should use at `tick`, including the
/// weekend leisure tilt. Mirrors `attractorAt`.
pub fn attractor_at(tick: u64) -> DirBias {
    let base = hour_attractor()[hour_bucket(tick)];
    if !is_weekend(tick) {
        return base;
    }
    let job = base.job * 0.55;
    let home = base.home;
    let leisure = base.leisure * 1.9 + 0.05;
    let s = {
        let t = job + home + leisure;
        if t == 0.0 {
            1.0
        } else {
            t
        }
    };
    DirBias {
        job: job / s,
        home: home / s,
        leisure: leisure / s,
    }
}

/// Live time-of-day demand factor (daily mean 1.0) at `tick`. Mirrors
/// `cohortDemandFactor`.
pub fn cohort_demand_factor(tick: u64) -> f64 {
    let h = hour_bucket(tick);
    if !is_weekend(tick) {
        return hourly_demand()[h];
    }
    let models = cohort_models();
    let mut s = 0.0;
    for c in &models {
        s += c.base_share * c.weekend_tilt * c.hourly[h];
    }
    let mut mean = 0.0;
    for hh in 0..24 {
        for c in &models {
            mean += c.base_share * c.weekend_tilt * c.hourly[hh];
        }
    }
    mean /= 24.0;
    if mean > 0.0 {
        s / mean
    } else {
        1.0
    }
}

// ── POI surges (System B, B2) ────────────────────────────────────────────────

/// Upper bound on any single POI surge multiplier. Mirrors `MAX_POI_SURGE`.
pub const MAX_POI_SURGE: f64 = 6.0;

/// Deterministic per-day, per-anchor hash in `[0,1)`. Mirrors `anchorDayHash`.
fn anchor_day_hash(seed: u32, anchor_id: &str, day: i64) -> f64 {
    let mut h: u32 = seed ^ 0x9e37_79b1;
    for u in anchor_id.encode_utf16() {
        h = (h ^ u as u32).wrapping_mul(16_777_619);
    }
    h = (h ^ ((day & 0xffff) as u32)).wrapping_mul(16_777_619);
    h = (h ^ (((day >> 16) & 0xffff) as u32)).wrapping_mul(16_777_619);
    (h as f64) / 4_294_967_296.0
}

/// Surge multiplier for a POI anchor at a given tick. Always in
/// `[1, MAX_POI_SURGE]`. Mirrors `poiSurge`.
pub fn poi_surge(anchor: &PoiAnchor, seed: u32, tick: u64) -> f64 {
    let day = (tick / TICKS_PER_DAY as u64) as i64;
    let hour = hour_of_day(tick);
    let weekend = is_weekend(tick);
    match anchor.kind {
        PoiKind::Stadium => {
            let game_day = anchor_day_hash(seed, &anchor.id, day) < 0.28;
            if !game_day {
                return 1.0;
            }
            1.0 + bump(hour, 19.0, 2.2, MAX_POI_SURGE - 1.0)
        }
        PoiKind::Airport => {
            let peak = bump(hour, 7.0, 2.5, 1.4) + bump(hour, 18.0, 3.0, 1.6);
            (1.3 + peak).min(MAX_POI_SURGE)
        }
        PoiKind::University => {
            if weekend {
                return 1.0;
            }
            let peak = bump(hour, 8.0, 1.4, 1.1) + bump(hour, 16.0, 2.0, 0.8);
            (1.0 + peak).min(MAX_POI_SURGE)
        }
        _ => (1.0 + bump(hour, 13.0, 4.0, 0.5)).min(MAX_POI_SURGE),
    }
}
