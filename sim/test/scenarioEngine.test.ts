/**
 * Scenario content system — catalog from JSON, progression, condition trees,
 * mid-run events, and per-scenario win/lose playthrough proofs.
 */
import { describe, expect, it } from 'vitest';
import { applyCommand } from '../src/core/commands';
import { TICKS_PER_DAY } from '../src/core/constants';
import { newGame } from '../src/core/newGame';
import {
  PLAYABLE_SCENARIOS,
  SCENARIO_PROGRESSION,
  availableScenarios,
  buildScenarioState,
  evalCondition,
  playableScenario,
  readMetrics,
  requiresFor,
  rulesFromScenario,
  treeProgress,
  unlocksFrom,
  type ScenarioDef,
} from '../src/core/scenario';
import { setBankruptDays, simTick } from '../src/core/sim';
import { stateHash } from '../src/core/save';
import { uiExtras } from '../src/host/uiExtras';
import type { Command, GameState, TransitMode } from '../src/core/types';
import type { OsmCityData } from '../src/core/city/osmCity';
import cleveland from '../src/data/cities/cleveland.json';
import nyc from '../src/data/cities/nyc.json';

const CITY: Record<'cleveland' | 'nyc', OsmCityData> = {
  cleveland: cleveland as OsmCityData,
  nyc: nyc as OsmCityData,
};

/** Scripted win plans — tuned so each scenario wins with measurable surplus. */
const winPlan: Record<
  string,
  { stations: number; vehicles: number; maxDays: number; spacing: number; mode?: TransitMode }
> = {
  'cleveland-first-riders': { stations: 3, vehicles: 4, maxDays: 20, spacing: 600 },
  'cleveland-five-hundred': { stations: 3, vehicles: 4, maxDays: 20, spacing: 600 },
  'cleveland-farebox-30': { stations: 4, vehicles: 5, maxDays: 25, spacing: 600 },
  'cleveland-farebox-80': { stations: 4, vehicles: 4, maxDays: 25, spacing: 600 },
  'cleveland-reach': { stations: 6, vehicles: 6, maxDays: 35, spacing: 550 },
  'cleveland-roadworks': { stations: 5, vehicles: 12, maxDays: 30, spacing: 500 },
  'cleveland-tram-line': { stations: 6, vehicles: 7, maxDays: 40, spacing: 550 },
  'cleveland-austerity': { stations: 5, vehicles: 4, maxDays: 30, spacing: 550 },
  'nyc-first-thousand': { stations: 6, vehicles: 6, maxDays: 25, spacing: 450 },
  'nyc-farebox-80': { stations: 8, vehicles: 8, maxDays: 30, spacing: 400 },
  'nyc-bus-spine': { stations: 10, vehicles: 10, maxDays: 40, spacing: 400 },
  'nyc-dig-season': { stations: 8, vehicles: 18, maxDays: 35, spacing: 400 },
  'nyc-pressure': { stations: 10, vehicles: 10, maxDays: 45, spacing: 400 },
  'nyc-express': { stations: 11, vehicles: 12, maxDays: 40, spacing: 360 },
  'nyc-last-stand': { stations: 10, vehicles: 12, maxDays: 50, spacing: 380 },
};

/** Exported for the balance table helper / PR. */
export interface BalanceRow {
  id: string;
  city: string;
  tier: number;
  winDay: number;
  deadline: number;
  daysLeft: number;
  trips: number;
  farebox: number;
  coverage: number;
  overcrowded: number;
}

function startScenario(def: ScenarioDef, seed = 42): GameState {
  setBankruptDays(0);
  return newGame(seed, def.difficulty, {
    presetKey: def.cityKey,
    osm: CITY[def.cityKey],
    scenario: def,
  });
}

/** Build a bus/tram spine across the N densest districts. */
function buildSpine(
  state: GameState,
  nStations: number,
  mode: TransitMode = 'bus',
  vehicles = 6,
  minSpacing = 600,
): void {
  const byDemand = [...state.districts].sort((a, b) => b.population + b.jobs - (a.population + a.jobs));
  const picks: { x: number; y: number }[] = [];
  for (const d of byDemand) {
    if (picks.every((p) => Math.hypot(p.x - d.centroid.x, p.y - d.centroid.y) > minSpacing)) {
      picks.push(d.centroid);
    }
    if (picks.length === nStations) break;
  }
  expect(picks.length).toBe(nStations);
  const ids: number[] = [];
  for (const pos of picks) {
    const r = applyCommand(state, { kind: 'buildStation', mode, pos });
    expect(r.ok).toBe(true);
    ids.push(r.createdId!);
  }
  for (let i = 0; i < ids.length - 1; i++) {
    const t = applyCommand(state, {
      kind: 'buildTrack',
      mode,
      grade: 'surface',
      fromStationId: ids[i]!,
      toStationId: ids[i + 1]!,
      waypoints: [],
    });
    expect(t.ok).toBe(true);
  }
  const route = applyCommand(state, { kind: 'createRoute', mode, stationIds: ids });
  expect(route.ok).toBe(true);
  applyCommand(state, {
    kind: 'editRoute',
    routeId: route.createdId!,
    vehicleCount: vehicles,
    headwaySeconds: 240,
  });
}

function advanceDays(state: GameState, days: number): void {
  for (let t = 0; t < TICKS_PER_DAY * days; t++) {
    simTick(state);
    if (state.scenarioWon || state.failed) return;
  }
}

describe('scenario catalog (JSON content)', () => {
  it('ships fifteen Cleveland/NYC scenarios with unique ids', () => {
    expect(PLAYABLE_SCENARIOS).toHaveLength(15);
    const ids = PLAYABLE_SCENARIOS.map((s) => s.id);
    expect(new Set(ids).size).toBe(15);
    for (const s of PLAYABLE_SCENARIOS) {
      expect(['cleveland', 'nyc']).toContain(s.cityKey);
      expect(s.deadlineDays).toBeGreaterThan(0);
      expect(s.startingModes.length).toBeGreaterThan(0);
      expect(playableScenario(s.id)?.id).toBe(s.id);
      // dash-free copy
      expect(s.label).not.toMatch(/[\u2012\u2013\u2014\u2015]/);
      expect(s.description).not.toMatch(/[\u2012\u2013\u2014\u2015]/);
    }
    expect(PLAYABLE_SCENARIOS.filter((s) => s.cityKey === 'cleveland').length).toBeGreaterThanOrEqual(7);
    expect(PLAYABLE_SCENARIOS.filter((s) => s.cityKey === 'nyc').length).toBeGreaterThanOrEqual(7);
  });

  it('maps onto ScenarioRules for newGame', () => {
    const def = playableScenario('cleveland-farebox-30')!;
    const rules = rulesFromScenario(def);
    expect(rules.scenarioId).toBe(def.id);
    expect(rules.startingCash).toBe(def.startingBudget);
    expect(rules.maxDay).toBe(def.deadlineDays);
    expect(rules.startingModes).toEqual(def.startingModes);
  });
});

describe('scenario progression manifest', () => {
  it('starters unlock the rest; every non-starter is reachable', () => {
    expect(SCENARIO_PROGRESSION.starters).toEqual(
      expect.arrayContaining(['cleveland-first-riders', 'nyc-first-thousand']),
    );
    const catalogIds = PLAYABLE_SCENARIOS.map((s) => s.id);
    const open0 = availableScenarios([], catalogIds);
    expect(open0.sort()).toEqual([...SCENARIO_PROGRESSION.starters].sort());

    // clearing first riders opens its unlocks
    const after = availableScenarios(['cleveland-first-riders'], catalogIds);
    for (const id of unlocksFrom('cleveland-first-riders')) {
      expect(after).toContain(id);
    }

    // every catalog id is either a starter or an unlock target
    const targets = new Set<string>();
    for (const list of Object.values(SCENARIO_PROGRESSION.unlocks)) {
      for (const id of list) targets.add(id);
    }
    for (const id of catalogIds) {
      if (SCENARIO_PROGRESSION.starters.includes(id)) continue;
      expect(targets.has(id)).toBe(true);
      expect(requiresFor(id).length).toBeGreaterThan(0);
    }
  });

  it('exposes unlocks/requires on additive scenarioState + full graph on uiExtras', () => {
    const def = playableScenario('cleveland-first-riders')!;
    const state = startScenario(def);
    const snap = buildScenarioState(def, state);
    expect(snap.unlocks).toEqual(unlocksFrom(def.id));
    expect(snap.requires ?? []).toEqual([]);
    const extras = uiExtras(state);
    expect(extras.scenarioProgression).toEqual(SCENARIO_PROGRESSION);
    expect(extras.scenarioState?.unlocks).toEqual(unlocksFrom(def.id));

    const locked = playableScenario('nyc-last-stand')!;
    const lockedSnap = buildScenarioState(locked, startScenario(locked));
    expect(lockedSnap.requires!.length).toBeGreaterThan(0);
    expect(lockedSnap.unlocks ?? []).toEqual([]);
  });
});

describe('condition tree evaluator', () => {
  it('evaluates AND / OR / NOT / compares without RNG', () => {
    const m = {
      dailyTransitTrips: 500,
      fareboxRecovery: 0.7,
      coverage: 0.1,
      transitShare: 0.05,
      approval: 55,
      cash: 1_000_000,
      population: 200_000,
      day: 12,
      overcrowdedRoutes: 0,
    };
    expect(evalCondition({ metric: 'dailyTransitTrips', op: '>=', value: 500 }, m)).toBe(true);
    expect(evalCondition({ metric: 'fareboxRecovery', op: '>', value: 0.6 }, m)).toBe(true);
    expect(evalCondition({ metric: 'overcrowdedRoutes', op: '<=', value: 0 }, m)).toBe(true);
    expect(
      evalCondition(
        {
          and: [
            { metric: 'dailyTransitTrips', op: '>=', value: 500 },
            { metric: 'fareboxRecovery', op: '>', value: 0.6 },
          ],
        },
        m,
      ),
    ).toBe(true);
    expect(
      evalCondition(
        {
          or: [
            { metric: 'dailyTransitTrips', op: '>=', value: 9_999 },
            { metric: 'coverage', op: '>=', value: 0.1 },
          ],
        },
        m,
      ),
    ).toBe(true);
    expect(evalCondition({ not: { metric: 'cash', op: '<', value: 0 } }, m)).toBe(true);
    expect(treeProgress({ metric: 'dailyTransitTrips', op: '>=', value: 1000 }, m)).toBeCloseTo(0.5, 5);
  });

  it('buildScenarioState is additive UI shape', () => {
    const def = PLAYABLE_SCENARIOS[0]!;
    const state = startScenario(def);
    const snap = buildScenarioState(def, state);
    expect(snap.scenarioId).toBe(def.id);
    expect(snap.outcome).toBe('playing');
    expect(snap.won).toBe(false);
    expect(snap.lost).toBe(false);
    expect(snap.deadline).toBe(def.deadlineDays);
    expect(snap.objectives.length).toBeGreaterThan(0);
    expect(uiExtras(state).scenarioState?.scenarioId).toBe(def.id);
  });
});

describe('mid-run scenario events', () => {
  it('doubles densest-district demand on the scheduled day (deterministic)', () => {
    const def = playableScenario('cleveland-farebox-30')!;
    const a = startScenario(def, 7);
    const b = startScenario(def, 7);
    advanceDays(a, 10);
    advanceDays(b, 10);
    expect(a.firedScenarioEvents).toContain('cle-demand-surge');
    expect(b.firedScenarioEvents).toEqual(a.firedScenarioEvents);
    expect(a.districtDemandMult).toBeDefined();
    expect(Object.keys(a.districtDemandMult!).length).toBe(1);
    expect(a.districtDemandMult).toEqual(b.districtDemandMult);
    expect(stateHash(a)).toBe(stateHash(b));
  });
});

describe('playable scenario playthroughs', () => {
  const balance: BalanceRow[] = [];

  for (const def of PLAYABLE_SCENARIOS) {
    describe(def.id, () => {
      it('win path: scripted network meets the objective before the deadline', () => {
        const plan = winPlan[def.id]!;
        const state = startScenario(def, 42);
        expect(state.scenario?.id).toBe(def.id);
        expect(state.budget.cash).toBe(def.startingBudget);
        expect(state.unlockedModes).toEqual(def.startingModes);
        const mode = plan.mode ?? def.startingModes[0]!;
        buildSpine(state, plan.stations, mode, plan.vehicles, plan.spacing);
        advanceDays(state, plan.maxDays);
        const m = readMetrics(state);
        balance.push({
          id: def.id,
          city: def.cityKey,
          tier: def.tier,
          winDay: m.day,
          deadline: def.deadlineDays,
          daysLeft: def.deadlineDays - m.day,
          trips: Math.round(m.dailyTransitTrips),
          farebox: Math.round(m.fareboxRecovery * 1000) / 1000,
          coverage: Math.round(m.coverage * 1000) / 1000,
          overcrowded: m.overcrowdedRoutes,
        });
        expect(state.scenarioWon).toBe(true);
        expect(state.failed).toBeNull();
        const ui = uiExtras(state).scenarioState!;
        expect(ui.won).toBe(true);
        expect(ui.outcome).toBe('won');
        expect(ui.progress).toBe(1);
        expect(ui.day).toBeLessThanOrEqual(def.deadlineDays + 1);
      }, 60_000);

      it('lose path: null strategy (or explicit lose condition) ends the run', () => {
        const state = startScenario(def, 99);
        if (def.id === 'nyc-pressure' || def.id === 'cleveland-austerity' || def.id === 'nyc-last-stand') {
          // explicit cash lose tree
          const floor =
            def.lose && 'metric' in def.lose && def.lose.metric === 'cash' ? def.lose.value - 50_000 : -250_000;
          state.budget.cash = floor;
          advanceDays(state, 1);
          expect(state.failed).toBe('condition');
          expect(state.scenarioWon).toBeFalsy();
          const ui = uiExtras(state).scenarioState!;
          expect(ui.lost).toBe(true);
          expect(ui.outcome).toBe('lost');
          expect(ui.loseReason).toBe('condition');
          return;
        }
        if (def.id === 'cleveland-roadworks' || def.id === 'nyc-dig-season') {
          // underfleet → overcrowding lose during the surge
          const plan = winPlan[def.id]!;
          buildSpine(state, plan.stations, 'bus', 1, plan.spacing);
          advanceDays(state, plan.maxDays);
          expect(state.failed).toBe('condition');
          expect(state.scenarioWon).toBeFalsy();
          expect(uiExtras(state).scenarioState?.loseReason).toBe('condition');
          return;
        }
        // idle — no network — jump to the deadline window, then let the clock expire
        state.tick = TICKS_PER_DAY * def.deadlineDays;
        advanceDays(state, 3);
        expect(state.failed).toBe('time');
        expect(state.scenarioWon).toBeFalsy();
        const ui = uiExtras(state).scenarioState!;
        expect(ui.lost).toBe(true);
        expect(ui.outcome).toBe('lost');
        expect(ui.loseReason).toBe('time');
      }, 60_000);

      it('determinism: identical seed + commands ⇒ identical outcome hash', () => {
        const plan = winPlan[def.id]!;
        const run = (seed: number): GameState => {
          const state = startScenario(def, seed);
          buildSpine(state, plan.stations, def.startingModes[0]!, plan.vehicles, plan.spacing);
          advanceDays(state, Math.min(12, plan.maxDays));
          return state;
        };
        const a = run(123);
        const b = run(123);
        expect(stateHash(a)).toBe(stateHash(b));
        expect(a.scenarioWon).toBe(b.scenarioWon);
        expect(a.failed).toBe(b.failed);
        expect(readMetrics(a)).toEqual(readMetrics(b));
      }, 60_000);
    });
  }

  it('bankruptcy remains an implicit lose even without a lose tree', () => {
    const def = playableScenario('cleveland-first-riders')!;
    const state = startScenario(def, 3);
    setBankruptDays(0);
    state.budget.cash = -5_000_000;
    advanceDays(state, 10);
    expect(state.failed).toBe('bankrupt');
    expect(uiExtras(state).scenarioState?.outcome).toBe('lost');
  });

  it('balance table: every scenario has a non-negative winning margin (days left)', () => {
    expect(balance).toHaveLength(15);
    for (const r of balance) {
      expect(r.daysLeft).toBeGreaterThanOrEqual(0);
    }
    // Stable catalog order for PR capture (win-path pushes may interleave).
    const byId = new Map(balance.map((r) => [r.id, r]));
    const rows = PLAYABLE_SCENARIOS.map((s) => byId.get(s.id)!);
    const header =
      '| Scenario | City | Tier | Win day | Deadline | Days left | Trips | Farebox | Coverage |';
    const sep = '|---|---|---|---|---|---|---|---|---|';
    const lines = rows.map(
      (r) =>
        `| ${r.id} | ${r.city} | ${r.tier} | ${r.winDay} | ${r.deadline} | **${r.daysLeft}** | ${r.trips} | ${(r.farebox * 100).toFixed(0)}% | ${(r.coverage * 100).toFixed(1)}% |`,
    );
    // eslint-disable-next-line no-console
    console.log('\nSCENARIO_BALANCE_TABLE\n' + [header, sep, ...lines].join('\n') + '\n');
  });
});

describe('free-play unchanged', () => {
  it('omits scenarioState when no scenario is attached but still emits progression', () => {
    setBankruptDays(0);
    const state = newGame(1, 'normal');
    expect(state.scenario).toBeUndefined();
    const extras = uiExtras(state);
    expect(extras.scenarioState).toBeUndefined();
    expect(extras.scenarioProgression).toEqual(SCENARIO_PROGRESSION);
    const cmds: Command[] = [];
    void cmds;
  });
});
