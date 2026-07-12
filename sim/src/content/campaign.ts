// Campaign progression + entitlement — the stars analogue of ahd-sim's packs.
// Each scenario is worth up to 3 stars, earned by how far past its goal you
// finish. Banked stars unlock higher-tier cities. Persisted locally and, when
// signed in, synced to the account via /api/campaign.
import { REGISTRY_BY_ID, STARS_PER_SCENARIO, type ScenarioMeta } from './scenarioRegistry';

const KEY = 'metroforge:stars';

/** Best stars earned per scenarioId. */
export type StarMap = Record<string, number>;

export function loadStars(): StarMap {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    if (parsed && typeof parsed === 'object') return parsed as StarMap;
  } catch {
    /* corrupt or unavailable storage → start fresh */
  }
  return {};
}

export function persistStars(map: StarMap): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(map));
  } catch {
    /* storage full/unavailable — progression just won't persist */
  }
}

export function totalStars(map: StarMap): number {
  let n = 0;
  for (const id in map) if (REGISTRY_BY_ID[id]) n += map[id] ?? 0;
  return n;
}

/** How many stars a finishing run earns: 1 for meeting the goal, 2 at 1.3x,
 *  3 at 1.7x. `progress` is the scenario's own 0..∞ progress at win time. */
export function starsForProgress(progress: number): number {
  if (progress >= 1.7) return 3;
  if (progress >= 1.3) return 2;
  if (progress >= 1) return 1;
  return 0;
}

/** Merge two star maps, keeping the best per scenario. */
export function mergeStars(a: StarMap, b: StarMap): StarMap {
  const out: StarMap = { ...a };
  for (const id in b) {
    const v = Math.min(STARS_PER_SCENARIO, Math.max(out[id] ?? 0, b[id] ?? 0));
    if (v > 0) out[id] = v;
  }
  return out;
}

/** Record a run's stars, keeping the best; returns the updated map. */
export function recordStars(scenarioId: string, stars: number): StarMap {
  const map = loadStars();
  if (stars > (map[scenarioId] ?? 0)) {
    map[scenarioId] = Math.min(STARS_PER_SCENARIO, stars);
    persistStars(map);
  }
  return map;
}

/** Replace local stars with a merged cloud+local map and persist. */
export function applyCloudStars(cloud: StarMap): StarMap {
  const merged = mergeStars(loadStars(), cloud);
  persistStars(merged);
  return merged;
}

export function isUnlocked(meta: ScenarioMeta, banked: number): boolean {
  return meta.unlockStars <= 0 || banked >= meta.unlockStars;
}

/** Stars still needed to unlock a locked scenario (0 if already unlocked). */
export function starsToUnlock(meta: ScenarioMeta, banked: number): number {
  return Math.max(0, meta.unlockStars - banked);
}
