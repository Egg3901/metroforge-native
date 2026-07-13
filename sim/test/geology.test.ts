/**
 * Geology (v0.8 Underground) tests: per-city profile sanity, strata
 * determinism, the tunnel cost curve (monotonic vs depth, rock-vs-soil boring
 * crossover, water-table surcharge), a fixed strata-probe snapshot, station
 * depth + access penalty, and a 10k-tick same-seed stateHash proof.
 */
import { describe, expect, it } from 'vitest';
import {
  BEDROCK_NOMINAL_BOTTOM,
  columnAt,
  geologyProfile,
  strataColumn,
  stratumAtDepth,
  waterTableDepthAt,
  type StrataColumn,
} from '../src/core/geology';
import {
  boredMult,
  stationDepthAccessPenaltySec,
  stationDepthSurcharge,
  undergroundSegmentCost,
  WATERPROOF_SURCHARGE,
} from '../src/core/geologyCost';
import { applyCommand, trackCostDetailed } from '../src/core/commands';
import { newGame } from '../src/core/newGame';
import { stateHash } from '../src/core/save';
import { setBankruptDays, simTick } from '../src/core/sim';
import { WORLD_SIZE } from '../src/core/constants';
import type { GameState } from '../src/core/types';

const CITY_KEYS = [
  'generic', 'nyc', 'la', 'seattle', 'chicago', 'boston', 'atlanta', 'sf', 'dc', 'philly', 'cleveland',
];

describe('geology profiles', () => {
  it('every city has a profile with positive, top-down-ordered layers', () => {
    for (const key of CITY_KEYS) {
      const p = geologyProfile(key);
      expect(p.soil).toBeGreaterThan(0);
      expect(p.clay).toBeGreaterThan(0);
      expect(p.rockThickness).toBeGreaterThan(0);
      expect(p.rockHardness).toBeGreaterThan(0);
      expect(p.rockHardness).toBeLessThanOrEqual(1);
      expect(p.baseWaterTable).toBeGreaterThan(0);
    }
  });

  it('unknown city falls back to the generic profile', () => {
    expect(geologyProfile('atlantis')).toEqual(geologyProfile('generic'));
    expect(geologyProfile(undefined)).toEqual(geologyProfile('generic'));
  });

  it('reads like the real ground: Manhattan rock is shallow + hard, LA is deep + soft', () => {
    const nyc = geologyProfile('nyc');
    const la = geologyProfile('la');
    const rockTopNyc = nyc.soil + nyc.clay; // ~12 m
    const rockTopLa = la.soil + la.clay; // ~48 m
    expect(rockTopNyc).toBeLessThan(20); // schist within 10-20 m
    expect(rockTopLa).toBeGreaterThan(40); // deep alluvium
    expect(nyc.rockHardness).toBeGreaterThan(la.rockHardness);
  });
});

describe('strata columns are deterministic', () => {
  it('same (seed, city, point) reconstructs an identical column', () => {
    for (const key of ['nyc', 'chicago', 'sf']) {
      const a = columnAt(key, 777, WORLD_SIZE, undefined, undefined, { x: 1234, y: -567 });
      const b = columnAt(key, 777, WORLD_SIZE, undefined, undefined, { x: 1234, y: -567 });
      expect(a).toEqual(b);
    }
  });

  it('bands are contiguous, ordered fill→clay→rock→bedrock', () => {
    const col = columnAt('nyc', 42, WORLD_SIZE, undefined, undefined, { x: 0, y: 0 });
    expect(col.bands.map((b) => b.kind)).toEqual(['fill', 'clay', 'rock', 'bedrock']);
    let prev = 0;
    for (const b of col.bands) {
      expect(b.top).toBeCloseTo(prev, 6);
      expect(b.bottom).toBeGreaterThan(b.top);
      prev = b.bottom;
    }
    expect(col.bands[3]!.bottom).toBe(BEDROCK_NOMINAL_BOTTOM);
  });

  it('different seeds give different columns', () => {
    const a = columnAt('nyc', 1, WORLD_SIZE, undefined, undefined, { x: 500, y: 500 });
    const b = columnAt('nyc', 2, WORLD_SIZE, undefined, undefined, { x: 500, y: 500 });
    expect(a.bands[0]!.bottom).not.toBeCloseTo(b.bands[0]!.bottom, 6);
  });
});

describe('water table', () => {
  it('is shallower in low ground, deeper on high ground', () => {
    const p = geologyProfile('nyc');
    const low = waterTableDepthAt(p, 0, 99, 100, 100);
    const high = waterTableDepthAt(p, 60, 99, 100, 100);
    expect(high).toBeGreaterThan(low);
  });
});

// A synthetic column with a controllable water table for the surcharge test.
function fakeColumn(waterTableDepth: number, rockHardness = 0.5): StrataColumn {
  return {
    bands: [
      { kind: 'fill', top: 0, bottom: 6 },
      { kind: 'clay', top: 6, bottom: 40 },
      { kind: 'rock', top: 40, bottom: 200 },
      { kind: 'bedrock', top: 200, bottom: BEDROCK_NOMINAL_BOTTOM },
    ],
    waterTableDepth,
    rockHardness,
    surfaceElevation: 0,
  };
}

describe('tunnel cost curve', () => {
  const surfacePerM = 100; // reference units

  it('cost per metre rises monotonically with depth in constant soil', () => {
    const col = fakeColumn(50); // water table below the tested depths → no step
    let last = -Infinity;
    for (let d = 2; d <= 14; d += 2) {
      const r = undergroundSegmentCost(surfacePerM, 1, col, d);
      expect(['fill', 'clay']).toContain(r.stratum); // soft ground, no rock step
      expect(r.costPerM).toBeGreaterThan(last);
      last = r.costPerM;
    }
  });

  it('boring is cheaper in solid rock than in wet mixed soil (TBM economics)', () => {
    const col = fakeColumn(4); // shallow table → soil bores are below it (wet)
    const rock = boredMult(col, 60, false); // in competent rock
    const wetSoil = boredMult(col, 10, true); // in clay, below water table
    expect(rock).toBeLessThan(wetSoil);
  });

  it('applies a waterproofing surcharge below the water table', () => {
    const dry = fakeColumn(30); // table at 30 m
    const wet = fakeColumn(2); // table at 2 m
    // price the same 12 m bored/cut segment: wet is below table, dry is not
    const cDry = undergroundSegmentCost(surfacePerM, 1, dry, 12);
    const cWet = undergroundSegmentCost(surfacePerM, 1, wet, 12);
    expect(cDry.belowWaterTable).toBe(false);
    expect(cWet.belowWaterTable).toBe(true);
    expect(cWet.floodRisk).toBeGreaterThan(0);
    expect(cDry.floodRisk).toBe(0);
    // surcharge is at least the waterproofing factor (same method chosen: soil→cutCover)
    expect(cWet.costPerM / cDry.costPerM).toBeGreaterThanOrEqual(1 + WATERPROOF_SURCHARGE - 1e-9);
  });

  it('cut-and-cover is chosen shallow-in-soil; bored is chosen in shallow rock', () => {
    const soft = fakeColumn(50); // deep table, rock deep
    expect(undergroundSegmentCost(surfacePerM, 1, soft, 8).method).toBe('cutCover');
    const rocky = { ...fakeColumn(50), bands: [
      { kind: 'fill', top: 0, bottom: 3 },
      { kind: 'clay', top: 3, bottom: 6 },
      { kind: 'rock', top: 6, bottom: 200 },
      { kind: 'bedrock', top: 200, bottom: BEDROCK_NOMINAL_BOTTOM },
    ] } as StrataColumn;
    // at 12 m we are in rock: cut-and-cover through rock is penalised → bored wins
    expect(undergroundSegmentCost(surfacePerM, 1, rocky, 12).method).toBe('bored');
  });
});

describe('station depth', () => {
  it('deeper stations cost more to sink', () => {
    expect(stationDepthSurcharge(1_000_000, 24)).toBeGreaterThan(stationDepthSurcharge(1_000_000, 12));
    expect(stationDepthSurcharge(1_000_000, 0)).toBe(0);
  });

  it('access penalty is zero to 10 m, then +30 s per 10 m', () => {
    expect(stationDepthAccessPenaltySec(undefined)).toBe(0);
    expect(stationDepthAccessPenaltySec(10)).toBe(0);
    expect(stationDepthAccessPenaltySec(20)).toBeCloseTo(30, 6);
    expect(stationDepthAccessPenaltySec(30)).toBeCloseTo(60, 6);
  });
});

// Build a two-station metro tunnel; return the state for reuse.
function metroTunnel(seed: number): { state: GameState; s1: number; s2: number; cost: number } {
  setBankruptDays(0);
  const state = newGame(seed, 'easy');
  if (!state.unlockedModes.includes('metro')) state.unlockedModes.push('metro');
  state.budget.cash = 2_000_000_000; // fund the (deliberately pricey) tunnel
  const picks = [...state.districts].sort((a, b) => b.population + b.jobs - (a.population + a.jobs));
  const a = picks[0]!.centroid;
  const b = picks.find((d) => Math.hypot(d.centroid.x - a.x, d.centroid.y - a.y) > 900)!.centroid;
  const r1 = applyCommand(state, { kind: 'buildStation', mode: 'metro', pos: a });
  const r2 = applyCommand(state, { kind: 'buildStation', mode: 'metro', pos: b });
  expect(r1.ok && r2.ok).toBe(true);
  const s1 = r1.createdId!;
  const s2 = r2.createdId!;
  const cashBefore = state.budget.cash;
  const rt = applyCommand(state, { kind: 'buildTrack', mode: 'metro', grade: 'tunnel', fromStationId: s1, toStationId: s2, waypoints: [] });
  expect(rt.ok).toBe(true);
  const cost = cashBefore - state.budget.cash;
  applyCommand(state, { kind: 'createRoute', mode: 'metro', stationIds: [s1, s2] });
  const route = state.routes[state.routes.length - 1]!;
  applyCommand(state, { kind: 'editRoute', routeId: route.id, vehicleCount: 4, headwaySeconds: 240 });
  return { state, s1, s2, cost };
}

describe('underground build integration', () => {
  it('a tunnel is meaningfully pricier than the surface alignment (3-10x)', () => {
    const { state, s1, s2 } = metroTunnel(31337);
    const from = state.stations.find((s) => s.id === s1)!;
    const to = state.stations.find((s) => s.id === s2)!;
    const pts = [from.pos, to.pos];
    const tun = trackCostDetailed(state, 'metro', 'tunnel', pts);
    const surf = trackCostDetailed(state, 'metro', 'surface', pts);
    const ratio = tun.cost / surf.cost;
    expect(ratio).toBeGreaterThan(3);
    expect(ratio).toBeLessThan(11);
    expect(tun.breakdown.surface).toBeGreaterThan(0);
    expect(tun.breakdown.strata.length).toBeGreaterThan(0);
  });

  it('tunnel connection sinks the end stations underground', () => {
    const { state, s1 } = metroTunnel(31337);
    const st = state.stations.find((s) => s.id === s1)!;
    expect(st.depth).toBeGreaterThan(0);
  });
});

describe('determinism: 10k-tick same-seed stateHash proof', () => {
  it('two identical underground runs land on the same hash', () => {
    // 10k-tick double run: needs headroom beyond vitest's 5s default on a loaded box.

    const runA = metroTunnel(20260808).state;
    const runB = metroTunnel(20260808).state;
    for (let i = 0; i < 10_000; i++) {
      simTick(runA);
      simTick(runB);
    }
    expect(stateHash(runA)).toBe(stateHash(runB));
  }, 30_000);
});

describe('strata probe snapshot (seed 12345, NYC, origin)', () => {
  it('matches the documented golden column', () => {
    const col = strataColumn(geologyProfile('nyc'), 12345, WORLD_SIZE, undefined, undefined, { x: 0, y: 0 });
    const rounded = {
      bands: col.bands.map((b) => ({ kind: b.kind, top: Math.round(b.top * 100) / 100, bottom: Math.round(b.bottom * 100) / 100 })),
      waterTable: Math.round(col.waterTableDepth * 100) / 100,
      rockHardness: col.rockHardness,
      surfaceElevation: col.surfaceElevation,
    };
    expect(rounded).toMatchSnapshot();
  });
});
