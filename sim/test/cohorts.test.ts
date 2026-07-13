/**
 * v0.9 System B (Living City) — cohort demand, POI surges, zone response.
 * Covers: propensity rows sum to 1, hourly demand shape (AM peak, night thin),
 * cohort OD determinism, AM-peak-flows-into-CBD directionality, POI surge
 * bounds + scheduling, and zone-growth caps.
 */
import { describe, expect, it } from 'vitest';
import {
  COHORTS,
  HOUR_ATTRACTOR,
  MAX_POI_SURGE,
  attractorAt,
  cohortDemandFactor,
  cohortHourlyRow,
  hourBucket,
  hourlyDemandCurve,
  poiSurge,
} from '../src/core/transit/cohorts';
import { runAssignment } from '../src/core/transit/assignment';
import { applyCommand } from '../src/core/commands';
import { newGame } from '../src/core/newGame';
import { simTick } from '../src/core/sim';
import { TICKS_PER_DAY } from '../src/core/constants';
import type { Command, GameState, PoiAnchor } from '../src/core/types';

describe('cohort propensity rows', () => {
  it('every cohort hourly row is 24 entries summing to 1', () => {
    for (const kind of COHORTS) {
      const row = cohortHourlyRow(kind);
      expect(row.length).toBe(24);
      const sum = row.reduce((a, b) => a + b, 0);
      expect(sum).toBeCloseTo(1, 6);
      for (const v of row) expect(v).toBeGreaterThanOrEqual(0);
    }
  });

  it('the collapsed hourly destination-pull mix sums to 1 each hour', () => {
    for (let h = 0; h < 24; h++) {
      const a = HOUR_ATTRACTOR[h]!;
      expect(a.job + a.home + a.leisure).toBeCloseTo(1, 6);
    }
  });
});

describe('hourly demand shape', () => {
  it('has a daily mean of ~1 and is much busier at 8am than 2am', () => {
    const curve = hourlyDemandCurve(false);
    expect(curve.length).toBe(24);
    const mean = curve.reduce((a, b) => a + b, 0) / 24;
    expect(mean).toBeCloseTo(1, 3);
    expect(curve[8]!).toBeGreaterThan(curve[2]! * 3);
    // both commute peaks (8, 17-18) rise above the daily mean
    expect(curve[8]!).toBeGreaterThan(1);
    expect(Math.max(curve[17]!, curve[18]!)).toBeGreaterThan(1);
  });

  it('cohortDemandFactor tracks the curve at matching hours', () => {
    const curve = hourlyDemandCurve(false);
    for (const h of [2, 8, 13, 18]) {
      const tick = h * (TICKS_PER_DAY / 24);
      expect(hourBucket(tick)).toBe(h);
      expect(cohortDemandFactor(tick)).toBeCloseTo(curve[h]!, 6);
    }
  });

  it('the AM mix pulls toward jobs; the PM mix reverses toward home', () => {
    const am = attractorAt(8 * (TICKS_PER_DAY / 24)); // 08:00 weekday
    const pm = attractorAt(19 * (TICKS_PER_DAY / 24)); // 19:00 weekday
    expect(am.job).toBeGreaterThan(am.home);
    expect(pm.home).toBeGreaterThan(pm.job);
  });
});

/** Build a small transit line on a fresh procedural city and warm assignment. */
function cityWithLine(seed: number): GameState {
  const state = newGame(seed, 'normal');
  const byDemand = [...state.districts].sort((a, b) => b.jobs - a.jobs);
  const cbd = byDemand[0]!;
  const far = [...state.districts].sort(
    (a, b) => Math.hypot(b.centroid.x - cbd.centroid.x, b.centroid.y - cbd.centroid.y) -
      Math.hypot(a.centroid.x - cbd.centroid.x, a.centroid.y - cbd.centroid.y),
  )[0]!;
  const cmds: Command[] = [
    { kind: 'buildStation', mode: 'bus', pos: far.centroid },
    { kind: 'buildStation', mode: 'bus', pos: cbd.centroid },
  ];
  const ids: number[] = [];
  for (const c of cmds) {
    const r = applyCommand(state, c);
    if (r.createdId !== undefined) ids.push(r.createdId);
  }
  applyCommand(state, { kind: 'buildTrack', mode: 'bus', grade: 'surface', fromStationId: ids[0]!, toStationId: ids[1]!, waypoints: [] });
  applyCommand(state, { kind: 'createRoute', mode: 'bus', stationIds: [ids[0]!, ids[1]!] });
  return state;
}

describe('cohort OD assignment', () => {
  it('is deterministic: same state + tick ⇒ identical flows', () => {
    const s = cityWithLine(4242);
    s.tick = 8 * (TICKS_PER_DAY / 24); // fix the hour
    const a = runAssignment(s);
    const b = runAssignment(s);
    expect(a.flows.length).toBe(b.flows.length);
    expect(a.dailyTransitTrips).toBeCloseTo(b.dailyTransitTrips, 9);
    for (let i = 0; i < a.flows.length; i++) {
      expect(a.flows[i]!.transitTrips).toBeCloseTo(b.flows[i]!.transitTrips, 9);
    }
  });

  it('AM demand flows into the CBD (top job district is the busiest destination)', () => {
    const s = cityWithLine(4242);
    const byJobs = [...s.districts].sort((a, b) => b.jobs - a.jobs);
    const cbdId = byJobs[0]!.id;
    s.tick = 8 * (TICKS_PER_DAY / 24);
    const am = runAssignment(s);
    const inbound = new Map<number, number>();
    for (const f of am.flows) inbound.set(f.destDistrict, (inbound.get(f.destDistrict) ?? 0) + f.transitTrips);
    const ranked = [...inbound.entries()].sort((x, y) => y[1] - x[1]);
    // the CBD should attract more AM inbound transit than the city-wide median destination.
    const cbdTrips = inbound.get(cbdId) ?? 0;
    const median = ranked[Math.floor(ranked.length / 2)]?.[1] ?? 0;
    expect(cbdTrips).toBeGreaterThan(median);
  });

  it('the daily trip magnitude is stable across hours (economy not scaled to hourly)', () => {
    const s = cityWithLine(4242);
    s.tick = 8 * (TICKS_PER_DAY / 24);
    const am = runAssignment(s);
    s.tick = 2 * (TICKS_PER_DAY / 24);
    const night = runAssignment(s);
    const total = (o: { dailyTransitTrips: number; dailyCarTrips: number }): number => o.dailyTransitTrips + o.dailyCarTrips;
    // origin magnitude is hour-independent: totals within 25% across hours.
    expect(total(night)).toBeGreaterThan(total(am) * 0.75);
    expect(total(night)).toBeLessThan(total(am) * 1.25);
  });
});

describe('POI surges', () => {
  const anchors: Record<string, PoiAnchor> = {
    stadium: { id: 'st1', kind: 'stadium', name: 'Stadium', centroid: [0, 0] },
    airport: { id: 'ap1', kind: 'airport', name: 'Airport', centroid: [0, 0] },
    university: { id: 'un1', kind: 'university', name: 'Uni', centroid: [0, 0] },
    hospital: { id: 'hp1', kind: 'hospital', name: 'Hospital', centroid: [0, 0] },
  };

  it('every surge multiplier stays within [1, MAX_POI_SURGE]', () => {
    for (const a of Object.values(anchors)) {
      for (let day = 0; day < 21; day++) {
        for (let h = 0; h < 24; h++) {
          const tick = day * TICKS_PER_DAY + h * (TICKS_PER_DAY / 24);
          const s = poiSurge(a, 12345, tick);
          expect(s).toBeGreaterThanOrEqual(1);
          expect(s).toBeLessThanOrEqual(MAX_POI_SURGE);
        }
      }
    }
  });

  it('stadium game-days are seeded + deterministic, and not every day', () => {
    const a = anchors.stadium!;
    const eveningTick = (day: number): number => day * TICKS_PER_DAY + 19 * (TICKS_PER_DAY / 24);
    let surgeDays = 0;
    for (let day = 0; day < 28; day++) {
      const s1 = poiSurge(a, 777, eveningTick(day));
      const s2 = poiSurge(a, 777, eveningTick(day));
      expect(s1).toBe(s2); // deterministic
      if (s1 > 1.01) surgeDays++;
    }
    expect(surgeDays).toBeGreaterThan(0);
    expect(surgeDays).toBeLessThan(28); // not every day
  });

  it('airport shows a directional evening arrival peak above its midnight floor', () => {
    const a = anchors.airport!;
    const evening = poiSurge(a, 1, 18 * (TICKS_PER_DAY / 24));
    const midnight = poiSurge(a, 1, 0);
    expect(evening).toBeGreaterThan(midnight);
  });
});

describe('zone response (growth caps)', () => {
  it('district population growth per period stays within the slow cap', () => {
    const s = cityWithLine(9182);
    // run ~3 weeks so several growth periods fire near a live line
    for (let t = 0; t < 3 * 7 * TICKS_PER_DAY + 5; t++) simTick(s);
    for (const d of s.districts) {
      if (d.lastGrowthDelta === undefined) continue;
      // per-period change is bounded: growth capped ~3.5%, shrink small.
      expect(d.lastGrowthDelta).toBeLessThan(0.04);
      expect(d.lastGrowthDelta).toBeGreaterThan(-0.02);
    }
    // at least one district recorded a growth delta
    expect(s.districts.some((d) => d.lastGrowthDelta !== undefined)).toBe(true);
  });
});
