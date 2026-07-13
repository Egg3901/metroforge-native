/**
 * Grade as an operating tradeoff (Egg3901/metroforge#38). ALL tunable
 * grade-congestion constants live here or in constants.ts (GRADE_MAINT_MULT,
 * SURFACE_CONGESTION_WEIGHT) and every effect is documented, so a balance pass
 * touches one place — the grade twin of weatherEffects.ts / geologyCost.ts.
 *
 * The design intent, per effect:
 *  - Speed: a SURFACE alignment shares the street and feels the diurnal
 *    congestion curve; ELEVATED / TUNNEL keep full mode speed at every hour.
 *    Density amplifies the surface slowdown (bus/tram in dense districts at rush
 *    hurt most). Surface slows ONLY when the diurnal factor is above its daily
 *    mean (rush) — off-peak/overnight leave surface at full mode speed, so grade
 *    is an operating tradeoff, not a permanent handicap.
 *  - Ridership: the slowdown feeds trip time in assignment (peak-biased ride
 *    edges), so a slower surface line sheds riders to the car and to its
 *    grade-separated twins — no new player-facing system, just the existing
 *    trip-time / crowding / wait penalties feeling the slowdown.
 *  - Maintenance: GRADE_MAINT_MULT (constants.ts) makes elevated/tunnel cost
 *    more to keep up — the upkeep half of the tradeoff, applied in the daily
 *    ledger. Build cost is already graded via MODES.gradeCostMult.
 *
 * ── COMPOSITION with weather (one place, documented) ─────────────────────────
 * Grade congestion (this module) and weather speed (weatherEffects.ts) are
 * ORTHOGONAL and compose MULTIPLICATIVELY on the base mode speed. In
 * moveVehicles a vehicle's live speed is:
 *
 *     route.moveGradeSpeed                                              // grade
 *       × weatherSpeedMult(weather, mode, route.surfaceExposure)        // weather
 *
 * where `route.moveGradeSpeed` is the length-weighted DAY-AVERAGE grade speed
 * (segmentDayAverageSpeedMps over the route's segments), cached each assignment
 * so the hot per-tick loop is a single read. Both factors are bounded
 * (grade speed ≤ cruise, weatherMult in (0,1]) so their product can never exceed
 * mode cruise nor go negative. Keeping the two knobs in separate modules that
 * multiply at the call site is deliberate: neither has to know about the other.
 *
 * Where the DIURNAL sharpness of the grade tradeoff shows up:
 *  - Ridership / mode split: the PEAK-biased assignment ride edges
 *    (segmentAssignmentSpeedMps) — rush is when peak load and crowding are
 *    measured, so grade separation earns its riders there.
 *  - Headway: the day-average cycle time (segmentDayAverageSpeedMps in
 *    routeCycleSeconds) — vehicles run all day, so headway uses the daily mean,
 *    matching the movement speed above.
 *  - UI inspector: routeExtras.avgEffectiveSpeed uses the live per-tick
 *    diurnal speed (segmentEffectiveSpeedMps) so the player sees the rush dip.
 *
 * Pure + deterministic — no Date/RNG — safe inside simTick and assignment.
 */
import { MODES, SURFACE_CONGESTION_WEIGHT, TICKS_PER_DAY } from '../constants';
import { sampleField } from '../fields';
import type { Vec2 } from '../geometry';
import { DIURNAL_MEAN, DIURNAL_PEAK, diurnalFactor } from '../timeOfDay';
import type { FieldGrid, TrackGrade, TrackSegment, TransitMode } from '../types';

/**
 * Mean of max(0, diurnalFactor − 1) over a game day. Precomputed once so the
 * headway / cycle-time derivation can apply a day-average surface slowdown
 * (vehicles run all day) without integrating the curve on every edge.
 */
export const MEAN_RUSH_EXCESS: number = (() => {
  let sum = 0;
  for (let t = 0; t < TICKS_PER_DAY; t++) sum += Math.max(0, diurnalFactor(t) - 1);
  return sum / TICKS_PER_DAY;
})();

/** Peak diurnalFactor (DIURNAL_PEAK / DIURNAL_MEAN) — used for assignment ride
 *  times so rush-hour surface pain shows up in the daily demand model. */
export const PEAK_DIURNAL_FACTOR: number = DIURNAL_PEAK / DIURNAL_MEAN;

/** Map land-value (~0..3) onto a [0,1] density weight for congestion. */
export function density01FromLandValue(lv: number): number {
  return Math.max(0, Math.min(1, lv / 2));
}

/**
 * Sample corridor density at a world point (land value). Falls back to a
 * mid-density default when fields are unavailable.
 */
export function sampleDensity01(fields: FieldGrid | undefined, pos: Vec2): number {
  if (!fields) return 0.5;
  return density01FromLandValue(sampleField(fields, fields.landValue, pos));
}

/** Density along a track segment (midpoint of its polyline). */
export function segmentDensity01(fields: FieldGrid | undefined, seg: TrackSegment): number {
  const pts = seg.polyline.points;
  if (!pts.length) return 0.5;
  const mid = pts[Math.floor(pts.length / 2)] as Vec2;
  return sampleDensity01(fields, mid);
}

/**
 * Congestion slowdown multiplier (≥1). Elevated/tunnel always 1. Surface slows
 * only when the diurnal factor is above its daily mean (rush); off-peak and
 * overnight leave surface at full mode speed so grade is an operating tradeoff,
 * not a permanent handicap.
 */
export function surfaceCongestionSlowdown(
  mode: TransitMode,
  density01: number,
  todFactor: number,
): number {
  const excess = Math.max(0, todFactor - 1);
  if (excess <= 0) return 1;
  const dens = 0.35 + 0.65 * Math.max(0, Math.min(1, density01));
  return 1 + excess * SURFACE_CONGESTION_WEIGHT[mode] * dens;
}

/** Day-average surface slowdown for cycle/headway (vehicles run all day). */
export function dayAverageSurfaceSlowdown(mode: TransitMode, density01: number): number {
  const dens = 0.35 + 0.65 * Math.max(0, Math.min(1, density01));
  return 1 + MEAN_RUSH_EXCESS * SURFACE_CONGESTION_WEIGHT[mode] * dens;
}

/** Peak surface slowdown for assignment trip times — rush is when the peak load
 *  (and crowding feedback) is measured, so grade separation shows up in the
 *  demand model without a separate time-of-day assignment. */
export function assignmentSurfaceSlowdown(mode: TransitMode, density01: number): number {
  return surfaceCongestionSlowdown(mode, density01, PEAK_DIURNAL_FACTOR);
}

/**
 * Grade-only effective speed (m/s) at `todFactor`. Elevated/tunnel keep full
 * mode cruise; surface is divided by the congestion slowdown. Compose with
 * weatherSpeedMult at the call site (see module header).
 */
export function segmentEffectiveSpeedMps(
  mode: TransitMode,
  grade: TrackGrade,
  todFactor: number,
  density01: number,
): number {
  const base = MODES[mode].speed;
  if (grade !== 'surface') return base;
  return base / surfaceCongestionSlowdown(mode, density01, todFactor);
}

/** Day-average effective speed used by cycle time / headway derivation. */
export function segmentDayAverageSpeedMps(
  mode: TransitMode,
  grade: TrackGrade,
  density01: number,
): number {
  const base = MODES[mode].speed;
  if (grade !== 'surface') return base;
  return base / dayAverageSurfaceSlowdown(mode, density01);
}

/** Peak-biased effective speed used by assignment ride edges. */
export function segmentAssignmentSpeedMps(
  mode: TransitMode,
  grade: TrackGrade,
  density01: number,
): number {
  const base = MODES[mode].speed;
  if (grade !== 'surface') return base;
  return base / assignmentSurfaceSlowdown(mode, density01);
}
