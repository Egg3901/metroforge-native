/**
 * v0.9 System A (Operations) unit tests: per-period frequency → vehicle-count
 * math, condition-decay bounds, breakdown determinism, reliability → demand
 * monotonicity, and the save migration for the ops sub-state.
 */
import { describe, expect, it } from 'vitest';
import { applyCommand } from '../src/core/commands';
import { newGame } from '../src/core/newGame';
import { deserialize, serialize, stateHash } from '../src/core/save';
import { simTick } from '../src/core/sim';
import {
  fleetSummary,
  peakUnitsRequired,
  reliabilityDemandMultFor,
  unitsForPeriod,
} from '../src/core/ops';
import { opsTunables } from '../src/core/ops/tunables';
import { periodForTick, PERIODS } from '../src/core/ops/periods';
import type { GameState } from '../src/core/types';

/** Build a single working bus route on the 3 densest, well-spaced districts. */
function buildRoute(state: GameState, vehicles = 6): number {
  const byDemand = [...state.districts].sort((a, b) => b.population + b.jobs - (a.population + a.jobs));
  const picks: { x: number; y: number }[] = [];
  for (const d of byDemand) {
    if (picks.every((p) => Math.hypot(p.x - d.centroid.x, p.y - d.centroid.y) > 800)) picks.push(d.centroid);
    if (picks.length === 3) break;
  }
  const ids: number[] = [];
  for (const p of picks) {
    const r = applyCommand(state, { kind: 'buildStation', mode: 'bus', pos: p });
    if (r.createdId !== undefined) ids.push(r.createdId);
  }
  applyCommand(state, { kind: 'buildTrack', mode: 'bus', grade: 'surface', fromStationId: ids[0]!, toStationId: ids[1]!, waypoints: [] });
  applyCommand(state, { kind: 'buildTrack', mode: 'bus', grade: 'surface', fromStationId: ids[1]!, toStationId: ids[2]!, waypoints: [] });
  const route = applyCommand(state, { kind: 'createRoute', mode: 'bus', stationIds: ids });
  applyCommand(state, { kind: 'editRoute', routeId: route.createdId!, vehicleCount: vehicles });
  return route.createdId!;
}

describe('periods', () => {
  it('resolves the five service periods across the game day', () => {
    // TICKS_PER_DAY = 1200 → 50 ticks per game-hour.
    const at = (hour: number): string => periodForTick(Math.round(hour * 50));
    expect(at(2)).toBe('night');
    expect(at(7)).toBe('amPeak');
    expect(at(12)).toBe('midday');
    expect(at(17)).toBe('pmPeak');
    expect(at(20)).toBe('evening');
    expect(at(23)).toBe('night');
  });
});

describe('frequency → vehicle-count math', () => {
  it('more service (shorter target headway) needs more vehicles, monotonically', () => {
    const state = newGame(4242, 'normal');
    const routeId = buildRoute(state, 8);
    for (let i = 0; i < 400; i++) simTick(state); // let cycle/density settle
    const route = state.routes.find((r) => r.id === routeId)!;
    const at120 = unitsForPeriod(state, route, 'amPeak'); // will use default (no override)
    // set explicit targets and check the units-needed grows as headway shrinks.
    route.frequency = { amPeak: 600 };
    const need600 = unitsForPeriod(state, route, 'amPeak');
    route.frequency = { amPeak: 300 };
    const need300 = unitsForPeriod(state, route, 'amPeak');
    route.frequency = { amPeak: 150 };
    const need150 = unitsForPeriod(state, route, 'amPeak');
    expect(need300).toBeGreaterThan(need600);
    expect(need150).toBeGreaterThan(need300);
    expect(at120).toBeGreaterThan(0);
    // peak requirement is the max across periods.
    expect(peakUnitsRequired(state, route)).toBeGreaterThanOrEqual(1);
  });

  it('buying vehicles mints discrete fleet units matching vehicleCount', () => {
    const state = newGame(4242, 'normal');
    const routeId = buildRoute(state, 5);
    const assigned = (state.fleet ?? []).filter((u) => u.routeId === routeId);
    expect(assigned.length).toBe(5);
    // retiring removes units back down to the new count.
    applyCommand(state, { kind: 'editRoute', routeId, vehicleCount: 2 });
    expect((state.fleet ?? []).filter((u) => u.routeId === routeId).length).toBe(2);
  });
});

describe('condition decay', () => {
  it('stays within [0,1] and falls under running + weather over a long horizon', () => {
    const state = newGame(4242, 'normal', { presetKey: 'nyc' });
    buildRoute(state, 6);
    const before = fleetSummary(state).avgCondition;
    for (let i = 0; i < 6000; i++) simTick(state);
    const after = fleetSummary(state).avgCondition;
    for (const u of state.fleet ?? []) {
      expect(u.condition).toBeGreaterThanOrEqual(0);
      expect(u.condition).toBeLessThanOrEqual(1);
    }
    // running units wear (no depot built → no maintenance restore in this run).
    expect(after).toBeLessThan(before);
  });
});

describe('breakdowns', () => {
  it('are deterministic: same seed → identical incident + fleet state', () => {
    const run = (): GameState => {
      // hard mode → higher breakdown rate so incidents actually fire in the window.
      const s = newGame(90210, 'hard', { presetKey: 'nyc' });
      buildRoute(s, 8);
      for (let i = 0; i < 8000; i++) simTick(s);
      return s;
    };
    const a = run();
    const b = run();
    expect(stateHash(a)).toBe(stateHash(b));
    // and some ops actually happened (fleet exists, conditions moved off 1).
    expect((a.fleet ?? []).length).toBeGreaterThan(0);
    expect(fleetSummary(a).avgCondition).toBeLessThan(1);
  });

  it('hard mode breaks down more than forgiving over the same horizon', () => {
    const countBreakdownsSeen = (difficulty: 'normal' | 'hard'): number => {
      const s = newGame(1357, difficulty, { presetKey: 'nyc' });
      buildRoute(s, 10);
      let everBroken = 0;
      for (let i = 0; i < 12000; i++) {
        simTick(s);
        // count units currently down as a proxy for breakdown pressure.
        everBroken += (s.fleet ?? []).filter((u) => u.status === 'brokenDown').length;
      }
      return everBroken;
    };
    expect(countBreakdownsSeen('hard')).toBeGreaterThan(countBreakdownsSeen('normal'));
  });
});

describe('reliability → demand monotonicity (the keystone)', () => {
  it('the demand multiplier is non-decreasing in on-time% and clamps to [floor,1]', () => {
    const t = opsTunables('normal');
    let prev = -1;
    for (let p = 0; p <= 1.0001; p += 0.05) {
      const m = reliabilityDemandMultFor(p, t);
      expect(m).toBeGreaterThanOrEqual(t.demandMultAtZeroOnTime - 1e-9);
      expect(m).toBeLessThanOrEqual(1 + 1e-9);
      expect(m).toBeGreaterThanOrEqual(prev - 1e-9); // non-decreasing
      prev = m;
    }
    // at/above target → full demand (1.0); at zero on-time → the floor.
    expect(reliabilityDemandMultFor(1, t)).toBe(1);
    expect(reliabilityDemandMultFor(t.onTimeTarget, t)).toBe(1);
    expect(reliabilityDemandMultFor(0, t)).toBeCloseTo(t.demandMultAtZeroOnTime, 9);
  });

  it('a perfectly reliable forgiving route keeps its ridership (mult ~ 1)', () => {
    const state = newGame(4242, 'normal');
    const routeId = buildRoute(state, 6);
    for (let i = 0; i < 3000; i++) simTick(state);
    const route = state.routes.find((r) => r.id === routeId)!;
    expect(route.onTimePct).toBeGreaterThanOrEqual(0.9);
    expect(route.reliabilityDemandMult).toBeGreaterThan(0.98);
  });
});

describe('save migration', () => {
  it('a legacy save with no ops sub-state loads and seeds ops deterministically', () => {
    const state = newGame(777, 'normal');
    buildRoute(state, 4);
    for (let i = 0; i < 1200; i++) simTick(state);
    const json = serialize(state);
    // strip the ops sub-state to simulate a pre-v0.9 (v2) save.
    const parsed = JSON.parse(json) as { version: number; state: Record<string, unknown> };
    parsed.version = 2;
    delete parsed.state.fleet;
    delete parsed.state.depots;
    delete parsed.state.incidents;
    delete parsed.state.opsRngState;
    delete parsed.state.opsPeriod;
    delete parsed.state.opsDaily;
    const restored = deserialize(JSON.stringify(parsed));
    // ops sub-state is rebuilt; fleet reconciled to each route's vehicleCount.
    expect(restored.fleet).toBeDefined();
    expect(restored.opsRngState).toBeDefined();
    for (const r of restored.routes) {
      expect((restored.fleet ?? []).filter((u) => u.routeId === r.id).length).toBe(r.vehicleCount);
    }
    // and it continues deterministically from the migrated state.
    const a = deserialize(JSON.stringify(parsed));
    const b = deserialize(JSON.stringify(parsed));
    for (let i = 0; i < 1500; i++) {
      simTick(a);
      simTick(b);
    }
    expect(stateHash(a)).toBe(stateHash(b));
  });

  it('a v3 save round-trips its ops state bit-for-bit and continues identically', () => {
    const state = newGame(31337, 'hard', { presetKey: 'nyc' });
    buildRoute(state, 8);
    for (let i = 0; i < 5000; i++) simTick(state);
    const before = stateHash(state);
    const restored = deserialize(serialize(state));
    expect(stateHash(restored)).toBe(before);
    for (let i = 0; i < 1000; i++) {
      simTick(state);
      simTick(restored);
    }
    expect(stateHash(restored)).toBe(stateHash(state));
  });
});

describe('depots + maintenance', () => {
  it('a depot enables maintenance windows that restore worn units', () => {
    const state = newGame(2024, 'hard', { presetKey: 'nyc' });
    const routeId = buildRoute(state, 8);
    expect(routeId).toBeGreaterThan(0);
    for (let i = 0; i < 400; i++) simTick(state); // let service settle
    // wear units well below the maintenance threshold (the decay magnitude is an
    // owner-tuned placeholder, so drive the dispatch/restore logic directly).
    for (const u of state.fleet ?? []) u.condition = 0.2;
    const worn = fleetSummary(state).avgCondition;
    expect(worn).toBeLessThan(0.4);
    // NO depot yet → worn units are NOT dispatched (deferred maintenance).
    for (let i = 0; i < 1300; i++) simTick(state); // cross a day close
    expect((state.fleet ?? []).some((u) => u.status === 'maintenance')).toBe(false);
    // place a bus depot; a second of the same mode is rejected.
    const near = state.stations[0]!.pos;
    expect(applyCommand(state, { kind: 'buildDepot', mode: 'bus', pos: near }).ok).toBe(true);
    expect(applyCommand(state, { kind: 'buildDepot', mode: 'bus', pos: near }).ok).toBe(false);
    // run on: worn units now cycle through maintenance and come back restored.
    let sawMaintenance = false;
    for (let i = 0; i < 2000; i++) {
      simTick(state);
      if ((state.fleet ?? []).some((u) => u.status === 'maintenance')) sawMaintenance = true;
    }
    expect(sawMaintenance).toBe(true);
    expect(fleetSummary(state).avgCondition).toBeGreaterThan(worn);
  });
});
