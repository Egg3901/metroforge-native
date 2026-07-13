/**
 * Weather → gameplay coupling (v0.7). ALL tunable constants live here and every
 * effect is documented, so balance passes touch one file. Pure and
 * deterministic: each effect is a function of the current WeatherSnapshot only.
 *
 * The design intent, per effect:
 *  - Ridership: bad weather trims total trips (a storm keeps people home), but
 *    it also makes DRIVING worse, so transit's *share* of the trips that still
 *    happen edges up. Snow suppresses everything, transit least of all.
 *  - Vehicle speed: rain/snow/storm slow surface running. A blizzard nearly
 *    stops the surface network while underground routes sail through — grade
 *    separation pays off exactly when it matters most.
 *  - Construction: pouring track in rain or snow costs more (and, for the future
 *    build-queue, takes longer — the time surcharge is defined now, unused).
 *  - Reliability: a per-vehicle breakdown chance is DEFINED here now and left
 *    unconsumed; the v0.9 ops sim reads it. Landing the field early keeps the
 *    ops lane from having to re-touch this module.
 *
 * Intensity scaling: every multiplier is written for a FULL-STRENGTH state and
 * then eased toward 1.0 (no effect) as intensity → 0, via `lerpByIntensity`, so
 * a light drizzle barely matters and a downpour bites.
 */
import type { TransitMode } from './types';
import type { WeatherSnapshot, WeatherState } from './weather';

/** Ease a full-strength multiplier toward 1.0 (no effect) as intensity → 0. */
function lerpByIntensity(fullMult: number, intensity: number): number {
  const t = Math.max(0, Math.min(1, intensity));
  return 1 + (fullMult - 1) * t;
}

// ── Ridership ─────────────────────────────────────────────────────────────────
/** Total travel-demand multiplier at full strength (people making fewer trips). */
export const WEATHER_DEMAND_MULT: Record<WeatherState, number> = {
  clear: 1.0,
  overcast: 0.99,
  rain: 0.93, // ~7% fewer trips in steady rain
  fog: 0.98,
  snow: 0.82, // snow keeps people home
  storm: 0.72, // strong suppression
};

/** Walk-catchment multiplier at full strength: how far people will walk to a
 *  stop. Rain ~-15%, snow worse. Applied to station walk radius in assignment. */
export const WEATHER_WALK_MULT: Record<WeatherState, number> = {
  clear: 1.0,
  overcast: 1.0,
  rain: 0.85, // ~15% shorter walk tolerance
  fog: 0.95,
  snow: 0.75,
  storm: 0.7,
};

/** Extra generalized-cost MINUTES added to a car trip at full strength: driving
 *  is worse in bad weather, which nudges the mode split toward transit. Snow and
 *  storms hurt the car most, so transit share rises even as total trips fall. */
export const WEATHER_CAR_PENALTY_MIN: Record<WeatherState, number> = {
  clear: 0,
  overcast: 0,
  rain: 6,
  fog: 4,
  snow: 12,
  storm: 15,
};

/** Extra multiplier on total demand when a headline event is active. */
export const BLIZZARD_DEMAND_MULT = 0.6; // most people stay put; the metro carries the rest
export const HEATWAVE_DEMAND_MULT = 0.9; // a noticeable ridership dip

export function weatherDemandMult(weather: WeatherSnapshot | undefined): number {
  if (!weather) return 1;
  let m = lerpByIntensity(WEATHER_DEMAND_MULT[weather.state], weather.intensity);
  if (weather.event === 'blizzard') m *= BLIZZARD_DEMAND_MULT;
  else if (weather.event === 'heatwave') m *= HEATWAVE_DEMAND_MULT;
  return m;
}

export function weatherWalkMult(weather: WeatherSnapshot | undefined): number {
  if (!weather) return 1;
  return lerpByIntensity(WEATHER_WALK_MULT[weather.state], weather.intensity);
}

export function weatherCarPenaltyMin(weather: WeatherSnapshot | undefined): number {
  if (!weather) return 0;
  return WEATHER_CAR_PENALTY_MIN[weather.state] * Math.max(0, Math.min(1, weather.intensity));
}

// ── Vehicle speed ─────────────────────────────────────────────────────────────
/** Surface-running speed multiplier at full strength. */
export const WEATHER_SPEED_MULT: Record<WeatherState, number> = {
  clear: 1.0,
  overcast: 1.0,
  rain: 0.9, // -10%
  fog: 0.92,
  snow: 0.75, // -25%
  storm: 0.7, // -30%
};

/** During a blizzard the surface network nearly stops; this is the surface
 *  speed multiplier, applied in proportion to a route's surface exposure so a
 *  fully-underground line is untouched. */
export const BLIZZARD_SURFACE_SPEED_MULT = 0.35;

/** Heat-wave rail speed restriction (buckling risk): metro + commuter rail slow
 *  down. Buses/trams are unaffected by rail heat orders. */
export const HEATWAVE_RAIL_SPEED_MULT = 0.9;

/**
 * Speed multiplier for a route this tick. `surfaceExposure` is the fraction of
 * the route NOT in tunnel (0 = fully underground, 1 = fully surface/elevated);
 * weather speed penalties scale with it, so grade separation buys immunity.
 */
export function weatherSpeedMult(
  weather: WeatherSnapshot | undefined,
  mode: TransitMode,
  surfaceExposure: number,
): number {
  if (!weather) return 1;
  const exposure = Math.max(0, Math.min(1, surfaceExposure));
  // Base surface penalty, scaled toward 1.0 by intensity, then by exposure.
  const full = WEATHER_SPEED_MULT[weather.state];
  let mult = 1 + (full - 1) * Math.max(0, Math.min(1, weather.intensity)) * exposure;
  if (weather.event === 'blizzard') {
    mult *= 1 + (BLIZZARD_SURFACE_SPEED_MULT - 1) * exposure;
  }
  if (weather.event === 'heatwave' && (mode === 'metro' || mode === 'rail')) {
    // Rail heat order applies regardless of grade (tracks buckle underground too).
    mult *= HEATWAVE_RAIL_SPEED_MULT;
  }
  return mult;
}

// ── Construction ──────────────────────────────────────────────────────────────
/** Build-cost surcharge (added fraction) while a state is active. */
export const WEATHER_BUILD_SURCHARGE: Record<WeatherState, number> = {
  clear: 0,
  overcast: 0,
  rain: 0.08, // +8%
  fog: 0.03,
  snow: 0.2, // +20%
  storm: 0.3, // +30%
};

/** Build-cost multiplier (>= 1) for track laid under the current sky. */
export function weatherBuildCostMult(weather: WeatherSnapshot | undefined): number {
  if (!weather) return 1;
  return 1 + WEATHER_BUILD_SURCHARGE[weather.state] * Math.max(0, Math.min(1, weather.intensity));
}

/** Build-TIME surcharge (added fraction). DEFINED for the future build-queue;
 *  no build-time model exists yet, so nothing consumes this today. */
export const WEATHER_BUILD_TIME_SURCHARGE: Record<WeatherState, number> = {
  clear: 0,
  overcast: 0,
  rain: 0.1,
  fog: 0.05,
  snow: 0.3,
  storm: 0.5,
};

// ── Reliability (hook for v0.9 ops sim) ───────────────────────────────────────
/** Base per-vehicle, per-day breakdown probability by weather, at full strength.
 *  DEFINED now, consumed later: the ops sim will roll against this each day. */
export const WEATHER_BREAKDOWN_CHANCE: Record<WeatherState, number> = {
  clear: 0.002,
  overcast: 0.002,
  rain: 0.006,
  fog: 0.004,
  snow: 0.02,
  storm: 0.03,
};

/** Per-vehicle-per-day breakdown chance for a mode under the current sky. Not
 *  yet consumed by the sim; here so v0.9 ops can read a stable surface. */
export function weatherBreakdownChance(
  weather: WeatherSnapshot | undefined,
  _mode: TransitMode,
): number {
  if (!weather) return WEATHER_BREAKDOWN_CHANCE.clear;
  const base = WEATHER_BREAKDOWN_CHANCE[weather.state];
  return base * (0.5 + 0.5 * Math.max(0, Math.min(1, weather.intensity)));
}
