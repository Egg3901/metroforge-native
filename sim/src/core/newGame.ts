import { STARTING_CASH } from './constants';
import { generateCity } from './city/generator';
import { MAP_SIZE_METERS, presetByKey, type MapSize } from './city/presets';
import type { OsmCityData } from './city/osmCity';
import { nextInstanceId } from './instance';
import { Rng } from './rng';
import type { ScenarioDef } from './scenario/types';
import { rulesFromScenario } from './scenario/evaluate';
import type { ScenarioRules } from './scenarioRules';
import type { Difficulty, GameState, TransitMode } from './types';

export interface NewGameOptions {
  size?: MapSize | undefined;
  presetKey?: string | undefined;
  /** preloaded real-city dataset (loaded async by the host before calling) */
  osm?: OsmCityData | undefined;
  /** era / challenge constraints applied at kickoff */
  rules?: ScenarioRules | undefined;
  /** data-driven scenario (win/lose trees + events); implies rules when rules omitted */
  scenario?: ScenarioDef | undefined;
}

export function newGame(seed: number, difficulty: Difficulty, options: NewGameOptions = {}): GameState {
  const city = generateCity(seed, difficulty, {
    worldSize: options.size ? MAP_SIZE_METERS[options.size] : undefined,
    preset: presetByKey(options.presetKey),
    osm: options.osm,
  });
  const rng = new Rng((seed ^ 0x5bd1e995) >>> 0);
  let population = 0;
  let jobs = 0;
  for (const d of city.districts) {
    population += d.population;
    jobs += d.jobs;
  }
  const scenario = options.scenario;
  const rules = options.rules ?? (scenario ? rulesFromScenario(scenario) : undefined);
  const startingModes: TransitMode[] = rules?.startingModes?.length ? [...rules.startingModes] : ['bus'];
  const state: GameState = {
    seed,
    instanceId: nextInstanceId(),
    tick: 0,
    rngState: rng.state(),
    difficulty,
    fields: city.fields,
    roads: city.roads,
    districts: city.districts,
    osmWaterMask: city.waterMaskHi,
    osmParkMask: city.parkMaskHi,
    osmBuildingMask: city.buildingMaskHi,
    osmMaskRes: city.maskRes,
    osmLabels: city.labels,
    stations: [],
    tracks: [],
    routes: [],
    vehicles: [],
    flows: [],
    budget: {
      cash: rules?.startingCash ?? STARTING_CASH[difficulty],
      loanBalance: 0,
      loanRate: 0.08,
      lastDay: { fares: 0, subsidy: 0, operations: 0, maintenance: 0, interest: 0 },
      netHistory: [],
    },
    stats: {
      population,
      jobs,
      dailyTransitTrips: 0,
      dailyCarTrips: 0,
      transitShare: 0,
      coverage: 0,
      approval: 50,
    },
    nextId: 1,
    demandDirty: true,
    unlockedModes: startingModes,
    activeEvents: [],
    nextEventDay: 8, // no events in the first week
    commandLog: [],
    lowApprovalDays: 0,
    failed: null,
  };
  if (rules) state.scenarioRules = rules;
  if (scenario) {
    state.scenario = scenario;
    state.scenarioWon = false;
    state.firedScenarioEvents = [];
  }
  return state;
}
