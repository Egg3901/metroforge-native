/**
 * Cohort demand (v0.9 System B "Living City", perf-first).
 *
 * The city's residents split into four behavioural cohorts, each with its own
 * hourly departure rhythm and its own idea of *where* a trip goes at a given
 * hour (to work, back home, out to leisure). This reshapes the origin-destination
 * generation in `assignment.ts` so demand becomes schedule-driven — the AM peak
 * flows into job districts, the PM peak reverses toward home, weekends tilt to
 * leisure, nights thin out — while the Dijkstra + logit assignment stages are
 * left untouched.
 *
 * PERFORMANCE CONTRACT
 * --------------------
 * Everything a per-tick / per-assignment caller needs is precomputed ONCE at
 * module load and indexed by an integer hour bucket [0,24):
 *   - `HOUR_ATTRACTOR[h]`  → the (job, home, leisure) destination-pull mix for
 *     hour h, collapsed across all cohorts. The assignment adds a single blended
 *     attractor term per destination — no per-cohort loop in the hot path.
 *   - `HOURLY_DEMAND[h]`   → normalized total demand factor for hour h (daily
 *     mean = 1.0), for the demand-by-hour summary and visual-passenger scaling.
 * These tables are pure deterministic functions of the cohort constants below;
 * no RNG, no Date. POI surges + weekend tilt are cheap closed-form multipliers
 * layered on top, seeded off the game seed so they reproduce bit-for-bit.
 */
import { TICKS_PER_DAY } from '../constants';
import { hourOfDay } from '../timeOfDay';
import type { PoiAnchor } from '../types';

export type CohortKind = 'commuter' | 'student' | 'leisure' | 'nightShift';

export const COHORTS: readonly CohortKind[] = ['commuter', 'student', 'leisure', 'nightShift'] as const;

/** Where a cohort's trip wants to go, per hour: a job district, back home, or
 *  out to a leisure/POI destination. The three pulls sum to 1 for every hour. */
interface DirBias {
  job: number;
  home: number;
  leisure: number;
}

interface CohortModel {
  kind: CohortKind;
  /** baseline share of a district's residents in this cohort (before local tilt) */
  baseShare: number;
  /** 24 hourly departure propensities; sums to 1 (a "propensity row"). */
  hourly: number[];
  /** 24 per-hour destination-pull mixes (each sums to 1). */
  dir: DirBias[];
  /** how strongly weekends amplify (>1) or damp (<1) this cohort's trips. */
  weekendTilt: number;
}

/** Normalize an arbitrary 24-length weight vector into a propensity row (Σ=1). */
function row(weights: number[]): number[] {
  let s = 0;
  for (const w of weights) s += w;
  if (s <= 0) return weights.map(() => 1 / 24);
  return weights.map((w) => w / s);
}

/** A gaussian bump centred on `mu` hours with width `sigma`, evaluated at hour h
 *  (wrapping midnight), scaled by `amp`. Used to shape the departure curves. */
function bump(h: number, mu: number, sigma: number, amp: number): number {
  let d = Math.abs(h - mu);
  if (d > 12) d = 24 - d; // wrap around midnight
  return amp * Math.exp(-(d * d) / (2 * sigma * sigma));
}

/** Build a per-hour direction-bias curve that swings from outbound (job/leisure)
 *  in the morning to inbound (home) in the evening, controlled by `amHome` /
 *  `pmHome` anchor fractions and a `leisureFloor` baseline. */
function dirCurve(leisureFloor: number, outboundKind: 'job' | 'leisure'): DirBias[] {
  const out: DirBias[] = [];
  for (let h = 0; h < 24; h++) {
    // homeward fraction rises across the day: low pre-noon, high after ~15:00.
    const homeFrac = Math.max(0, Math.min(1, (h - 6) / 13)); // 0 at 6:00 → 1 at 19:00
    const leisure = leisureFloor * (0.5 + 0.5 * bump(h, 19, 4, 1)); // evening-weighted leisure
    const remain = Math.max(0, 1 - leisure);
    const home = remain * homeFrac;
    const outbound = remain * (1 - homeFrac);
    if (outboundKind === 'leisure') {
      out.push({ job: outbound * 0.35, home, leisure: leisure + outbound * 0.65 });
    } else {
      out.push({ job: outbound, home, leisure });
    }
  }
  return out;
}

const COHORT_MODELS: CohortModel[] = [
  {
    kind: 'commuter',
    baseShare: 0.55,
    // classic bimodal work commute: AM ~8:00, PM ~17:30
    hourly: row(
      Array.from({ length: 24 }, (_, h) => 0.05 + bump(h, 8, 1.1, 1) + bump(h, 17.5, 1.4, 0.95)),
    ),
    dir: dirCurve(0.08, 'job'),
    weekendTilt: 0.45,
  },
  {
    kind: 'student',
    baseShare: 0.15,
    // school start ~8:00, early-afternoon return ~15:00, some evening study
    hourly: row(
      Array.from({ length: 24 }, (_, h) => 0.04 + bump(h, 8, 1.0, 1) + bump(h, 15, 1.6, 0.8) + bump(h, 19, 1.5, 0.3)),
    ),
    dir: dirCurve(0.15, 'job'),
    weekendTilt: 0.35,
  },
  {
    kind: 'leisure',
    baseShare: 0.2,
    // midday + evening spread, no sharp rush
    hourly: row(
      Array.from({ length: 24 }, (_, h) => 0.06 + bump(h, 13, 3, 0.7) + bump(h, 20, 3.2, 1)),
    ),
    dir: dirCurve(0.55, 'leisure'),
    weekendTilt: 1.7,
  },
  {
    kind: 'nightShift',
    baseShare: 0.1,
    // out to work late (22:00–00:00), home in the small hours / early morning
    hourly: row(
      Array.from({ length: 24 }, (_, h) => 0.03 + bump(h, 22.5, 1.6, 1) + bump(h, 5.5, 1.6, 0.9)),
    ),
    dir: dirCurve(0.05, 'job'),
    weekendTilt: 0.8,
  },
];

/** The 24-entry hourly departure propensity row for a cohort (each sums to 1).
 *  Exposed for tests / tooling; the hot path uses the precomputed tables below. */
export function cohortHourlyRow(kind: CohortKind): number[] {
  const m = COHORT_MODELS.find((c) => c.kind === kind);
  return m ? [...m.hourly] : [];
}

// ── Precomputed, hour-bucketed tables (the perf contract) ────────────────────

/** Collapsed destination-pull mix per hour across all cohorts, weighted by each
 *  cohort's population share and its share of trips departing that hour. This is
 *  what the assignment reads: one lookup, one blended attractor term. */
export const HOUR_ATTRACTOR: DirBias[] = (() => {
  const table: DirBias[] = [];
  for (let h = 0; h < 24; h++) {
    let job = 0;
    let home = 0;
    let leisure = 0;
    let w = 0;
    for (const c of COHORT_MODELS) {
      const cw = c.baseShare * (c.hourly[h] as number);
      const d = c.dir[h] as DirBias;
      job += cw * d.job;
      home += cw * d.home;
      leisure += cw * d.leisure;
      w += cw;
    }
    if (w > 0) {
      job /= w;
      home /= w;
      leisure /= w;
    } else {
      job = 1;
    }
    table.push({ job, home, leisure });
  }
  return table;
})();

/** Un-normalized total demand weight per hour (Σ over cohorts of share×hourly). */
const HOURLY_RAW: number[] = (() => {
  const out: number[] = [];
  for (let h = 0; h < 24; h++) {
    let s = 0;
    for (const c of COHORT_MODELS) s += c.baseShare * (c.hourly[h] as number);
    out.push(s);
  }
  return out;
})();

/** Total demand factor per hour, normalized so the 24-hour mean is exactly 1.0.
 *  Multiply a daily-average passenger quantity by `HOURLY_DEMAND[h]` to get its
 *  value at hour h (busy at the rush, thin at 2am). */
export const HOURLY_DEMAND: number[] = (() => {
  const mean = HOURLY_RAW.reduce((a, b) => a + b, 0) / 24;
  return HOURLY_RAW.map((v) => (mean > 0 ? v / mean : 1));
})();

/** Integer hour bucket [0,24) for a sim tick. */
export function hourBucket(tick: number): number {
  const h = Math.floor(hourOfDay(tick)) % 24;
  return h < 0 ? h + 24 : h;
}

/** True on weekend game-days (day-of-week 5,6 from tick), for the leisure tilt. */
export function isWeekend(tick: number): boolean {
  const day = Math.floor(tick / TICKS_PER_DAY);
  const dow = ((day % 7) + 7) % 7;
  return dow >= 5;
}

/** The destination-pull mix the assignment should use at `tick`, including the
 *  weekend leisure tilt. Cheap: one table lookup + a handful of multiplies. */
export function attractorAt(tick: number): DirBias {
  const base = HOUR_ATTRACTOR[hourBucket(tick)] as DirBias;
  if (!isWeekend(tick)) return base;
  // weekends: pull the mix toward leisure and away from jobs, then renormalize.
  const job = base.job * 0.55;
  const home = base.home;
  const leisure = base.leisure * 1.9 + 0.05;
  const s = job + home + leisure || 1;
  return { job: job / s, home: home / s, leisure: leisure / s };
}

/** Live time-of-day demand factor (daily mean 1.0) at `tick`, with the weekend
 *  tilt folded in. Drives the demand-by-hour summary + visual passenger counts.
 *  Pure/deterministic; NOT part of the economy hash. */
export function cohortDemandFactor(tick: number): number {
  const h = hourBucket(tick);
  if (!isWeekend(tick)) return HOURLY_DEMAND[h] as number;
  // recompute the hour weight with weekend tilts, renormalized against a
  // weekend daily mean so the factor still averages ~1 over a weekend day.
  let s = 0;
  for (const c of COHORT_MODELS) s += c.baseShare * c.weekendTilt * (c.hourly[h] as number);
  let mean = 0;
  for (let hh = 0; hh < 24; hh++) {
    for (const c of COHORT_MODELS) mean += c.baseShare * c.weekendTilt * (c.hourly[hh] as number);
  }
  mean /= 24;
  return mean > 0 ? s / mean : 1;
}

/** Per-cohort relative demand weight at `tick` (share × hourly × weekend tilt),
 *  for the wire summary. Values are relative, not normalized to 1. */
export function cohortMix(tick: number): Record<CohortKind, number> {
  const h = hourBucket(tick);
  const weekend = isWeekend(tick);
  const out = { commuter: 0, student: 0, leisure: 0, nightShift: 0 } as Record<CohortKind, number>;
  for (const c of COHORT_MODELS) {
    const tilt = weekend ? c.weekendTilt : 1;
    out[c.kind] = c.baseShare * tilt * (c.hourly[h] as number);
  }
  return out;
}

/** The full 24-entry normalized hourly demand curve for a given day-of-week
 *  parity (weekday by default). Used by tests / the perf harness to dump the
 *  demand shape, and by the wire summary. */
export function hourlyDemandCurve(weekend = false): number[] {
  const tickForHour = (h: number): number =>
    (weekend ? 5 * TICKS_PER_DAY : 0) + h * (TICKS_PER_DAY / 24);
  return Array.from({ length: 24 }, (_, h) => cohortDemandFactor(tickForHour(h)));
}

// ── POI surges (System B, B2) ────────────────────────────────────────────────

/** Upper bound on any single POI surge multiplier (tested). Keeps a stadium
 *  game-day from swamping the whole assignment. */
export const MAX_POI_SURGE = 6;

/** Deterministic per-day, per-anchor hash in [0,1) — seeds stadium game-days
 *  without any stored schedule or RNG stream. */
function anchorDayHash(seed: number, anchorId: string, day: number): number {
  let h = (seed ^ 0x9e3779b1) >>> 0;
  for (let i = 0; i < anchorId.length; i++) {
    h = Math.imul(h ^ anchorId.charCodeAt(i), 16777619) >>> 0;
  }
  h = Math.imul(h ^ (day & 0xffff), 16777619) >>> 0;
  h = Math.imul(h ^ ((day >> 16) & 0xffff), 16777619) >>> 0;
  return (h >>> 0) / 4294967296;
}

/**
 * Surge multiplier for a POI anchor at a given tick — a bounded, deterministic
 * demand spike layered on top of the cohort baseline. Always ≥1 and
 * ≤ MAX_POI_SURGE.
 *   - stadium:    seeded game-days (~2/week), a sharp 17:00–22:00 event spike.
 *   - airport:    every day, twin directional peaks (AM departures, PM arrivals).
 *   - university: weekday AM/PM aligned with the student cohort.
 *   - others:     mild, flat.
 */
export function poiSurge(anchor: PoiAnchor, seed: number, tick: number): number {
  const day = Math.floor(tick / TICKS_PER_DAY);
  const hour = hourOfDay(tick);
  const weekend = isWeekend(tick);
  switch (anchor.kind) {
    case 'stadium': {
      const gameDay = anchorDayHash(seed, anchor.id, day) < 0.28; // ~2 days/week
      if (!gameDay) return 1;
      return 1 + bump(hour, 19, 2.2, MAX_POI_SURGE - 1);
    }
    case 'airport': {
      // steady all-day floor + morning departure & evening arrival peaks
      const peak = bump(hour, 7, 2.5, 1.4) + bump(hour, 18, 3, 1.6);
      return Math.min(MAX_POI_SURGE, 1.3 + peak);
    }
    case 'university': {
      if (weekend) return 1;
      const peak = bump(hour, 8, 1.4, 1.1) + bump(hour, 16, 2, 0.8);
      return Math.min(MAX_POI_SURGE, 1 + peak);
    }
    default:
      return Math.min(MAX_POI_SURGE, 1 + bump(hour, 13, 4, 0.5));
  }
}
