import type { TrackGrade, TransitMode } from './types';

/** 1 tick = 1 game-second. */
export const TICK_SECONDS = 1;
/** A game day is 20 real minutes at 1× → compressed: 1 game-day = 1200 ticks. */
export const TICKS_PER_DAY = 1200;
/** Demand assignment reruns at most this often (and when dirty). */
export const ASSIGNMENT_INTERVAL_TICKS = 300;
/** Growth pass cadence. */
export const GROWTH_INTERVAL_DAYS = 7;

export const WORLD_SIZE = 12000; // meters, square, centered on 0
export const FIELD_W = 96;
export const FIELD_H = 96;
export const FIELD_CELL = WORLD_SIZE / FIELD_W; // 125 m

export interface ModeConfig {
  label: string;
  /** $ per meter of new dedicated right-of-way at surface grade */
  trackCostPerMeter: number;
  stationCost: number;
  vehicleCost: number;
  vehicleCapacity: number;
  /** m/s cruise */
  speed: number;
  dwellSeconds: number;
  maintPerKmPerDay: number;
  maintPerVehiclePerDay: number;
  /** includes driver */
  opsPerVehiclePerDay: number;
  defaultHeadway: number;
  minHeadway: number;
  /** meters people will walk to reach this mode */
  walkRadius: number;
  gradeOptions: TrackGrade[];
  gradeCostMult: Record<TrackGrade, number>;
  /** population threshold to unlock */
  unlockPopulation: number;
  /** max useful spacing hint shown in UI (not enforced) */
  stationSpacing: [number, number];
}

export const MODES: Record<TransitMode, ModeConfig> = {
  bus: {
    label: 'Bus',
    trackCostPerMeter: 150,
    stationCost: 8000,
    vehicleCost: 90000,
    vehicleCapacity: 60,
    speed: 8.3, // 30 km/h with stops factored via dwell
    dwellSeconds: 20,
    maintPerKmPerDay: 12,
    maintPerVehiclePerDay: 60,
    opsPerVehiclePerDay: 260,
    defaultHeadway: 600,
    minHeadway: 120,
    walkRadius: 450,
    gradeOptions: ['surface'],
    gradeCostMult: { surface: 1, elevated: 3, tunnel: 8 },
    unlockPopulation: 0,
    stationSpacing: [300, 500],
  },
  tram: {
    label: 'Tram',
    trackCostPerMeter: 9000,
    stationCost: 120000,
    vehicleCost: 1600000,
    vehicleCapacity: 200,
    speed: 11, // 40 km/h
    dwellSeconds: 25,
    maintPerKmPerDay: 90,
    maintPerVehiclePerDay: 220,
    opsPerVehiclePerDay: 420,
    defaultHeadway: 360,
    minHeadway: 90,
    walkRadius: 600,
    gradeOptions: ['surface', 'elevated'],
    gradeCostMult: { surface: 1, elevated: 2.6, tunnel: 7 },
    unlockPopulation: 50000,
    stationSpacing: [400, 800],
  },
  metro: {
    label: 'Metro',
    trackCostPerMeter: 45000,
    stationCost: 4500000,
    vehicleCost: 3200000,
    vehicleCapacity: 900,
    speed: 19.5, // 70 km/h
    dwellSeconds: 30,
    maintPerKmPerDay: 450,
    maintPerVehiclePerDay: 550,
    opsPerVehiclePerDay: 900,
    defaultHeadway: 240,
    minHeadway: 90,
    walkRadius: 800,
    gradeOptions: ['tunnel', 'elevated', 'surface'],
    gradeCostMult: { tunnel: 1, elevated: 0.65, surface: 0.4 },
    unlockPopulation: 150000,
    stationSpacing: [800, 1500],
  },
  rail: {
    label: 'Commuter Rail',
    trackCostPerMeter: 14000,
    stationCost: 1800000,
    vehicleCost: 5200000,
    vehicleCapacity: 1400,
    speed: 27.7, // 100 km/h
    dwellSeconds: 45,
    maintPerKmPerDay: 160,
    maintPerVehiclePerDay: 850,
    opsPerVehiclePerDay: 1400,
    defaultHeadway: 900,
    minHeadway: 300,
    walkRadius: 1000,
    gradeOptions: ['surface', 'elevated'],
    gradeCostMult: { surface: 1, elevated: 2.4, tunnel: 6 },
    unlockPopulation: 300000,
    stationSpacing: [1500, 4000],
  },
};

export const TRANSFER_PENALTY_MIN = 5; // minutes of generalized cost per transfer
export const WALK_SPEED = 1.35; // m/s

// ── Frequency & capacity (Phase 1) ──────────────────────────────────────────
/** Ceiling on derived headway so a single vehicle on a huge loop still shows a
 *  finite (if terrible) service level rather than "never comes". */
export const MAX_HEADWAY = 1800; // 30 min
/** Share of daily ridership that rides in the single busiest hour. Used to turn
 *  daily route ridership into a peak-hour load for the capacity check. */
export const PEAK_HOUR_FRACTION = 0.14;
/** Crowding = peakLoad / capacity. Below the knee it is comfortable; above it,
 *  each unit of crowding adds discomfort minutes to the route's in-vehicle cost
 *  (BPR-style), so overcrowded lines shed riders to alternates or the car. */
export const CROWD_KNEE = 0.8;
export const CROWD_PENALTY_MIN = 22; // minutes added per unit of crowding past the knee
/** Sustained crowding above this drags approval down a little each day. */
export const CROWD_APPROVAL_THRESHOLD = 1.1;
/** Water crossing multiplies track cost (bridge/tube). */
export const WATER_CROSSING_MULT = 5;
/** Demolition refund fraction. */
export const REFUND_FRACTION = 0.25;

/**
 * Ongoing track maintenance multiplier by grade (grade as an operating
 * tradeoff — Egg3901/metroforge#38). Elevated sits modestly above surface;
 * tunnel is highest. Tuned so a busy corridor still clearly rewards grade
 * separation (faster + more reliable) despite the upkeep premium. Consumed in
 * runDailyEconomy; the full design rationale + composition with weather lives in
 * transit/gradeEffects.ts.
 */
export const GRADE_MAINT_MULT: Record<TrackGrade, number> = {
  surface: 1,
  elevated: 1.35,
  tunnel: 1.8,
};

/**
 * How strongly each mode feels street congestion when running at SURFACE grade
 * (Egg3901/metroforge#38). Bus/tram share the roadway (full weight); metro/rail
 * on surface are partially protected but still slowed in dense rush corridors.
 * Elevated/tunnel are immune (see gradeEffects.ts). This is the grade-congestion
 * knob; the orthogonal weather-speed knobs live in weatherEffects.ts and the two
 * compose multiplicatively in moveVehicles.
 */
export const SURFACE_CONGESTION_WEIGHT: Record<TransitMode, number> = {
  bus: 1,
  tram: 0.9,
  metro: 0.35,
  rail: 0.25,
};

export const STARTING_CASH: Record<'easy' | 'normal' | 'hard', number> = {
  easy: 30_000_000,
  normal: 15_000_000,
  hard: 8_000_000,
};

export const BASE_DAILY_SUBSIDY: Record<'easy' | 'normal' | 'hard', number> = {
  easy: 60_000,
  normal: 40_000,
  hard: 25_000,
};

/** Cash floor + grace period → bankruptcy. */
export const BANKRUPTCY_FLOOR = -500_000;
export const BANKRUPTCY_GRACE_DAYS = 7;

export const ROUTE_COLORS = [
  // ColorBrewer-ish qualitative, colorblind-aware
  '#e6a817', '#4dabf7', '#f06595', '#69db7c', '#b197fc',
  '#ff922b', '#3bc9db', '#ffd43b', '#63e6be', '#e599f7',
];
