// ─────────────────────────────────────────────────────────────────────────
// SCENARIO REGISTRY — one row per playable city, metadata only (difficulty,
// campaign tier, unlock cost in stars, routing to the engine, picker copy).
// The scenario *content* (the objective, its progress + readout) lives
// separately in app/scenarios.ts, keyed by the same scenarioId. Shared by
// the client (picker + locks) and the win flow (star awards).
// ─────────────────────────────────────────────────────────────────────────

import type { ScenarioRules } from '@core/scenarioRules';
import type { TransitMode } from '@core/types';

export type CityCode =
  | 'nyc'
  | 'boston'
  | 'chicago'
  | 'cleveland'
  | 'atlanta'
  | 'la'
  | 'philly'
  | 'sf'
  | 'dc'
  | 'seattle';

export interface ScenarioMeta {
  scenarioId: string; // global id, stable for saves + leaderboards ("nyc-1904")
  cityKey: CityCode; // the OSM preset the engine loads
  difficulty: 'easy' | 'normal' | 'hard';
  size: 'small' | 'medium' | 'large';
  /** campaign tier — cities in a higher tier unlock once you bank enough stars */
  tier: number;
  /** total stars required to unlock (0 = a starter city, always playable) */
  unlockStars: number;
  label: string;
  city: string;
  description: string;
  flag: string;
  /** year / era shown in the picker and HUD */
  era: string;
  /** engine constraints applied at newGame */
  rules: ScenarioRules;
}

const sc = (
  scenarioId: string,
  cityKey: CityCode,
  city: string,
  flag: string,
  era: string,
  difficulty: ScenarioMeta['difficulty'],
  tier: number,
  unlockStars: number,
  label: string,
  description: string,
  rules: Omit<ScenarioRules, 'scenarioId' | 'eraLabel'> & { startingModes: TransitMode[] },
): ScenarioMeta => ({
  scenarioId,
  cityKey,
  city,
  flag,
  era,
  difficulty,
  size: 'medium',
  tier,
  unlockStars,
  label,
  description,
  rules: { ...rules, scenarioId, eraLabel: era },
});

export const SCENARIO_REGISTRY: ScenarioMeta[] = [
  // ── Tier 1 · founding eras (always unlocked) ──
  sc('nyc-1904', 'nyc', 'New York', '🗽', '1904', 'normal', 1, 0,
    'First Subway',
    'IRT Day One. Dig the first underground line before the elevateds own the island.',
    {
      startingModes: ['metro'],
      lockModes: true,
      startingCash: 12_000_000,
      dailySubsidy: 35_000,
      maxDay: 120,
      approvalFloor: 20,
    }),
  sc('boston-1897', 'boston', 'Boston', '⚓', '1897', 'normal', 1, 0,
    'Tremont Tunnel',
    'America\'s first subway. Thread the peninsula and the harbor before the streets choke.',
    {
      startingModes: ['tram', 'metro'],
      lockModes: true,
      startingCash: 10_000_000,
      dailySubsidy: 32_000,
      maxDay: 100,
      approvalFloor: 22,
    }),
  sc('chicago-1892', 'chicago', 'Chicago', '🌊', '1892', 'normal', 1, 0,
    'The L Rises',
    'Build the elevated Loop. Cover the grid before the World\'s Fair crowds arrive.',
    {
      startingModes: ['tram'],
      lockModes: true,
      startingCash: 9_000_000,
      dailySubsidy: 30_000,
      maxDay: 90,
      approvalFloor: 25,
    }),

  // ── Tier 2 · mid-century pressure ──
  sc('cleveland-1955', 'cleveland', 'Cleveland', '🏭', '1955', 'normal', 2, 4,
    'Rapid Transit',
    'Post-war sprawl meets a shrinking downtown. Make the network pay for itself.',
    {
      startingModes: ['bus', 'tram'],
      lockModes: false,
      startingCash: 8_000_000,
      dailySubsidy: 28_000,
      maxDay: 150,
      approvalFloor: 25,
    }),
  sc('atlanta-1979', 'atlanta', 'Atlanta', '🌳', '1979', 'hard', 2, 4,
    'MARTA Opens',
    'A new heavy-rail spine in a car city. Stitch the sprawl before the freeways win.',
    {
      startingModes: ['bus', 'metro'],
      lockModes: true,
      startingCash: 11_000_000,
      dailySubsidy: 22_000,
      maxDay: 140,
      approvalFloor: 22,
    }),
  sc('philly-1907', 'philly', 'Philadelphia', '🔔', '1907', 'normal', 2, 4,
    'Market Street Subway',
    'Elevate and dig Market Street. Link the rivers before the streetcars own Center City.',
    {
      startingModes: ['tram', 'metro'],
      lockModes: true,
      startingCash: 10_500_000,
      dailySubsidy: 30_000,
      maxDay: 130,
      approvalFloor: 22,
    }),
  sc('sf-1912', 'sf', 'San Francisco', '🌉', '1912', 'normal', 2, 5,
    'Municipal Railway',
    'Muni takes the streets. Climb the hills and stitch the bayfront before the private lines fold.',
    {
      startingModes: ['tram'],
      lockModes: true,
      startingCash: 9_500_000,
      dailySubsidy: 28_000,
      maxDay: 120,
      approvalFloor: 24,
    }),

  // ── Tier 3 · the hardest sell ──
  sc('la-1963', 'la', 'Los Angeles', '🌴', '1963', 'hard', 3, 8,
    'After the Red Cars',
    'The streetcars are gone. Rebuild transit from buses alone and win riders off the freeway.',
    {
      startingModes: ['bus'],
      lockModes: true,
      startingCash: 7_000_000,
      dailySubsidy: 18_000,
      maxDay: 180,
      approvalFloor: 18,
    }),
  sc('dc-1976', 'dc', 'Washington', '🏛️', '1976', 'hard', 3, 8,
    'Metro Opens',
    'A monumental heavy-rail system for a monumental city. Cover the core before the Beltway wins.',
    {
      startingModes: ['bus', 'metro'],
      lockModes: true,
      startingCash: 12_000_000,
      dailySubsidy: 24_000,
      maxDay: 150,
      approvalFloor: 20,
    }),
  sc('seattle-2009', 'seattle', 'Seattle', '🌲', '2009', 'hard', 3, 9,
    'Link Light Rail',
    'Rain, hills, and a thin downtown spine. Grow light rail ridership before the ferries and freeways take over.',
    {
      startingModes: ['bus', 'tram'],
      lockModes: true,
      startingCash: 9_000_000,
      dailySubsidy: 20_000,
      maxDay: 160,
      approvalFloor: 20,
    }),
];

export const REGISTRY_BY_ID: Record<string, ScenarioMeta> = Object.fromEntries(
  SCENARIO_REGISTRY.map((m) => [m.scenarioId, m]),
);

/** Highest number of stars any single scenario can be worth (for UI). */
export const STARS_PER_SCENARIO = 3;
export const MAX_STARS = SCENARIO_REGISTRY.length * STARS_PER_SCENARIO;
