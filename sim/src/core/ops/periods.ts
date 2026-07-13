/**
 * Service periods for per-period frequency (v0.9 System A / A1). A pure,
 * deterministic function of the sim tick (via hourOfDay) — no Date, no RNG — so
 * it reproduces bit-for-bit across runs and a native port.
 *
 * Five periods span the game day: AM peak, midday, PM peak, evening, night.
 * Each route carries a target headway per period; more service (shorter target
 * headway) draws more riders but needs more vehicles and costs more to run.
 */
import { hourOfDay } from '../timeOfDay';

export type Period = 'amPeak' | 'midday' | 'pmPeak' | 'evening' | 'night';

/** All periods in day order (stable iteration for schedules / peak sizing). */
export const PERIODS: readonly Period[] = ['amPeak', 'midday', 'pmPeak', 'evening', 'night'];

/** Which service period an absolute tick falls in, by hour of the game day.
 *  Boundaries are fixed (not tunable) so they never enter economy balance:
 *   night   [0,6)      amPeak [6,9.5)   midday [9.5,16)
 *   pmPeak  [16,19)    evening[19,22)   night  [22,24)  */
export function periodForTick(tick: number): Period {
  const h = hourOfDay(tick);
  if (h < 6) return 'night';
  if (h < 9.5) return 'amPeak';
  if (h < 16) return 'midday';
  if (h < 19) return 'pmPeak';
  if (h < 22) return 'evening';
  return 'night';
}

/** Human label for HUD / toasts (no em/en dashes). */
export const PERIOD_LABEL: Record<Period, string> = {
  amPeak: 'AM peak',
  midday: 'Midday',
  pmPeak: 'PM peak',
  evening: 'Evening',
  night: 'Night',
};
