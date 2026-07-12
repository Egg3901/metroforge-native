/**
 * City presets + map sizes. These do NOT import real GIS data — they retune the
 * tensor-field generator so each city reads like its real counterpart: grid
 * regularity, downtown pull, coastline, and sprawl. Everything stays procedural
 * and seed-deterministic; a preset just picks the knobs.
 */

export type MapSize = 'small' | 'medium' | 'large';

/** World edge length in meters. Field cell size is fixed, so bigger = more cells. */
export const MAP_SIZE_METERS: Record<MapSize, number> = {
  small: 8000,
  medium: 12000, // the original default
  large: 18000,
};

export interface WaterConfig {
  /** a straight coastline (ocean / great lake) along one edge */
  coast: boolean;
  /** fixed coast bearing in degrees, or null for a seed-random bearing */
  coastAngleDeg: number | null;
  /** 0..1 — how far inland the coast sits (higher = more land) */
  coastInset: number;
  /** carve a meandering river */
  river: boolean;
}

export interface CityPreset {
  key: string;
  label: string;
  blurb: string;
  /** backed by a real OpenStreetMap import (real roads + coastline) */
  real?: boolean;
  /** street-grid regularity */
  grid: {
    /** tensor grid patch weight (higher = streets snap harder to the grid) */
    weight: number;
    /** base grid bearing in degrees; grids all align to it when rigid */
    angleDeg: number;
    /** true = rigid rectilinear (NYC/Chicago); false = organic (Boston) */
    rigid: boolean;
    /** field noise weight — the wobble in street direction */
    noiseWeight: number;
  };
  /** downtown radial convergence (Atlanta's highways vs a flat grid) */
  radialWeight: number;
  water: WaterConfig;
  /** >1 spreads density out (LA/Atlanta sprawl); <1 concentrates it (NYC) */
  sprawl: number;
}

const GENERIC: CityPreset = {
  key: 'generic',
  label: 'Random City',
  blurb: 'A fresh procedural city each seed.',
  grid: { weight: 1, angleDeg: 0, rigid: false, noiseWeight: 0.22 },
  radialWeight: 2.2,
  water: { coast: true, coastAngleDeg: null, coastInset: 0.7, river: true },
  sprawl: 1,
};

export const CITY_PRESETS: CityPreset[] = [
  GENERIC,
  {
    key: 'nyc',
    label: 'New York',
    real: true,
    blurb: 'Real Manhattan street grid between the Hudson and East River (OSM).',
    grid: { weight: 1.5, angleDeg: 29, rigid: true, noiseWeight: 0.06 },
    radialWeight: 1.4,
    water: { coast: true, coastAngleDeg: 120, coastInset: 0.78, river: true },
    sprawl: 0.72,
  },
  {
    key: 'chicago',
    label: 'Chicago',
    real: true,
    blurb: 'Real Chicago: the grid, Lake Michigan, and the branching river (OSM).',
    grid: { weight: 1.6, angleDeg: 0, rigid: true, noiseWeight: 0.05 },
    radialWeight: 1.6,
    water: { coast: true, coastAngleDeg: 0, coastInset: 0.82, river: true },
    sprawl: 0.95,
  },
  {
    key: 'la',
    label: 'Los Angeles',
    real: true,
    blurb: 'Real downtown LA: the sprawling grid and the LA River (OSM).',
    grid: { weight: 1.2, angleDeg: 12, rigid: true, noiseWeight: 0.12 },
    radialWeight: 0.9,
    water: { coast: true, coastAngleDeg: 210, coastInset: 0.85, river: false },
    sprawl: 1.7,
  },
  {
    key: 'boston',
    label: 'Boston',
    real: true,
    blurb: 'Real Boston: harbor, the Charles, and the downtown peninsula (OSM).',
    grid: { weight: 0.7, angleDeg: 40, rigid: false, noiseWeight: 0.5 },
    radialWeight: 2.6,
    water: { coast: true, coastAngleDeg: 75, coastInset: 0.62, river: true },
    sprawl: 0.85,
  },
  {
    key: 'atlanta',
    label: 'Atlanta',
    real: true,
    blurb: 'Real Atlanta: landlocked sprawl fanning out along the highways (OSM).',
    grid: { weight: 0.9, angleDeg: 20, rigid: false, noiseWeight: 0.3 },
    radialWeight: 3.2,
    water: { coast: false, coastAngleDeg: null, coastInset: 1, river: false },
    sprawl: 1.8,
  },
  {
    key: 'cleveland',
    label: 'Cleveland',
    real: true,
    blurb: 'Real Cleveland: Lake Erie and the winding Cuyahoga through the Flats (OSM).',
    grid: { weight: 1.3, angleDeg: 8, rigid: true, noiseWeight: 0.1 },
    radialWeight: 1.8,
    water: { coast: true, coastAngleDeg: 0, coastInset: 0.8, river: true },
    sprawl: 1.1,
  },
  {
    key: 'philly',
    label: 'Philadelphia',
    real: true,
    blurb: 'Real Center City: the William Penn grid between the Schuylkill and Delaware (OSM).',
    grid: { weight: 1.55, angleDeg: 0, rigid: true, noiseWeight: 0.06 },
    radialWeight: 1.5,
    water: { coast: false, coastAngleDeg: null, coastInset: 1, river: true },
    sprawl: 0.9,
  },
  {
    key: 'sf',
    label: 'San Francisco',
    real: true,
    blurb: 'Real San Francisco: hills, the bay, and a tight downtown grid (OSM).',
    grid: { weight: 1.35, angleDeg: 0, rigid: true, noiseWeight: 0.18 },
    radialWeight: 2.0,
    water: { coast: true, coastAngleDeg: 45, coastInset: 0.7, river: false },
    sprawl: 0.8,
  },
  {
    key: 'dc',
    label: 'Washington',
    real: true,
    blurb: 'Real Washington: L\'Enfant avenues, the Mall, and the Potomac (OSM).',
    grid: { weight: 1.1, angleDeg: 0, rigid: false, noiseWeight: 0.2 },
    radialWeight: 2.8,
    water: { coast: false, coastAngleDeg: null, coastInset: 1, river: true },
    sprawl: 1.05,
  },
  {
    key: 'seattle',
    label: 'Seattle',
    real: true,
    blurb: 'Real Seattle: Elliott Bay, Lake Union, and the downtown ridge (OSM).',
    grid: { weight: 1.25, angleDeg: 0, rigid: true, noiseWeight: 0.14 },
    radialWeight: 2.2,
    water: { coast: true, coastAngleDeg: 270, coastInset: 0.72, river: false },
    sprawl: 1.15,
  },
];

export function presetByKey(key: string | undefined): CityPreset {
  return CITY_PRESETS.find((p) => p.key === key) ?? GENERIC;
}
