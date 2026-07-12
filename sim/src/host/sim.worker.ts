/// <reference lib="webworker" />
/**
 * Sim worker: owns the GameState, advances fixed-timestep ticks, and streams
 * render snapshots + UI state to the main thread. The renderer never touches
 * the sim directly.
 */
import { TICKS_PER_DAY, WORLD_SIZE } from '@core/constants';
import { applyCommand, trackCostDetailed } from '@core/commands';
import { columnAt } from '@core/geology';
import { pointAlong } from '@core/geometry';
import { newGame } from '@core/newGame';
import { loadOsmCity } from '@core/city/osmRegistry';
import { EVENT_DEFS } from '@core/events';
import { deserialize, serialize, stateHash } from '@core/save';
import { simTick } from '@core/sim';
import { getRoutePath } from '@core/transit/routePath';
import type { GameState } from '@core/types';
import type { ScenarioRules } from '@core/scenarioRules';
import { playableScenario, type ScenarioDef } from '@core/scenario';
import { AgentPool } from './agents';
import type { FromSim, ToSim, UiState } from './protocol';
import { routeExtras, todFactorOf, uiExtras } from './uiExtras';
import { analyticsInsightLines, buildDemandOverlay } from '@core/analytics';
import type { HeatmapPayload } from '@core/analytics';

let state: GameState | null = null;
let speed = 1; // game-seconds per real second (1x = 1); UI offers 1/10/30/120
let fieldsVersion = 1;
let bankrupt = false;
let won = false;
let initMeta: { presetKey?: string; size?: 'small' | 'medium' | 'large' } = {};
const agents = new AgentPool();
let lastFlowsRef: unknown = null;

function resolveScenario(msg: Extract<ToSim, { type: 'init' }>): ScenarioDef | undefined {
  if (msg.scenario) return msg.scenario;
  if (msg.scenarioId) return playableScenario(msg.scenarioId);
  return undefined;
}

const post = (msg: FromSim, transfer?: Transferable[]): void => {
  (self as unknown as Worker).postMessage(msg, transfer ?? []);
};

function sendStatic(s: GameState): void {
  post({
    type: 'ready',
    staticCity: {
      fieldW: s.fields.w,
      fieldH: s.fields.h,
      cellSize: s.fields.cellSize,
      originX: s.fields.originX,
      originY: s.fields.originY,
      worldSize: s.fields.w * s.fields.cellSize,
      // dense real-city imports have ~5-10k roads; thin them right down
      roadScale: s.roads.length > 3000 ? 0.28 : s.roads.length > 1500 ? 0.5 : 1,
      waterMask: s.osmWaterMask,
      parkMask: s.osmParkMask,
      buildingMask: s.osmBuildingMask,
      maskRes: s.osmMaskRes,
      labels: s.osmLabels,
      roads: s.roads.map((r) => ({
        cls: r.cls,
        points: r.polyline.points.flatMap((p) => [p.x, p.y]),
        gradeLevel: r.gradeLevel ?? 0,
        isBridge: r.isBridge ?? false,
        isTunnel: r.isTunnel ?? false,
      })),
    },
  });
  sendFields(s);
}

function sendFields(s: GameState): void {
  post({
    type: 'fields',
    payload: {
      version: fieldsVersion,
      terrain: Float32Array.from(s.fields.terrain),
      water: Uint8Array.from(s.fields.water),
      parks: Uint8Array.from(s.fields.parks),
      population: Float32Array.from(s.fields.population),
      jobs: Float32Array.from(s.fields.jobs),
      landValue: Float32Array.from(s.fields.landValue),
    },
  });
}

/** Plain-language "why is it like this" cues derived from current state, so a
 *  player can read the network's health without inferring it from raw numbers. */
function computeInsights(s: GameState): string[] {
  const out: string[] = [];
  const packed = s.routes.filter((r) => r.crowding > 1).sort((a, b) => b.crowding - a.crowding);
  if (packed.length > 0) {
    const r = packed[0]!;
    out.push(`${r.name} is over capacity (${Math.round(r.crowding * 100)}%) and turning riders away. Add vehicles.`);
  }
  const ld = s.budget.lastDay;
  if (ld.fares > 0 && ld.fares < ld.operations + ld.maintenance) {
    out.push(`Fares cover only ${Math.round((ld.fares / (ld.operations + ld.maintenance)) * 100)}% of running costs.`);
  }
  if (s.stats.coverage < 0.35 && s.stations.length > 0) {
    out.push(`Only ${Math.round(s.stats.coverage * 100)}% of residents live near a stop. Extend your reach.`);
  }
  const gap = (s.unserved ?? [])[0];
  if (gap && s.stats.transitShare < 0.4) {
    out.push('Big travel demand is still driving. Check the Gaps overlay for where to build next.');
  }
  if (s.analytics) out.push(...analyticsInsightLines(s.analytics.insights, 2));
  for (const a of s.activeEvents) {
    const d = EVENT_DEFS.find((e) => e.id === a.id);
    if (d) out.push(`${d.name}: ${d.desc}`);
  }
  return out.slice(0, 4);
}

function buildUi(s: GameState): UiState {
  const tod = todFactorOf(s);
  return {
    ...uiExtras(s),
    tick: s.tick,
    insights: computeInsights(s),
    day: Math.floor(s.tick / TICKS_PER_DAY) + 1,
    speed,
    cash: s.budget.cash,
    loanBalance: s.budget.loanBalance,
    lastDay: s.budget.lastDay,
    netHistory: s.budget.netHistory ? [...s.budget.netHistory] : [],
    population: s.stats.population,
    approval: s.stats.approval,
    transitShare: s.stats.transitShare,
    coverage: s.stats.coverage,
    dailyTransitTrips: s.stats.dailyTransitTrips,
    unlockedModes: [...s.unlockedModes],
    stations: s.stations.map((st) => ({
      id: st.id,
      name: st.name,
      x: st.pos.x,
      y: st.pos.y,
      mode: st.mode,
      level: st.level,
      ridership: st.ridership,
      alightings: st.alightings ?? 0,
    })),
    tracks: s.tracks.map((t) => ({
      id: t.id,
      mode: t.mode,
      grade: t.grade,
      points: t.polyline.points.flatMap((p) => [p.x, p.y]),
      fromStationId: t.fromStationId,
      toStationId: t.toStationId,
    })),
    routes: s.routes.map((r) => {
      const path = getRoutePath(s, r);
      return {
        id: r.id,
        name: r.name,
        color: r.color,
        mode: r.mode,
        stationIds: [...r.stationIds],
        headwaySeconds: r.headwaySeconds,
        fare: r.fare,
        vehicleCount: r.vehicleCount,
        dailyRidership: r.dailyRidership,
        dailyRevenue: r.dailyRevenue,
        lengthMeters: path ? path.length / 2 : 0,
        capacity: r.capacity ?? 0,
        load: r.load ?? 0,
        crowding: r.crowding ?? 0,
        segmentLoads: r.segmentLoads ? [...r.segmentLoads] : [],
        ...routeExtras(r, tod),
      };
    }),
    activeEvents: s.activeEvents.map((a) => ({ id: a.id, name: EVENT_DEFS.find((e) => e.id === a.id)?.name ?? a.id, daysLeft: a.daysLeft })),
    fieldsVersion,
    bankrupt: bankrupt || s.failed === 'bankrupt',
    failed: s.failed,
    maxDay: s.scenarioRules?.maxDay ?? null,
    eraLabel: s.scenarioRules?.eraLabel ?? null,
    commandCount: s.commandLog.length,
  };
}

function sendTraffic(s: GameState): void {
  const t = s.traffic;
  if (!t) return;
  const values = Float32Array.from(t.values);
  post(
    {
      type: 'traffic',
      payload: {
        w: t.w,
        h: t.h,
        cellSize: t.cellSize,
        originX: t.originX,
        originY: t.originY,
        values,
        hotspots: t.hotspots.map((h) => ({ x: h.x, y: h.y, severity: h.severity })),
      },
    },
    [values.buffer],
  );
}

function sendDemand(s: GameState): void {
  // Overlay is built from the station-independent baseline gravity field
  // (analytics layer), not `s.unserved`, so demand/gaps show everywhere demand
  // exists — not only near stations the assignment router enumerated (#20).
  const lines = buildDemandOverlay(s);
  let maxWeight = 0;
  for (const l of lines) if (l.weight > maxWeight) maxWeight = l.weight;
  post({ type: 'demand', payload: { lines: lines.map((l) => ({ ...l })), maxWeight } });
}

function sendHeatmap(payload: HeatmapPayload): void {
  const cells = Uint8Array.from(payload.cells);
  post(
    {
      type: 'heatmap',
      payload: {
        w: payload.w,
        h: payload.h,
        cellSize: payload.cellSize,
        originX: payload.originX,
        originY: payload.originY,
        maxValue: payload.maxValue,
        day: payload.day,
        cells,
      },
    },
    [cells.buffer],
  );
}

function sendFrame(s: GameState): void {
  const routeColorOf: Record<number, string> = {};
  const buf = new Float32Array(s.vehicles.length * 6);
  let n = 0;
  s.routes.forEach((r, i) => {
    routeColorOf[i] = r.color;
  });
  const routeIndex = new Map(s.routes.map((r, i) => [r.id, i]));
  for (const v of s.vehicles) {
    const route = s.routes.find((r) => r.id === v.routeId);
    if (!route) continue;
    const path = getRoutePath(s, route);
    if (!path) continue;
    const { pos, heading } = pointAlong(path, v.along);
    buf[n * 6] = v.id;
    buf[n * 6 + 1] = pos.x;
    buf[n * 6 + 2] = pos.y;
    buf[n * 6 + 3] = heading;
    buf[n * 6 + 4] = v.occupancy;
    buf[n * 6 + 5] = routeIndex.get(v.routeId) ?? 0;
    n++;
  }
  const agentBuf = agents.buffer.slice(0, agents.count * 3);
  post(
    {
      type: 'frame',
      snapshot: {
        tick: s.tick,
        vehicles: buf,
        vehicleCount: n,
        agents: agentBuf,
        agentCount: agents.count,
        routeColorOf,
      },
    },
    [buf.buffer, agentBuf.buffer],
  );
}

// ── Main loop: 20 host steps/sec; each step advances `speed/20` game-seconds ──
let accumulator = 0;
let uiCountdown = 0;
setInterval(() => {
  if (!state || bankrupt || state.failed || won || state.scenarioWon) return;
  accumulator += speed / 20;
  let ticksRun = 0;
  while (accumulator >= 1 && ticksRun < 400) {
    const events = simTick(state);
    accumulator -= 1;
    ticksRun++;
    for (const m of events.messages) post({ type: 'toast', message: m, tone: 'info' });
    for (const t of events.toasts ?? []) post({ type: 'toast', message: t.message, tone: t.tone });
    if (events.modeUnlocked) post({ type: 'toast', message: `${events.modeUnlocked} unlocked!`, tone: 'good' });
    if (events.won) {
      won = true;
      post({ type: 'ui', ui: buildUi(state) });
    }
    if (events.bankrupt || events.failed) {
      bankrupt = events.bankrupt === true;
      const reason = events.bankrupt ? 'bankrupt' : events.failed;
      const copy =
        reason === 'approval'
          ? 'Approval collapsed — the board has fired you.'
          : reason === 'time'
            ? 'Time is up — the objective was not met.'
            : reason === 'condition'
              ? 'Scenario failed — a lose condition was met.'
              : 'Bankruptcy — the city has taken over your transit authority.';
      post({ type: 'toast', message: copy, tone: 'warn' });
      post({ type: 'ui', ui: buildUi(state) });
    }
    if (events.dayCompleted !== undefined && events.dayCompleted % 7 === 0) {
      fieldsVersion++;
      sendFields(state);
    }
    if (events.heatmap) sendHeatmap(events.heatmap);
  }
  if (state.flows !== lastFlowsRef) {
    lastFlowsRef = state.flows;
    agents.resample(state);
    sendTraffic(state); // congestion recomputed with the flows
    sendDemand(state); // unserved-demand desire lines, same cadence
  }
  agents.update(speed / 20);
  sendFrame(state);
  if (--uiCountdown <= 0) {
    uiCountdown = 10; // UI state at 2 Hz
    post({ type: 'ui', ui: buildUi(state) });
  }
}, 50);

self.onmessage = (e: MessageEvent<ToSim>) => {
  const msg = e.data;
  switch (msg.type) {
    case 'init':
      // real-city presets load their OSM bundle before generating
      initMeta = {};
      if (msg.presetKey !== undefined) initMeta.presetKey = msg.presetKey;
      if (msg.size !== undefined) initMeta.size = msg.size;
      {
        const scenario = resolveScenario(msg);
        const presetKey = msg.presetKey ?? scenario?.cityKey;
        if (presetKey !== undefined) initMeta.presetKey = presetKey;
        loadOsmCity(presetKey).then((osm) => {
          state = newGame(msg.seed, msg.difficulty, {
            size: msg.size,
            presetKey,
            osm,
            rules: msg.rules as ScenarioRules | undefined,
            scenario,
          });
          bankrupt = false;
          won = false;
          fieldsVersion++;
          sendStatic(state);
          post({ type: 'ui', ui: buildUi(state) });
        });
      }
      break;
    case 'loadSave':
      try {
        state = deserialize(msg.json);
        bankrupt = state.failed === 'bankrupt';
        won = state.scenarioWon === true;
        fieldsVersion++;
        sendStatic(state);
        post({ type: 'ui', ui: buildUi(state) });
      } catch (err) {
        post({ type: 'toast', message: `Load failed: ${err instanceof Error ? err.message : 'corrupt save'}`, tone: 'warn' });
      }
      break;
    case 'requestSave':
      if (state) post({ type: 'saved', json: serialize(state) });
      break;
    case 'requestReplay':
      if (state) {
        const payload: import('./protocol').ReplayPayload = {
          seed: state.seed,
          difficulty: state.difficulty,
          commandLog: state.commandLog,
          finalTick: state.tick,
          stateHash: stateHash(state),
          scoreHint: Math.round(state.stats.dailyTransitTrips),
        };
        if (initMeta.presetKey !== undefined) payload.presetKey = initMeta.presetKey;
        if (initMeta.size !== undefined) payload.size = initMeta.size;
        if (state.scenarioRules) payload.rules = state.scenarioRules;
        post({ type: 'replay', payload });
      }
      break;
    case 'setSpeed':
      speed = msg.speed;
      break;
    case 'command': {
      if (!state) break;
      const result = applyCommand(state, msg.cmd);
      post({ type: 'commandResult', requestId: msg.requestId, result });
      post({ type: 'ui', ui: buildUi(state) });
      break;
    }
    case 'queryTrackCost': {
      if (!state) break;
      const { cost, breakdown } = trackCostDetailed(state, msg.mode, msg.grade, msg.points);
      post({ type: 'trackCost', requestId: msg.requestId, cost, breakdown });
      break;
    }
    case 'strataProbe': {
      if (!state) break;
      const col = columnAt(state.cityKey, state.seed, WORLD_SIZE, state.osmElevation, state.osmElevRes, { x: msg.x, y: msg.y });
      post({
        type: 'strataProbe',
        requestId: msg.requestId,
        probe: {
          bands: col.bands.map((b) => ({ kind: b.kind, top: b.top, bottom: b.bottom })),
          waterTable: col.waterTableDepth,
          rockHardness: col.rockHardness,
          surfaceElevation: col.surfaceElevation,
        },
      });
      break;
    }
  }
};
