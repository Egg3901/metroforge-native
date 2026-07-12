/**
 * Weather state machine (v0.7 Weather & Light — sim side).
 *
 * Design goals, in priority order:
 *  1. DETERMINISM. Weather is a *pure function* of (seed, tick, city climate).
 *     Same seed ⇒ same weather forever, and the function reconstructs the
 *     current sky in O(1) from the seed alone, so it survives save/load with no
 *     stored history and no migration. Crucially it draws from a DERIVED seed
 *     stream (seed ⊕ salts ⊕ day), never from `state.rngState`, so turning
 *     weather on does not perturb the existing city-event / growth RNG stream —
 *     old replays still reproduce bit-for-bit.
 *  2. SEASONAL CLIMATE per city. Each city id carries a compact climate profile
 *     (four seasonal propensity vectors). NYC snows in winter, LA basically
 *     never, Seattle rains most of the year. Profiles are hardcoded here keyed
 *     by preset key, so no data rebake is needed.
 *  3. CLUSTERING. Real weather persists — a rainy day tends to follow a rainy
 *     day. A 1-step persistence rule (look back exactly one day, itself a pure
 *     draw) gives that autocorrelation while staying O(1) reconstructable.
 *
 * The advance is a seeded Markov-style chain: each day draws a base state from
 * the month's climate distribution, then a persistence roll can carry the
 * (heavier) previous day forward. Within a day the state is softened hour to
 * hour (a storm eases to rain for a few hours) and an intensity 0..1 is shaped
 * by a diurnal curve plus a per-hour seeded jitter — the "daily + hourly blend".
 */
import { TICKS_PER_DAY } from './constants';
import { Rng } from './rng';
import { hourOfDay } from './timeOfDay';

export type WeatherState = 'clear' | 'overcast' | 'rain' | 'fog' | 'snow' | 'storm';

/** Canonical order — the index space the climate distributions are written in. */
export const WEATHER_STATES: readonly WeatherState[] = [
  'clear', 'overcast', 'rain', 'fog', 'snow', 'storm',
] as const;

export type Season = 'winter' | 'spring' | 'summer' | 'autumn';

/** Derived, headline weather event (surfaced to gameplay + UI). */
export type WeatherEvent = 'blizzard' | 'heatwave';

export interface WeatherSnapshot {
  state: WeatherState;
  /** 0..1 — how hard it is coming down (or, for `clear` in summer, heat level). */
  intensity: number;
  season: Season;
  /** 0..11, January = 0. */
  month: number;
  /** headline event, when the sky crosses a gameplay threshold */
  event?: WeatherEvent;
}

// ── Calendar ────────────────────────────────────────────────────────────────
// Season is derived from the sim date. Day tracking already exists as
// floor(tick / TICKS_PER_DAY) (see sim.ts / timeOfDay.ts); the daily economy
// pass uses a 365-day year, so weather uses the same year length for a shared
// calendar. Twelve equal months over 365 days.

export const DAYS_PER_YEAR = 365;
const DAYS_PER_MONTH = DAYS_PER_YEAR / 12; // 30.4167

/** Absolute day index since the run began. */
export function dayIndex(tick: number): number {
  return Math.floor(tick / TICKS_PER_DAY);
}

/** Day of the year in [0, 365). */
export function dayOfYear(tick: number): number {
  return ((dayIndex(tick) % DAYS_PER_YEAR) + DAYS_PER_YEAR) % DAYS_PER_YEAR;
}

/** Month for an absolute day index, [0, 11]. */
export function monthForDay(day: number): number {
  const doy = ((day % DAYS_PER_YEAR) + DAYS_PER_YEAR) % DAYS_PER_YEAR;
  return Math.min(11, Math.floor(doy / DAYS_PER_MONTH));
}

/** Season for a month index. Northern-hemisphere meteorological seasons. */
export function seasonOfMonth(month: number): Season {
  // Dec,Jan,Feb = winter; Mar,Apr,May = spring; Jun,Jul,Aug = summer; Sep,Oct,Nov = autumn.
  if (month === 11 || month <= 1) return 'winter';
  if (month <= 4) return 'spring';
  if (month <= 7) return 'summer';
  return 'autumn';
}

// ── City climate profiles ────────────────────────────────────────────────────
// A profile gives, per season, the RELATIVE propensity of each non-clear state.
// `clear` fills whatever weight is left (floored so no city is 100% grim). The
// numbers are hand-tuned to read like each city; they are not observed climate
// normals. Keyed by the city preset key so no content rebake is required.

interface SeasonWeights {
  overcast: number;
  rain: number;
  fog: number;
  snow: number;
  storm: number;
}
interface ClimateProfile {
  winter: SeasonWeights;
  spring: SeasonWeights;
  summer: SeasonWeights;
  autumn: SeasonWeights;
}

const w = (overcast: number, rain: number, fog: number, snow: number, storm: number): SeasonWeights => ({
  overcast, rain, fog, snow, storm,
});

/** Temperate four-season baseline; every unknown city falls back to this. */
const GENERIC_PROFILE: ClimateProfile = {
  winter: w(0.30, 0.16, 0.08, 0.22, 0.03),
  spring: w(0.26, 0.24, 0.06, 0.03, 0.06),
  summer: w(0.18, 0.18, 0.03, 0.00, 0.10),
  autumn: w(0.28, 0.22, 0.09, 0.02, 0.05),
};

const CITY_PROFILES: Record<string, ClimateProfile> = {
  generic: GENERIC_PROFILE,
  // Cold snowy winters, humid thundery summers.
  nyc: {
    winter: w(0.32, 0.14, 0.05, 0.30, 0.04),
    spring: w(0.26, 0.26, 0.05, 0.04, 0.07),
    summer: w(0.18, 0.20, 0.02, 0.00, 0.14),
    autumn: w(0.28, 0.22, 0.06, 0.02, 0.06),
  },
  // Semi-arid: mostly clear, a little winter rain, marine-layer morning fog.
  la: {
    winter: w(0.16, 0.14, 0.10, 0.00, 0.02),
    spring: w(0.12, 0.06, 0.10, 0.00, 0.01),
    summer: w(0.06, 0.01, 0.12, 0.00, 0.00),
    autumn: w(0.10, 0.05, 0.11, 0.00, 0.01),
  },
  // Marine wet: overcast and rainy most of the year, rare snow, plenty of fog.
  seattle: {
    winter: w(0.40, 0.34, 0.12, 0.05, 0.02),
    spring: w(0.38, 0.28, 0.10, 0.01, 0.02),
    summer: w(0.22, 0.10, 0.06, 0.00, 0.01),
    autumn: w(0.40, 0.32, 0.12, 0.01, 0.03),
  },
  // Windy, snowy winters (lake effect), stormy summers.
  chicago: {
    winter: w(0.34, 0.12, 0.05, 0.34, 0.04),
    spring: w(0.28, 0.26, 0.05, 0.05, 0.09),
    summer: w(0.18, 0.18, 0.02, 0.00, 0.16),
    autumn: w(0.30, 0.22, 0.06, 0.03, 0.07),
  },
  // Nor'easter winters (heavy snow + winter storms), wet shoulder seasons.
  boston: {
    winter: w(0.32, 0.16, 0.06, 0.32, 0.08),
    spring: w(0.28, 0.26, 0.07, 0.05, 0.06),
    summer: w(0.18, 0.18, 0.03, 0.00, 0.11),
    autumn: w(0.30, 0.24, 0.08, 0.02, 0.06),
  },
  // Mild, rainy, big summer thunderstorms, rare snow dustings.
  atlanta: {
    winter: w(0.28, 0.22, 0.07, 0.05, 0.05),
    spring: w(0.24, 0.26, 0.05, 0.00, 0.12),
    summer: w(0.18, 0.22, 0.03, 0.00, 0.20),
    autumn: w(0.24, 0.20, 0.06, 0.00, 0.08),
  },
  // Fog capital: heavy summer fog, mild wet winters, no snow.
  sf: {
    winter: w(0.24, 0.24, 0.14, 0.00, 0.03),
    spring: w(0.20, 0.12, 0.18, 0.00, 0.02),
    summer: w(0.14, 0.03, 0.30, 0.00, 0.00),
    autumn: w(0.18, 0.10, 0.20, 0.00, 0.01),
  },
  // Humid mid-Atlantic: light winter snow, summer storms.
  dc: {
    winter: w(0.30, 0.18, 0.06, 0.16, 0.04),
    spring: w(0.26, 0.26, 0.05, 0.02, 0.09),
    summer: w(0.18, 0.20, 0.03, 0.00, 0.16),
    autumn: w(0.28, 0.22, 0.06, 0.01, 0.06),
  },
  // Similar to NYC/DC, a touch less snow than NYC.
  philly: {
    winter: w(0.30, 0.16, 0.06, 0.24, 0.04),
    spring: w(0.26, 0.26, 0.05, 0.03, 0.08),
    summer: w(0.18, 0.20, 0.02, 0.00, 0.15),
    autumn: w(0.28, 0.22, 0.06, 0.02, 0.06),
  },
  // Cloudiest of the set, heavy lake-effect snow.
  cleveland: {
    winter: w(0.40, 0.14, 0.06, 0.34, 0.03),
    spring: w(0.34, 0.26, 0.06, 0.05, 0.07),
    summer: w(0.24, 0.18, 0.03, 0.00, 0.12),
    autumn: w(0.38, 0.22, 0.07, 0.03, 0.06),
  },
};

/** Minimum probability mass kept on `clear` so nowhere is perpetually grim. */
const CLEAR_FLOOR = 0.05;

/** A normalized 6-vector (indexed by WEATHER_STATES) from a season's weights. */
function distFromWeights(sw: SeasonWeights): number[] {
  const nonClear = sw.overcast + sw.rain + sw.fog + sw.snow + sw.storm;
  const clear = Math.max(CLEAR_FLOOR, 1 - nonClear);
  const raw = [clear, sw.overcast, sw.rain, sw.fog, sw.snow, sw.storm];
  const total = raw.reduce((a, b) => a + b, 0);
  return raw.map((x) => x / total);
}

/**
 * A city's 12-month × 6-state climate table, each row summing to 1. Built once
 * per city from its seasonal weights (a fixed loop, no RNG/Date), so it is safe
 * to compute at module load and cache.
 */
export function climateTable(cityKey: string | undefined): number[][] {
  const key = cityKey && CITY_PROFILES[cityKey] ? cityKey : 'generic';
  const cached = TABLE_CACHE.get(key);
  if (cached) return cached;
  const profile = CITY_PROFILES[key] as ClimateProfile;
  const table: number[][] = [];
  for (let m = 0; m < 12; m++) {
    const season = seasonOfMonth(m);
    table.push(distFromWeights(profile[season]));
  }
  TABLE_CACHE.set(key, table);
  return table;
}
const TABLE_CACHE = new Map<string, number[][]>();

// ── The chain ────────────────────────────────────────────────────────────────

// Independent, fixed salts so weather never collides with the main RNG stream
// or with other derived streams. Large odd constants (splitmix-style).
const DAY_SALT = 0x7f4a7c15;
const PERSIST_SALT = 0x94d049bb;
const INTENSITY_SALT = 0x2545f491;

/** Higher = heavier/wetter. Used by persistence (a heavier day carries forward)
 *  and to shape default intensity. */
const SEVERITY: Record<WeatherState, number> = {
  clear: 0, overcast: 1, fog: 2, rain: 3, snow: 4, storm: 5,
};

/** Probability that a heavier previous day carries into today (clustering). */
export const WEATHER_PERSISTENCE = 0.45;

/** A single day's raw climate draw — a pure function of (seed, day). */
function rawDayDraw(seed: number, day: number, table: number[][]): WeatherState {
  const month = monthForDay(day);
  const dist = table[month] as number[];
  const rng = new Rng((seed ^ DAY_SALT ^ Math.imul(day + 1, 0x9e3779b1)) >>> 0);
  return WEATHER_STATES[rng.weighted(dist)] as WeatherState;
}

/**
 * The day's settled base state, with 1-step persistence. Pure and O(1): it looks
 * back exactly one day, and that day is itself a pure draw, so no history need be
 * stored. A heavier previous day can carry forward (rain follows rain), which is
 * what gives weather its runs instead of independent daily noise.
 */
export function weatherDayState(seed: number, day: number, table: number[][]): WeatherState {
  const today = rawDayDraw(seed, day, table);
  if (day <= 0) return today;
  const yesterday = rawDayDraw(seed, day - 1, table);
  if (SEVERITY[yesterday] > SEVERITY[today]) {
    const pr = new Rng((seed ^ PERSIST_SALT ^ Math.imul(day + 1, 0x85ebca77)) >>> 0);
    if (pr.next() < WEATHER_PERSISTENCE) return yesterday;
  }
  return today;
}

/** Softening ladder — one step milder. Snow stays snow (it does not "ease"). */
const MILDER: Record<WeatherState, WeatherState> = {
  storm: 'rain', rain: 'overcast', fog: 'overcast', overcast: 'clear', snow: 'snow', clear: 'clear',
};

/** Baseline intensity for a state at full strength, before hourly shaping. */
const BASE_INTENSITY: Record<WeatherState, number> = {
  clear: 0.15, overcast: 0.35, fog: 0.55, rain: 0.6, snow: 0.65, storm: 0.85,
};

/** Blizzard when snow or storm is coming down hard. */
export const BLIZZARD_INTENSITY = 0.62;
/** Heat wave when a clear summer day runs hot (intensity is heat for clear). */
export const HEATWAVE_INTENSITY = 0.7;

/**
 * The full sky at an absolute tick — pure function of (seed, tick, climate).
 * Combines the daily base state with an hourly blend: the state can soften for a
 * few hours (a storm eases to rain), and intensity follows a gentle diurnal
 * shape plus a per-hour seeded jitter. Cheap: a handful of integer ops and at
 * most three tiny RNG constructions.
 */
export function weatherAt(seed: number, tick: number, table: number[][]): WeatherSnapshot {
  const day = dayIndex(tick);
  const base = weatherDayState(seed, day, table);
  const month = monthForDay(day);
  const season = seasonOfMonth(month);
  const hour = hourOfDay(tick);
  const hourSlot = Math.floor(hour);

  const jr = new Rng((seed ^ INTENSITY_SALT ^ Math.imul(day * 24 + hourSlot + 1, 0x27d4eb2f)) >>> 0);
  const jitter = 0.6 + 0.4 * jr.next(); // 0.6..1.0

  // Diurnal shaping: fog favors dawn, storms/heat favor afternoon, otherwise flat-ish.
  let diurnal = 1;
  if (base === 'fog') diurnal = hour < 9 ? 1 : Math.max(0.25, 1 - (hour - 9) * 0.12);
  else if (base === 'storm') diurnal = 0.7 + 0.3 * Math.exp(-((hour - 16) ** 2) / 20);
  else if (base === 'clear') diurnal = 0.6 + 0.4 * Math.exp(-((hour - 15) ** 2) / 24); // heat peaks mid-afternoon

  let intensity = Math.max(0, Math.min(1, BASE_INTENSITY[base] * jitter * diurnal));

  // Hourly softening: on a low-intensity hour the *displayed* state can drop one
  // rung (texture within the day). Deterministic from the same jitter stream.
  let state = base;
  if (intensity < 0.4 && jr.next() < 0.5) {
    state = MILDER[base];
    intensity = Math.max(0, Math.min(1, BASE_INTENSITY[state] * jitter * diurnal));
  }

  const snap: WeatherSnapshot = { state, intensity, season, month };

  // Headline events.
  if ((state === 'snow' || state === 'storm') && intensity >= BLIZZARD_INTENSITY) {
    snap.event = 'blizzard';
  } else if (season === 'summer' && (state === 'clear' || state === 'overcast')) {
    // For clear summer days intensity is a heat proxy; a hot afternoon is a heat wave.
    const heat = BASE_INTENSITY.clear * 4 * jitter * diurnal; // rescale clear→heat 0..~1
    if (heat >= HEATWAVE_INTENSITY) {
      snap.event = 'heatwave';
      snap.intensity = Math.max(intensity, Math.min(1, heat));
    }
  }
  return snap;
}
