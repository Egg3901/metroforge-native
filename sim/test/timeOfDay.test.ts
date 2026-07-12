/**
 * Time-of-day demand curve (sim-depth subsystem A). Pure, deterministic checks
 * on the diurnal model plus an integration check that vehicle occupancy pulses
 * with the rush.
 */
import { describe, expect, it } from 'vitest';
import { TICKS_PER_DAY } from '../src/core/constants';
import { DIURNAL_MEAN, PEAK_HOUR_SHARE, diurnalDemand, diurnalFactor, hourOfDay } from '../src/core/timeOfDay';
import { applyCommand } from '../src/core/commands';
import { newGame } from '../src/core/newGame';
import { setBankruptDays, simTick } from '../src/core/sim';
import type { Command } from '../src/core/types';

const tickAtHour = (h: number): number => Math.round((h / 24) * TICKS_PER_DAY);

describe('time-of-day demand curve', () => {
  it('hourOfDay wraps into [0,24) and handles multi-day / negative ticks', () => {
    expect(hourOfDay(0)).toBeCloseTo(0, 6);
    expect(hourOfDay(TICKS_PER_DAY / 2)).toBeCloseTo(12, 6);
    expect(hourOfDay(TICKS_PER_DAY)).toBeCloseTo(0, 6); // next day, same hour
    expect(hourOfDay(TICKS_PER_DAY * 3 + TICKS_PER_DAY / 4)).toBeCloseTo(6, 6);
    const h = hourOfDay(-1);
    expect(h).toBeGreaterThanOrEqual(0);
    expect(h).toBeLessThan(24);
  });

  it('is deterministic (pure function of tick)', () => {
    for (const t of [0, 137, 599, 1200, 9001]) {
      expect(diurnalDemand(t)).toBe(diurnalDemand(t));
      expect(diurnalFactor(t)).toBe(diurnalFactor(t));
    }
  });

  it('normalized factor has a daily mean of 1.0', () => {
    let sum = 0;
    for (let t = 0; t < TICKS_PER_DAY; t++) sum += diurnalFactor(t);
    expect(sum / TICKS_PER_DAY).toBeCloseTo(1, 6);
    expect(DIURNAL_MEAN).toBeGreaterThan(0);
  });

  it('rush hours peak above the mean; the dead of night sits well below it', () => {
    const amRush = diurnalFactor(tickAtHour(8));
    const pmRush = diurnalFactor(tickAtHour(17.5));
    const night = diurnalFactor(tickAtHour(3));
    expect(amRush).toBeGreaterThan(1.2);
    expect(pmRush).toBeGreaterThan(1.2);
    expect(night).toBeLessThan(0.6);
    expect(amRush).toBeGreaterThan(night);
  });

  it('peak-hour share is a sensible fraction of the day', () => {
    expect(PEAK_HOUR_SHARE).toBeGreaterThan(1 / 24); // busier than an even hour
    expect(PEAK_HOUR_SHARE).toBeLessThan(0.25);
  });

  it('vehicle occupancy is higher at the rush than overnight on a loaded line', () => {
    setBankruptDays(0);
    const state = newGame(20240711, 'normal');
    const byDemand = [...state.districts].sort((a, b) => b.population + b.jobs - (a.population + a.jobs));
    const picks: { x: number; y: number }[] = [];
    for (const d of byDemand) {
      if (picks.every((p) => Math.hypot(p.x - d.centroid.x, p.y - d.centroid.y) > 700)) picks.push(d.centroid);
      if (picks.length === 3) break;
    }
    const ids: number[] = [];
    for (const pos of picks) {
      const r = applyCommand(state, { kind: 'buildStation', mode: 'bus', pos });
      ids.push(r.createdId!);
    }
    const build: Command[] = [
      { kind: 'buildTrack', mode: 'bus', grade: 'surface', fromStationId: ids[0]!, toStationId: ids[1]!, waypoints: [] },
      { kind: 'buildTrack', mode: 'bus', grade: 'surface', fromStationId: ids[1]!, toStationId: ids[2]!, waypoints: [] },
    ];
    for (const c of build) expect(applyCommand(state, c).ok).toBe(true);
    const route = applyCommand(state, { kind: 'createRoute', mode: 'bus', stationIds: ids });
    expect(route.ok).toBe(true);
    // warm the assignment so segmentLoads are populated
    for (let t = 0; t < 1300; t++) simTick(state);

    const occAtHour = (h: number): number => {
      state.tick = tickAtHour(h);
      simTick(state);
      return state.vehicles.reduce((a, v) => a + v.occupancy, 0);
    };
    const rush = occAtHour(8);
    const night = occAtHour(3);
    expect(state.vehicles.length).toBeGreaterThan(0);
    expect(rush).toBeGreaterThan(night);
  });
});
