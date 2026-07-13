//! Weather state machine (v0.7 Weather & Light -- sim side).
//!
//! Port of `sim/src/core/weather.ts`. Weather is a pure function of
//! `(seed, tick, city climate)`: same seed => same weather forever, and the
//! current sky is reconstructed in O(1) from the seed alone, so it survives
//! save/load with no stored history. It draws from DERIVED salted streams (never
//! `state.rng_state`), so enabling weather does not perturb the city-event /
//! growth RNG. Seasonal climate is per-city; a 1-step persistence rule gives
//! realistic clustering (a rainy day tends to follow a rainy day).

use crate::constants::TICKS_PER_DAY;
use crate::rng::Rng;

/// Sky state. Canonical order below is the index space the climate
/// distributions are written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeatherState {
    /// Clear sky.
    Clear,
    /// Overcast.
    Overcast,
    /// Rain.
    Rain,
    /// Fog.
    Fog,
    /// Snow.
    Snow,
    /// Storm.
    Storm,
}

/// Canonical order (index space for the climate distributions).
pub const WEATHER_STATES: [WeatherState; 6] = [
    WeatherState::Clear,
    WeatherState::Overcast,
    WeatherState::Rain,
    WeatherState::Fog,
    WeatherState::Snow,
    WeatherState::Storm,
];

impl WeatherState {
    /// Lowercase string name (mirrors the TS union member).
    pub fn as_str(self) -> &'static str {
        match self {
            WeatherState::Clear => "clear",
            WeatherState::Overcast => "overcast",
            WeatherState::Rain => "rain",
            WeatherState::Fog => "fog",
            WeatherState::Snow => "snow",
            WeatherState::Storm => "storm",
        }
    }
    /// "Higher = heavier/wetter"; drives persistence + default intensity.
    fn severity(self) -> u32 {
        match self {
            WeatherState::Clear => 0,
            WeatherState::Overcast => 1,
            WeatherState::Fog => 2,
            WeatherState::Rain => 3,
            WeatherState::Snow => 4,
            WeatherState::Storm => 5,
        }
    }
    /// Softening ladder -- one step milder. Snow stays snow.
    fn milder(self) -> WeatherState {
        match self {
            WeatherState::Storm => WeatherState::Rain,
            WeatherState::Rain => WeatherState::Overcast,
            WeatherState::Fog => WeatherState::Overcast,
            WeatherState::Overcast => WeatherState::Clear,
            WeatherState::Snow => WeatherState::Snow,
            WeatherState::Clear => WeatherState::Clear,
        }
    }
    /// Baseline intensity for a state at full strength, before hourly shaping.
    fn base_intensity(self) -> f64 {
        match self {
            WeatherState::Clear => 0.15,
            WeatherState::Overcast => 0.35,
            WeatherState::Fog => 0.55,
            WeatherState::Rain => 0.6,
            WeatherState::Snow => 0.65,
            WeatherState::Storm => 0.85,
        }
    }
}

/// Meteorological season.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Season {
    /// Dec, Jan, Feb.
    Winter,
    /// Mar, Apr, May.
    Spring,
    /// Jun, Jul, Aug.
    Summer,
    /// Sep, Oct, Nov.
    Autumn,
}

impl Season {
    /// Lowercase string name.
    pub fn as_str(self) -> &'static str {
        match self {
            Season::Winter => "winter",
            Season::Spring => "spring",
            Season::Summer => "summer",
            Season::Autumn => "autumn",
        }
    }
}

/// Derived, headline weather event (surfaced to gameplay + UI).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeatherEvent {
    /// Heavy snow/storm.
    Blizzard,
    /// Hot clear summer afternoon.
    Heatwave,
}

impl WeatherEvent {
    /// Lowercase string name.
    pub fn as_str(self) -> &'static str {
        match self {
            WeatherEvent::Blizzard => "blizzard",
            WeatherEvent::Heatwave => "heatwave",
        }
    }
}

/// The full sky at an absolute tick. Mirrors `WeatherSnapshot`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherSnapshot {
    /// Sky state.
    pub state: WeatherState,
    /// 0..1 -- how hard it is coming down (or, for clear summer, heat level).
    pub intensity: f64,
    /// Season.
    pub season: Season,
    /// 0..11, January = 0.
    pub month: u32,
    /// Headline event, when the sky crosses a gameplay threshold.
    pub event: Option<WeatherEvent>,
}

// -- Calendar -----------------------------------------------------------------
/// Days in a weather/economy year.
pub const DAYS_PER_YEAR: i64 = 365;
const DAYS_PER_MONTH: f64 = DAYS_PER_YEAR as f64 / 12.0;

/// Absolute day index since the run began.
pub fn day_index(tick: u64) -> i64 {
    (tick / u64::from(TICKS_PER_DAY)) as i64
}

/// Day of the year in `[0, 365)`.
pub fn day_of_year(tick: u64) -> i64 {
    (day_index(tick) % DAYS_PER_YEAR + DAYS_PER_YEAR) % DAYS_PER_YEAR
}

/// Month for an absolute day index, `[0, 11]`.
pub fn month_for_day(day: i64) -> u32 {
    let doy = (day % DAYS_PER_YEAR + DAYS_PER_YEAR) % DAYS_PER_YEAR;
    (11).min((doy as f64 / DAYS_PER_MONTH).floor() as i64) as u32
}

/// Season for a month index (northern-hemisphere meteorological seasons).
pub fn season_of_month(month: u32) -> Season {
    if month == 11 || month <= 1 {
        Season::Winter
    } else if month <= 4 {
        Season::Spring
    } else if month <= 7 {
        Season::Summer
    } else {
        Season::Autumn
    }
}

/// Hour of the game day in `[0,24)` for an absolute tick (mirrors
/// `timeOfDay.hourOfDay`).
fn hour_of_day(tick: u64) -> f64 {
    (tick % u64::from(TICKS_PER_DAY)) as f64 / f64::from(TICKS_PER_DAY) * 24.0
}

// -- City climate profiles ----------------------------------------------------
#[derive(Clone, Copy)]
struct SeasonWeights {
    overcast: f64,
    rain: f64,
    fog: f64,
    snow: f64,
    storm: f64,
}

const fn sw(overcast: f64, rain: f64, fog: f64, snow: f64, storm: f64) -> SeasonWeights {
    SeasonWeights {
        overcast,
        rain,
        fog,
        snow,
        storm,
    }
}

struct ClimateProfile {
    winter: SeasonWeights,
    spring: SeasonWeights,
    summer: SeasonWeights,
    autumn: SeasonWeights,
}

impl ClimateProfile {
    fn for_season(&self, s: Season) -> SeasonWeights {
        match s {
            Season::Winter => self.winter,
            Season::Spring => self.spring,
            Season::Summer => self.summer,
            Season::Autumn => self.autumn,
        }
    }
}

const GENERIC_PROFILE: ClimateProfile = ClimateProfile {
    winter: sw(0.30, 0.16, 0.08, 0.22, 0.03),
    spring: sw(0.26, 0.24, 0.06, 0.03, 0.06),
    summer: sw(0.18, 0.18, 0.03, 0.00, 0.10),
    autumn: sw(0.28, 0.22, 0.09, 0.02, 0.05),
};

fn city_profile(key: &str) -> &'static ClimateProfile {
    match key {
        "nyc" => &NYC,
        "la" => &LA,
        "seattle" => &SEATTLE,
        "chicago" => &CHICAGO,
        "boston" => &BOSTON,
        "atlanta" => &ATLANTA,
        "sf" => &SF,
        "dc" => &DC,
        "philly" => &PHILLY,
        "cleveland" => &CLEVELAND,
        _ => &GENERIC_PROFILE,
    }
}

const NYC: ClimateProfile = ClimateProfile {
    winter: sw(0.32, 0.14, 0.05, 0.30, 0.04),
    spring: sw(0.26, 0.26, 0.05, 0.04, 0.07),
    summer: sw(0.18, 0.20, 0.02, 0.00, 0.14),
    autumn: sw(0.28, 0.22, 0.06, 0.02, 0.06),
};
const LA: ClimateProfile = ClimateProfile {
    winter: sw(0.16, 0.14, 0.10, 0.00, 0.02),
    spring: sw(0.12, 0.06, 0.10, 0.00, 0.01),
    summer: sw(0.06, 0.01, 0.12, 0.00, 0.00),
    autumn: sw(0.10, 0.05, 0.11, 0.00, 0.01),
};
const SEATTLE: ClimateProfile = ClimateProfile {
    winter: sw(0.40, 0.34, 0.12, 0.05, 0.02),
    spring: sw(0.38, 0.28, 0.10, 0.01, 0.02),
    summer: sw(0.22, 0.10, 0.06, 0.00, 0.01),
    autumn: sw(0.40, 0.32, 0.12, 0.01, 0.03),
};
const CHICAGO: ClimateProfile = ClimateProfile {
    winter: sw(0.34, 0.12, 0.05, 0.34, 0.04),
    spring: sw(0.28, 0.26, 0.05, 0.05, 0.09),
    summer: sw(0.18, 0.18, 0.02, 0.00, 0.16),
    autumn: sw(0.30, 0.22, 0.06, 0.03, 0.07),
};
const BOSTON: ClimateProfile = ClimateProfile {
    winter: sw(0.32, 0.16, 0.06, 0.32, 0.08),
    spring: sw(0.28, 0.26, 0.07, 0.05, 0.06),
    summer: sw(0.18, 0.18, 0.03, 0.00, 0.11),
    autumn: sw(0.30, 0.24, 0.08, 0.02, 0.06),
};
const ATLANTA: ClimateProfile = ClimateProfile {
    winter: sw(0.28, 0.22, 0.07, 0.05, 0.05),
    spring: sw(0.24, 0.26, 0.05, 0.00, 0.12),
    summer: sw(0.18, 0.22, 0.03, 0.00, 0.20),
    autumn: sw(0.24, 0.20, 0.06, 0.00, 0.08),
};
const SF: ClimateProfile = ClimateProfile {
    winter: sw(0.24, 0.24, 0.14, 0.00, 0.03),
    spring: sw(0.20, 0.12, 0.18, 0.00, 0.02),
    summer: sw(0.14, 0.03, 0.30, 0.00, 0.00),
    autumn: sw(0.18, 0.10, 0.20, 0.00, 0.01),
};
const DC: ClimateProfile = ClimateProfile {
    winter: sw(0.30, 0.18, 0.06, 0.16, 0.04),
    spring: sw(0.26, 0.26, 0.05, 0.02, 0.09),
    summer: sw(0.18, 0.20, 0.03, 0.00, 0.16),
    autumn: sw(0.28, 0.22, 0.06, 0.01, 0.06),
};
const PHILLY: ClimateProfile = ClimateProfile {
    winter: sw(0.30, 0.16, 0.06, 0.24, 0.04),
    spring: sw(0.26, 0.26, 0.05, 0.03, 0.08),
    summer: sw(0.18, 0.20, 0.02, 0.00, 0.15),
    autumn: sw(0.28, 0.22, 0.06, 0.02, 0.06),
};
const CLEVELAND: ClimateProfile = ClimateProfile {
    winter: sw(0.40, 0.14, 0.06, 0.34, 0.03),
    spring: sw(0.34, 0.26, 0.06, 0.05, 0.07),
    summer: sw(0.24, 0.18, 0.03, 0.00, 0.12),
    autumn: sw(0.38, 0.22, 0.07, 0.03, 0.06),
};

/// Minimum probability mass kept on `clear` so nowhere is perpetually grim.
const CLEAR_FLOOR: f64 = 0.05;

/// A normalized 6-vector (indexed by [`WEATHER_STATES`]) from a season's weights.
fn dist_from_weights(w: SeasonWeights) -> [f64; 6] {
    let non_clear = w.overcast + w.rain + w.fog + w.snow + w.storm;
    let clear = CLEAR_FLOOR.max(1.0 - non_clear);
    let raw = [clear, w.overcast, w.rain, w.fog, w.snow, w.storm];
    let total: f64 = raw.iter().sum();
    let mut out = [0.0; 6];
    for i in 0..6 {
        out[i] = raw[i] / total;
    }
    out
}

/// A city's 12-month x 6-state climate table, each row summing to 1. Built from
/// the city's seasonal weights (a fixed loop, no RNG / clock).
pub fn climate_table(city_key: Option<&str>) -> Vec<[f64; 6]> {
    let profile = city_profile(city_key.unwrap_or("generic"));
    (0..12)
        .map(|m| dist_from_weights(profile.for_season(season_of_month(m))))
        .collect()
}

// -- The chain ----------------------------------------------------------------
const DAY_SALT: u32 = 0x7f4a_7c15;
const PERSIST_SALT: u32 = 0x94d0_49bb;
const INTENSITY_SALT: u32 = 0x2545_f491;

/// Probability that a heavier previous day carries into today (clustering).
pub const WEATHER_PERSISTENCE: f64 = 0.45;
/// Blizzard when snow or storm is coming down hard.
pub const BLIZZARD_INTENSITY: f64 = 0.62;
/// Heat wave when a clear summer day runs hot.
pub const HEATWAVE_INTENSITY: f64 = 0.7;

/// `Math.imul` mirror: low-32-bit signed multiply of an integer by a constant.
fn imul(a: i64, b: u32) -> u32 {
    (a as u32).wrapping_mul(b)
}

/// A single day's raw climate draw -- a pure function of `(seed, day)`.
fn raw_day_draw(seed: u32, day: i64, table: &[[f64; 6]]) -> WeatherState {
    let month = month_for_day(day) as usize;
    let dist = &table[month];
    let mut rng = Rng::from_seed(seed ^ DAY_SALT ^ imul(day + 1, 0x9e37_79b1));
    WEATHER_STATES[rng.weighted(dist)]
}

/// The day's settled base state, with 1-step persistence.
pub fn weather_day_state(seed: u32, day: i64, table: &[[f64; 6]]) -> WeatherState {
    let today = raw_day_draw(seed, day, table);
    if day <= 0 {
        return today;
    }
    let yesterday = raw_day_draw(seed, day - 1, table);
    if yesterday.severity() > today.severity() {
        let mut pr = Rng::from_seed(seed ^ PERSIST_SALT ^ imul(day + 1, 0x85eb_ca77));
        if pr.next_f64() < WEATHER_PERSISTENCE {
            return yesterday;
        }
    }
    today
}

/// The full sky at an absolute tick -- pure function of `(seed, tick, climate)`.
/// Mirrors `weatherAt`.
pub fn weather_at(seed: u32, tick: u64, table: &[[f64; 6]]) -> WeatherSnapshot {
    let day = day_index(tick);
    let base = weather_day_state(seed, day, table);
    let month = month_for_day(day);
    let season = season_of_month(month);
    let hour = hour_of_day(tick);
    let hour_slot = hour.floor() as i64;

    let mut jr =
        Rng::from_seed(seed ^ INTENSITY_SALT ^ imul(day * 24 + hour_slot + 1, 0x27d4_eb2f));
    let jitter = 0.6 + 0.4 * jr.next_f64(); // 0.6..1.0

    // Diurnal shaping.
    let diurnal = match base {
        WeatherState::Fog => {
            if hour < 9.0 {
                1.0
            } else {
                (1.0 - (hour - 9.0) * 0.12).max(0.25)
            }
        }
        WeatherState::Storm => 0.7 + 0.3 * (-((hour - 16.0).powi(2)) / 20.0).exp(),
        WeatherState::Clear => 0.6 + 0.4 * (-((hour - 15.0).powi(2)) / 24.0).exp(),
        _ => 1.0,
    };

    let mut intensity = (base.base_intensity() * jitter * diurnal).clamp(0.0, 1.0);

    // Hourly softening.
    let mut state = base;
    if intensity < 0.4 && jr.next_f64() < 0.5 {
        state = base.milder();
        intensity = (state.base_intensity() * jitter * diurnal).clamp(0.0, 1.0);
    }

    let mut snap = WeatherSnapshot {
        state,
        intensity,
        season,
        month,
        event: None,
    };

    // Headline events.
    if (state == WeatherState::Snow || state == WeatherState::Storm)
        && intensity >= BLIZZARD_INTENSITY
    {
        snap.event = Some(WeatherEvent::Blizzard);
    } else if season == Season::Summer
        && (state == WeatherState::Clear || state == WeatherState::Overcast)
    {
        let heat = WeatherState::Clear.base_intensity() * 4.0 * jitter * diurnal;
        if heat >= HEATWAVE_INTENSITY {
            snap.event = Some(WeatherEvent::Heatwave);
            snap.intensity = intensity.max(heat.min(1.0));
        }
    }
    snap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weather_at_is_deterministic_per_seed_tick() {
        let table = climate_table(Some("nyc"));
        for tick in [0u64, 1, 600, 1200, 100_000, 5_000_000] {
            let a = weather_at(12345, tick, &table);
            let b = weather_at(12345, tick, &table);
            assert_eq!(a, b);
        }
    }

    #[test]
    fn climate_rows_sum_to_one() {
        for city in ["generic", "nyc", "la", "sf", "seattle"] {
            for row in climate_table(Some(city)) {
                let s: f64 = row.iter().sum();
                assert!((s - 1.0).abs() < 1e-9, "{city} row sum {s}");
            }
        }
    }

    #[test]
    fn seasons_map_correctly() {
        assert_eq!(season_of_month(0), Season::Winter);
        assert_eq!(season_of_month(11), Season::Winter);
        assert_eq!(season_of_month(3), Season::Spring);
        assert_eq!(season_of_month(6), Season::Summer);
        assert_eq!(season_of_month(9), Season::Autumn);
    }

    #[test]
    fn la_barely_snows() {
        // LA has 0 snow weight all year: no snow state should appear.
        let table = climate_table(Some("la"));
        let mut snow = 0;
        for day in 0..365i64 {
            if weather_day_state(777, day, &table) == WeatherState::Snow {
                snow += 1;
            }
        }
        assert_eq!(snow, 0);
    }
}
