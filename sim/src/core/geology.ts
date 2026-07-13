/**
 * Strata / geology model (v0.8 Underground — sim side).
 *
 * Design goals, mirroring weather.ts:
 *  1. DETERMINISM. The subsurface at any column (x, y) is a *pure function* of
 *     (seed, x, y, city geology profile). There is NO stored 3D grid: band
 *     depths are reconstructed on demand from seeded value-noise, so the model
 *     costs O(1) memory, adds zero bytes to saves, and reproduces bit-for-bit.
 *     Like weather it draws from a DERIVED seed stream (seed ⊕ geology salts),
 *     never from `state.rngState`, so enabling geology cannot perturb the
 *     existing city-event / growth RNG — old replays still reproduce.
 *  2. PER-CITY PROFILES. Each city id carries a compact geology profile
 *     (nominal layer thicknesses, rock hardness, water-table behaviour),
 *     hardcoded here like the climate tables — no content rebake. Manhattan is
 *     shallow hard schist; Boston is deep fill+clay; Chicago is deep soft clay;
 *     SF is mixed with bay mud; and so on.
 *  3. WATER TABLE from elevation. The table is shallow in low ground near water
 *     and deeper on high ground, computed from the city's StaticElevation
 *     surface height relative to sea level plus the profile's wetness.
 *
 * The tunable *economics* (what a tunnel costs to bore through each stratum)
 * live in the sibling module `geologyCost.ts`, exactly as weatherEffects.ts
 * holds the weather economics — one file for balance passes.
 */
import type { Vec2 } from './geometry';

/** Top-down stratum kinds. `fill` = made ground / soil, `clay` = clay/sand
 *  mixed overburden, `rock` = competent rock, `bedrock` = deep basement rock. */
export type Stratum = 'fill' | 'clay' | 'rock' | 'bedrock';

/** Canonical top-down order. */
export const STRATA: readonly Stratum[] = ['fill', 'clay', 'rock', 'bedrock'] as const;

/** A single band in a reconstructed column. Depths are metres BELOW the
 *  surface (top < bottom); the bedrock band uses a large nominal bottom. */
export interface StrataBand {
  kind: Stratum;
  top: number;
  bottom: number;
}

/** A fully reconstructed subsurface column at one (x, y). */
export interface StrataColumn {
  bands: StrataBand[];
  /** depth (m below surface) to the top of the water table */
  waterTableDepth: number;
  /** 0..1 — how hard the competent rock is to cut (schist/granite high) */
  rockHardness: number;
  /** surface elevation (m above sea level) used to build this column */
  surfaceElevation: number;
}

/** Nominal bottom of the (unbounded) bedrock band, metres. */
export const BEDROCK_NOMINAL_BOTTOM = 1000;

// ── Per-city geology profiles (content, like the climate tables) ─────────────
// Thicknesses are nominal metres of each overburden layer above competent rock;
// `rockThickness` is the competent-rock band above deep bedrock. Numbers are
// hand-tuned to read like each city's ground, not survey data.

interface GeologyProfile {
  /** nominal fill/soil thickness (m) */
  soil: number;
  /** nominal clay/sand overburden thickness (m) */
  clay: number;
  /** nominal competent-rock band thickness before bedrock (m) */
  rockThickness: number;
  /** 0..1 hardness of the competent rock (drives bored-TBM cost) */
  rockHardness: number;
  /** nominal water-table depth (m) on flat ground at sea level */
  baseWaterTable: number;
  /** 0..1 wetness: higher pulls the table up (low-lying, near water) */
  wetness: number;
  /** metres the water table drops per metre of surface elevation */
  wtElevFactor: number;
}

const GENERIC_PROFILE: GeologyProfile = {
  soil: 4, clay: 15, rockThickness: 40, rockHardness: 0.55,
  baseWaterTable: 8, wetness: 0.35, wtElevFactor: 0.4,
};

const CITY_PROFILES: Record<string, GeologyProfile> = {
  generic: GENERIC_PROFILE,
  // Manhattan schist: thin overburden, competent rock at ~10-20 m, very hard.
  nyc: { soil: 4, clay: 8, rockThickness: 60, rockHardness: 0.88, baseWaterTable: 8, wetness: 0.4, wtElevFactor: 0.45 },
  // Boston: deep made-ground fill over thick soft Boston Blue Clay, rock deep,
  // shallow table on the made land near the harbour.
  boston: { soil: 8, clay: 22, rockThickness: 45, rockHardness: 0.5, baseWaterTable: 4, wetness: 0.6, wtElevFactor: 0.35 },
  // Chicago: very deep soft glacial clay over dolomite; classic soft-ground bore.
  chicago: { soil: 3, clay: 28, rockThickness: 40, rockHardness: 0.45, baseWaterTable: 5, wetness: 0.45, wtElevFactor: 0.3 },
  // San Francisco: mixed ground plus soft Bay Mud in the low made land.
  sf: { soil: 5, clay: 14, rockThickness: 50, rockHardness: 0.62, baseWaterTable: 4, wetness: 0.55, wtElevFactor: 0.5 },
  // Seattle: dense glacial till (hard to bore but not rock), rock deep.
  seattle: { soil: 4, clay: 26, rockThickness: 45, rockHardness: 0.55, baseWaterTable: 6, wetness: 0.45, wtElevFactor: 0.45 },
  // Los Angeles: deep soft alluvium, rock very deep, deep table (semi-arid).
  la: { soil: 6, clay: 42, rockThickness: 35, rockHardness: 0.3, baseWaterTable: 12, wetness: 0.2, wtElevFactor: 0.35 },
  // Washington DC: Atlantic coastal-plain sands/clays over deeper rock.
  dc: { soil: 5, clay: 24, rockThickness: 40, rockHardness: 0.42, baseWaterTable: 5, wetness: 0.5, wtElevFactor: 0.4 },
  // Philadelphia: Wissahickon schist under a modest coastal-plain wedge.
  philly: { soil: 5, clay: 15, rockThickness: 50, rockHardness: 0.62, baseWaterTable: 6, wetness: 0.4, wtElevFactor: 0.45 },
  // Atlanta: residual saprolite soil over hard Piedmont granite, upland table.
  atlanta: { soil: 6, clay: 15, rockThickness: 55, rockHardness: 0.78, baseWaterTable: 10, wetness: 0.25, wtElevFactor: 0.5 },
  // Cleveland: glacial clay over shale, cloudy-wet lowland table.
  cleveland: { soil: 4, clay: 20, rockThickness: 40, rockHardness: 0.5, baseWaterTable: 5, wetness: 0.45, wtElevFactor: 0.35 },
};

/** Resolve a city's geology profile, falling back to the generic temperate one. */
export function geologyProfile(cityKey: string | undefined): GeologyProfile {
  return (cityKey && CITY_PROFILES[cityKey]) || GENERIC_PROFILE;
}

// ── Seeded value-noise (deterministic, no RNG-stream draw) ───────────────────
// Independent, fixed salts so geology never collides with the main RNG stream
// or with weather. Separate band/table salts keep the two fields uncorrelated.
const BAND_SALT = 0x51ed270b;
const TABLE_SALT = 0x2c1b3c6d;

/** Cell size (m) of the coarse noise lattice; band depths vary smoothly on this. */
export const STRATA_NOISE_CELL = 250;
/** ±fraction a layer thickness can wander from nominal. */
export const STRATA_NOISE_FRAC = 0.35;

function hash2(seed: number, salt: number, ix: number, iy: number): number {
  let h = (seed ^ salt) >>> 0;
  h = Math.imul(h ^ (ix + 0x9e3779b9), 0x85ebca6b) >>> 0;
  h = Math.imul(h ^ (iy + 0x165667b1), 0xc2b2ae35) >>> 0;
  h = (h ^ (h >>> 15)) >>> 0;
  return h / 4294967296; // [0,1)
}

/** Smooth bilinear value noise in [0,1) at world (x,y) on the coarse lattice. */
function valueNoise(seed: number, salt: number, x: number, y: number): number {
  const gx = x / STRATA_NOISE_CELL;
  const gy = y / STRATA_NOISE_CELL;
  const ix = Math.floor(gx);
  const iy = Math.floor(gy);
  const fx = gx - ix;
  const fy = gy - iy;
  // smoothstep for continuity
  const sx = fx * fx * (3 - 2 * fx);
  const sy = fy * fy * (3 - 2 * fy);
  const v00 = hash2(seed, salt, ix, iy);
  const v10 = hash2(seed, salt, ix + 1, iy);
  const v01 = hash2(seed, salt, ix, iy + 1);
  const v11 = hash2(seed, salt, ix + 1, iy + 1);
  return (v00 * (1 - sx) + v10 * sx) * (1 - sy) + (v01 * (1 - sx) + v11 * sx) * sy;
}

/** Signed ±STRATA_NOISE_FRAC multiplier for a layer, from a salted noise field. */
function thicknessJitter(seed: number, salt: number, x: number, y: number): number {
  return 1 + (valueNoise(seed, salt, x, y) - 0.5) * 2 * STRATA_NOISE_FRAC;
}

// ── Elevation sampling (StaticElevation grid, co-registered with the masks) ──
/** Sample the surface elevation (m above sea level) at world (x,y). Mirrors the
 *  mask sampling in osmCity.maskAt so elevation and water line up. Returns 0
 *  (sea level, flat) when a city has no baked elevation (procedural cities). */
export function sampleSurfaceElevation(
  elev: Int16Array | undefined,
  res: number | undefined,
  worldSize: number,
  x: number,
  y: number,
): number {
  if (!elev || !res) return 0;
  const half = worldSize / 2;
  let c = Math.floor(((x + half) / worldSize) * res);
  let r = Math.floor(((y + half) / worldSize) * res);
  if (c < 0) c = 0; else if (c >= res) c = res - 1;
  if (r < 0) r = 0; else if (r >= res) r = res - 1;
  return elev[r * res + c] as number;
}

// ── Water table ──────────────────────────────────────────────────────────────
/** Water table can never be modelled shallower/deeper than these bounds. */
export const MIN_WATER_TABLE = 1.5;
export const MAX_WATER_TABLE = 45;

/**
 * Depth (m) to the water table at a column. Low-lying ground near water level
 * (small surface elevation) has a SHALLOW table; high ground has a deep one.
 * `surfaceElevation` is taken as the height above the nearest water surface,
 * approximated as sea level (0) — the case that matters for the coastal /
 * riverine cities where tunnelling meets groundwater.
 */
export function waterTableDepthAt(profile: GeologyProfile, surfaceElevation: number, seed: number, x: number, y: number): number {
  const noise = (valueNoise(seed, TABLE_SALT, x, y) - 0.5) * 2 * 3; // ±3 m
  const wt =
    profile.baseWaterTable * (1 - 0.4 * profile.wetness) +
    Math.max(0, surfaceElevation) * profile.wtElevFactor +
    noise;
  if (wt < MIN_WATER_TABLE) return MIN_WATER_TABLE;
  if (wt > MAX_WATER_TABLE) return MAX_WATER_TABLE;
  return wt;
}

// ── Column reconstruction ────────────────────────────────────────────────────
/**
 * Reconstruct the full subsurface column at world (x,y) for a city. Pure
 * function of (seed, profile, elevation) — no RNG-stream draw, O(1).
 */
export function strataColumn(
  profile: GeologyProfile,
  seed: number,
  worldSize: number,
  elev: Int16Array | undefined,
  elevRes: number | undefined,
  p: Vec2,
): StrataColumn {
  const surfaceElevation = sampleSurfaceElevation(elev, elevRes, worldSize, p.x, p.y);
  // three independent salted noise draws for the three layer boundaries
  const soilBottom = profile.soil * thicknessJitter(seed, BAND_SALT, p.x, p.y);
  const clayBottom = soilBottom + profile.clay * thicknessJitter(seed, BAND_SALT ^ 0x1111, p.x, p.y);
  const bedrockTop = clayBottom + profile.rockThickness * thicknessJitter(seed, BAND_SALT ^ 0x2222, p.x, p.y);
  const bands: StrataBand[] = [
    { kind: 'fill', top: 0, bottom: soilBottom },
    { kind: 'clay', top: soilBottom, bottom: clayBottom },
    { kind: 'rock', top: clayBottom, bottom: bedrockTop },
    { kind: 'bedrock', top: bedrockTop, bottom: BEDROCK_NOMINAL_BOTTOM },
  ];
  return {
    bands,
    waterTableDepth: waterTableDepthAt(profile, surfaceElevation, seed, p.x, p.y),
    rockHardness: profile.rockHardness,
    surfaceElevation,
  };
}

/** Which stratum sits at a given depth (m below surface) in a column. */
export function stratumAtDepth(col: StrataColumn, depth: number): Stratum {
  for (const b of col.bands) {
    if (depth < b.bottom) return b.kind;
  }
  return 'bedrock';
}

/** Convenience: build a column straight from GameState-shaped inputs. */
export function columnAt(
  cityKey: string | undefined,
  seed: number,
  worldSize: number,
  elev: Int16Array | undefined,
  elevRes: number | undefined,
  p: Vec2,
): StrataColumn {
  return strataColumn(geologyProfile(cityKey), seed, worldSize, elev, elevRes, p);
}
