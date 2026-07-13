/**
 * Versioned save format. Plain JSON of core state only. A native client
 * implements this exact schema; bump SAVE_VERSION + add a migration for any
 * breaking change.
 */
import Ajv from 'ajv';
import { fieldsFromJSON, fieldsToJSON } from './fields';
import { makePolyline } from './geometry';
import { nextInstanceId } from './instance';
import { initOps } from './ops';
import { climateTable, weatherAt } from './weather';
import type { GameState, RoadEdge, TrackSegment } from './types';

/** v3 (v0.9 System A): adds the operations sub-state (fleet ledger, depots,
 *  breakdown incidents, dedicated ops RNG stream, per-route frequency +
 *  reliability). All additive; legacy v1/v2 saves load and have their ops
 *  sub-state seeded by `initOps` on deserialize (SaveMeta-style migration). */
export const SAVE_VERSION = 3;

/**
 * Runtime schema for the untrusted save envelope. Deserialization used to cast
 * parsed JSON straight to `GameState` (`as`), so a malformed-but-parseable save
 * (missing `stations`, a string `budget`, …) slipped past load and only blew up
 * later inside `simTick` with a cryptic error. Validating the shape here turns
 * that into a clean, catchable "corrupt save" rejection at the load boundary —
 * callers (sidecar/worker) already surface it as a warn toast and keep the prior
 * state. `additionalProperties` stays open for forward-compat with newer saves.
 */
const ajv = new Ajv({ allErrors: false, allowUnionTypes: true });
const saveSchema = {
  type: 'object',
  required: ['version', 'state'],
  properties: {
    version: { type: 'number' },
    bankruptDays: { type: 'number' },
    state: {
      type: 'object',
      required: ['seed', 'tick', 'budget', 'stats', 'fields', 'roads', 'districts', 'stations', 'tracks', 'routes', 'vehicles'],
      properties: {
        seed: { type: 'number' },
        tick: { type: 'number' },
        budget: { type: 'object', required: ['cash', 'loanBalance', 'loanRate'] },
        stats: { type: 'object' },
        fields: { type: 'object' },
        roads: { type: 'array' },
        districts: { type: 'array' },
        stations: { type: 'array' },
        tracks: { type: 'array' },
        routes: { type: 'array' },
        vehicles: { type: 'array' },
      },
    },
  },
} as const;
const validateSave = ajv.compile(saveSchema);

export function serialize(state: GameState): string {
  // transient fields (recomputed / reloaded), never serialized
  const {
    traffic: _t,
    unserved: _u,
    analytics: _a,
    osmWaterMask: _w,
    osmParkMask: _p,
    osmBuildingMask: _b,
    osmMaskRes: _r,
    osmElevation: _e,
    osmElevRes: _er,
    osmLabels: _l,
    instanceId: _i,
    // weather is a pure fn of (seed, tick, cityKey); recomputed on load, never
    // serialized, so it needs no migration and can't drift. cityKey DOES persist
    // (it rides in ...persist) so the loaded save keeps its city's climate.
    weather: _wx,
    lastWeatherEvent: _we,
    ...persist
  } = state;
  return JSON.stringify({
    version: SAVE_VERSION,
    // kept top-level for backward-compat with older/native readers; also lives
    // inside `state` now (instance-scoped field), and deserialize prefers that.
    bankruptDays: state.bankruptDays,
    state: {
      ...persist,
      fields: fieldsToJSON(state.fields),
      // polylines: store points only; cumulative lengths rebuilt on load
      roads: state.roads.map((r) => ({
        id: r.id,
        cls: r.cls,
        points: r.polyline.points,
        ...(r.gradeLevel ? { gradeLevel: r.gradeLevel } : {}),
        ...(r.isBridge ? { isBridge: true as const } : {}),
        ...(r.isTunnel ? { isTunnel: true as const } : {}),
      })),
      tracks: state.tracks.map((t) => ({
        id: t.id,
        mode: t.mode,
        grade: t.grade,
        fromStationId: t.fromStationId,
        toStationId: t.toStationId,
        buildCost: t.buildCost,
        points: t.polyline.points,
      })),
    },
  });
}

export function deserialize(json: string): GameState {
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    throw new Error('corrupt save: not valid JSON');
  }
  if (!validateSave(parsed)) {
    const e = validateSave.errors?.[0];
    const where = e?.instancePath || '(root)';
    throw new Error(`corrupt save: ${where} ${e?.message ?? 'failed schema validation'}`);
  }
  const raw = parsed as { version: number; bankruptDays?: number; state: Record<string, unknown> };
  if (raw.version < 1 || raw.version > SAVE_VERSION) {
    throw new Error(`Unsupported save version ${raw.version} (expected 1..${SAVE_VERSION})`);
  }
  const s = raw.state as unknown as Omit<GameState, 'fields' | 'roads' | 'tracks'> & {
    fields: Parameters<typeof fieldsFromJSON>[0];
    roads: {
      id: number;
      cls: RoadEdge['cls'];
      points: { x: number; y: number }[];
      gradeLevel?: number;
      isBridge?: boolean;
      isTunnel?: boolean;
    }[];
    tracks: (Omit<TrackSegment, 'polyline'> & { points: { x: number; y: number }[] })[];
  };
  const restored: GameState = {
    ...s,
    instanceId: nextInstanceId(),
    activeEvents: s.activeEvents ?? [],
    nextEventDay: s.nextEventDay ?? 8,
    commandLog: s.commandLog ?? [],
    lowApprovalDays: s.lowApprovalDays ?? 0,
    // instance-scoped now: prefer the in-state field, fall back to the legacy
    // top-level `bankruptDays`, then 0 for saves predating either.
    bankruptDays: s.bankruptDays ?? raw.bankruptDays ?? 0,
    failed: s.failed ?? null,
    fields: fieldsFromJSON(s.fields),
    roads: s.roads.map((r) => {
      const edge: RoadEdge = { id: r.id, cls: r.cls, polyline: makePolyline(r.points) };
      if (r.gradeLevel) edge.gradeLevel = r.gradeLevel;
      if (r.isBridge) edge.isBridge = true;
      if (r.isTunnel) edge.isTunnel = true;
      return edge;
    }),
    tracks: s.tracks.map((t) => ({
      id: t.id,
      mode: t.mode,
      grade: t.grade,
      fromStationId: t.fromStationId,
      toStationId: t.toStationId,
      buildCost: t.buildCost,
      polyline: makePolyline(t.points),
    })),
  };
  if (s.scenarioRules) restored.scenarioRules = s.scenarioRules;
  if (s.scenario) restored.scenario = s.scenario;
  if (s.scenarioWon !== undefined) restored.scenarioWon = s.scenarioWon;
  if (s.firedScenarioEvents) restored.firedScenarioEvents = s.firedScenarioEvents;
  if (s.districtDemandMult) restored.districtDemandMult = s.districtDemandMult;
  if (s.globalDemandMult !== undefined) restored.globalDemandMult = s.globalDemandMult;
  if (s.globalDemandMultDaysLeft !== undefined) restored.globalDemandMultDaysLeft = s.globalDemandMultDaysLeft;
  // migrate older saves missing rolling cash-flow history
  if (!restored.budget.netHistory) restored.budget.netHistory = [];
  // v0.9 System A migration: seed (or reconcile) the ops sub-state. Idempotent —
  // a v3 save already carries fleet/depots/incidents; a v1/v2 save gets them
  // built here and its fleet reconciled to each route's vehicleCount.
  initOps(restored);
  // recompute the (transient) sky from seed+tick+city so a just-loaded save has
  // weather before its first tick. Pure fn ⇒ identical to a never-saved run.
  restored.weather = weatherAt(restored.seed, restored.tick, climateTable(restored.cityKey));
  restored.lastWeatherEvent = restored.weather.event ?? null;
  return restored;
}

/** Cheap deterministic state fingerprint for replay verification / port acceptance. */
export function stateHash(state: GameState): number {
  let h = 2166136261 >>> 0;
  const mix = (v: number): void => {
    // hash the float's rounded micro-units to tolerate JSON round-trips, not FP drift
    const x = Math.round(v * 1000);
    h = Math.imul(h ^ (x & 0xffff), 16777619) >>> 0;
    h = Math.imul(h ^ ((x >> 16) & 0xffff), 16777619) >>> 0;
  };
  mix(state.tick);
  mix(state.budget.cash);
  mix(state.stats.population);
  mix(state.stations.length);
  mix(state.tracks.length);
  mix(state.routes.length);
  for (const r of state.routes) {
    mix(r.dailyRidership);
    mix(r.vehicleCount);
    // v0.9 ops: reliability is part of the deterministic state. Hashing it means
    // ops legitimately changes hashes vs the pre-v0.9 base, while same-seed
    // reproducibility (the real invariant) is preserved.
    mix(r.onTimePct ?? 1);
  }
  for (const v of state.vehicles) mix(v.along);
  // v0.9 ops: fleet condition + status and active incident count.
  for (const u of state.fleet ?? []) {
    mix(u.condition);
    mix(u.status === 'active' ? 0 : u.status === 'maintenance' ? 1 : 2);
  }
  mix((state.incidents ?? []).length);
  return h;
}
