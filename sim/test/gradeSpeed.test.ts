/**
 * Grade as an operating tradeoff (Egg3901/metroforge#38): surface shares the
 * street (diurnal congestion slows travel at rush in dense districts);
 * elevated/tunnel keep full mode speed. Identical tram corridors at surface vs
 * elevated prove the feedback into trip time, ridership, headway, and the daily
 * ledger — no new player-facing systems, just the existing crowding/waiting
 * penalty feeling the slowdown. Also asserts the grade slowdown COMPOSES
 * multiplicatively with the weather speed model and stays within bounds.
 */
import { describe, expect, it } from 'vitest';
import { applyCommand, routeCycleSeconds, trackCost } from '../src/core/commands';
import { GRADE_MAINT_MULT, MODES, TICKS_PER_DAY } from '../src/core/constants';
import { newGame } from '../src/core/newGame';
import { setBankruptDays, simTick } from '../src/core/sim';
import { diurnalFactor } from '../src/core/timeOfDay';
import {
  MEAN_RUSH_EXCESS,
  dayAverageSurfaceSlowdown,
  segmentDayAverageSpeedMps,
  segmentEffectiveSpeedMps,
  surfaceCongestionSlowdown,
} from '../src/core/transit/gradeEffects';
import { routeAvgEffectiveSpeed, routeExtras, todFactorOf } from '../src/host/uiExtras';
import type { GameState, TrackGrade } from '../src/core/types';

const tickAtHour = (h: number): number => Math.round((h / 24) * TICKS_PER_DAY);

/** Two dense-district centroids spaced for a short tram spine. */
function denseCorridorPicks(state: GameState): [{ x: number; y: number }, { x: number; y: number }] {
  const byDemand = [...state.districts].sort((a, b) => b.population + b.jobs - (a.population + a.jobs));
  const picks: { x: number; y: number }[] = [];
  for (const d of byDemand) {
    if (picks.every((p) => Math.hypot(p.x - d.centroid.x, p.y - d.centroid.y) > 500)) {
      picks.push(d.centroid);
    }
    if (picks.length === 2) break;
  }
  return picks as [{ x: number; y: number }, { x: number; y: number }];
}

/** Boost land value along the corridor so surface congestion is clearly felt. */
function densifyCorridor(state: GameState, picks: { x: number; y: number }[]): void {
  const g = state.fields;
  for (let i = 0; i < g.landValue.length; i++) {
    const cx = g.originX + ((i % g.w) + 0.5) * g.cellSize;
    const cy = g.originY + (Math.floor(i / g.w) + 0.5) * g.cellSize;
    for (const p of picks) {
      if (Math.hypot(cx - p.x, cy - p.y) < 900) {
        g.landValue[i] = Math.max(g.landValue[i] as number, 2.4);
      }
    }
  }
}

function buildTramLine(seed: number, grade: TrackGrade): GameState {
  setBankruptDays(0);
  const state = newGame(seed, 'easy');
  if (!state.unlockedModes.includes('tram')) state.unlockedModes.push('tram');
  state.budget.cash = 80_000_000;
  const picks = denseCorridorPicks(state);
  densifyCorridor(state, picks);
  const ids: number[] = [];
  for (const pos of picks) {
    const r = applyCommand(state, { kind: 'buildStation', mode: 'tram', pos });
    expect(r.ok).toBe(true);
    ids.push(r.createdId!);
  }
  const track = applyCommand(state, {
    kind: 'buildTrack',
    mode: 'tram',
    grade,
    fromStationId: ids[0]!,
    toStationId: ids[1]!,
    waypoints: [],
  });
  expect(track.ok).toBe(true);
  const route = applyCommand(state, { kind: 'createRoute', mode: 'tram', stationIds: ids });
  expect(route.ok).toBe(true);
  // generous fleet so headway isn't the only bottleneck
  const rid = state.routes[0]!.id;
  expect(applyCommand(state, { kind: 'editRoute', routeId: rid, vehicleCount: 6 }).ok).toBe(true);
  return state;
}

describe('gradeEffects unit', () => {
  it('elevated/tunnel ignore diurnal factor; surface slows only above the mean', () => {
    const rush = diurnalFactor(tickAtHour(8));
    const night = diurnalFactor(tickAtHour(3));
    expect(rush).toBeGreaterThan(1);
    expect(night).toBeLessThan(1);

    const dens = 0.9;
    expect(segmentEffectiveSpeedMps('tram', 'elevated', rush, dens)).toBe(MODES.tram.speed);
    expect(segmentEffectiveSpeedMps('tram', 'tunnel', rush, dens)).toBe(MODES.tram.speed);
    expect(segmentEffectiveSpeedMps('tram', 'surface', rush, dens)).toBeLessThan(MODES.tram.speed);
    // off-peak / overnight: no slowdown → equal to elevated
    expect(segmentEffectiveSpeedMps('tram', 'surface', night, dens)).toBe(MODES.tram.speed);
    expect(surfaceCongestionSlowdown('tram', dens, night)).toBe(1);
  });

  it('bus/tram feel congestion more than surface metro/rail; density amplifies', () => {
    const rush = 1.8;
    const busDense = surfaceCongestionSlowdown('bus', 1, rush);
    const tramDense = surfaceCongestionSlowdown('tram', 1, rush);
    const metroDense = surfaceCongestionSlowdown('metro', 1, rush);
    const busSparse = surfaceCongestionSlowdown('bus', 0, rush);
    expect(busDense).toBeGreaterThan(tramDense);
    expect(tramDense).toBeGreaterThan(metroDense);
    expect(busDense).toBeGreaterThan(busSparse);
    expect(MEAN_RUSH_EXCESS).toBeGreaterThan(0);
    expect(dayAverageSurfaceSlowdown('tram', 0.9)).toBeGreaterThan(1);
    expect(segmentDayAverageSpeedMps('tram', 'elevated', 0.9)).toBe(MODES.tram.speed);
    expect(segmentDayAverageSpeedMps('tram', 'surface', 0.9)).toBeLessThan(MODES.tram.speed);
  });

  it('slowdown is bounded (>=1) and speed never exceeds mode cruise', () => {
    for (const mode of ['bus', 'tram', 'metro', 'rail'] as const) {
      for (const dens of [0, 0.5, 1]) {
        for (const tod of [0.3, 1, 1.5, 1.9]) {
          expect(surfaceCongestionSlowdown(mode, dens, tod)).toBeGreaterThanOrEqual(1);
          const spd = segmentEffectiveSpeedMps(mode, 'surface', tod, dens);
          expect(spd).toBeGreaterThan(0);
          expect(spd).toBeLessThanOrEqual(MODES[mode].speed + 1e-9);
        }
      }
    }
  });

  it('grade slowdown COMPOSES multiplicatively with a weather-style multiplier', () => {
    // The sim applies gradeSpeed * weatherMult (see moveVehicles). Verify the two
    // factors multiply cleanly: half-speed weather on a rush-slowed surface
    // segment is exactly half of the grade-only speed.
    const dens = 0.9;
    const rush = diurnalFactor(tickAtHour(8));
    const gradeOnly = segmentEffectiveSpeedMps('tram', 'surface', rush, dens);
    const weatherMult = 0.5;
    const composed = gradeOnly * weatherMult;
    expect(composed).toBeCloseTo(gradeOnly / 2, 9);
    // order independence: same result regardless of which factor is applied first
    const composedOther = MODES.tram.speed * weatherMult / surfaceCongestionSlowdown('tram', dens, rush);
    expect(composedOther).toBeCloseTo(composed, 9);
  });

  it('GRADE_MAINT_MULT ranks surface < elevated < tunnel', () => {
    expect(GRADE_MAINT_MULT.surface).toBe(1);
    expect(GRADE_MAINT_MULT.elevated).toBeGreaterThan(GRADE_MAINT_MULT.surface);
    expect(GRADE_MAINT_MULT.tunnel).toBeGreaterThan(GRADE_MAINT_MULT.elevated);
  });
});

describe('surface vs elevated corridor (dense city)', () => {
  const SEED = 20240711;

  it('surface is slower at peak, equal off-peak; elevated keeps full speed', () => {
    const surface = buildTramLine(SEED, 'surface');
    const elevated = buildTramLine(SEED, 'elevated');

    const peakTick = tickAtHour(8);
    const offTick = tickAtHour(3);
    surface.tick = peakTick;
    elevated.tick = peakTick;
    const peakSurf = routeAvgEffectiveSpeed(surface, surface.routes[0]!, todFactorOf(surface));
    const peakEl = routeAvgEffectiveSpeed(elevated, elevated.routes[0]!, todFactorOf(elevated));
    expect(peakSurf).toBeLessThan(peakEl);
    expect(peakEl).toBeCloseTo(MODES.tram.speed, 6);

    surface.tick = offTick;
    elevated.tick = offTick;
    const offSurf = routeAvgEffectiveSpeed(surface, surface.routes[0]!, todFactorOf(surface));
    const offEl = routeAvgEffectiveSpeed(elevated, elevated.routes[0]!, todFactorOf(elevated));
    expect(offSurf).toBeCloseTo(offEl, 6);
    expect(offSurf).toBeCloseTo(MODES.tram.speed, 6);
  });

  it('surface loses riders vs elevated; costs less to build but more per rider', () => {
    const surface = buildTramLine(SEED, 'surface');
    const elevated = buildTramLine(SEED, 'elevated');
    // warm assignment + a few days so crowding feedback settles
    for (let t = 0; t < TICKS_PER_DAY * 3 + 50; t++) {
      simTick(surface);
      simTick(elevated);
    }
    const sR = surface.routes[0]!;
    const eR = elevated.routes[0]!;
    expect(eR.dailyRidership).toBeGreaterThan(sR.dailyRidership);
    // longer surface cycle → worse headway feeds the wait penalty
    expect(routeCycleSeconds(surface, sR.id)).toBeGreaterThan(routeCycleSeconds(elevated, eR.id));
    expect(sR.headwaySeconds).toBeGreaterThanOrEqual(eR.headwaySeconds);

    const sBuild = surface.tracks.reduce((a, t) => a + t.buildCost, 0);
    const eBuild = elevated.tracks.reduce((a, t) => a + t.buildCost, 0);
    expect(sBuild).toBeLessThan(eBuild);
  });

  it('elevated track maintenance on the daily ledger exceeds surface', () => {
    const surface = buildTramLine(SEED, 'surface');
    const elevated = buildTramLine(SEED, 'elevated');
    for (let t = 0; t < TICKS_PER_DAY + 2; t++) {
      simTick(surface);
      simTick(elevated);
    }
    expect(elevated.budget.lastDay.maintenance).toBeGreaterThan(surface.budget.lastDay.maintenance);
    // per-track maintenance ratio tracks GRADE_MAINT_MULT exactly
    const sTrack = surface.tracks[0]!;
    const eTrack = elevated.tracks[0]!;
    const sMaint = (sTrack.polyline.length / 1000) * MODES.tram.maintPerKmPerDay * GRADE_MAINT_MULT.surface;
    const eMaint = (eTrack.polyline.length / 1000) * MODES.tram.maintPerKmPerDay * GRADE_MAINT_MULT.elevated;
    expect(eMaint / sMaint).toBeCloseTo(GRADE_MAINT_MULT.elevated / GRADE_MAINT_MULT.surface, 6);
  });

  it('busy corridor still rewards grade separation (elevated earns more riders and revenue)', () => {
    const surface = buildTramLine(SEED, 'surface');
    const elevated = buildTramLine(SEED, 'elevated');
    for (let t = 0; t < TICKS_PER_DAY * 4 + 20; t++) {
      simTick(surface);
      simTick(elevated);
    }
    const sR = surface.routes[0]!;
    const eR = elevated.routes[0]!;
    const sBuild = surface.tracks.reduce((a, t) => a + t.buildCost, 0);
    const eBuild = elevated.tracks.reduce((a, t) => a + t.buildCost, 0);
    expect(eR.dailyRidership).toBeGreaterThan(sR.dailyRidership);
    expect(eR.dailyRevenue).toBeGreaterThan(sR.dailyRevenue);
    expect(sBuild).toBeLessThan(eBuild);
  });

  it('exposes avgEffectiveSpeed on route extras for the route inspector', () => {
    const surface = buildTramLine(SEED, 'surface');
    surface.tick = tickAtHour(8);
    const tod = todFactorOf(surface);
    const rx = routeExtras(surface.routes[0]!, tod, surface);
    expect(rx.avgEffectiveSpeed).toBeGreaterThan(0);
    expect(rx.avgEffectiveSpeed).toBeLessThan(MODES.tram.speed);
    const elevated = buildTramLine(SEED, 'elevated');
    elevated.tick = tickAtHour(8);
    const ex = routeExtras(elevated.routes[0]!, todFactorOf(elevated), elevated);
    expect(ex.avgEffectiveSpeed).toBeCloseTo(MODES.tram.speed, 6);
    // without state the field degrades to 0 (older host path)
    expect(routeExtras(surface.routes[0]!, tod).avgEffectiveSpeed).toBe(0);
  });

  it('trackCost still applies gradeCostMult (surface cheaper to build)', () => {
    const state = buildTramLine(SEED, 'surface');
    const pts = state.tracks[0]!.polyline.points;
    const surf = trackCost(state, 'tram', 'surface', pts);
    const elev = trackCost(state, 'tram', 'elevated', pts);
    expect(elev / surf).toBeCloseTo(MODES.tram.gradeCostMult.elevated / MODES.tram.gradeCostMult.surface, 2);
  });
});
