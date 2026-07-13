/**
 * Operating-cost / fare economy (sim-depth subsystem D): per-route running
 * cost, farebox recovery, and the cumulative lifetime ledger.
 */
import { describe, expect, it } from 'vitest';
import { MODES } from '../src/core/constants';
import { fareboxRecovery, routeOperatingCost } from '../src/core/economy';
import { applyCommand } from '../src/core/commands';
import { newGame } from '../src/core/newGame';
import { deserialize, serialize } from '../src/core/save';
import { setBankruptDays, simTick } from '../src/core/sim';
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
  return state;
}

describe('operating cost + farebox', () => {
  it('routeOperatingCost = vehicleCount × (ops + maint) per vehicle/day', () => {
    const cfg = MODES.bus;
    expect(routeOperatingCost('bus', 0)).toBe(0);
    expect(routeOperatingCost('bus', 4)).toBe(4 * (cfg.opsPerVehiclePerDay + cfg.maintPerVehiclePerDay));
  });

  it('fareboxRecovery is fares / running costs, 0 when nothing runs', () => {
    expect(fareboxRecovery({ fares: 0, subsidy: 0, operations: 0, maintenance: 0, interest: 0 })).toBe(0);
    expect(fareboxRecovery({ fares: 300, subsidy: 0, operations: 200, maintenance: 100, interest: 5 })).toBeCloseTo(1, 6);
    expect(fareboxRecovery({ fares: 600, subsidy: 0, operations: 200, maintenance: 100, interest: 5 })).toBeCloseTo(2, 6);
  });
});

describe('lifetime ledger', () => {
  it('accumulates once days close and is >= a single day', () => {
    const state = runningNetwork(4242);
    for (let t = 0; t < 3600; t++) simTick(state); // ~3 days
    const life = state.budget.lifetime;
    expect(life).toBeDefined();
    expect(life!.days).toBeGreaterThanOrEqual(2);
    expect(life!.operations).toBeGreaterThan(0);
    expect(life!.fares).toBeGreaterThanOrEqual(state.budget.lastDay.fares - 1e-6);
    expect(life!.subsidy).toBeGreaterThan(0);
  });

  it('survives a save round-trip and keeps accumulating', () => {
    const state = runningNetwork(555);
    for (let t = 0; t < 2400; t++) simTick(state);
    const before = state.budget.lifetime!;
    const restored = deserialize(serialize(state));
    expect(restored.budget.lifetime).toEqual(before);
    const days0 = restored.budget.lifetime!.days;
    for (let t = 0; t < 1200; t++) simTick(restored);
    expect(restored.budget.lifetime!.days).toBe(days0 + 1);
  });
});
