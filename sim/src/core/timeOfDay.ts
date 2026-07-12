/**
 * Time-of-day travel-demand model. Every function here is a deterministic pure
 * function of the sim tick — no Date.now, no RNG — so it is safe to call from
 * inside simTick and reproduces bit-for-bit across runs and across a native
 * port. Two commute peaks (morning / evening) sit over a quiet overnight lull.
 *
 * The raw curve (`diurnalDemand`) is unchanged from the previous inline copy in
 * sim.ts (it fed, and still feeds, the congestion overlay). What is new is the
 * *normalized* view — `diurnalFactor` — whose daily mean is 1.0, so a daily-
 * average load can be scaled into "the load right now": vehicles fill up at the
 * rush and empty out overnight.
 */
import { TICKS_PER_DAY } from './constants';

/** Hour of the game day in [0,24) for an absolute tick. */
export function hourOfDay(tick: number): number {
  return ((((tick % TICKS_PER_DAY) + TICKS_PER_DAY) % TICKS_PER_DAY) / TICKS_PER_DAY) * 24;
}

/** Raw diurnal travel-demand multiplier: two rush peaks, a quiet night
 *  (~0.19 overnight .. ~1.9 at peak). Kept identical to the original sim.ts
 *  curve so the traffic overlay's behavior does not change. */
export function diurnalDemand(tick: number): number {
  const hour = hourOfDay(tick);
  const am = Math.exp(-((hour - 8) ** 2) / 6);
  const pm = Math.exp(-((hour - 17.5) ** 2) / 8);
  let f = 0.55 + 1.35 * (am + pm);
  if (hour < 5.5) f *= 0.35;
  else if (hour > 22) f *= 0.45;
  return f;
}

/** Daily mean of `diurnalDemand`, integrated once at module load over a whole
 *  game day. Deterministic: a fixed loop, no RNG / Date. */
export const DIURNAL_MEAN: number = (() => {
  let sum = 0;
  for (let t = 0; t < TICKS_PER_DAY; t++) sum += diurnalDemand(t);
  return sum / TICKS_PER_DAY;
})();

/** The busiest single tick's demand in raw units (for peak analysis). */
export const DIURNAL_PEAK: number = (() => {
  let peak = 0;
  for (let t = 0; t < TICKS_PER_DAY; t++) {
    const d = diurnalDemand(t);
    if (d > peak) peak = d;
  }
  return peak;
})();

/** Share of a whole day's travel demand that falls in the single busiest hour,
 *  derived analytically from the curve (max contiguous one-hour window / daily
 *  total) rather than hand-tuned. Exposed for callers that want a peak-hour
 *  ridership share grounded in the same curve the live factor uses. */
export const PEAK_HOUR_SHARE: number = (() => {
  const perTick: number[] = [];
  let total = 0;
  for (let t = 0; t < TICKS_PER_DAY; t++) {
    const d = diurnalDemand(t);
    perTick.push(d);
    total += d;
  }
  const win = Math.max(1, Math.round(TICKS_PER_DAY / 24));
  let running = 0;
  for (let i = 0; i < win && i < perTick.length; i++) running += perTick[i] as number;
  let best = running;
  for (let i = win; i < perTick.length; i++) {
    running += (perTick[i] as number) - (perTick[i - win] as number);
    if (running > best) best = running;
  }
  return total > 0 ? best / total : 0;
})();

/** Live time-of-day multiplier, normalized so its daily mean is exactly 1.0.
 *  >1 at the AM/PM rush (fuller vehicles, worse crowding); ~0.3 overnight.
 *  Multiply a daily-average quantity by this to get its value at `tick`. */
export function diurnalFactor(tick: number): number {
  return diurnalDemand(tick) / DIURNAL_MEAN;
}
