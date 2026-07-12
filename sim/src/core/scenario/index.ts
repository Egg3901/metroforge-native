/**
 * Scenario engine — data-driven win/lose trees + mid-run events.
 * Evaluated each sim-day inside the core tick; hosts only mirror `scenarioState`.
 */
export type {
  CompareOp,
  ConditionLeaf,
  ConditionNode,
  ScenarioCityKey,
  ScenarioDef,
  ScenarioEvent,
  ScenarioMetric,
  ScenarioObjectiveState,
  ScenarioState,
} from './types';

export {
  buildScenarioState,
  compare,
  defaultLeafLabel,
  evalCondition,
  flattenObjectives,
  isLeaf,
  leafProgress,
  readMetrics,
  rulesFromScenario,
  treeProgress,
} from './evaluate';
export type { MetricSnapshot } from './evaluate';

export { applyScenarioEvents, tickGlobalDemandMult } from './events';
export type { ScenarioEventResult } from './events';

export {
  PLAYABLE_SCENARIOS,
  PLAYABLE_BY_ID,
  playableScenario,
} from './catalog';

export {
  SCENARIO_PROGRESSION,
  availableScenarios,
  isProgressionKnown,
  requiresFor,
  unlocksFrom,
} from './progression';
export type { ScenarioProgressionManifest } from './progression';

import { evalCondition, readMetrics } from './evaluate';
import { applyScenarioEvents, tickGlobalDemandMult } from './events';
import type { ScenarioDef } from './types';
import type { GameState } from '../types';

export interface ScenarioDayResult {
  won?: boolean;
  lostCondition?: boolean;
  messages: string[];
  toasts: { message: string; tone: 'info' | 'warn' | 'good' }[];
}

/**
 * Run the scenario engine for a completed calendar day.
 * Call after economy / approval updates and before (or as part of) failure checks.
 * Does not itself set bankruptcy / approval / time — those stay in sim.checkFailure;
 * this only evaluates the data-driven win tree, optional lose tree, and events.
 */
export function evaluateScenarioDay(state: GameState, def: ScenarioDef, day: number): ScenarioDayResult {
  const result: ScenarioDayResult = { messages: [], toasts: [] };
  if (state.scenarioWon || state.failed) return result;

  tickGlobalDemandMult(state);
  const fired = applyScenarioEvents(state, def, day);
  result.messages.push(...fired.messages);
  result.toasts.push(...fired.toasts);

  const m = readMetrics(state);
  // day metric in conditions is the completed-day count (same as dayCompleted)
  m.day = day;

  if (evalCondition(def.win, m)) {
    state.scenarioWon = true;
    result.won = true;
    result.messages.push(`Objective met — ${def.label} complete`);
    result.toasts.push({ message: `Victory — ${def.label}`, tone: 'good' });
    return result;
  }

  if (def.lose && evalCondition(def.lose, m)) {
    state.failed = 'condition';
    result.lostCondition = true;
    result.messages.push('Scenario lose condition triggered');
    result.toasts.push({ message: 'Scenario failed', tone: 'warn' });
  }

  return result;
}
