/**
 * Deterministic scenario condition evaluation + UI snapshot builder.
 * No Date.now / Math.random — metrics come only from GameState.
 */
import { fareboxRecovery } from '../economy';
import { TICKS_PER_DAY } from '../constants';
import type { GameState } from '../types';
import { requiresFor, unlocksFrom } from './progression';
import type {
  CompareOp,
  ConditionLeaf,
  ConditionNode,
  ScenarioDef,
  ScenarioMetric,
  ScenarioObjectiveState,
  ScenarioState,
} from './types';

export interface MetricSnapshot {
  dailyTransitTrips: number;
  fareboxRecovery: number;
  coverage: number;
  transitShare: number;
  approval: number;
  cash: number;
  population: number;
  day: number;
  overcrowdedRoutes: number;
}

export function readMetrics(state: GameState): MetricSnapshot {
  return {
    dailyTransitTrips: state.stats.dailyTransitTrips,
    fareboxRecovery: fareboxRecovery(state.budget.lastDay),
    coverage: state.stats.coverage,
    transitShare: state.stats.transitShare,
    approval: state.stats.approval,
    cash: state.budget.cash,
    population: state.stats.population,
    day: Math.floor(state.tick / TICKS_PER_DAY),
    overcrowdedRoutes: state.routes.filter((r) => (r.crowding ?? 0) > 1).length,
  };
}

export function compare(op: CompareOp, current: number, target: number): boolean {
  switch (op) {
    case '>=':
      return current >= target;
    case '>':
      return current > target;
    case '<=':
      return current <= target;
    case '<':
      return current < target;
    case '==':
      return current === target;
  }
}

export function isLeaf(node: ConditionNode): node is ConditionLeaf {
  return (node as ConditionLeaf).metric !== undefined;
}

export function evalCondition(node: ConditionNode, m: MetricSnapshot): boolean {
  if (isLeaf(node)) {
    return compare(node.op, m[node.metric], node.value);
  }
  if ('and' in node) return node.and.every((c) => evalCondition(c, m));
  if ('or' in node) return node.or.some((c) => evalCondition(c, m));
  if ('not' in node) return !evalCondition(node.not, m);
  return false;
}

/** Leaf progress 0..1 — for >=/> toward a positive target; inverted for <=/<. */
export function leafProgress(leaf: ConditionLeaf, m: MetricSnapshot): number {
  const cur = m[leaf.metric];
  const t = leaf.value;
  if (leaf.op === '>=' || leaf.op === '>') {
    if (t <= 0) return cur >= t ? 1 : 0;
    return Math.max(0, Math.min(1, cur / t));
  }
  if (leaf.op === '<=' || leaf.op === '<') {
    // closer to target from above counts as progress; already under = 1
    if (cur <= t) return 1;
    if (t <= 0) return 0;
    return Math.max(0, Math.min(1, t / cur));
  }
  return cur === t ? 1 : 0;
}

export function treeProgress(node: ConditionNode, m: MetricSnapshot): number {
  if (isLeaf(node)) return leafProgress(node, m);
  if ('and' in node) {
    if (node.and.length === 0) return 1;
    let min = 1;
    for (const c of node.and) min = Math.min(min, treeProgress(c, m));
    return min;
  }
  if ('or' in node) {
    if (node.or.length === 0) return 0;
    let max = 0;
    for (const c of node.or) max = Math.max(max, treeProgress(c, m));
    return max;
  }
  if ('not' in node) return evalCondition(node, m) ? 1 : 0;
  return 0;
}

const METRIC_LABEL: Record<ScenarioMetric, string> = {
  dailyTransitTrips: 'Daily riders',
  fareboxRecovery: 'Farebox recovery',
  coverage: 'Coverage',
  transitShare: 'Transit share',
  approval: 'Approval',
  cash: 'Cash',
  population: 'Population',
  day: 'Day',
  overcrowdedRoutes: 'Overcrowded routes',
};

function formatMetric(metric: ScenarioMetric, value: number): string {
  if (metric === 'fareboxRecovery' || metric === 'coverage' || metric === 'transitShare') {
    return `${Math.round(value * 100)}%`;
  }
  if (metric === 'cash') return `$${Math.round(value).toLocaleString()}`;
  if (metric === 'approval') return `${Math.round(value)}%`;
  return Math.round(value).toLocaleString();
}

export function defaultLeafLabel(leaf: ConditionLeaf): string {
  return `${METRIC_LABEL[leaf.metric]} ${leaf.op} ${formatMetric(leaf.metric, leaf.value)}`;
}

/** Flatten a win tree into UI objective rows (top-level AND leaves; compounds otherwise). */
export function flattenObjectives(node: ConditionNode, m: MetricSnapshot): ScenarioObjectiveState[] {
  const nodes: ConditionNode[] = 'and' in node && !isLeaf(node) ? node.and : [node];
  const out: ScenarioObjectiveState[] = [];
  for (let i = 0; i < nodes.length; i++) {
    const n = nodes[i]!;
    if (isLeaf(n)) {
      out.push({
        id: `obj-${i}-${n.metric}`,
        label: n.label ?? defaultLeafLabel(n),
        metric: n.metric,
        current: m[n.metric],
        target: n.value,
        op: n.op,
        met: compare(n.op, m[n.metric], n.value),
        progress: leafProgress(n, m),
      });
    } else {
      out.push({
        id: `obj-${i}-compound`,
        label: 'Compound objective',
        metric: 'dailyTransitTrips',
        current: treeProgress(n, m),
        target: 1,
        op: '>=',
        met: evalCondition(n, m),
        progress: treeProgress(n, m),
      });
    }
  }
  return out;
}

export function buildScenarioState(def: ScenarioDef, state: GameState): ScenarioState {
  const m = readMetrics(state);
  const won = state.scenarioWon === true;
  const lost = state.failed !== null;
  const objectives = flattenObjectives(def.win, m);
  const unlocks = unlocksFrom(def.id);
  const requires = requiresFor(def.id);
  const snap: ScenarioState = {
    scenarioId: def.id,
    label: def.label,
    objectives,
    progress: won ? 1 : treeProgress(def.win, m),
    deadline: def.deadlineDays,
    day: Math.floor(state.tick / TICKS_PER_DAY) + 1,
    won,
    lost,
    outcome: won ? 'won' : lost ? 'lost' : 'playing',
  };
  if (lost) snap.loseReason = state.failed;
  if (unlocks.length) snap.unlocks = unlocks;
  if (requires.length) snap.requires = requires;
  return snap;
}

/** Map a ScenarioDef onto the existing newGame ScenarioRules shape. */
export function rulesFromScenario(def: ScenarioDef): import('../scenarioRules').ScenarioRules {
  const rules: import('../scenarioRules').ScenarioRules = {
    scenarioId: def.id,
    startingModes: [...def.startingModes],
    startingCash: def.startingBudget,
    maxDay: def.deadlineDays,
  };
  if (def.lockModes !== undefined) rules.lockModes = def.lockModes;
  if (def.dailySubsidy !== undefined) rules.dailySubsidy = def.dailySubsidy;
  if (def.eraLabel !== undefined) rules.eraLabel = def.eraLabel;
  return rules;
}
