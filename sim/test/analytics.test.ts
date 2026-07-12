/**
 * Analytics layer: ridership heatmap, OD matrix, insight metrics, payload budget.
 */
import { describe, expect, it } from 'vitest';
import {
  ANALYTICS_PAYLOAD_BUDGET_BYTES,
  ANALYTICS_WINDOW_DAYS,
  CATCHMENT_RADIUS_M,
  HEATMAP_EMIT_INTERVAL_DAYS,
  HEATMAP_HEADER_BYTES,
  HEATMAP_MSG_TYPE,
  HEATMAP_VERSION,
  buildDemandOverlay,
  buildHeatmapPayload,
  buildOdMatrixExact,
  captureAssignmentAnalytics,
  catchmentCoverage,
  commitAnalyticsDay,
  computeInsights,
  createAnalyticsState,
  decodeHeatmapPayload,
  encodeHeatmapPayload,
  findOverloadedCorridor,
  findUnderservedDistrict,
  quantizeHeatmap,
  smoothedHeatmap,
  splatStationActivity,
} from '../src/core/analytics';
import { applyCommand } from '../src/core/commands';
import { TICKS_PER_DAY } from '../src/core/constants';
import { makePolyline } from '../src/core/geometry';
import { newGame } from '../src/core/newGame';
import { setBankruptDays, simTick } from '../src/core/sim';
import type { Command, FieldGrid, FlowResult, GameState, RouteDef, Station } from '../src/core/types';
import type { CarFlow } from '../src/core/transit/assignment';
import { uiExtras } from '../src/host/uiExtras';

/** Tiny 4×4 field grid with known cell geometry (100 m cells, origin at 0). */
function syntheticGrid(): FieldGrid {
  const w = 4;
  const h = 4;
  const n = w * h;
  const population = new Float32Array(n);
  // populate a 2×2 block of cells around (1,1) so catchment math is exact
  population[1 * w + 1] = 100; // cell center (150,150)
  population[1 * w + 2] = 100; // (250,150)
  population[2 * w + 1] = 100; // (150,250)
  population[2 * w + 2] = 50; // (250,250)
  population[0] = 200; // far cell (50,50) — outside 400m of a station at (150,150)
  return {
    w,
    h,
    cellSize: 100,
    originX: 0,
    originY: 0,
    terrain: new Float32Array(n),
    water: new Uint8Array(n),
    parks: new Uint8Array(n),
    population,
    jobs: new Float32Array(n),
    landValue: new Float32Array(n),
    nimby: new Float32Array(n),
  };
}

function station(id: number, x: number, y: number): Station {
  return {
    id,
    name: `S${id}`,
    pos: { x, y },
    mode: 'bus',
    level: 1,
    ridership: 0,
    alightings: 0,
    buildTick: 0,
  };
}

describe('heatmap splat + rolling smooth', () => {
  it('deposits boardings+alightings into the exact station cell', () => {
    const g = syntheticGrid();
    const out = new Float32Array(g.w * g.h);
    const stations = [station(1, 150, 150), station(2, 350, 350)];
    const boardings = new Map<number, number>([
      [1, 100],
      [2, 40],
    ]);
    const alightings = new Map<number, number>([
      [1, 50],
      [2, 10],
    ]);
    splatStationActivity(g, stations, boardings, alightings, out);
    // cell (1,1) = index 5; cell (3,3) = index 15
    expect(out[1 * 4 + 1]).toBe(150);
    expect(out[3 * 4 + 3]).toBe(50);
    expect(out[0]).toBe(0);
    const sum = out.reduce((a, b) => a + b, 0);
    expect(sum).toBe(200);
  });

  it('rolling 7-day mean is exact for constant daily deposits', () => {
    const days: Float32Array[] = [];
    for (let d = 0; d < ANALYTICS_WINDOW_DAYS; d++) {
      const day = new Float32Array(4);
      day[0] = 70;
      day[1] = 14;
      days.push(day);
    }
    const mean = smoothedHeatmap(days);
    expect(mean[0]).toBe(70);
    expect(mean[1]).toBe(14);
  });

  it('rolling window drops the oldest day', () => {
    const days: Float32Array[] = [];
    for (let d = 0; d < 3; d++) {
      const day = new Float32Array(1);
      day[0] = (d + 1) * 10; // 10, 20, 30
      days.push(day);
    }
    expect(smoothedHeatmap(days)[0]).toBe(20);
  });
});

describe('OD matrix from gravity-model trips', () => {
  it('sums transit+car from flows and fills car-only pairs without double-counting', () => {
    const flows: FlowResult[] = [
      {
        originDistrict: 0,
        destDistrict: 1,
        transitTrips: 10,
        carTrips: 5,
        transitCost: 20,
        routeIds: [1],
        stationIds: [1, 2],
      },
    ];
    const carFlows: CarFlow[] = [
      { originDistrict: 0, destDistrict: 1, carTrips: 5 }, // duplicate of flow's car leg
      { originDistrict: 2, destDistrict: 3, carTrips: 7 }, // car-only
    ];
    const od = buildOdMatrixExact(flows, carFlows);
    expect(od.get('0:1')).toBe(15);
    expect(od.get('2:3')).toBe(7);
    expect(od.size).toBe(2);
  });
});

describe('insight metrics', () => {
  it('picks the worst underserved district by demand × (1 − service)', () => {
    const districts = [
      { id: 1, name: 'A', centroid: { x: 0, y: 0 }, cellIndices: [], population: 1000, jobs: 100, landValue: 1 },
      { id: 2, name: 'B', centroid: { x: 1, y: 0 }, cellIndices: [], population: 500, jobs: 100, landValue: 1 },
    ];
    const od = new Map<string, number>([
      ['1:2', 100], // district 1 demand 100, no transit → score 100
      ['2:1', 80], // district 2 demand 80
    ]);
    const flows: FlowResult[] = [
      {
        originDistrict: 2,
        destDistrict: 1,
        transitTrips: 40,
        carTrips: 40,
        transitCost: 15,
        routeIds: [1],
        stationIds: [1, 2],
      },
    ];
    const under = findUnderservedDistrict(districts, od, flows);
    expect(under).not.toBeNull();
    expect(under!.id).toBe(1);
    expect(under!.demand).toBe(100);
    expect(under!.service).toBe(0);
  });

  it('finds the most overloaded corridor from segmentLoads', () => {
    const routes: RouteDef[] = [
      {
        id: 10,
        name: 'Red',
        color: '#f00',
        mode: 'bus',
        stationIds: [1, 2, 3],
        segmentIds: [100, 101],
        headwaySeconds: 600,
        fare: 2,
        vehicleCount: 2,
        dailyRidership: 0,
        dailyRevenue: 0,
        capacity: 100,
        load: 50,
        crowding: 0.5,
        segmentLoads: [200, 500],
      },
      {
        id: 11,
        name: 'Blue',
        color: '#00f',
        mode: 'bus',
        stationIds: [4, 5],
        segmentIds: [102],
        headwaySeconds: 600,
        fare: 2,
        vehicleCount: 1,
        dailyRidership: 0,
        dailyRevenue: 0,
        capacity: 100,
        load: 50,
        crowding: 0.5,
        segmentLoads: [400],
      },
    ];
    const c = findOverloadedCorridor(routes);
    expect(c).toEqual({
      routeId: 10,
      routeName: 'Red',
      fromStationId: 2,
      toStationId: 3,
      load: 500,
    });
  });

  it('catchment coverage uses a fixed 400m radius around served stations', () => {
    const g = syntheticGrid();
    g.w = 8;
    g.h = 8;
    const n = 64;
    g.population = new Float32Array(n);
    g.jobs = new Float32Array(n);
    g.terrain = new Float32Array(n);
    g.water = new Uint8Array(n);
    g.parks = new Uint8Array(n);
    g.landValue = new Float32Array(n);
    g.nimby = new Float32Array(n);
    // station at (150,150); covered cell center (150,150); uncovered (750,750) ≈ 848m away
    g.population[1 * 8 + 1] = 100;
    g.population[7 * 8 + 7] = 100;
    const stations = [station(1, 150, 150)];
    const routes: RouteDef[] = [
      {
        id: 1,
        name: 'Line',
        color: '#0f0',
        mode: 'bus',
        stationIds: [1],
        segmentIds: [],
        headwaySeconds: 600,
        fare: 1,
        vehicleCount: 1,
        dailyRidership: 0,
        dailyRevenue: 0,
        capacity: 60,
        load: 0,
        crowding: 0,
        segmentLoads: [],
      },
    ];
    const cov = catchmentCoverage(g, stations, routes, CATCHMENT_RADIUS_M);
    expect(cov).toBe(0.5);
  });
});

describe('quantized heatmap payload', () => {
  it('encodes the documented byte layout and stays under 50KB', () => {
    const w = 96;
    const h = 96;
    const raw = new Float32Array(w * h);
    raw[0] = 100;
    raw[1] = 50;
    const { cells, maxValue } = quantizeHeatmap(raw);
    expect(maxValue).toBe(100);
    expect(cells[0]).toBe(255);
    expect(cells[1]).toBe(128); // round(50/100*255)=128
    const payload = {
      w,
      h,
      cellSize: 125,
      originX: -6000,
      originY: -6000,
      maxValue,
      day: 7,
      cells,
    };
    const buf = encodeHeatmapPayload(payload);
    expect(buf.byteLength).toBe(HEATMAP_HEADER_BYTES + w * h);
    expect(buf.byteLength).toBeLessThan(ANALYTICS_PAYLOAD_BUDGET_BYTES);
    const dv = new DataView(buf);
    expect(dv.getUint8(0)).toBe(HEATMAP_MSG_TYPE);
    expect(dv.getUint8(1)).toBe(HEATMAP_VERSION);
    expect(dv.getUint32(4, true)).toBe(96);
    expect(dv.getUint32(8, true)).toBe(96);
    expect(dv.getFloat32(12, true)).toBe(125);
    expect(dv.getFloat32(24, true)).toBe(100);
    expect(dv.getUint32(28, true)).toBe(7);
    const roundTrip = decodeHeatmapPayload(buf);
    expect(roundTrip.cells[0]).toBe(255);
    expect(roundTrip.cells[1]).toBe(128);
    expect(roundTrip.maxValue).toBe(100);
  });
});

describe('synthetic city end-to-end day commit', () => {
  it('asserts exact heatmap and OD values after one analytics day', () => {
    const g = syntheticGrid();
    const state = {
      fields: g,
      districts: [
        { id: 0, name: 'North', centroid: { x: 150, y: 150 }, cellIndices: [5], population: 1000, jobs: 200, landValue: 1 },
        { id: 1, name: 'South', centroid: { x: 350, y: 350 }, cellIndices: [15], population: 800, jobs: 400, landValue: 1 },
      ],
      stations: [station(1, 150, 150), station(2, 350, 350)],
      tracks: [
        {
          id: 10,
          mode: 'bus' as const,
          grade: 'surface' as const,
          fromStationId: 1,
          toStationId: 2,
          polyline: makePolyline([
            { x: 150, y: 150 },
            { x: 350, y: 350 },
          ]),
          buildCost: 0,
        },
      ],
      routes: [
        {
          id: 1,
          name: 'Spine',
          color: '#e6a817',
          mode: 'bus' as const,
          stationIds: [1, 2],
          segmentIds: [10],
          headwaySeconds: 600,
          fare: 2,
          vehicleCount: 2,
          dailyRidership: 120,
          dailyRevenue: 240,
          capacity: 100,
          load: 50,
          crowding: 0.5,
          segmentLoads: [120],
        },
      ],
      flows: [
        {
          originDistrict: 0,
          destDistrict: 1,
          transitTrips: 80,
          carTrips: 20,
          transitCost: 18,
          routeIds: [1],
          stationIds: [1, 2],
        },
      ] as FlowResult[],
      vehicles: [],
      roads: [],
      budget: {
        cash: 1e6,
        loanBalance: 0,
        loanRate: 0,
        lastDay: { fares: 0, subsidy: 0, operations: 0, maintenance: 0, interest: 0 },
        netHistory: [],
      },
      stats: {
        population: 1800,
        jobs: 600,
        dailyTransitTrips: 80,
        dailyCarTrips: 20,
        transitShare: 0.8,
        coverage: 0.5,
        approval: 60,
      },
      seed: 1,
      tick: TICKS_PER_DAY,
      rngState: { s0: 1, s1: 2, s2: 3, s3: 4 },
      difficulty: 'normal' as const,
      nextId: 100,
      demandDirty: false,
      unlockedModes: ['bus' as const],
      activeEvents: [],
      nextEventDay: 99,
      commandLog: [],
      lowApprovalDays: 0,
      failed: null,
      analytics: createAnalyticsState(),
    } as unknown as GameState;

    captureAssignmentAnalytics(
      state,
      new Map([
        [1, 80],
        [2, 0],
      ]),
      new Map([
        [1, 0],
        [2, 80],
      ]),
      state.flows,
      [{ originDistrict: 1, destDistrict: 0, carTrips: 40 }],
    );

    const result = commitAnalyticsDay(state, HEATMAP_EMIT_INTERVAL_DAYS);
    expect(result.emitHeatmap).toBe(true);
    expect(result.payload).not.toBeNull();

    const heat = state.analytics!.dayHeat[0]!;
    expect(heat[1 * 4 + 1]).toBe(80); // boardings at S1
    expect(heat[3 * 4 + 3]).toBe(80); // alightings at S2

    const od = state.analytics!.dayOd[0]!;
    expect(od.get('0:1')).toBe(100); // 80+20
    expect(od.get('1:0')).toBe(40); // car-only

    expect(state.analytics!.insights.overloadedCorridor).toEqual({
      routeId: 1,
      routeName: 'Spine',
      fromStationId: 1,
      toStationId: 2,
      load: 120,
    });
    expect(state.analytics!.insights.underservedDistrictId).toBe(1); // South: 40 demand, 0 transit
    expect(state.analytics!.insights.networkEfficiency).toBeGreaterThan(0);
    expect(state.analytics!.insights.catchmentCoverage).toBeGreaterThan(0);

    const encoded = encodeHeatmapPayload(result.payload!);
    expect(encoded.byteLength).toBe(HEATMAP_HEADER_BYTES + 16);
    expect(encoded.byteLength).toBeLessThan(ANALYTICS_PAYLOAD_BUDGET_BYTES);

    const extras = uiExtras(state);
    expect(extras.analytics?.underservedDistrictId).toBe(1);
    expect(extras.analytics?.overloadedCorridor?.load).toBe(120);
  });
});

describe('determinism preserved with analytics', () => {
  function play(seed: number): GameState {
    setBankruptDays(0);
    const state = newGame(seed, 'normal');
    const byDemand = [...state.districts].sort((a, b) => b.population + b.jobs - (a.population + a.jobs));
    const picks: { x: number; y: number }[] = [];
    for (const d of byDemand) {
      if (picks.every((p) => Math.hypot(p.x - d.centroid.x, p.y - d.centroid.y) > 800)) picks.push(d.centroid);
      if (picks.length === 3) break;
    }
    const ids: number[] = [];
    for (const pos of picks) ids.push(applyCommand(state, { kind: 'buildStation', mode: 'bus', pos }).createdId!);
    const cmds: Command[] = [
      { kind: 'buildTrack', mode: 'bus', grade: 'surface', fromStationId: ids[0]!, toStationId: ids[1]!, waypoints: [] },
      { kind: 'buildTrack', mode: 'bus', grade: 'surface', fromStationId: ids[1]!, toStationId: ids[2]!, waypoints: [] },
      { kind: 'createRoute', mode: 'bus', stationIds: ids },
    ];
    for (const c of cmds) expect(applyCommand(state, c).ok).toBe(true);
    for (let t = 0; t < TICKS_PER_DAY * 8; t++) simTick(state);
    return state;
  }

  it('two identical runs produce identical analytics insights and heatmap bytes', () => {
    const a = play(424242);
    const b = play(424242);
    expect(a.analytics).toBeDefined();
    expect(b.analytics).toBeDefined();
    expect(a.analytics!.insights).toEqual(b.analytics!.insights);
    expect(a.analytics!.daysRecorded).toBe(b.analytics!.daysRecorded);
    const pa = buildHeatmapPayload(a, a.analytics!.lastHeatmapDay || 7);
    const pb = buildHeatmapPayload(b, b.analytics!.lastHeatmapDay || 7);
    expect(Array.from(pa.cells)).toEqual(Array.from(pb.cells));
    expect(pa.maxValue).toBe(pb.maxValue);
    expect(encodeHeatmapPayload(pa).byteLength).toBeLessThan(ANALYTICS_PAYLOAD_BUDGET_BYTES);
  });

  it('insights recompute stably from the same OD + routes', () => {
    const state = play(111);
    const od = state.analytics!.dayOd.length
      ? new Map(state.analytics!.dayOd[state.analytics!.dayOd.length - 1])
      : new Map<string, number>();
    const i1 = computeInsights(state, od);
    const i2 = computeInsights(state, od);
    expect(i1).toEqual(i2);
  });
});

describe('demand/gaps overlay is station-independent (issue #20)', () => {
  /** Minimal GameState carrying just what buildDemandOverlay reads. */
  function demandState(
    districts: GameState['districts'],
    flows: FlowResult[],
  ): GameState {
    return {
      districts,
      flows,
      stations: [],
      routes: [],
      activeEvents: [],
    } as unknown as GameState;
  }

  it('surfaces demand for far-apart pairs with NO stations and NO flows', () => {
    // Four districts spread across the map; the assignment router would never
    // enumerate served paths for any pair (there are no stations at all).
    const districts = [
      { id: 0, name: 'NW', centroid: { x: 0, y: 0 }, cellIndices: [], population: 5000, jobs: 200, landValue: 1 },
      { id: 1, name: 'NE', centroid: { x: 6000, y: 0 }, cellIndices: [], population: 4000, jobs: 3000, landValue: 1 },
      { id: 2, name: 'SW', centroid: { x: 0, y: 6000 }, cellIndices: [], population: 4500, jobs: 2500, landValue: 1 },
      { id: 3, name: 'SE', centroid: { x: 6000, y: 6000 }, cellIndices: [], population: 3800, jobs: 4000, landValue: 1 },
    ] as unknown as GameState['districts'];

    const overlay = buildDemandOverlay(demandState(districts, []));

    // Station-biased state.unserved would be empty here (no router paths); the
    // baseline field must still show unmet demand.
    expect(overlay.length).toBeGreaterThan(0);
    // With zero served trips everywhere, every reported gap has share 0.
    for (const l of overlay) expect(l.share).toBe(0);

    // A long cross-map pair (NW -> SE, distance ~8485 m) must appear, i.e. the
    // overlay is NOT limited to short trips clustered near infrastructure.
    const nw = districts[0]!;
    const se = districts[3]!;
    const hasCrossMap = overlay.some(
      (l) => l.x1 === nw.centroid.x && l.y1 === nw.centroid.y && l.x2 === se.centroid.x && l.y2 === se.centroid.y,
    );
    expect(hasCrossMap).toBe(true);
  });

  it('gap weight = baselineDemand × (1 − servedShare) from assignment flows', () => {
    const districts = [
      { id: 0, name: 'A', centroid: { x: 0, y: 0 }, cellIndices: [], population: 6000, jobs: 100, landValue: 1 },
      { id: 1, name: 'B', centroid: { x: 1000, y: 0 }, cellIndices: [], population: 100, jobs: 5000, landValue: 1 },
    ] as unknown as GameState['districts'];

    const baseUnserved = buildDemandOverlay(demandState(districts, []));
    const ab = baseUnserved.find((l) => l.x1 === 0 && l.x2 === 1000);
    expect(ab).toBeDefined();
    const baselineDemand = ab!.weight; // share 0 → weight == baseline trips
    expect(baselineDemand).toBeGreaterThan(0);

    // Now serve ~30% of that demand via transit; the gap must shrink to
    // baseline × (1 − 0.3) and stay below the UNSERVED_SHARE_MAX cutoff.
    const transit = baselineDemand * 0.3;
    const served = buildDemandOverlay(
      demandState(districts, [
        { originDistrict: 0, destDistrict: 1, transitTrips: transit, carTrips: 0, transitCost: 10, routeIds: [], stationIds: [] },
      ]),
    );
    const abServed = served.find((l) => l.x1 === 0 && l.x2 === 1000);
    expect(abServed).toBeDefined();
    expect(abServed!.share).toBeCloseTo(0.3, 5);
    expect(abServed!.weight).toBeCloseTo(baselineDemand * 0.7, 4);
  });
});
