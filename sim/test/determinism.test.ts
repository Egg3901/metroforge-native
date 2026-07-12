/**
 * Golden determinism tests — the acceptance suite for a future native port.
 * Same seed + same command stream ⇒ identical state hashes.
 */
import { describe, expect, it } from 'vitest';
import { applyCommand } from '../src/core/commands';
import { newGame } from '../src/core/newGame';
import { deserialize, serialize, stateHash } from '../src/core/save';
import { setBankruptDays, simTick } from '../src/core/sim';
import type { Command, GameState } from '../src/core/types';

/** The 3 densest districts spaced ≥800m apart — where a sane player builds first. */
function densePicks(state: GameState): [{ x: number; y: number }, { x: number; y: number }, { x: number; y: number }] {
  const byDemand = [...state.districts].sort((a, b) => b.population + b.jobs - (a.population + a.jobs));
  const picks: { x: number; y: number }[] = [];
  for (const d of byDemand) {
    if (picks.every((p) => Math.hypot(p.x - d.centroid.x, p.y - d.centroid.y) > 800)) picks.push(d.centroid);
    if (picks.length === 3) break;
  }
  return picks as [{ x: number; y: number }, { x: number; y: number }, { x: number; y: number }];
}

function playScript(seed: number): GameState {
  setBankruptDays(0);
  const state = newGame(seed, 'normal');
  const [a, b, c] = densePicks(state);
  const cmds: Command[] = [
    { kind: 'buildStation', mode: 'bus', pos: a },
    { kind: 'buildStation', mode: 'bus', pos: b },
    { kind: 'buildStation', mode: 'bus', pos: c },
  ];
  const ids: number[] = [];
  for (const cmd of cmds) {
    const r = applyCommand(state, cmd);
    expect(r.ok).toBe(true);
    if (r.createdId !== undefined) ids.push(r.createdId);
  }
  const [s1, s2, s3] = ids as [number, number, number];
  expect(applyCommand(state, { kind: 'buildTrack', mode: 'bus', grade: 'surface', fromStationId: s1, toStationId: s2, waypoints: [] }).ok).toBe(true);
  expect(applyCommand(state, { kind: 'buildTrack', mode: 'bus', grade: 'surface', fromStationId: s2, toStationId: s3, waypoints: [] }).ok).toBe(true);
  const route = applyCommand(state, { kind: 'createRoute', mode: 'bus', stationIds: [s1, s2, s3] });
  expect(route.ok).toBe(true);
  for (let t = 0; t < 3000; t++) simTick(state);
  return state;
}

describe('determinism', () => {
  it('same seed + same commands ⇒ identical state hash', () => {
    const h1 = stateHash(playScript(1234567));
    const h2 = stateHash(playScript(1234567));
    expect(h1).toBe(h2);
  });

  it('different seeds ⇒ different cities', () => {
    const s1 = newGame(42, 'normal');
    const s2 = newGame(43, 'normal');
    expect(stateHash(s1)).not.toBe(stateHash(s2));
    expect(s1.districts.length).toBeGreaterThan(20);
    expect(s2.districts.length).toBeGreaterThan(20);
  });

  it('city generation produces a viable city', () => {
    const s = newGame(999, 'normal');
    expect(s.stats.population).toBeGreaterThan(100_000);
    expect(s.stats.jobs).toBeGreaterThan(30_000);
    expect(s.roads.length).toBeGreaterThan(10);
  });

  it('save round-trip preserves state hash and continues deterministically', () => {
    const state = playScript(777);
    const hashBefore = stateHash(state);
    const restored = deserialize(serialize(state));
    expect(stateHash(restored)).toBe(hashBefore);
    // both continue identically
    for (let t = 0; t < 500; t++) {
      simTick(state);
      simTick(restored);
    }
    expect(stateHash(restored)).toBe(stateHash(state));
  });

  it('a working bus line attracts riders and earns fares', () => {
    const state = playScript(31337);
    const route = state.routes[0];
    expect(route).toBeDefined();
    expect(route!.dailyRidership).toBeGreaterThan(0);
    expect(route!.dailyRevenue).toBeGreaterThan(0);
    expect(state.stats.transitShare).toBeGreaterThan(0);
  });
});
