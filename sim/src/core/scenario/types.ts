/**
 * Data-driven scenario definitions + runtime UI snapshot.
 * Pure data / types — no Date, no Math.random.
 */
import type { TransitMode } from '../types';

/** Metrics readable by condition leaves (derived from GameState each day). */
export type ScenarioMetric =
  | 'dailyTransitTrips'
  | 'fareboxRecovery'
  | 'coverage'
  | 'transitShare'
  | 'approval'
  | 'cash'
  | 'population'
  | 'day'
  /** count of routes with crowding > 1 */
  | 'overcrowdedRoutes';

export type CompareOp = '>=' | '>' | '<=' | '<' | '==';

/** A single threshold check against a live metric. */
export interface ConditionLeaf {
  metric: ScenarioMetric;
  op: CompareOp;
  value: number;
  /** optional UI label; defaults to a generated readout */
  label?: string;
}

/** Boolean tree over leaves — AND / OR / NOT compose arbitrarily. */
export type ConditionNode =
  | ConditionLeaf
  | { and: ConditionNode[] }
  | { or: ConditionNode[] }
  | { not: ConditionNode };

/**
 * Mid-run scripted beat. Fires once when the calendar day equals `day`
 * (after that day's economy pass). Targets are resolved deterministically
 * from the live city (density rank), never by wall-clock.
 */
export type ScenarioEvent =
  | {
      id: string;
      day: number;
      kind: 'districtDemandMult';
      /** 0 = densest district by population+jobs at fire time */
      densityRank: number;
      mult: number;
      message: string;
    }
  | {
      id: string;
      day: number;
      kind: 'globalDemandMult';
      mult: number;
      durationDays: number;
      message: string;
    }
  | {
      id: string;
      day: number;
      kind: 'cashDelta';
      amount: number;
      message: string;
    };

export type ScenarioCityKey = 'cleveland' | 'nyc';

/** Authoring shape — JSON/TS object fully describing a playable scenario. */
export interface ScenarioDef {
  id: string;
  label: string;
  description: string;
  cityKey: ScenarioCityKey;
  /** escalating difficulty band for picker / tests */
  tier: 1 | 2 | 3 | 4 | 5;
  difficulty: 'easy' | 'normal' | 'hard';
  startingBudget: number;
  startingModes: TransitMode[];
  /** when true, population/goal unlocks cannot add modes beyond startingModes */
  lockModes?: boolean;
  dailySubsidy?: number;
  /** lose if calendar day exceeds this before the win tree is satisfied */
  deadlineDays: number;
  /** optional HUD era tag */
  eraLabel?: string;
  win: ConditionNode;
  /**
   * Extra lose tree evaluated each day (bankruptcy is always an implicit lose).
   * Omit for deadline + bankruptcy only.
   */
  lose?: ConditionNode;
  events?: ScenarioEvent[];
}

/** One flattened objective row for the UI envelope. */
export interface ScenarioObjectiveState {
  id: string;
  label: string;
  metric: ScenarioMetric;
  current: number;
  target: number;
  op: CompareOp;
  met: boolean;
  /** 0..1 progress toward this leaf (clamped) */
  progress: number;
}

/**
 * Additive `ui.scenarioState` payload. Older clients (incl. the Rust native
 * client) ignore unknown fields and keep working unchanged.
 */
export interface ScenarioState {
  scenarioId: string;
  label: string;
  objectives: ScenarioObjectiveState[];
  /** 0..1 aggregate progress across top-level AND leaves (min), or OR (max) */
  progress: number;
  /** calendar deadline in sim-days; null if unlimited */
  deadline: number | null;
  /** current calendar day (1-based, matches UiState.day) */
  day: number;
  won: boolean;
  lost: boolean;
  outcome: 'playing' | 'won' | 'lost';
  loseReason?: 'bankrupt' | 'approval' | 'time' | 'condition' | null;
  /**
   * Additive progression edges from the content manifest.
   * Completing this scenario unlocks these ids; `requires` lists OR-prereqs.
   */
  unlocks?: string[];
  requires?: string[];
}
