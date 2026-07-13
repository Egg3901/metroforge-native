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
  /** grade-separation level (signed; 0 = ground, +N up, -N below). Static
   *  presentation data from OSM bridge/tunnel/layer tags; not part of
   *  stateHash. Absent = 0. */
  gradeLevel?: number;
  /** segment is an OSM bridge (deck). Absent = false. */
  isBridge?: boolean;
  /** segment is an OSM tunnel. Absent = false. */
  isTunnel?: boolean;
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
  /** underground depth (m below surface), set when a tunnel connects here;
   *  undefined = surface station. Drives build surcharge + rider access time. */
  depth?: number | undefined;
}

/** Additive cost breakdown returned alongside a track-cost quote (v0.8). All
 *  components are money; `strata` is a human summary of the ground crossed. */
export interface TrackCostBreakdown {
  /** cost of the equivalent surface alignment (reference for the UI) */
  surface: number;
  /** cost of the equivalent elevated alignment (reference for the UI) */
  elevated: number;
  /** total cut-and-cover component actually chosen along the line */
  cutCover: number;
  /** total bored component actually chosen along the line */
  bored: number;
  /** dominant strata crossed, e.g. "fill/clay/rock" */
  strata: string;
  /** does any part of the alignment sit below the water table? */
  belowWaterTable: boolean;
}

export interface TrackSegment {
  id: number;
  mode: TransitMode;
  grade: TrackGrade;
  fromStationId: number;
  toStationId: number;
  polyline: Polyline;
  buildCost: number;
  /**
   * Cached corridor density (land value → [0,1]) at the segment midpoint, for
   * the grade congestion model (Egg3901/metroforge#38). Set on build and
   * refreshed each assignment (same cadence/lag as route.surfaceExposure), so
   * the per-tick movement loop reads a number instead of resampling the field.
   */
  congestionDensity?: number;
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
  /**
   * Fraction of the route NOT in tunnel (0 = fully underground, 1 = fully
   * surface/elevated), derived from its track grades each assignment. Weather
   * speed penalties scale with this, so grade-separated lines shrug off snow.
   *
   * (see gradeProfile below for the grade-congestion movement cache.)
   * Optional/derived; absent on legacy saves (treated as 1 = fully exposed).
   */
  surfaceExposure?: number;
  /**
   * Length-weighted day-average grade-effective running speed (m/s), refreshed
   * each assignment (Egg3901/metroforge#38). Surface segments in dense corridors
   * pull it below mode cruise; elevated/tunnel keep cruise. The per-tick vehicle
   * loop just reads this, so grade adds no per-tick cost. Day-average matches the
   * cycle-time → headway model; the diurnal (rush) sharpness of the tradeoff
   * lives in the peak-biased assignment ride edges. Derived — absent on legacy
   * saves (falls back to mode cruise until the next assignment).
   */
  moveGradeSpeed?: number;

  // ── v0.9 System A (Operations) — all optional/additive, absent on legacy
  //    saves (treated as unset until the first ops tick refreshes them) ──
  /**
   * Per-period target headway (seconds) for the frequency schedule (A1). More
   * service (a shorter target) draws more riders but needs more vehicles and
   * costs more to run. Absent = the default profile from ops/tunables. Keyed by
   * Period ('amPeak' | 'midday' | 'pmPeak' | 'evening' | 'night').
   */
  frequency?: Partial<Record<import('./ops/periods').Period, number>>;
  /** The command-set base headway (frozen by deriveHeadway at create/edit), the
   *  neutral value ops restores when the route runs at full availability with no
   *  period throttle. Ops only degrades headwaySeconds BELOW this when breakdowns
   *  or maintenance cut availability, or a period frequency cap applies — so a
   *  healthy, unthrottled route behaves exactly like the pre-v0.9 model. */
  scheduledHeadway?: number;
  /** In-service unit count RIGHT NOW: assigned fleet minus units down for
   *  breakdown/maintenance, capped by the current period's target. Derived each
   *  ops tick; drives the effective headway written into headwaySeconds. */
  inServiceVehicles?: number;
  /** Rolling on-time fraction 0..1 for the route (A5 keystone). */
  onTimePct?: number;
  /** Rolling average delay per departure, seconds (A5 keystone). */
  avgDelaySec?: number;
  /** Lagged reliability→ridership multiplier (0..1); reliable service keeps its
   *  riders, chronic delays shed them. Applied AFTER assignment so it never
   *  edits the demand pipeline the parallel lane owns. */
  reliabilityDemandMult?: number;
}

// ── v0.9 System A (Operations) ────────────────────────────────────────────────

/** One discrete rolling-stock unit: an individual vehicle with age + condition
 *  that can be bought, retired, assigned to a route, break down, and be sent for
 *  maintenance. Distinct from `VehicleState` (the transient render/movement
 *  marker); the fleet is the persistent ledger. */
export interface FleetUnit {
  id: number;
  mode: TransitMode;
  /** route this unit is assigned to; null = idle in the pool / depot. */
  routeId: number | null;
  /** age in sim-days (drives slow condition floor + resale value later). */
  ageDays: number;
  /** health 0..1; decays with distance run and weather exposure. */
  condition: number;
  /** operational state. 'active' runs service; the others are out of service. */
  status: 'active' | 'maintenance' | 'brokenDown';
  /** ticks remaining in the current non-active status (0 when active). */
  statusTicksLeft: number;
}

/** A depot / maintenance facility for one mode. Placeable (buildDepot command);
 *  the renderer draws it later — this is the sim entity + a wire flag. Its
 *  presence enables maintenance windows for that mode. */
export interface Depot {
  id: number;
  mode: TransitMode;
  pos: Vec2;
  buildTick: number;
}

/** An active breakdown incident: a disabled unit blocking a route segment for a
 *  duration, cascading delay to following vehicles. */
export interface BreakdownIncident {
  id: number;
  routeId: number;
  unitId: number;
  /** index into the route's segmentIds that is blocked. */
  segmentIndex: number;
  /** ticks remaining until the blockage clears. */
  ticksLeft: number;
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
  /**
   * City preset key (e.g. 'nyc', 'seattle'), used to select the seeded weather
   * climate profile. Persisted so a loaded save keeps its city's weather.
   * Absent on pre-weather saves → the generic temperate climate is used.
   */
  cityKey?: string | undefined;
  /**
   * Current sky, a pure deterministic function of (seed, tick, cityKey). Cached
   * here for the assignment/vehicle/economy hooks and the UI; recomputed each
   * game-hour. TRANSIENT (recomputed on load, never serialized), so it needs no
   * save migration and never enters the determinism hash.
   */
  weather?: import('./weather').WeatherSnapshot | undefined;
  /** last tick's headline weather event, for begin/end toasts (transient) */
  lastWeatherEvent?: import('./weather').WeatherEvent | null | undefined;
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
  /** transient: real-elevation heightfield (meters, row-major, elevRes²) for
   *  the dedicated static elevation channel; real cities only (not saved) */
  osmElevation?: Int16Array | undefined;
  osmElevRes?: number | undefined;
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
  /** consecutive days below the bankruptcy cash floor (grace-period counter).
   *  Instance-scoped (was a module global) so warm processes don't leak it
   *  across games. Not part of stateHash. */
  bankruptDays: number;
  /** why the run ended, if it failed */
  failed: 'bankrupt' | 'approval' | 'time' | 'condition' | null;

  // ── v0.9 System A (Operations) — optional/additive; absent on legacy saves,
  //    lazily initialized on first ops tick / load migration ──
  /** discrete rolling-stock ledger (aged/condition units). */
  fleet?: FleetUnit[];
  /** placed maintenance depots (one-per-mode enforced in the command). */
  depots?: Depot[];
  /** active breakdown incidents blocking segments. */
  incidents?: BreakdownIncident[];
  /** dedicated seeded RNG stream for ops (breakdown rolls), kept separate from
   *  the events/growth stream so ops randomness can't reorder other systems. */
  opsRngState?: RngState;
  /** the service period the last ops tick resolved; a change triggers an
   *  effective-headway recompute (and a demand refresh). */
  opsPeriod?: import('./ops/periods').Period;
  /** per-route daily reliability accumulators (reset each day close). */
  opsDaily?: Record<number, { departures: number; delayedDepartures: number; delaySec: number }>;
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
  | { kind: 'renameStation'; stationId: number; name: string }
  // ── v0.9 System A (Operations) ──
  | { kind: 'setRouteFrequency'; routeId: number; period: import('./ops/periods').Period; headwaySeconds: number }
  | { kind: 'buildDepot'; mode: TransitMode; pos: Vec2 };

export interface CommandResult {
  ok: boolean;
  error?: string;
  /** id of a created entity, when applicable */
  createdId?: number;
}
