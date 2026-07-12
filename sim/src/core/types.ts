/**
 * Core simulation types. This file plus ARCHITECTURE.md is the portable spec.
 * Everything here must be plain-JSON-serializable (saves) except where noted.
 */
import type { Polyline, Vec2 } from './geometry';
import type { RngState } from './rng';
import type { ScenarioRules } from './scenarioRules';

// ── World / fields ──────────────────────────────────────────────────────────

/** Scalar fields on a coarse grid. Data, not geometry. Row-major, size w*h. */
export interface FieldGrid {
  w: number;
  h: number;
  /** meters per cell */
  cellSize: number;
  /** world-space origin of cell (0,0) corner */
  originX: number;
  originY: number;
  terrain: Float32Array; // elevation 0..1
  water: Uint8Array; // 0|1
  parks: Uint8Array; // 0|1 — green space, unbuildable-by-city, renders as park
  population: Float32Array; // residents per cell
  jobs: Float32Array; // jobs per cell
  landValue: Float32Array; // relative 0..~3
  nimby: Float32Array; // resistance 0..100
}

export type RoadClass = 'arterial' | 'collector' | 'local';

export interface RoadEdge {
  id: number;
  cls: RoadClass;
  polyline: Polyline;
}

/** Demand aggregation unit: a cluster of field cells. */
export interface District {
  id: number;
  name: string;
  centroid: Vec2;
  cellIndices: number[];
  population: number;
  jobs: number;
  /** mean land value, drives NIMBY + fares elasticity later */
  landValue: number;
}

// ── Transit ─────────────────────────────────────────────────────────────────

export type TransitMode = 'bus' | 'tram' | 'metro' | 'rail';
export type TrackGrade = 'surface' | 'elevated' | 'tunnel';

export interface Station {
  id: number;
  name: string;
  pos: Vec2;
  mode: TransitMode;
  level: number; // 1..5
  /** rolling daily boardings, from flow assignment */
  ridership: number;
  /** rolling daily alightings (trips ending here), from flow assignment */
  alightings: number;
  buildTick: number;
}

export interface TrackSegment {
  id: number;
  mode: TransitMode;
  grade: TrackGrade;
  fromStationId: number;
  toStationId: number;
  polyline: Polyline;
  buildCost: number;
}

export interface RouteDef {
  id: number;
  name: string;
  color: string;
  mode: TransitMode;
  /** ordered station ids; consecutive pairs must have a track segment */
  stationIds: number[];
  /** ordered track segment ids, length = stationIds.length - 1 */
  segmentIds: number[];
  headwaySeconds: number;
  fare: number;
  vehicleCount: number;
  /** derived, from assignment */
  dailyRidership: number;
  dailyRevenue: number;
  /** peak-hour capacity, pax/hour/direction (derived from fleet + headway) */
  capacity: number;
  /** peak-hour load, pax/hour (derived from ridership) */
  load: number;
  /** load / capacity; >1 is over capacity. Feeds the crowding penalty (lagged). */
  crowding: number;
  /** derived per-segment daily load, aligned to segmentIds (from assignment) */
  segmentLoads: number[];
}

export interface VehicleState {
  id: number;
  routeId: number;
  /** distance along the route's full polyline (out-and-back path) */
  along: number;
  /** total out-and-back length cached at spawn */
  pathLength: number;
  dwellRemaining: number;
  /** 0..1 crowding, derived from segment flows */
  occupancy: number;
}

// ── Demand / flows ──────────────────────────────────────────────────────────

/** One assigned origin-destination flow over the transit network. */
export interface FlowResult {
  originDistrict: number;
  destDistrict: number;
  /** trips per day choosing transit */
  transitTrips: number;
  /** trips per day choosing car (mode share denominator) */
  carTrips: number;
  /** generalized cost minutes for the transit path */
  transitCost: number;
  /** route ids traversed in order (for agent sampling + revenue attribution) */
  routeIds: number[];
  /** station ids traversed in order: [board, ...transfers..., alight] */
  stationIds: number[];
}

// ── Economy ─────────────────────────────────────────────────────────────────

export interface DayLedger {
  fares: number;
  subsidy: number;
  operations: number;
  maintenance: number;
  interest: number;
}

/** Cumulative since the run began — one entry per closed day summed. Optional so
 *  legacy saves (which never wrote it) deserialize cleanly; it is rebuilt going
 *  forward from the first day that closes after load. */
export interface LifetimeLedger {
  fares: number;
  subsidy: number;
  operations: number;
  maintenance: number;
  interest: number;
  /** number of days accumulated into the totals above */
  days: number;
}

export interface Budget {
  cash: number;
  loanBalance: number;
  loanRate: number; // annual
  /** yesterday's totals for UI */
  lastDay: DayLedger;
  /** rolling net/day history (oldest → newest), capped at 7 */
  netHistory: number[];
  /** cumulative lifetime totals (optional; absent in legacy saves) */
  lifetime?: LifetimeLedger;
}

export interface CityStats {
  population: number;
  jobs: number;
  dailyTransitTrips: number;
  dailyCarTrips: number;
  transitShare: number; // 0..1
  coverage: number; // fraction of population within walk radius of a station
  approval: number; // 0..100
}

// ── Game state ──────────────────────────────────────────────────────────────

export type Difficulty = 'easy' | 'normal' | 'hard';

export interface GameState {
  seed: number;
  tick: number; // 1 tick = 1 game-second
  rngState: RngState;
  difficulty: Difficulty;
  fields: FieldGrid;
  roads: RoadEdge[];
  districts: District[];
  stations: Station[];
  tracks: TrackSegment[];
  routes: RouteDef[];
  vehicles: VehicleState[];
  flows: FlowResult[];
  /** transient: road congestion + bottlenecks, recomputed each assignment (not saved) */
  traffic?: import('./transit/traffic').TrafficField;
  /** transient: OD pairs poorly served by transit, for the unserved-demand overlay (not saved) */
  unserved?: import('./transit/assignment').UnservedDesire[];
  /** transient: ridership heatmap / OD / insight analytics (not saved; presentation only) */
  analytics?: import('./analytics').AnalyticsState;
  /** transient: high-res OSM water/park masks for crisp rendering (real cities only) */
  osmWaterMask?: Uint8Array | undefined;
  osmParkMask?: Uint8Array | undefined;
  osmBuildingMask?: Uint8Array | undefined;
  osmMaskRes?: number | undefined;
  osmLabels?: import('./city/osmCity').MapLabel[] | undefined;
  budget: Budget;
  stats: CityStats;
  /**
   * Transient per-process game instance id (see ./instance.ts). Scopes
   * process-global geometry caches to one game so a prior game in the same
   * process cannot leak into this one. Never serialized; never hashed.
   */
  instanceId: number;
  /** monotonic entity id counter */
  nextId: number;
  /** set when land use / network changed; assignment reruns on next demand pass */
  demandDirty: boolean;
  unlockedModes: TransitMode[];
  /** active city events (festivals, closures, …), saved with the game */
  activeEvents: import('./events').ActiveEvent[];
  /** earliest day a new event may start (enforces spacing between events) */
  nextEventDay: number;
  /** optional scenario constraints (era starts, daily challenge, …) */
  scenarioRules?: ScenarioRules;
  /**
   * Active data-driven scenario (win/lose trees + mid-run events). Optional so
   * free-play / legacy era starts keep working; when set, the scenario engine
   * evaluates each sim-day and hosts mirror `scenarioState` on the UI envelope.
   */
  scenario?: import('./scenario/types').ScenarioDef;
  /** true once the scenario win tree is satisfied */
  scenarioWon?: boolean;
  /** ids of mid-run scenario events that have already fired */
  firedScenarioEvents?: string[];
  /** per-district travel-demand multipliers (scenario events); keys are district ids */
  districtDemandMult?: Record<number, number>;
  /** temporary citywide demand multiplier from a scenario event */
  globalDemandMult?: number;
  /** days remaining on globalDemandMult (ticked down each sim-day) */
  globalDemandMultDaysLeft?: number;
  /** stamped command stream for replay / anti-cheat (also in save) */
  commandLog: { tick: number; cmd: Command }[];
  /** consecutive days at/below approvalFloor */
  lowApprovalDays: number;
  /** why the run ended, if it failed */
  failed: 'bankrupt' | 'approval' | 'time' | 'condition' | null;
}

// ── Commands (the only mutation API) ────────────────────────────────────────

export type Command =
  | { kind: 'buildStation'; mode: TransitMode; pos: Vec2 }
  | { kind: 'buildTrack'; mode: TransitMode; grade: TrackGrade; fromStationId: number; toStationId: number; waypoints: Vec2[] }
  | { kind: 'createRoute'; mode: TransitMode; stationIds: number[] }
  | { kind: 'editRoute'; routeId: number; headwaySeconds?: number; fare?: number; vehicleCount?: number; name?: string; color?: string }
  | { kind: 'deleteRoute'; routeId: number }
  | { kind: 'demolishStation'; stationId: number }
  | { kind: 'demolishTrack'; trackId: number }
  | { kind: 'upgradeStation'; stationId: number }
  | { kind: 'takeLoan'; amount: number }
  | { kind: 'repayLoan'; amount: number }
  | { kind: 'renameStation'; stationId: number; name: string };

export interface CommandResult {
  ok: boolean;
  error?: string;
  /** id of a created entity, when applicable */
  createdId?: number;
}
