/**
 * Scenario constraints applied at newGame / simTick. Kept in core so replays
 * and the native port see the same rules as the live game.
 */
import type { TransitMode } from './types';

export interface ScenarioRules {
  /** stable id for leaderboards / replays */
  scenarioId?: string;
  /** modes available at kickoff (defaults to bus-only) */
  startingModes: TransitMode[];
  /** when true, population/goal unlocks cannot add modes beyond startingModes */
  lockModes?: boolean;
  /** lose if the calendar day exceeds this before the objective is met */
  maxDay?: number;
  /** lose if approval stays at or below this for APPROVAL_GRACE_DAYS */
  approvalFloor?: number;
  /** override STARTING_CASH[difficulty] when set */
  startingCash?: number;
  /** override BASE_DAILY_SUBSIDY[difficulty] when set */
  dailySubsidy?: number;
  /** human-readable era label for HUD ("1904") */
  eraLabel?: string;
}

export const APPROVAL_GRACE_DAYS = 5;

/** Free-play / modern unlock thresholds — goals first, population as fallback. */
export function modeUnlockReady(
  mode: TransitMode,
  stats: { population: number; dailyTransitTrips: number; transitShare: number; coverage: number },
): boolean {
  if (mode === 'bus') return true;
  if (mode === 'tram') return stats.dailyTransitTrips >= 1_000 || stats.population >= 50_000;
  if (mode === 'metro') return stats.transitShare >= 0.1 || stats.coverage >= 0.5 || stats.population >= 150_000;
  if (mode === 'rail') return stats.transitShare >= 0.25 || stats.dailyTransitTrips >= 50_000 || stats.population >= 300_000;
  return false;
}
