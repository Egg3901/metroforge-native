/**
 * Host ↔ sim message protocol. This boundary is deliberately shaped like the
 * FFI surface a native core would expose: JSON control messages + typed-array
 * render snapshots.
 */
import type { Command, CommandResult, Difficulty, GameState, TransitMode } from '@core/types';

export interface UiStation {
  id: number;
  name: string;
  x: number;
  y: number;
  mode: TransitMode;
  level: number;
  ridership: number;
  alightings: number;
}

export interface UiTrack {
  id: number;
  mode: TransitMode;
  grade: string;
  points: number[]; // flat x,y pairs
  fromStationId: number;
  toStationId: number;
}

export interface UiRoute {
  id: number;
  name: string;
  color: string;
  mode: TransitMode;
  stationIds: number[];
  headwaySeconds: number;
  fare: number;
  vehicleCount: number;
  dailyRidership: number;
  dailyRevenue: number;
  lengthMeters: number;
  capacity: number;
  load: number;
  crowding: number;
  segmentLoads: number[];
  // ── optional, additive (v0.4.2 sim-depth); older clients ignore these ──
  /** daily fleet running cost (operations + maintenance) */
  operatingCost?: number;
  /** dailyRevenue / operatingCost; >1 = line covers its own running cost */
  farebox?: number;
  /** crowding scaled by the current time-of-day factor (fullness right now) */
  liveCrowding?: number;
}

export interface UiState {
  tick: number;
  insights: string[];
  day: number;
  speed: number;
  cash: number;
  loanBalance: number;
  lastDay: GameState['budget']['lastDay'];
  /** rolling net/day (oldest → newest), up to 7 entries */
  netHistory: number[];
  population: number;
  approval: number;
  transitShare: number;
  coverage: number;
  dailyTransitTrips: number;
  unlockedModes: TransitMode[];
  stations: UiStation[];
  tracks: UiTrack[];
  routes: UiRoute[];
  /** active city events (name + days remaining) */
  activeEvents: { id: string; name: string; daysLeft: number }[];
  /** bumped when land-use fields changed (renderer re-bakes) */
  fieldsVersion: number;
  bankrupt: boolean;
  /** non-playing terminal reason, if any (additive: 'condition' is scenario-engine) */
  failed: 'bankrupt' | 'approval' | 'time' | 'condition' | null;
  /** scenario calendar limit, if any */
  maxDay: number | null;
  /** era label for HUD, if any */
  eraLabel: string | null;
  /** command count recorded this run (for replay submit) */
  commandCount: number;
  // ── optional, additive (v0.4.2 sim-depth); older clients ignore these ──
  /** hour of the game day in [0,24) */
  hourOfDay?: number;
  /** live time-of-day demand multiplier (daily mean = 1.0) */
  demandFactor?: number;
  /** farebox recovery for yesterday's ledger (fares / running costs) */
  fareboxRecovery?: number;
  /** per-district catchment population + jobs (building-derived), world coords */
  districts?: { id: number; name: string; x: number; y: number; population: number; jobs: number }[];
  /** count of routes over capacity (crowding > 1) */
  overcrowdedRoutes?: number;
  /** cumulative lifetime ledger, once at least one day has closed */
  lifetime?: import('@core/types').LifetimeLedger;
  /**
   * Data-driven scenario progress (objectives / deadline / won|lost). Additive —
   * the Rust native client may ignore this field and keep working unchanged.
   */
  scenarioState?: import('@core/scenario').ScenarioState;
  /**
   * Completing X unlocks Y — full progression manifest. Additive; older clients
   * ignore unknown fields.
   */
  scenarioProgression?: import('@core/scenario').ScenarioProgressionManifest;
  /**
   * Spatial analytics insights (underserved district, overloaded corridor,
   * network efficiency, 400m catchment). Additive — older clients ignore.
   */
  analytics?: import('@core/analytics').AnalyticsInsights;
}

export interface StaticCity {
  fieldW: number;
  fieldH: number;
  cellSize: number;
  originX: number;
  originY: number;
  worldSize: number;
  /** road-width multiplier — dense real-city grids draw much thinner than the
   *  sparse procedural network the default widths were tuned for */
  roadScale: number;
  /** high-res water/park masks (1=set) over the world square, for crisp
   *  real-city coastline/park rendering; absent for procedural cities */
  waterMask?: Uint8Array | undefined;
  parkMask?: Uint8Array | undefined;
  buildingMask?: Uint8Array | undefined;
  maskRes?: number | undefined;
  labels?: import('@core/city/osmCity').MapLabel[] | undefined;
  roads: {
    cls: string;
    points: number[];
    /** grade-separation level (signed int; 0 = ground). */
    gradeLevel?: number;
    /** segment is a bridge deck. */
    isBridge?: boolean;
    /** segment is a tunnel. */
    isTunnel?: boolean;
  }[];
}

export interface FieldsPayload {
  version: number;
  terrain: Float32Array;
  water: Uint8Array;
  parks: Uint8Array;
  population: Float32Array;
  jobs: Float32Array;
  landValue: Float32Array;
}

/** vehicles: stride 6 = [id, x, y, heading, occupancy, routeColorIndex] */
/** agents: stride 3 = [x, y, phase(0 walk,1 ride,2 wait)] */
export interface FrameSnapshot {
  tick: number;
  vehicles: Float32Array;
  vehicleCount: number;
  agents: Float32Array;
  agentCount: number;
  routeColorOf: Record<number, string>;
}

export type ToSim =
  | {
      type: 'init';
      seed: number;
      difficulty: Difficulty;
      size?: 'small' | 'medium' | 'large' | undefined;
      presetKey?: string | undefined;
      rules?: import('@core/scenarioRules').ScenarioRules | undefined;
      /** data-driven scenario id from PLAYABLE_SCENARIOS (additive) */
      scenarioId?: string | undefined;
      /** inline scenario def (additive; overrides scenarioId when both set) */
      scenario?: import('@core/scenario').ScenarioDef | undefined;
    }
  | { type: 'loadSave'; json: string }
  | { type: 'requestSave' }
  | { type: 'setSpeed'; speed: number }
  | { type: 'command'; requestId: number; cmd: Command }
  | { type: 'queryTrackCost'; requestId: number; mode: TransitMode; grade: 'surface' | 'elevated' | 'tunnel'; points: { x: number; y: number }[] }
  | { type: 'requestReplay' };

export interface TrafficPayload {
  w: number;
  h: number;
  cellSize: number;
  originX: number;
  originY: number;
  values: Float32Array; // per-cell congestion 0..1
  hotspots: { x: number; y: number; severity: number }[];
}

/** Unserved-demand desire lines: OD pairs that mostly drive because transit
 *  serves them poorly. Coords are world-space; weight ∝ car trips. */
export interface DemandPayload {
  lines: { x1: number; y1: number; x2: number; y2: number; weight: number; share: number }[];
  maxWeight: number;
}

export type FromSim =
  | { type: 'ready'; staticCity: StaticCity }
  | { type: 'fields'; payload: FieldsPayload }
  | { type: 'traffic'; payload: TrafficPayload }
  | { type: 'demand'; payload: DemandPayload }
  | { type: 'heatmap'; payload: import('@core/analytics').HeatmapPayload }
  | { type: 'frame'; snapshot: FrameSnapshot }
  | { type: 'ui'; ui: UiState }
  | { type: 'commandResult'; requestId: number; result: CommandResult }
  | { type: 'trackCost'; requestId: number; cost: number }
  | { type: 'saved'; json: string }
  | { type: 'replay'; payload: ReplayPayload }
  | { type: 'toast'; message: string; tone: 'info' | 'warn' | 'good' };

export interface ReplayPayload {
  seed: number;
  difficulty: Difficulty;
  presetKey?: string;
  size?: 'small' | 'medium' | 'large';
  rules?: import('@core/scenarioRules').ScenarioRules;
  commandLog: { tick: number; cmd: Command }[];
  finalTick: number;
  stateHash: number;
  scoreHint: number;
}
