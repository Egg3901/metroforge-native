/**
 * Geology → construction economics (v0.8). ALL tunable tunnel-cost constants
 * live here and every factor is documented, so balance passes touch one file
 * (the geology twin of weatherEffects.ts). Pure and deterministic: costs are a
 * function of (segment length, chosen depth, the strata column, water table).
 *
 * The model, in one paragraph: an underground segment is priced RELATIVE to the
 * same segment built at grade (`surfaceCostPerM`). Two build methods compete
 * per segment and the cheaper feasible one is chosen automatically:
 *   • CUT-AND-COVER — cheap, but only shallow (<= CUT_COVER_MAX_DEPTH) and
 *     miserable in rock (you excavate it from the surface). Adds a temporary
 *     surface-disruption effect while building (flagged for the build queue).
 *   • BORED (TBM) — works at any depth. Reflecting real TBM economics it is
 *     CHEAPER in competent rock (a stable, self-supporting bore) than in soft,
 *     wet, mixed soils (which need shielding and settle) — but very hard rock
 *     costs more to cut, so hardness adds back a premium.
 * Below the water table a waterproofing surcharge applies (auto-included for
 * now; a residual flood-risk factor is stored per segment for the weather storm
 * events to consume in a follow-up). Deeper tunnels cost more (shoring/access);
 * that same depth term drives underground STATION cost and the rider access-time
 * penalty below.
 *
 * ── BALANCE: the cost curve, in multiples of the SAME segment at grade ───────
 * Surface = 1.0x by definition; elevated is ~2.4-2.6x (mode gradeCostMult).
 * Underground lands 3-10x, with the deep under-river bore the premium option.
 * Five worked examples (numbers are the exact per-metre multiplier the model
 * returns today; regenerate if the constants below move):
 *
 *   1. Shallow cut-and-cover under a street, soft dry soil, depth 8 m
 *      → cutCover, above water table                → 2.90x   (the cheap tunnel)
 *   2. Cut-and-cover in deep clay below the table, depth 12 m (Chicago)
 *      → cutCover + waterproofing                   → 3.97x
 *   3. Bored through Manhattan schist (hard rock), below table, depth 12 m
 *      → bored in rock, hardness premium + wp       → 6.30x
 *   4. Bored in deep soft clay, below table, depth 20 m (Boston fill/clay)
 *      → bored in wet soil (unstable) + wp          → 9.14x
 *   5. Under-river bored, wet soft ground, below table, depth 24 m
 *      → bored wet soil, deepest + wp               → 9.66x   (the premium)
 *
 * Read-outs: (a) tunnels are always meaningfully pricier than surface (>=3x);
 * (b) a hard-rock bore (ex.3) is CHEAPER per metre than a soft wet-ground bore
 * (ex.4) even though the rock is harder to cut, because stable ground needs no
 * shielding — real TBM economics; (c) in soft ground the model prefers cheap
 * shallow cut-and-cover (ex.1-2) and only escalates to a bore when depth forces
 * it (ex.4-5); (d) the under-river deep bore is the ceiling (~10x), expensive
 * but not economy-breaking. All five knobs live in the constants below.
 */
import type { StrataColumn } from './geology';
import { stratumAtDepth } from './geology';

// ── Method feasibility / default depths ──────────────────────────────────────
/** Cut-and-cover is only viable down to here; deeper forces a bored tunnel. */
export const CUT_COVER_MAX_DEPTH = 15;
/** Default tunnel depth (m) under land when the caller supplies none. */
export const DEFAULT_TUNNEL_DEPTH = 12;
/** Default tunnel depth (m) under water — tunnels dip below the river/bay bed. */
export const RIVER_TUNNEL_DEPTH = 24;

// ── Cost multipliers, all relative to the surface cost of the same segment ───
/** Cut-and-cover base multiplier (shallow soft ground). */
export const CUT_COVER_BASE = 2.5;
/** Penalty factor when cut-and-cover has to chew through rock from above. */
export const CUT_COVER_ROCK_PENALTY = 2.0;
/** Bored base multiplier when the bore runs through competent rock. */
export const BORED_ROCK_BASE = 3.0;
/** Bored base multiplier when the bore runs through soft overburden. */
export const BORED_SOIL_BASE = 4.2;
/** Extra added to the soil bore when it is below the water table (wet, unstable). */
export const BORED_WET_SOIL_PENALTY = 0.9;
/** Multiplier on rock hardness (0..1) added to a rock bore (harder = pricier). */
export const ROCK_HARDNESS_PREMIUM = 1.1;
/** Fraction of surface cost added per metre of depth (shoring, access, spoil). */
export const DEPTH_COST_PER_M = 0.02;
/** Waterproofing surcharge (fraction) applied below the water table. */
export const WATERPROOF_SURCHARGE = 0.28;

// ── Flood risk (stored, consumed later by weather storm events) ──────────────
/** Residual per-segment flood-risk factor for a fully waterproofed below-table
 *  tunnel. Scales up with how far below the table the invert sits. Unused today;
 *  the v0.8+ storm coupling reads it. */
export const FLOOD_RISK_BASE = 0.04;
export const FLOOD_RISK_PER_M_BELOW = 0.01;

// ── Station depth economics ──────────────────────────────────────────────────
/** Underground-station cost surcharge = base station cost × this × depth(m). */
export const STATION_DEPTH_COST_FACTOR = 0.03;
/** Below this depth (m) a station has no access-time penalty. */
export const STATION_DEPTH_FREE_M = 10;
/** Access-time penalty added per 10 m of station depth below the free depth. */
export const STATION_DEPTH_ACCESS_SEC_PER_10M = 30;

export type BuildMethod = 'cutCover' | 'bored';

export interface SegmentCostResult {
  /** total cost for this segment (money) */
  cost: number;
  /** cost per metre (money/m) */
  costPerM: number;
  /** method chosen (cheaper feasible one) */
  method: BuildMethod;
  /** chosen tunnel depth (m) */
  depth: number;
  /** is the invert below the water table? */
  belowWaterTable: boolean;
  /** residual flood-risk factor stored for future storm coupling */
  floodRisk: number;
  /** stratum the bore/box sits in, for the breakdown summary */
  stratum: string;
}

/** Bored multiplier for a column at a depth (before depth/waterproof factors). */
export function boredMult(col: StrataColumn, depth: number, belowWaterTable: boolean): number {
  const s = stratumAtDepth(col, depth);
  if (s === 'rock' || s === 'bedrock') {
    return BORED_ROCK_BASE + col.rockHardness * ROCK_HARDNESS_PREMIUM;
  }
  let m = BORED_SOIL_BASE;
  if (belowWaterTable) m += BORED_WET_SOIL_PENALTY;
  return m;
}

/** Cut-and-cover multiplier for a column at a depth. */
export function cutCoverMult(col: StrataColumn, depth: number): number {
  const s = stratumAtDepth(col, depth);
  const base = CUT_COVER_BASE;
  return s === 'rock' || s === 'bedrock' ? base * CUT_COVER_ROCK_PENALTY : base;
}

/**
 * Price one underground segment of length `lenM` through column `col`, choosing
 * the cheaper feasible method. `depth` defaults to the land/river default.
 */
export function undergroundSegmentCost(
  surfaceCostPerM: number,
  lenM: number,
  col: StrataColumn,
  depth: number,
): SegmentCostResult {
  const belowWaterTable = depth >= col.waterTableDepth;
  const depthMult = 1 + DEPTH_COST_PER_M * depth;
  const waterproofMult = belowWaterTable ? 1 + WATERPROOF_SURCHARGE : 1;

  const boredPerM = surfaceCostPerM * boredMult(col, depth, belowWaterTable) * depthMult * waterproofMult;
  const cutFeasible = depth <= CUT_COVER_MAX_DEPTH;
  const cutPerM = cutFeasible
    ? surfaceCostPerM * cutCoverMult(col, depth) * depthMult * waterproofMult
    : Infinity;

  const useCut = cutPerM <= boredPerM;
  const costPerM = useCut ? cutPerM : boredPerM;
  const method: BuildMethod = useCut ? 'cutCover' : 'bored';

  const floodRisk = belowWaterTable
    ? FLOOD_RISK_BASE + Math.max(0, depth - col.waterTableDepth) * FLOOD_RISK_PER_M_BELOW
    : 0;

  return {
    cost: costPerM * lenM,
    costPerM,
    method,
    depth,
    belowWaterTable,
    floodRisk,
    stratum: stratumAtDepth(col, depth),
  };
}

/** Extra cost of sinking a station to `depth` metres for `baseStationCost`. */
export function stationDepthSurcharge(baseStationCost: number, depth: number): number {
  return baseStationCost * STATION_DEPTH_COST_FACTOR * Math.max(0, depth);
}

/**
 * Rider access-time penalty (SECONDS) for a station at `depth` metres:
 * +STATION_DEPTH_ACCESS_SEC_PER_10M per 10 m below STATION_DEPTH_FREE_M. A
 * 30 m deep station therefore costs +60 s of access. Surface stations
 * (undefined/0 depth) pay nothing.
 */
export function stationDepthAccessPenaltySec(depth: number | undefined): number {
  if (!depth || depth <= STATION_DEPTH_FREE_M) return 0;
  return ((depth - STATION_DEPTH_FREE_M) / 10) * STATION_DEPTH_ACCESS_SEC_PER_10M;
}
