#!/usr/bin/env bun
/**
 * mf-wire v1 smoke test (native-spec.md §2.6) — the CI regression gate.
 *
 * Runs the sidecar twice against the NYC preset, builds a small bus network
 * on real street geometry pulled straight out of the `ready` payload, drives
 * the sim forward, and asserts:
 *   1. vehicles actually move frame-to-frame (two consecutive `frame`
 *      binaries show a changed (x,y) for at least one vehicle), and
 *   2. the two independent runs — same seed, same deterministically-chosen
 *      command log — agree on `requestReplay`'s `stateHash`.
 *
 * Exits non-zero (via `process.exit(1)`) on any failure.
 *
 * Target selection: by default this runs the sidecar interpreted
 * (`bun run index.ts`). Set `MF_SIDECAR_BIN=/path/to/metroforge-sidecar` to
 * exercise a `bun build --compile` binary instead (proves the embedded city
 * JSON survives compilation) — same assertions, same test.
 *
 * Why setSpeed(0) around the build phase: ticks only advance inside the
 * sidecar's 50 ms interval, and `speed` is read once per firing. If station/
 * track/route commands were applied while ticks were live, the two runs
 * could apply them at different tick numbers (pure wall-clock jitter in how
 * long the setup round-trips take) and diverge for reasons that have nothing
 * to do with sim determinism. Freezing at speed=0 during setup, then running
 * at a speed that's an exact multiple of 20 (so `speed/20` is an integer and
 * the accumulator never carries a fraction), makes tick count a pure function
 * of "how many 50 ms firings happened" for both runs alike.
 */

const SIDECAR_BIN = process.env.MF_SIDECAR_BIN;
const DEBUG = process.env.MF_SMOKE_DEBUG === '1';
const SEED = 12345;
const DIFFICULTY: 'easy' | 'normal' | 'hard' = 'normal';
const PRESET_KEY = 'nyc';
const TARGET_TICK = 500;
const RUN_SPEED = 240; // multiple of 20 -> exactly 12 ticks/step, zero accumulator drift

interface Envelope {
  t: string;
  seq?: number;
  p?: unknown;
}

function encode(t: string, p?: unknown, seq?: number): string {
  return JSON.stringify({ t, seq, p });
}

function fail(message: string): never {
  console.error(`SMOKE TEST FAILED: ${message}`);
  process.exit(1);
}

// ── sidecar process + handshake ──────────────────────────────────────────────

class SidecarHandle {
  private constructor(
    public readonly proc: ReturnType<typeof Bun.spawn>,
    public readonly port: number,
    public readonly pid: number,
  ) {}

  static async spawn(): Promise<SidecarHandle> {
    const cmd = SIDECAR_BIN ? [SIDECAR_BIN, '--port', '0'] : ['bun', 'run', `${import.meta.dir}/index.ts`, '--port', '0'];
    const proc = Bun.spawn(cmd, { stdout: 'pipe', stderr: 'inherit', stdin: 'ignore' });
    const line = await SidecarHandle.readFirstLine(proc.stdout as ReadableStream<Uint8Array>);
    let parsed: { mf?: string; protocolVersion?: number; port?: number; pid?: number };
    try {
      parsed = JSON.parse(line) as typeof parsed;
    } catch {
      throw new Error(`sidecar handshake line was not JSON: ${JSON.stringify(line)}`);
    }
    if (parsed.mf !== 'sidecar' || parsed.protocolVersion !== 1 || typeof parsed.port !== 'number') {
      throw new Error(`unexpected handshake: ${line}`);
    }
    return new SidecarHandle(proc, parsed.port, parsed.pid ?? -1);
  }

  private static async readFirstLine(stream: ReadableStream<Uint8Array>): Promise<string> {
    const reader = stream.getReader();
    const decoder = new TextDecoder();
    let buf = '';
    for (;;) {
      const { value, done } = await reader.read();
      if (done) throw new Error('sidecar process exited before printing its handshake line');
      buf += decoder.decode(value, { stream: true });
      const nl = buf.indexOf('\n');
      if (nl >= 0) {
        reader.releaseLock();
        return buf.slice(0, nl);
      }
    }
  }

  kill(): void {
    try {
      this.proc.kill();
    } catch {
      // already dead
    }
  }
}

// ── WS client with a generic "wait for the next message matching a predicate" ──

type Inbound = { kind: 'text'; env: Envelope } | { kind: 'binary'; msgType: number; buf: ArrayBuffer };

class Client {
  private readonly ws: WebSocket;
  private waiters: { pred: (m: Inbound) => boolean; resolve: (m: Inbound) => void }[] = [];
  frames: { tick: number; vehicleCount: number; vehicles: Float32Array }[] = [];
  lastUiTick = -1;
  lastFrameTick = -1;
  maskCount = 0;
  /** msgType=5 StaticBuildings — captured unconditionally (like maskCount)
   *  since it's sent before `fields` in sendStatic, so by the time our
   *  `fields` wait resolves this has already been processed in order. */
  buildingsBuf: ArrayBuffer | undefined;

  constructor(port: number) {
    this.ws = new WebSocket(`ws://127.0.0.1:${port}`);
    this.ws.binaryType = 'arraybuffer';
    this.ws.onerror = (e) => console.error('ws error', e);
    this.ws.onmessage = (ev: MessageEvent) => {
      let msg: Inbound;
      if (typeof ev.data === 'string') {
        msg = { kind: 'text', env: JSON.parse(ev.data) as Envelope };
        if (msg.env.t === 'ui') this.lastUiTick = (msg.env.p as { tick: number }).tick;
      } else {
        const buf = ev.data as ArrayBuffer;
        const dv = new DataView(buf);
        const msgType = dv.getUint8(0);
        msg = { kind: 'binary', msgType, buf };
        if (msgType === 1) this.captureFrame(dv, buf);
        if (msgType === 4) this.maskCount++;
        if (msgType === 5) this.buildingsBuf = buf;
      }
      for (let i = this.waiters.length - 1; i >= 0; i--) {
        const w = this.waiters[i]!;
        if (w.pred(msg)) {
          this.waiters.splice(i, 1);
          w.resolve(msg);
        }
      }
    };
  }

  private captureFrame(dv: DataView, buf: ArrayBuffer): void {
    const tick = dv.getUint32(4, true);
    const vehicleCount = dv.getUint32(8, true);
    const colorTableLen = dv.getUint32(16, true);
    const vehOff = 24 + 4 * colorTableLen;
    const vehicles = new Float32Array(buf.slice(vehOff, vehOff + vehicleCount * 6 * 4));
    this.frames.push({ tick, vehicleCount, vehicles });
    if (this.frames.length > 2) this.frames.shift();
    this.lastFrameTick = tick;
  }

  send(t: string, p?: unknown, seq?: number): void {
    this.ws.send(encode(t, p, seq));
  }

  waitFor(pred: (m: Inbound) => boolean, timeoutMs = 10_000, label = 'message'): Promise<Inbound> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiters = this.waiters.filter((w) => w.resolve !== wrappedResolve);
        reject(new Error(`timed out waiting for ${label}`));
      }, timeoutMs);
      const wrappedResolve = (m: Inbound): void => {
        clearTimeout(timer);
        resolve(m);
      };
      this.waiters.push({ pred, resolve: wrappedResolve });
    });
  }

  waitForType(t: string, timeoutMs = 10_000): Promise<Envelope> {
    return this.waitFor((m) => m.kind === 'text' && m.env.t === t, timeoutMs, `t=${t}`).then((m) => (m as { kind: 'text'; env: Envelope }).env);
  }

  close(): void {
    this.ws.close();
  }
}

// ── deterministic road-pair picker (same city data -> same pick, every run) ──

interface Vec2 {
  x: number;
  y: number;
}

function pickStationCandidates(roads: { cls: string; points: number[] }[]): { a: Vec2; b: Vec2 }[] {
  const candidates: { a: Vec2; b: Vec2; len: number }[] = [];
  for (const r of roads) {
    if (r.points.length < 4) continue;
    const a = { x: r.points[0]!, y: r.points[1]! };
    const b = { x: r.points[r.points.length - 2]!, y: r.points[r.points.length - 1]! };
    const len = Math.hypot(b.x - a.x, b.y - a.y);
    if (len >= 250) candidates.push({ a, b, len });
  }
  candidates.sort((x, y) => y.len - x.len);
  return candidates.map(({ a, b }) => ({ a, b }));
}

// ── one full run: build a network, drive it forward, replay it ──────────────

interface RunResult {
  stateHash: number;
  finalTick: number;
  motionDetected: boolean;
  buildingCount: number;
  vertexTotal: number;
  buildingsBuf: ArrayBuffer;
}

interface ParsedStaticBuildings {
  version: number;
  buildingCount: number;
  vertexTotal: number;
  vertexCounts: number[];
  heights: number[];
  minHeights: number[];
  sumVertexCounts: number;
  consumedBytes: number;
}

/** Mirrors sidecar/wire.ts's encodeStaticBuildings layout: header (12 B)
 *  msgType u8=5 | version u8=2 | reserved u16 | buildingCount u32 |
 *  vertexTotal u32, then per building: vertexCount u8 | flags u8 | heightDm
 *  u16 | minHeightDm u16 | vertexCount × (i16 x, i16 y). */
function parseStaticBuildings(buf: ArrayBuffer): ParsedStaticBuildings {
  const dv = new DataView(buf);
  const version = dv.getUint8(1);
  const buildingCount = dv.getUint32(4, true);
  const vertexTotal = dv.getUint32(8, true);
  let off = 12;
  const vertexCounts: number[] = [];
  const heights: number[] = [];
  const minHeights: number[] = [];
  let sumVertexCounts = 0;
  for (let i = 0; i < buildingCount && off < buf.byteLength; i++) {
    const vc = dv.getUint8(off);
    const h = dv.getUint16(off + 2, true);
    const mh = dv.getUint16(off + 4, true);
    vertexCounts.push(vc);
    heights.push(h);
    minHeights.push(mh);
    sumVertexCounts += vc;
    off += 6 + vc * 4;
  }
  return { version, buildingCount, vertexTotal, vertexCounts, heights, minHeights, sumVertexCounts, consumedBytes: off };
}

async function runOnce(label: string): Promise<RunResult> {
  const sidecar = await SidecarHandle.spawn();
  const client = new Client(sidecar.port);
  try {
    const helloEnv = await client.waitForType('hello');
    const hello = helloEnv.p as { protocolVersion: number; cityList: { key: string; label: string }[] };
    if (hello.protocolVersion !== 1) fail(`[${label}] hello.protocolVersion !== 1`);
    if (!hello.cityList.some((c) => c.key === PRESET_KEY)) fail(`[${label}] cityList missing "${PRESET_KEY}"`);

    client.send('hello', { clientProtocolVersion: 1 });
    client.send('init', { seed: SEED, difficulty: DIFFICULTY, presetKey: PRESET_KEY });

    const readyEnv = await client.waitForType('ready', 15_000);
    const staticCity = (readyEnv.p as { staticCity: Record<string, unknown> }).staticCity;
    const expectedMasks = ['hasWaterMask', 'hasParkMask', 'hasBuildingMask'].filter((k) => staticCity[k] === true).length;
    await client.waitFor((m) => m.kind === 'binary' && m.msgType === 2, 15_000, 'fields'); // arrives after any masks + buildings
    if (client.maskCount !== expectedMasks) fail(`[${label}] expected ${expectedMasks} staticMask frames, saw ${client.maskCount}`);

    // msgType=5 StaticBuildings (metroforge-native issue #6): sent after masks,
    // before fields, so by now it has already been processed in order.
    if (!client.buildingsBuf) fail(`[${label}] no StaticBuildings (msgType=5) frame arrived for "${PRESET_KEY}"`);
    const buildings = parseStaticBuildings(client.buildingsBuf);
    if (buildings.version !== 2) fail(`[${label}] StaticBuildings version byte expected 2, got ${buildings.version}`);
    if (buildings.buildingCount <= 10_000) fail(`[${label}] expected buildingCount > 10,000 for NYC, got ${buildings.buildingCount}`);
    if (buildings.consumedBytes !== client.buildingsBuf.byteLength) {
      fail(`[${label}] StaticBuildings frame byte layout inconsistent: consumed ${buildings.consumedBytes}B of ${client.buildingsBuf.byteLength}B`);
    }
    if (buildings.sumVertexCounts !== buildings.vertexTotal) {
      fail(`[${label}] StaticBuildings vertexTotal header ${buildings.vertexTotal} != sum of per-building vertexCounts ${buildings.sumVertexCounts}`);
    }
    for (const vc of buildings.vertexCounts) {
      if (vc < 3 || vc > 64) fail(`[${label}] StaticBuildings vertexCount out of range 3..64: ${vc}`);
    }
    // minHeight consistency: mh is 0 (ground based or unknown) or strictly
    // less than h; also confirm NYC actually produced some minHeight>0
    // building:part masses (upper tower tiers above a podium).
    let minHeightViolations = 0;
    let partsWithMinHeight = 0;
    for (let i = 0; i < buildings.buildingCount; i++) {
      const h = buildings.heights[i]!;
      const mh = buildings.minHeights[i]!;
      if (mh > 0) partsWithMinHeight++;
      if (!(mh === 0 || mh < h)) minHeightViolations++;
    }
    if (minHeightViolations > 0) {
      fail(`[${label}] ${minHeightViolations} StaticBuildings entries have minHeight >= height (mh must be 0 or < h)`);
    }
    if (partsWithMinHeight === 0) fail(`[${label}] expected some building:part masses with minHeight > 0 for NYC, got none`);
    if (DEBUG) {
      console.error(
        `[${label}] buildings: count=${buildings.buildingCount} vertexTotal=${buildings.vertexTotal} withMinHeight=${partsWithMinHeight}`,
      );
    }

    // freeze ticking while we build the network (see module doc for why)
    client.send('setSpeed', { speed: 0 });

    const roads = (staticCity.roads as { cls: string; points: number[] }[]) ?? [];
    const candidates = pickStationCandidates(roads);
    if (candidates.length === 0) fail(`[${label}] no usable road candidates in NYC staticCity payload`);

    if (DEBUG) console.error(`[${label}] ${candidates.length} road candidates`);
    let routeId: number | null = null;
    for (const { a, b } of candidates.slice(0, 20)) {
      const stationA = await sendCommand(client, { kind: 'buildStation', mode: 'bus', pos: a });
      if (!stationA.ok) {
        if (DEBUG) console.error(`[${label}] stationA failed: ${stationA.error}`);
        continue;
      }
      const stationB = await sendCommand(client, { kind: 'buildStation', mode: 'bus', pos: b });
      if (!stationB.ok) {
        if (DEBUG) console.error(`[${label}] stationB failed: ${stationB.error}`);
        continue;
      }
      const track = await sendCommand(client, {
        kind: 'buildTrack',
        mode: 'bus',
        grade: 'surface',
        fromStationId: stationA.createdId!,
        toStationId: stationB.createdId!,
        waypoints: [],
      });
      if (!track.ok) {
        if (DEBUG) console.error(`[${label}] track failed: ${track.error}`);
        continue;
      }
      const route = await sendCommand(client, { kind: 'createRoute', mode: 'bus', stationIds: [stationA.createdId!, stationB.createdId!] });
      if (!route.ok) continue;
      routeId = route.createdId!;
      break;
    }
    if (routeId === null) fail(`[${label}] could not build a working bus network from any of ${candidates.length} road candidates`);
    if (DEBUG) console.error(`[${label}] built route ${routeId}`);

    // Run forward. Use FRAME ticks (sent unconditionally every 50 ms step),
    // not `ui` ticks: `ui` is gated by a countdown that free-runs even while
    // frozen at speed=0 during setup, so its phase-within-cycle depends on
    // how many setup steps happened to elapse — which varies run to run.
    // Frame tick has no such gating, so it is exactly `RUN_SPEED/20` ticks
    // per step from the moment speed changes, identically for both runs.
    client.send('setSpeed', { speed: RUN_SPEED });
    while (client.lastFrameTick < TARGET_TICK) {
      await client.waitFor((m) => m.kind === 'binary' && m.msgType === 1, 15_000, 'frame');
    }

    // Capture the vehicle-motion snapshot HERE, while still running: two
    // consecutive frames from the active phase. (Grabbing this after the
    // pause below would just show the same frozen positions twice.)
    const motionDetected =
      client.frames.length === 2 &&
      client.frames[0]!.vehicleCount > 0 &&
      client.frames[0]!.vehicleCount === client.frames[1]!.vehicleCount &&
      hasVehicleMoved(client.frames[0]!.vehicles, client.frames[1]!.vehicles);

    client.send('setSpeed', { speed: 0 }); // re-freeze as soon as the threshold is observed

    // let any in-flight ticks settle: wait until two consecutive frame ticks agree
    let settled = client.lastFrameTick;
    for (let i = 0; i < 10; i++) {
      await client.waitFor((m) => m.kind === 'binary' && m.msgType === 1, 5_000, 'frame');
      if (client.lastFrameTick === settled) break;
      settled = client.lastFrameTick;
    }

    if (DEBUG) {
      console.error(
        `[${label}] settled at tick=${client.lastFrameTick}, last frames vehicleCounts=${client.frames.map((f) => f.vehicleCount).join(',')}`,
      );
    }

    const replayEnv = await sendAndWait(client, 'requestReplay', undefined, (m) => m.kind === 'text' && m.env.t === 'replay');
    const replay = replayEnv.env.p as { stateHash: number; finalTick: number };

    client.send('shutdown');
    await client.waitFor((m) => m.kind === 'text' && m.env.t === 'bye', 5_000, 'bye').catch(() => undefined);

    return {
      stateHash: replay.stateHash,
      finalTick: replay.finalTick,
      motionDetected,
      buildingCount: buildings.buildingCount,
      vertexTotal: buildings.vertexTotal,
      buildingsBuf: client.buildingsBuf,
    };
  } finally {
    client.close();
    sidecar.kill();
  }
}

function hasVehicleMoved(a: Float32Array, b: Float32Array): boolean {
  const n = Math.min(a.length, b.length) / 6;
  for (let i = 0; i < n; i++) {
    const dx = a[i * 6 + 1]! - b[i * 6 + 1]!;
    const dy = a[i * 6 + 2]! - b[i * 6 + 2]!;
    if (Math.hypot(dx, dy) > 0.01) return true;
  }
  return false;
}

interface CommandResult {
  ok: boolean;
  error?: string;
  createdId?: number;
}

let nextSeq = 1;

/** Registers the waiter BEFORE sending, so a fast loopback reply can never
 *  arrive and be dropped before we start listening for it. */
async function sendAndWait(
  client: Client,
  t: string,
  p: unknown,
  pred: (m: Inbound) => boolean,
  seq?: number,
): Promise<{ kind: 'text'; env: Envelope }> {
  const waiter = client.waitFor(pred, 15_000, t);
  client.send(t, p, seq);
  return waiter as Promise<{ kind: 'text'; env: Envelope }>;
}

async function sendCommand(client: Client, cmd: unknown): Promise<CommandResult> {
  const seq = nextSeq++;
  const msg = await sendAndWait(client, 'command', { cmd }, (m) => m.kind === 'text' && m.env.t === 'commandResult' && m.env.seq === seq, seq);
  return (msg.env.p as { result: CommandResult }).result;
}

/** Save/load hydration (v0.4): a v2-wrapped save must round-trip the preset
 *  key so loadSave re-hydrates OSM masks + building vectors - without it a
 *  loaded NYC renders as a procedural city (native issue: saves-menu wave). */
async function runSaveLoadHydration(): Promise<void> {
  const label = 'save-load';
  const sidecar = await SidecarHandle.spawn();
  const client = new Client(sidecar.port);
  try {
    await client.waitForType('hello');
    client.send('hello', { clientProtocolVersion: 1 });
    client.send('init', { seed: SEED, difficulty: DIFFICULTY, presetKey: PRESET_KEY });
    await client.waitFor((m) => m.kind === 'binary' && m.msgType === 2, 15_000, 'fields');

    client.send('requestSave', {});
    const savedEnv = await client.waitForType('saved', 15_000);
    const savedJson = (savedEnv.p as { json: string }).json;
    const wrapper = JSON.parse(savedJson) as { mfSaveV?: number; presetKey?: string | null };
    if (wrapper.mfSaveV !== 2) fail(`[${label}] save wrapper version expected 2, got ${String(wrapper.mfSaveV)}`);
    if (wrapper.presetKey !== PRESET_KEY) fail(`[${label}] save wrapper presetKey expected ${PRESET_KEY}, got ${String(wrapper.presetKey)}`);

    client.maskCount = 0;
    client.buildingsBuf = undefined;
    client.send('loadSave', { json: savedJson });
    const readyEnv = await client.waitForType('ready', 15_000);
    const staticCity = (readyEnv.p as { staticCity: Record<string, unknown> }).staticCity;
    if (staticCity['hasBuildingMask'] !== true) fail(`[${label}] loaded ready lost hasBuildingMask (hydration failed)`);
    await client.waitFor((m) => m.kind === 'binary' && m.msgType === 2, 15_000, 'fields after load');
    if (client.maskCount < 1) fail(`[${label}] no staticMask frames after load`);
    if (!client.buildingsBuf) fail(`[${label}] no StaticBuildings frame after load (building vectors lost)`);
    console.log(`${label}: hydration OK (masks=${client.maskCount}, buildings frame present)`);
  } finally {
    client.close();
    await sidecar.kill();
  }
}

async function main(): Promise<void> {
  console.log(`mf-wire smoke test — sidecar: ${SIDECAR_BIN ?? '(interpreted) bun run index.ts'}`);

  const runA = await runOnce('run A');
  console.log(
    `run A: finalTick=${runA.finalTick} stateHash=${runA.stateHash} motion=${runA.motionDetected} ` +
      `buildings=${runA.buildingCount} vertices=${runA.vertexTotal}`,
  );
  const runB = await runOnce('run B');
  console.log(
    `run B: finalTick=${runB.finalTick} stateHash=${runB.stateHash} motion=${runB.motionDetected} ` +
      `buildings=${runB.buildingCount} vertices=${runB.vertexTotal}`,
  );

  if (!runA.motionDetected) fail('run A: no vehicle motion detected between consecutive frames');
  if (!runB.motionDetected) fail('run B: no vehicle motion detected between consecutive frames');
  if (runA.finalTick !== runB.finalTick) {
    fail(`runs settled at different ticks (A=${runA.finalTick}, B=${runB.finalTick}) — cannot compare stateHash meaningfully`);
  }
  if (runA.stateHash !== runB.stateHash) {
    fail(`stateHash mismatch at identical tick ${runA.finalTick}: A=${runA.stateHash} B=${runB.stateHash}`);
  }

  // StaticBuildings is baked from a static import, not derived from sim
  // state, so it must be byte-for-byte identical across two independent
  // sidecar processes — any diff would mean nondeterministic encoding.
  if (!Buffer.from(runA.buildingsBuf).equals(Buffer.from(runB.buildingsBuf))) {
    fail(`StaticBuildings frame bytes differ between run A (${runA.buildingsBuf.byteLength}B) and run B (${runB.buildingsBuf.byteLength}B)`);
  }

  await runSaveLoadHydration();

  console.log('PASS: vehicles move, both runs agree on stateHash at identical tick, and StaticBuildings is byte-identical.');
}

main().catch((err) => {
  console.error('SMOKE TEST FAILED:', err);
  process.exit(1);
});
