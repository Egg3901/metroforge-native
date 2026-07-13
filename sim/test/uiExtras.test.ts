/**
 * Optional UiState/UiRoute enrichments (sim-depth): per-district catchment
 * exposure (subsystem B), crowding surfacing (subsystem C), and the economy /
 * time-of-day summary fields (A + D).
 */
import { describe, expect, it } from 'vitest';
import { applyCommand } from '../src/core/commands';
import { newGame } from '../src/core/newGame';
import { setBankruptDays, simTick } from '../src/core/sim';
import { routeExtras, todFactorOf, uiExtras } from '../src/host/uiExtras';
import type { Command, GameState } from '../src/core/types';

function runningNetwork(seed: number): GameState {
  setBankruptDays(0);
  const state = newGame(seed, 'normal');
  const byDemand = [...state.districts].sort((a, b) => b.population + b.jobs - (a.population + a.jobs));
  const picks: { x: number; y: number }[] = [];
  for (const d of byDemand) {
    if (picks.every((p) => Math.hypot(p.x - d.centroid.x, p.y - d.centroid.y) > 700)) picks.push(d.centroid);
    if (picks.length === 3) break;
  }
  const ids: number[] = [];
  for (const pos of picks) ids.push(applyCommand(state, { kind: 'buildStation', mode: 'bus', pos }).createdId!);
  const build: Command[] = [
    { kind: 'buildTrack', mode: 'bus', grade: 'surface', fromStationId: ids[0]!, toStationId: ids[1]!, waypoints: [] },
    { kind: 'buildTrack', mode: 'bus', grade: 'surface', fromStationId: ids[1]!, toStationId: ids[2]!, waypoints: [] },
  ];
  for (const c of build) expect(applyCommand(state, c).ok).toBe(true);
  expect(applyCommand(state, { kind: 'createRoute', mode: 'bus', stationIds: ids }).ok).toBe(true);
  // a single vehicle on the whole line to make crowding real
  const rid = state.routes[0]!.id;
  applyCommand(state, { kind: 'editRoute', routeId: rid, vehicleCount: 1, headwaySeconds: 1800 });
  for (let t = 0; t < 1300; t++) simTick(state);
  return state;
}

describe('uiExtras district catchment (subsystem B)', () => {
  it('exposes every district with building-derived population and jobs', () => {
    const state = newGame(999, 'normal');
    const ex = uiExtras(state);
    expect(ex.districts.length).toBe(state.districts.length);
    expect(ex.districts.length).toBeGreaterThan(20);
    const totalPop = ex.districts.reduce((a, d) => a + d.population, 0);
    expect(totalPop).toBeGreaterThan(100_000);
    const d0 = ex.districts[0]!;
    expect(d0).toMatchObject({ id: state.districts[0]!.id, name: state.districts[0]!.name });
    expect(d0.x).toBe(state.districts[0]!.centroid.x);
  });
});

describe('uiExtras economy + time-of-day summary (A + D)', () => {
  it('reports hour, demand factor and farebox recovery', () => {
    const state = runningNetwork(7777);
    const ex = uiExtras(state);
    expect(ex.hourOfDay).toBeGreaterThanOrEqual(0);
    expect(ex.hourOfDay).toBeLessThan(24);
    expect(ex.demandFactor).toBeGreaterThan(0);
    expect(ex.fareboxRecovery).toBeGreaterThanOrEqual(0);
    expect(ex.lifetime).toBeDefined();
  });

  it('per-route extras: operating cost, farebox, live crowding, avg effective speed', () => {
    const state = runningNetwork(8888);
    const tod = todFactorOf(state);
    const r = state.routes[0]!;
    const rx = routeExtras(r, tod, state);
    expect(rx.operatingCost).toBeGreaterThan(0);
    expect(rx.farebox).toBeCloseTo(r.dailyRevenue / rx.operatingCost, 6);
    expect(rx.liveCrowding).toBeCloseTo((r.crowding ?? 0) * tod, 6);
    expect(rx.avgEffectiveSpeed).toBeGreaterThan(0);
  });
});

describe('uiExtras crowding surfacing (subsystem C)', () => {
  it('counts routes over capacity', () => {
    const state = runningNetwork(1357);
    const ex = uiExtras(state);
    const expected = state.routes.filter((r) => (r.crowding ?? 0) > 1).length;
    expect(ex.overcrowdedRoutes).toBe(expected);
    expect(ex.overcrowdedRoutes).toBeGreaterThanOrEqual(0);
  });
});
