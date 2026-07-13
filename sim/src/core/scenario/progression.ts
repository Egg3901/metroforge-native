/**
 * Scenario progression manifest — completing X unlocks Y.
 * Pure data helpers; hosts mirror unlock edges on additive protocol fields.
 */
import progressionJson from '@content/scenarios/progression.json';

export interface ScenarioProgressionManifest {
  /** scenario ids playable with no prior clears */
  starters: string[];
  /** completing key unlocks each listed id */
  unlocks: Record<string, string[]>;
}

export const SCENARIO_PROGRESSION: ScenarioProgressionManifest = {
  starters: [...progressionJson.starters],
  unlocks: Object.fromEntries(
    Object.entries(progressionJson.unlocks).map(([k, v]) => [k, [...v]]),
  ),
};

/** Scenario ids unlocked by completing `id` (empty if none). */
export function unlocksFrom(id: string): string[] {
  return SCENARIO_PROGRESSION.unlocks[id] ? [...SCENARIO_PROGRESSION.unlocks[id]!] : [];
}

/** Scenario ids that must be cleared before `id` is available (derived inverse). */
export function requiresFor(id: string): string[] {
  const out: string[] = [];
  for (const [completed, unlocked] of Object.entries(SCENARIO_PROGRESSION.unlocks)) {
    if (unlocked.includes(id)) out.push(completed);
  }
  return out.sort();
}

/** True when `id` is a starter or appears as an unlock target. */
export function isProgressionKnown(id: string): boolean {
  if (SCENARIO_PROGRESSION.starters.includes(id)) return true;
  for (const unlocked of Object.values(SCENARIO_PROGRESSION.unlocks)) {
    if (unlocked.includes(id)) return true;
  }
  return id in SCENARIO_PROGRESSION.unlocks;
}

/**
 * Given a set of completed scenario ids, return which catalog ids are playable.
 * Starters are always open; others open when at least one prerequisite is cleared
 * (OR semantics — any listed unlock edge is enough).
 */
export function availableScenarios(completed: ReadonlySet<string> | readonly string[], catalogIds: readonly string[]): string[] {
  const done = completed instanceof Set ? completed : new Set(completed);
  const open = new Set<string>(SCENARIO_PROGRESSION.starters);
  for (const [cleared, unlocked] of Object.entries(SCENARIO_PROGRESSION.unlocks)) {
    if (!done.has(cleared)) continue;
    for (const id of unlocked) open.add(id);
  }
  return catalogIds.filter((id) => open.has(id));
}
