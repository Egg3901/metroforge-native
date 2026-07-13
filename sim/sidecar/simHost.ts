/**
 * SimHost — the sidecar's port of `@host/sim.worker.ts`. Owns the GameState,
 * advances fixed-timestep ticks on the same 50 ms/accumulator cadence the
 * worker used, and streams render snapshots + UI state out over `send`
 * (mf-wire encoders instead of postMessage/Transferables).
 *
 * Everything here is ported VERBATIM from sim.worker.ts where possible (see
 * native-spec.md §2.2): simTick, applyCommand, trackCost, newGame,
 * (de)serialize/stateHash, getRoutePath, pointAlong, AgentPool, EVENT_DEFS,
 * computeInsights, buildUi, the accumulator loop, the flows-changed→
 * agents.resample+traffic+demand block, agents.update(speed/20), and the
 * 2 Hz UI countdown (uiCountdown=10). The only real behavior changes:
 *  - `self`/postMessage/webworker globals → class fields + injected `send`.
 *  - `loadOsmCity` (async dynamic import) → `resolveCity` (sync, static).
 *  - bare `setInterval` at module scope → `start()`/`stop()` so a
 *    connection close (or a `shutdown` message) can tear the loop down.
 *  - `sendStatic` splits into a JSON `ready` (masks replaced by
 *    maskRes/has*Mask flags) plus up to three binary `staticMask` frames.
 */
import { applyCommand, trackCostDetailed } from '@core/commands';
import { columnAt } from '@core/geology';
import { TICKS_PER_DAY, WORLD_SIZE } from '@core/constants';
import type { MapSize } from '@core/city/presets';
import { EVENT_DEFS } from '@core/events';
import { pointAlong } from '@core/geometry';
import { newGame } from '@core/newGame';
import { deserialize, serialize, stateHash } from '@core/save';
import { decodeB64Mask, decodeElevation } from '@core/city/osmCity';
import type { ScenarioRules } from '@core/scenarioRules';
import { playableScenario, type ScenarioDef } from '@core/scenario';
import { simTick } from '@core/sim';
import { getRoutePath } from '@core/transit/routePath';
import type { Command, Difficulty, GameState, TrackGrade, TransitMode } from '@core/types';
import { AgentPool } from '@host/agents';
import type { ReplayPayload, UiState } from '@host/protocol';
import { routeExtras, todFactorOf, uiExtras } from '@host/uiExtras';
import { analyticsInsightLines, encodeHeatmapPayload, type HeatmapPayload } from '@core/analytics';
import { resolveBuildings, resolveCity, type BuildingsData } from './cities';
import {
  binaryMessage,
  encodeFields,
  encodeFrame,
  encodeStaticBuildings,
  encodeStaticElevation,
  encodeStaticMask,
  encodeTraffic,
  jsonMessage,
  type Envelope,
  type OutMessage,
} from './wire';

interface InitPayload {
  seed: number;
  difficulty: Difficulty;
  size?: MapSize | undefined;
  presetKey?: string | undefined;
  rules?: ScenarioRules | undefined;
  scenarioId?: string | undefined;
  scenario?: ScenarioDef | undefined;
}

export class SimHost {
  private state: GameState | null = null;
  private speed = 1; // game-seconds per real second
  private fieldsVersion = 1;
  private bankrupt = false;
  private won = false;
  private initMeta: { presetKey?: string; size?: MapSize } = {};
  /** Building-vector data for the current city, set from `presetKey` at
   *  `init` time. `loadSave` carries no `presetKey` (see host/protocol.ts),
   *  so it clears this rather than risk streaming the wrong city's
   *  buildings — the frame is optional, so silently having none is fine. */
  private buildings: BuildingsData | undefined;
  private readonly agents = new AgentPool();
  private lastFlowsRef: unknown = null;

  private accumulator = 0;
  private uiCountdown = 0;
  private stepCap = 400; // matches the worker's `ticksRun < 400` guard
  private timer: ReturnType<typeof setInterval> | null = null;

  constructor(
    private readonly send: (msg: OutMessage) => void,
    private readonly onShutdown?: () => void,
  ) {}

  private resolveScenario(p: InitPayload): ScenarioDef | undefined {
    if (p.scenario) return p.scenario;
    if (p.scenarioId) return playableScenario(p.scenarioId);
    return undefined;
  }

  /** Raises (or restores) the per-step tick cap; `--headless-speed` uses
   *  Infinity so a fast-forwarded smoke test can catch up in one step. */
  setStepCap(n: number): void {
    this.stepCap = n;
  }

  setSpeed(speed: number): void {
    this.speed = speed;
  }

  start(): void {
    if (this.timer) return;
    this.timer = setInterval(() => this.step(), 50);
  }

  stop(): void {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  handleEnvelope(env: Envelope): void {
    switch (env.t) {
      case 'hello':
        // client's own greeting; sidecar already sent its hello on connect.
        break;
      case 'init':
        this.handleInit(env.p as InitPayload);
        break;
      case 'loadSave':
        this.handleLoadSave(env.p as { json: string });
        break;
      case 'requestSave':
        if (this.state) {
          // v2 save wrapper: serialize() strips the OSM masks/labels and no
          // building vectors persist, so the preset key rides along to let
          // loadSave re-hydrate them from the baked city data.
          const wrapped = JSON.stringify({
            mfSaveV: 2,
            presetKey: this.initMeta.presetKey ?? null,
            sim: JSON.parse(serialize(this.state)) as unknown,
          });
          this.send(jsonMessage('saved', { json: wrapped }));
        }
        break;
      case 'setSpeed':
        this.speed = (env.p as { speed: number }).speed;
        break;
      case 'command':
        this.handleCommand(env.p as { cmd: Command }, env.seq);
        break;
      case 'queryTrackCost':
        this.handleTrackCost(env.p as { mode: TransitMode; grade: TrackGrade; points: { x: number; y: number }[] }, env.seq);
        break;
      case 'strataProbe':
        this.handleStrataProbe(env.p as { x: number; y: number }, env.seq);
        break;
      case 'requestReplay':
        this.handleRequestReplay();
        break;
      case 'ping':
        this.send(jsonMessage('pong'));
        break;
      case 'shutdown':
        this.stop();
        this.send(jsonMessage('bye'));
        this.onShutdown?.();
        break;
      default:
        break;
    }
  }

  private handleInit(p: InitPayload): void {
    this.initMeta = {};
    const scenario = this.resolveScenario(p);
    const presetKey = p.presetKey ?? scenario?.cityKey;
    if (presetKey !== undefined) this.initMeta.presetKey = presetKey;
    if (p.size !== undefined) this.initMeta.size = p.size;
    const osm = resolveCity(presetKey);
    const state = newGame(p.seed, p.difficulty, {
      size: p.size,
      presetKey,
      osm,
      rules: p.rules,
      scenario,
    });
    this.state = state;
    this.buildings = resolveBuildings(presetKey);
    this.bankrupt = false;
    this.won = false;
    this.fieldsVersion++;
    this.accumulator = 0;
    this.uiCountdown = 0;
    this.sendStatic(state);
    this.sendUi(state);
  }

  private handleLoadSave(p: { json: string }): void {
    try {
      let simJson = p.json;
      let presetKey: string | undefined;
      try {
        const parsed = JSON.parse(p.json) as { mfSaveV?: number; presetKey?: string | null; sim?: unknown };
        if (parsed !== null && typeof parsed === 'object' && parsed.mfSaveV === 2 && parsed.sim !== undefined) {
          simJson = JSON.stringify(parsed.sim);
          presetKey = parsed.presetKey ?? undefined;
        }
      } catch {
        // legacy bare-state save; fall through with the raw json
      }
      const state = deserialize(simJson);
      // serialize() strips OSM masks/labels for size and building vectors
      // never persist; without re-hydration a loaded NYC renders as a
      // procedural city (no real footprints, no water/park masks). The v2
      // wrapper's preset key restores them from the baked city data.
      const osm = resolveCity(presetKey);
      if (osm) {
        const n = osm.maskRes * osm.maskRes;
        const packed = osm.maskPacked === true;
        state.osmWaterMask = decodeB64Mask(osm.waterMask, n, packed);
        state.osmParkMask = osm.parkMask ? decodeB64Mask(osm.parkMask, n, packed) : undefined;
        state.osmBuildingMask = osm.buildingMask ? decodeB64Mask(osm.buildingMask, n, packed) : undefined;
        state.osmMaskRes = osm.maskRes;
        if (osm.elevation && osm.elevRes) {
          state.osmElevation = decodeElevation(osm.elevation, osm.elevRes);
          state.osmElevRes = osm.elevRes;
        }
        state.osmLabels = osm.labels;
      }
      this.state = state;
      this.buildings = resolveBuildings(presetKey);
      if (presetKey !== undefined) this.initMeta.presetKey = presetKey;
      this.bankrupt = state.failed === 'bankrupt';
      this.won = state.scenarioWon === true;
      this.fieldsVersion++;
      this.sendStatic(state);
      this.sendUi(state);
    } catch (err) {
      this.toast(`Load failed: ${err instanceof Error ? err.message : 'corrupt save'}`, 'warn');
    }
  }

  private handleCommand(p: { cmd: Command }, seq: number | undefined): void {
    if (!this.state) return;
    const result = applyCommand(this.state, p.cmd);
    this.send(jsonMessage('commandResult', { result }, seq));
    this.sendUi(this.state);
  }

  private handleTrackCost(p: { mode: TransitMode; grade: TrackGrade; points: { x: number; y: number }[] }, seq: number | undefined): void {
    if (!this.state) return;
    const { cost, breakdown } = trackCostDetailed(this.state, p.mode, p.grade, p.points);
    this.send(jsonMessage('trackCost', { cost, breakdown }, seq));
  }

  private handleStrataProbe(p: { x: number; y: number }, seq: number | undefined): void {
    if (!this.state) return;
    const col = columnAt(this.state.cityKey, this.state.seed, WORLD_SIZE, this.state.osmElevation, this.state.osmElevRes, { x: p.x, y: p.y });
    this.send(jsonMessage('strataProbe', {
      bands: col.bands.map((b) => ({ kind: b.kind, top: b.top, bottom: b.bottom })),
      waterTable: col.waterTableDepth,
      rockHardness: col.rockHardness,
      surfaceElevation: col.surfaceElevation,
    }, seq));
  }

  private handleRequestReplay(): void {
    if (!this.state) return;
    const payload: ReplayPayload = {
      seed: this.state.seed,
      difficulty: this.state.difficulty,
      commandLog: this.state.commandLog,
      finalTick: this.state.tick,
      stateHash: stateHash(this.state),
      scoreHint: Math.round(this.state.stats.dailyTransitTrips),
    };
    if (this.initMeta.presetKey !== undefined) payload.presetKey = this.initMeta.presetKey;
    if (this.initMeta.size !== undefined) payload.size = this.initMeta.size;
    if (this.state.scenarioRules) payload.rules = this.state.scenarioRules;
    this.send(jsonMessage('replay', payload));
  }

  // ── outbound: static city + masks ──────────────────────────────────────────

  private sendStatic(s: GameState): void {
    const staticCity = {
      fieldW: s.fields.w,
      fieldH: s.fields.h,
      cellSize: s.fields.cellSize,
      originX: s.fields.originX,
      originY: s.fields.originY,
      worldSize: s.fields.w * s.fields.cellSize,
      // dense real-city imports have ~5-10k roads; thin them right down
      roadScale: s.roads.length > 3000 ? 0.28 : s.roads.length > 1500 ? 0.5 : 1,
      maskRes: s.osmMaskRes,
      hasWaterMask: s.osmWaterMask !== undefined,
      hasParkMask: s.osmParkMask !== undefined,
      hasBuildingMask: s.osmBuildingMask !== undefined,
      labels: s.osmLabels,
      roads: s.roads.map((r) => ({
        cls: r.cls,
        points: r.polyline.points.flatMap((p) => [p.x, p.y]),
      })),
    };
    this.send(jsonMessage('ready', { staticCity }));
    const res = s.osmMaskRes;
    if (res !== undefined) {
      if (s.osmWaterMask) this.send(binaryMessage('staticMask', encodeStaticMask(0, res, s.osmWaterMask)));
      if (s.osmParkMask) this.send(binaryMessage('staticMask', encodeStaticMask(1, res, s.osmParkMask)));
      if (s.osmBuildingMask) this.send(binaryMessage('staticMask', encodeStaticMask(2, res, s.osmBuildingMask)));
    }
    // dedicated real-elevation channel (msgType=7), decoupled from the sim
    // field; optional/additive like the masks above (see wire.ts).
    if (s.osmElevation && s.osmElevRes) {
      this.send(binaryMessage('staticElevation', encodeStaticElevation(s.osmElevRes, s.osmElevation)));
    }
    // real per-building footprint polygons + heights (optional; only cities
    // with a generated buildings file have one — see sidecar/cities.ts)
    if (this.buildings) {
      this.send(binaryMessage('staticBuildings', encodeStaticBuildings({ buildings: this.buildings.buildings })));
    }
    this.sendFields(s);
  }

  private sendFields(s: GameState): void {
    const N = s.fields.w * s.fields.h;
    this.send(
      binaryMessage(
        'fields',
        encodeFields({
          version: this.fieldsVersion,
          cellCount: N,
          terrain: s.fields.terrain,
          population: s.fields.population,
          jobs: s.fields.jobs,
          landValue: s.fields.landValue,
          water: s.fields.water,
          parks: s.fields.parks,
        }),
      ),
    );
  }

  // ── insights + UI (ported verbatim from sim.worker.ts) ─────────────────────

  private computeInsights(s: GameState): string[] {
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

  private buildUi(s: GameState): UiState {
    const tod = todFactorOf(s);
    return {
      ...uiExtras(s),
      tick: s.tick,
      insights: this.computeInsights(s),
      day: Math.floor(s.tick / TICKS_PER_DAY) + 1,
      speed: this.speed,
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
          ...routeExtras(r, tod, s),
        };
      }),
      activeEvents: s.activeEvents.map((a) => ({ id: a.id, name: EVENT_DEFS.find((e) => e.id === a.id)?.name ?? a.id, daysLeft: a.daysLeft })),
      fieldsVersion: this.fieldsVersion,
      bankrupt: this.bankrupt || s.failed === 'bankrupt',
      failed: s.failed,
      maxDay: s.scenarioRules?.maxDay ?? null,
      eraLabel: s.scenarioRules?.eraLabel ?? null,
      commandCount: s.commandLog.length,
    };
  }

  private sendUi(s: GameState): void {
    this.send(jsonMessage('ui', this.buildUi(s)));
  }

  private toast(message: string, tone: 'info' | 'warn' | 'good'): void {
    this.send(jsonMessage('toast', { message, tone }));
  }

  private sendTraffic(s: GameState): void {
    const t = s.traffic;
    if (!t) return;
    this.send(
      binaryMessage(
        'traffic',
        encodeTraffic({
          w: t.w,
          h: t.h,
          cellSize: t.cellSize,
          originX: t.originX,
          originY: t.originY,
          values: t.values,
          hotspots: t.hotspots.map((h) => ({ x: h.x, y: h.y, severity: h.severity })),
        }),
      ),
    );
  }

  private sendDemand(s: GameState): void {
    const lines = s.unserved ?? [];
    let maxWeight = 0;
    for (const l of lines) if (l.weight > maxWeight) maxWeight = l.weight;
    this.send(jsonMessage('demand', { lines: lines.map((l) => ({ ...l })), maxWeight }));
  }

  private sendHeatmap(payload: HeatmapPayload): void {
    this.send(binaryMessage('heatmap', encodeHeatmapPayload(payload)));
  }

  private sendFrame(s: GameState): void {
    const colorTable = new Uint32Array(s.routes.length);
    s.routes.forEach((r, i) => {
      colorTable[i] = parseInt(r.color.slice(1), 16);
    });
    const buf = new Float32Array(s.vehicles.length * 6);
    let n = 0;
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
    const vehicles = buf.subarray(0, n * 6);
    const agentBuf = this.agents.buffer.subarray(0, this.agents.count * 3);
    this.send(
      binaryMessage(
        'frame',
        encodeFrame({
          tick: s.tick,
          colorTable,
          vehicles,
          vehicleCount: n,
          agents: agentBuf,
          agentCount: this.agents.count,
        }),
      ),
    );
  }

  // ── main loop: 20 host steps/sec; each step advances `speed/20` game-seconds ──

  private step(): void {
    const s = this.state;
    if (!s || this.bankrupt || s.failed || this.won || s.scenarioWon) return;
    this.accumulator += this.speed / 20;
    let ticksRun = 0;
    while (this.accumulator >= 1 && ticksRun < this.stepCap) {
      const events = simTick(s);
      this.accumulator -= 1;
      ticksRun++;
      for (const m of events.messages) this.toast(m, 'info');
      for (const t of events.toasts ?? []) this.toast(t.message, t.tone);
      if (events.modeUnlocked) this.toast(`${events.modeUnlocked} unlocked!`, 'good');
      if (events.won) {
        this.won = true;
        this.sendUi(s);
      }
      if (events.bankrupt || events.failed) {
        this.bankrupt = events.bankrupt === true;
        const reason = events.bankrupt ? 'bankrupt' : events.failed;
        const copy =
          reason === 'approval'
            ? 'Approval collapsed — the board has fired you.'
            : reason === 'time'
              ? 'Time is up — the objective was not met.'
              : reason === 'condition'
                ? 'Scenario failed — a lose condition was met.'
                : 'Bankruptcy — the city has taken over your transit authority.';
        this.toast(copy, 'warn');
        this.sendUi(s);
      }
      if (events.dayCompleted !== undefined && events.dayCompleted % 7 === 0) {
        this.fieldsVersion++;
        this.sendFields(s);
      }
      if (events.heatmap) this.sendHeatmap(events.heatmap);
    }
    if (s.flows !== this.lastFlowsRef) {
      this.lastFlowsRef = s.flows;
      this.agents.resample(s);
      this.sendTraffic(s); // congestion recomputed with the flows
      this.sendDemand(s); // unserved-demand desire lines, same cadence
    }
    this.agents.update(this.speed / 20);
    this.sendFrame(s);
    if (--this.uiCountdown <= 0) {
      this.uiCountdown = 10; // UI state at 2 Hz
      this.sendUi(s);
    }
  }
}
