/**
 * Vehicle motion — segment-aware advance + per-segment occupancy.
 */
import { describe, expect, it } from 'vitest';
import { applyCommand } from '../src/core/commands';
import { newGame } from '../src/core/newGame';
import { setBankruptDays, simTick } from '../src/core/sim';
import type { GameState } from '../src/core/types';

function buildShortLine(seed: number): GameState {
  setBankruptDays(0);
  const state = newGame(seed, 'normal');
  const byDemand = [...state.districts].sort((a, b) => b.population + b.jobs - (a.population + a.jobs));
  const picks: { x: number; y: number }[] = [];
  for (const d of byDemand) {
    if (picks.every((p) => Math.hypot(p.x - d.centroid.x, p.y - d.centroid.y) > 600)) picks.push(d.centroid);
    if (picks.length === 2) break;
  }
  const [a, b] = picks as [{ x: number; y: number }, { x: number; y: number }];
  const ids: number[] = [];
  for (const pos of [a, b]) {
    const r = applyCommand(state, { kind: 'buildStation', mode: 'bus', pos });
    expect(r.ok).toBe(true);
    ids.push(r.createdId!);
  }
  expect(
    applyCommand(state, {
      kind: 'buildTrack',
      mode: 'bus',
      grade: 'surface',
      fromStationId: ids[0]!,
      toStationId: ids[1]!,
      waypoints: [],
    }).ok,
  ).toBe(true);
  const route = applyCommand(state, { kind: 'createRoute', mode: 'bus', stationIds: ids });
  expect(route.ok).toBe(true);
  return state;
}

describe('vehicle motion', () => {
  it('spawns vehicles evenly spaced along the loop', () => {
    const state = buildShortLine(424242);
    const route = state.routes[0]!;
    expect(route.vehicleCount).toBeGreaterThanOrEqual(2);
    const alongs = state.vehicles.filter((v) => v.routeId === route.id).map((v) => v.along);
    expect(alongs.length).toBe(route.vehicleCount);
    // not all piled at the first station
    const unique = new Set(alongs.map((a) => Math.round(a)));
    expect(unique.size).toBeGreaterThan(1);
  });

  it('dwells at stations without permanently overshooting', () => {
    const state = buildShortLine(515151);
    // force assignment so segment loads / crowding exist
    for (let t = 0; t < 400; t++) simTick(state);
    const before = state.vehicles.map((v) => ({ id: v.id, along: v.along, dwell: v.dwellRemaining }));
    for (let t = 0; t < 200; t++) simTick(state);
    // vehicles still on the path (finite along)
    for (const v of state.vehicles) {
      expect(Number.isFinite(v.along)).toBe(true);
      expect(v.along).toBeGreaterThanOrEqual(0);
      expect(v.pathLength).toBeGreaterThan(0);
      expect(v.along).toBeLessThan(v.pathLength + 1);
    }
    // something moved or dwelled
    const moved = state.vehicles.some((v) => {
      const prev = before.find((b) => b.id === v.id);
      return prev && (Math.abs(prev.along - v.along) > 1 || v.dwellRemaining > 0 || prev.dwell > 0);
    });
    expect(moved).toBe(true);
  });

  it('records rolling net history on daily economy', () => {
    const state = buildShortLine(606060);
    for (let t = 0; t < 1200 * 3 + 5; t++) simTick(state);
    expect(state.budget.netHistory.length).toBeGreaterThanOrEqual(2);
    expect(state.budget.netHistory.length).toBeLessThanOrEqual(7);
  });
});
