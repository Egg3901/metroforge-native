/**
 * Playable scenario catalog — loads one JSON file per scenario from
 * `src/content/scenarios/`. Pure data; the engine evaluates win/lose trees
 * and mid-run events. Stable ids for saves / replays / tests.
 */
import type { ScenarioDef } from './types';

import clevelandFirstRiders from '@content/scenarios/cleveland-first-riders.json';
import clevelandFiveHundred from '@content/scenarios/cleveland-five-hundred.json';
import clevelandFarebox30 from '@content/scenarios/cleveland-farebox-30.json';
import clevelandFarebox80 from '@content/scenarios/cleveland-farebox-80.json';
import clevelandReach from '@content/scenarios/cleveland-reach.json';
import clevelandRoadworks from '@content/scenarios/cleveland-roadworks.json';
import clevelandTramLine from '@content/scenarios/cleveland-tram-line.json';
import clevelandAusterity from '@content/scenarios/cleveland-austerity.json';
import nycFirstThousand from '@content/scenarios/nyc-first-thousand.json';
import nycFarebox80 from '@content/scenarios/nyc-farebox-80.json';
import nycBusSpine from '@content/scenarios/nyc-bus-spine.json';
import nycDigSeason from '@content/scenarios/nyc-dig-season.json';
import nycPressure from '@content/scenarios/nyc-pressure.json';
import nycExpress from '@content/scenarios/nyc-express.json';
import nycLastStand from '@content/scenarios/nyc-last-stand.json';

/** Catalog order: Cleveland chain, then NYC chain (tiers escalate within each). */
export const PLAYABLE_SCENARIOS: ScenarioDef[] = [
  clevelandFirstRiders,
  clevelandFiveHundred,
  clevelandFarebox30,
  clevelandFarebox80,
  clevelandReach,
  clevelandRoadworks,
  clevelandTramLine,
  clevelandAusterity,
  nycFirstThousand,
  nycFarebox80,
  nycBusSpine,
  nycDigSeason,
  nycPressure,
  nycExpress,
  nycLastStand,
] as ScenarioDef[];

export const PLAYABLE_BY_ID: Record<string, ScenarioDef> = Object.fromEntries(
  PLAYABLE_SCENARIOS.map((s) => [s.id, s]),
);

export function playableScenario(id: string): ScenarioDef | undefined {
  return PLAYABLE_BY_ID[id];
}
