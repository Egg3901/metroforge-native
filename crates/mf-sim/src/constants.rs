//! Economy / tuning baselines. Port of `sim/src/core/constants.ts`.
//!
//! Values are copied verbatim from the TS source; they define the economic and
//! transit balance of the sim and must stay in lockstep with it. Per-mode
//! configuration lives in [`MODES`]; look-ups are by [`TransitMode`].

use crate::types::{Difficulty, TrackGrade, TransitMode};

/// 1 tick = 1 game-second.
pub const TICK_SECONDS: u32 = 1;
/// A game day is 20 real minutes at 1x -> compressed: 1 game-day = 1200 ticks.
pub const TICKS_PER_DAY: u32 = 1200;
/// Demand assignment reruns at most this often (and when dirty).
pub const ASSIGNMENT_INTERVAL_TICKS: u32 = 300;
/// Growth pass cadence, in days.
pub const GROWTH_INTERVAL_DAYS: u32 = 7;

/// World edge length in meters (square, centered on origin).
pub const WORLD_SIZE: f64 = 12000.0;
/// Field grid width in cells.
pub const FIELD_W: u32 = 96;
/// Field grid height in cells.
pub const FIELD_H: u32 = 96;
/// Meters per field cell (`WORLD_SIZE / FIELD_W` = 125 m).
pub const FIELD_CELL: f64 = WORLD_SIZE / FIELD_W as f64;

/// Per-mode configuration. Mirrors the `ModeConfig` interface.
#[derive(Clone, Debug, PartialEq)]
pub struct ModeConfig {
    /// Display label.
    pub label: &'static str,
    /// $ per meter of new dedicated right-of-way at surface grade.
    pub track_cost_per_meter: f64,
    /// Cost to place one station.
    pub station_cost: f64,
    /// Cost of one vehicle.
    pub vehicle_cost: f64,
    /// Passengers per vehicle.
    pub vehicle_capacity: f64,
    /// Cruise speed, m/s.
    pub speed: f64,
    /// Dwell time per stop, seconds.
    pub dwell_seconds: f64,
    /// Track maintenance $ per km per day.
    pub maint_per_km_per_day: f64,
    /// Vehicle maintenance $ per vehicle per day.
    pub maint_per_vehicle_per_day: f64,
    /// Operating cost $ per vehicle per day (includes driver).
    pub ops_per_vehicle_per_day: f64,
    /// Default service headway, seconds.
    pub default_headway: f64,
    /// Minimum service headway, seconds.
    pub min_headway: f64,
    /// Meters people will walk to reach this mode.
    pub walk_radius: f64,
    /// Grades this mode may be built at.
    pub grade_options: &'static [TrackGrade],
    /// Cost multiplier by grade `(surface, elevated, tunnel)`.
    pub grade_cost_mult: GradeCostMult,
    /// Population threshold to unlock this mode.
    pub unlock_population: f64,
    /// Suggested station spacing hint `[min, max]` (not enforced).
    pub station_spacing: [f64; 2],
}

/// Per-grade cost multiplier triple. Mirrors `Record<TrackGrade, number>`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradeCostMult {
    /// Multiplier at surface grade.
    pub surface: f64,
    /// Multiplier when elevated.
    pub elevated: f64,
    /// Multiplier in tunnel.
    pub tunnel: f64,
}

impl GradeCostMult {
    /// Look up the multiplier for a grade.
    pub fn get(&self, grade: TrackGrade) -> f64 {
        match grade {
            TrackGrade::Surface => self.surface,
            TrackGrade::Elevated => self.elevated,
            TrackGrade::Tunnel => self.tunnel,
        }
    }
}

/// Configuration for the bus mode.
pub const BUS: ModeConfig = ModeConfig {
    label: "Bus",
    track_cost_per_meter: 150.0,
    station_cost: 8000.0,
    vehicle_cost: 90000.0,
    vehicle_capacity: 60.0,
    speed: 8.3,
    dwell_seconds: 20.0,
    maint_per_km_per_day: 12.0,
    maint_per_vehicle_per_day: 60.0,
    ops_per_vehicle_per_day: 260.0,
    default_headway: 600.0,
    min_headway: 120.0,
    walk_radius: 450.0,
    grade_options: &[TrackGrade::Surface],
    grade_cost_mult: GradeCostMult {
        surface: 1.0,
        elevated: 3.0,
        tunnel: 8.0,
    },
    unlock_population: 0.0,
    station_spacing: [300.0, 500.0],
};

/// Configuration for the tram mode.
pub const TRAM: ModeConfig = ModeConfig {
    label: "Tram",
    track_cost_per_meter: 9000.0,
    station_cost: 120000.0,
    vehicle_cost: 1600000.0,
    vehicle_capacity: 200.0,
    speed: 11.0,
    dwell_seconds: 25.0,
    maint_per_km_per_day: 90.0,
    maint_per_vehicle_per_day: 220.0,
    ops_per_vehicle_per_day: 420.0,
    default_headway: 360.0,
    min_headway: 90.0,
    walk_radius: 600.0,
    grade_options: &[TrackGrade::Surface, TrackGrade::Elevated],
    grade_cost_mult: GradeCostMult {
        surface: 1.0,
        elevated: 2.6,
        tunnel: 7.0,
    },
    unlock_population: 50000.0,
    station_spacing: [400.0, 800.0],
};

/// Configuration for the metro mode.
pub const METRO: ModeConfig = ModeConfig {
    label: "Metro",
    track_cost_per_meter: 45000.0,
    station_cost: 4500000.0,
    vehicle_cost: 3200000.0,
    vehicle_capacity: 900.0,
    speed: 19.5,
    dwell_seconds: 30.0,
    maint_per_km_per_day: 450.0,
    maint_per_vehicle_per_day: 550.0,
    ops_per_vehicle_per_day: 900.0,
    default_headway: 240.0,
    min_headway: 90.0,
    walk_radius: 800.0,
    grade_options: &[
        TrackGrade::Tunnel,
        TrackGrade::Elevated,
        TrackGrade::Surface,
    ],
    grade_cost_mult: GradeCostMult {
        tunnel: 1.0,
        elevated: 0.65,
        surface: 0.4,
    },
    unlock_population: 150000.0,
    station_spacing: [800.0, 1500.0],
};

/// Configuration for the commuter-rail mode.
pub const RAIL: ModeConfig = ModeConfig {
    label: "Commuter Rail",
    track_cost_per_meter: 14000.0,
    station_cost: 1800000.0,
    vehicle_cost: 5200000.0,
    vehicle_capacity: 1400.0,
    speed: 27.7,
    dwell_seconds: 45.0,
    maint_per_km_per_day: 160.0,
    maint_per_vehicle_per_day: 850.0,
    ops_per_vehicle_per_day: 1400.0,
    default_headway: 900.0,
    min_headway: 300.0,
    walk_radius: 1000.0,
    grade_options: &[TrackGrade::Surface, TrackGrade::Elevated],
    grade_cost_mult: GradeCostMult {
        surface: 1.0,
        elevated: 2.4,
        tunnel: 6.0,
    },
    unlock_population: 300000.0,
    station_spacing: [1500.0, 4000.0],
};

/// Configuration for one transit mode. Mirrors `MODES[mode]`.
pub fn modes(mode: TransitMode) -> &'static ModeConfig {
    match mode {
        TransitMode::Bus => &BUS,
        TransitMode::Tram => &TRAM,
        TransitMode::Metro => &METRO,
        TransitMode::Rail => &RAIL,
    }
}

/// Minutes of generalized cost added per transfer.
pub const TRANSFER_PENALTY_MIN: f64 = 5.0;
/// Walking speed, m/s.
pub const WALK_SPEED: f64 = 1.35;

/// Ceiling on derived headway (30 min) so one vehicle on a huge loop still
/// shows a finite service level.
pub const MAX_HEADWAY: f64 = 1800.0;
/// Share of daily ridership riding in the single busiest hour.
pub const PEAK_HOUR_FRACTION: f64 = 0.14;
/// Crowding knee: below it comfortable, above it discomfort minutes accrue.
pub const CROWD_KNEE: f64 = 0.8;
/// Minutes added per unit of crowding past the knee.
pub const CROWD_PENALTY_MIN: f64 = 22.0;
/// Sustained crowding above this drags approval down each day.
pub const CROWD_APPROVAL_THRESHOLD: f64 = 1.1;
/// Water crossing multiplier on track cost (bridge/tube).
pub const WATER_CROSSING_MULT: f64 = 5.0;
/// Demolition refund fraction.
pub const REFUND_FRACTION: f64 = 0.25;

/// Ongoing track maintenance multiplier by grade. Mirrors `GRADE_MAINT_MULT`.
pub fn grade_maint_mult(grade: TrackGrade) -> f64 {
    match grade {
        TrackGrade::Surface => 1.0,
        TrackGrade::Elevated => 1.35,
        TrackGrade::Tunnel => 1.8,
    }
}

/// How strongly each mode feels street congestion when running at surface
/// grade. Mirrors `SURFACE_CONGESTION_WEIGHT`.
pub fn surface_congestion_weight(mode: TransitMode) -> f64 {
    match mode {
        TransitMode::Bus => 1.0,
        TransitMode::Tram => 0.9,
        TransitMode::Metro => 0.35,
        TransitMode::Rail => 0.25,
    }
}

/// Starting cash by difficulty. Mirrors `STARTING_CASH`.
pub fn starting_cash(difficulty: Difficulty) -> f64 {
    match difficulty {
        Difficulty::Easy => 30_000_000.0,
        Difficulty::Normal => 15_000_000.0,
        Difficulty::Hard => 8_000_000.0,
    }
}

/// Base daily subsidy by difficulty. Mirrors `BASE_DAILY_SUBSIDY`.
pub fn base_daily_subsidy(difficulty: Difficulty) -> f64 {
    match difficulty {
        Difficulty::Easy => 60_000.0,
        Difficulty::Normal => 40_000.0,
        Difficulty::Hard => 25_000.0,
    }
}

/// Cash floor that, sustained past the grace period, ends the run.
pub const BANKRUPTCY_FLOOR: f64 = -500_000.0;
/// Grace period (days) below the cash floor before bankruptcy.
pub const BANKRUPTCY_GRACE_DAYS: u32 = 7;

/// Route colors (ColorBrewer-ish qualitative, colorblind-aware).
/// Mirrors `ROUTE_COLORS`.
pub const ROUTE_COLORS: [&str; 10] = [
    "#e6a817", "#4dabf7", "#f06595", "#69db7c", "#b197fc", "#ff922b", "#3bc9db", "#ffd43b",
    "#63e6be", "#e599f7",
];
