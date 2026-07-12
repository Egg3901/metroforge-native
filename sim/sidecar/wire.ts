/**
 * mf-wire v1 — JSON envelope + binary frame encoders for the sidecar↔client
 * protocol (see native-spec.md §1). Text frames carry low-rate JSON control
 * messages; binary frames carry hot-path typed payloads. All binary is
 * little-endian; f32/u32 arrays are copied out via DataView/typed-array
 * blits so the wire format never depends on the host's native endianness or
 * on zero-copy casts of a raw buffer.
 */

export const PROTOCOL_VERSION = 1;

/** Skip sending a droppable frame once the socket's outbound queue exceeds
 *  this many buffered bytes (spec §1.5). */
export const BACKPRESSURE_LIMIT_BYTES = 4 * 1024 * 1024;

/** Frame kinds that may be silently dropped under backpressure. Everything
 *  else (hello, ready, staticMask, fields, ui, commandResult, trackCost,
 *  saved, replay, toast, bye) is never dropped. */
const DROPPABLE_TYPES: ReadonlySet<string> = new Set(['frame', 'traffic', 'demand', 'heatmap']);

export interface Envelope {
  t: string;
  seq?: number;
  p?: unknown;
}

/** Build the `{t, seq?, p?}` JSON envelope. `JSON.stringify` drops
 *  undefined-valued keys, so omitted `seq`/`p` vanish from the wire text. */
export function encodeEnvelope(t: string, p?: unknown, seq?: number): string {
  return JSON.stringify({ t, seq, p });
}

export function decodeEnvelope(raw: string): Envelope {
  const parsed = JSON.parse(raw) as Partial<Envelope>;
  if (typeof parsed.t !== 'string') throw new Error('mf-wire: malformed envelope (missing "t")');
  const env: Envelope = { t: parsed.t };
  if (parsed.seq !== undefined) env.seq = parsed.seq;
  if (parsed.p !== undefined) env.p = parsed.p;
  return env;
}

/** A message ready to hand to the transport (index.ts owns the `ws` and
 *  applies the backpressure rule based on `droppable`). */
export type OutMessage =
  | { kind: 'text'; type: string; json: string; droppable: boolean }
  | { kind: 'binary'; type: string; buf: ArrayBuffer; droppable: boolean };

export function jsonMessage(t: string, p?: unknown, seq?: number): OutMessage {
  return { kind: 'text', type: t, json: encodeEnvelope(t, p, seq), droppable: DROPPABLE_TYPES.has(t) };
}

export function binaryMessage(t: string, buf: ArrayBuffer): OutMessage {
  return { kind: 'binary', type: t, buf, droppable: DROPPABLE_TYPES.has(t) };
}

// ── binary blit helpers ──────────────────────────────────────────────────────

/** Copy `len` floats from `arr` into `bytes` at `offset` (byte-for-byte, no
 *  zero-copy cast) and return the next write offset. */
function blitF32(bytes: Uint8Array, offset: number, arr: Float32Array, len: number): number {
  bytes.set(new Uint8Array(arr.buffer, arr.byteOffset, len * 4), offset);
  return offset + len * 4;
}

function blitU8(bytes: Uint8Array, offset: number, arr: Uint8Array, len: number): number {
  bytes.set(arr.subarray(0, len), offset);
  return offset + len;
}

function blitU32Array(dv: DataView, offset: number, arr: Uint32Array): number {
  let off = offset;
  for (let i = 0; i < arr.length; i++) {
    dv.setUint32(off, arr[i] as number, true);
    off += 4;
  }
  return off;
}

// ── msgType=1 FrameSnapshot ───────────────────────────────────────────────────

export interface FrameSnapshotInput {
  tick: number;
  /** packed 0x00RRGGBB per route-color index; vehicles[i*6+5] indexes this table */
  colorTable: Uint32Array;
  /** length MUST equal vehicleCount*6: [id,x,y,heading,occupancy,routeColorIdx] */
  vehicles: Float32Array;
  vehicleCount: number;
  /** length MUST equal agentCount*3: [x,y,phase] */
  agents: Float32Array;
  agentCount: number;
}

/** header (24 B): msgType u8=1 | version u8=1 | reserved u16 | tick u32 |
 *  vehicleCount u32 | agentCount u32 | colorTableLen u32 | reserved u32 */
export function encodeFrame(f: FrameSnapshotInput): ArrayBuffer {
  const c = f.colorTable.length;
  const n = f.vehicleCount;
  const m = f.agentCount;
  const headerLen = 24;
  const buf = new ArrayBuffer(headerLen + 4 * c + 4 * n * 6 + 4 * m * 3);
  const dv = new DataView(buf);
  const bytes = new Uint8Array(buf);
  dv.setUint8(0, 1);
  dv.setUint8(1, 1);
  dv.setUint16(2, 0, true);
  dv.setUint32(4, f.tick >>> 0, true);
  dv.setUint32(8, n >>> 0, true);
  dv.setUint32(12, m >>> 0, true);
  dv.setUint32(16, c >>> 0, true);
  dv.setUint32(20, 0, true);
  let off = blitU32Array(dv, headerLen, f.colorTable);
  off = blitF32(bytes, off, f.vehicles, n * 6);
  blitF32(bytes, off, f.agents, m * 3);
  return buf;
}

// ── msgType=2 FieldsPayload ───────────────────────────────────────────────────

export interface FieldsInput {
  version: number;
  cellCount: number;
  terrain: Float32Array;
  population: Float32Array;
  jobs: Float32Array;
  landValue: Float32Array;
  water: Uint8Array;
  parks: Uint8Array;
}

/** header (16 B): msgType u8=2 | version u8=1 | reserved u16 | fieldsVersion u32 |
 *  cellCount u32 | reserved u32. Body: 4×f32[N] THEN 2×u8[N] (differs from the
 *  TS FieldsPayload struct order — f32 arrays go first for 4-byte alignment). */
export function encodeFields(f: FieldsInput): ArrayBuffer {
  const N = f.cellCount;
  const headerLen = 16;
  const buf = new ArrayBuffer(headerLen + 4 * N * 4 + 2 * N);
  const dv = new DataView(buf);
  const bytes = new Uint8Array(buf);
  dv.setUint8(0, 2);
  dv.setUint8(1, 1);
  dv.setUint16(2, 0, true);
  dv.setUint32(4, f.version >>> 0, true);
  dv.setUint32(8, N >>> 0, true);
  dv.setUint32(12, 0, true);
  let off = headerLen;
  off = blitF32(bytes, off, f.terrain, N);
  off = blitF32(bytes, off, f.population, N);
  off = blitF32(bytes, off, f.jobs, N);
  off = blitF32(bytes, off, f.landValue, N);
  off = blitU8(bytes, off, f.water, N);
  blitU8(bytes, off, f.parks, N);
  return buf;
}

// ── msgType=3 TrafficPayload ──────────────────────────────────────────────────

export interface TrafficInput {
  w: number;
  h: number;
  cellSize: number;
  originX: number;
  originY: number;
  /** length MUST equal w*h */
  values: Float32Array;
  hotspots: { x: number; y: number; severity: number }[];
}

/** header (32 B): msgType u8=3 | version u8=1 | hotspotCount u16 | w u32 | h u32 |
 *  cellSize f32 | originX f32 | originY f32 | valueCount u32(=w*h) | reserved u32.
 *  Body: f32[w*h] values, then (f32,f32,f32)[hotspotCount]. */
export function encodeTraffic(t: TrafficInput): ArrayBuffer {
  const k = t.hotspots.length;
  const valueCount = t.w * t.h;
  const headerLen = 32;
  const buf = new ArrayBuffer(headerLen + 4 * valueCount + 12 * k);
  const dv = new DataView(buf);
  const bytes = new Uint8Array(buf);
  dv.setUint8(0, 3);
  dv.setUint8(1, 1);
  dv.setUint16(2, k & 0xffff, true);
  dv.setUint32(4, t.w >>> 0, true);
  dv.setUint32(8, t.h >>> 0, true);
  dv.setFloat32(12, t.cellSize, true);
  dv.setFloat32(16, t.originX, true);
  dv.setFloat32(20, t.originY, true);
  dv.setUint32(24, valueCount >>> 0, true);
  dv.setUint32(28, 0, true);
  let off = blitF32(bytes, headerLen, t.values, valueCount);
  for (const h of t.hotspots) {
    dv.setFloat32(off, h.x, true);
    dv.setFloat32(off + 4, h.y, true);
    dv.setFloat32(off + 8, h.severity, true);
    off += 12;
  }
  return buf;
}

// ── msgType=4 StaticMask ──────────────────────────────────────────────────────

export type StaticMaskWhich = 0 | 1 | 2; // 0=water, 1=park, 2=building

/** header (12 B): msgType u8=4 | version u8=1 | which u8 | reserved u8 |
 *  res u32 | reserved u32. Body: u8[res*res] mask. */
export function encodeStaticMask(which: StaticMaskWhich, res: number, mask: Uint8Array): ArrayBuffer {
  const headerLen = 12;
  const buf = new ArrayBuffer(headerLen + res * res);
  const dv = new DataView(buf);
  const bytes = new Uint8Array(buf);
  dv.setUint8(0, 4);
  dv.setUint8(1, 1);
  dv.setUint8(2, which);
  dv.setUint8(3, 0);
  dv.setUint32(4, res >>> 0, true);
  dv.setUint32(8, 0, true);
  blitU8(bytes, headerLen, mask, res * res);
  return buf;
}

// ── msgType=5 StaticBuildings ─────────────────────────────────────────────────

/** One real-OSM building footprint: `v` is a flat [x0,y0,x1,y1,...] outer ring
 *  in integer half-meters (world meters × 2, rounded; same projected world
 *  space as roads/masks, origin-centered, y down/north up), wound CCW per
 *  scripts/build-cities.ts's signedArea2D convention. `h` is the mass's top
 *  height in decimeters, 0 = unknown. `mh` is the mass's base height (min
 *  height) in decimeters, 0 = ground based or unknown; when nonzero it is
 *  always strictly less than `h` (a building:part sub-mass, e.g. an upper
 *  tower tier that starts above a podium). Vertex count MUST be in 3..64
 *  (the build script simplifies/caps to this range). */
export interface StaticBuildingsInput {
  buildings: { h: number; mh: number; v: number[] }[];
}

/** header (12 B): msgType u8=5 | version u8=2 | reserved u16 | buildingCount u32 |
 *  vertexTotal u32. Body, per building: vertexCount u8 (3..64) | flags u8=0 |
 *  heightDm u16 | minHeightDm u16 | vertexCount × (xHalfM i16, yHalfM i16).
 *  Static/non-droppable, same class as StaticMask: optional for clients,
 *  does not affect protocolVersion or gate any other behavior.
 *
 *  Version note: v1 clients (metroforge-native 0.1.3 and earlier) expect a
 *  4-byte per-building header (no minHeightDm) and read the version byte
 *  before parsing the body, so a v1 client reading a v2 frame sees the
 *  version mismatch first and can reject the frame cleanly (log an
 *  UnsupportedVersion warning, fall back to the building mask) instead of
 *  misreading the now-6-byte stride as vertex data. */
export function encodeStaticBuildings(f: StaticBuildingsInput): ArrayBuffer {
  const buildingCount = f.buildings.length;
  let vertexTotal = 0;
  for (const b of f.buildings) vertexTotal += b.v.length / 2;
  const headerLen = 12;
  const bodyLen = buildingCount * 6 + vertexTotal * 4; // per-building 6B header + 4B/vertex (2×i16)
  const buf = new ArrayBuffer(headerLen + bodyLen);
  const dv = new DataView(buf);
  dv.setUint8(0, 5);
  dv.setUint8(1, 2);
  dv.setUint16(2, 0, true);
  dv.setUint32(4, buildingCount >>> 0, true);
  dv.setUint32(8, vertexTotal >>> 0, true);
  let off = headerLen;
  for (const b of f.buildings) {
    const vertexCount = b.v.length / 2;
    dv.setUint8(off, vertexCount);
    dv.setUint8(off + 1, 0); // flags, reserved
    dv.setUint16(off + 2, b.h, true);
    dv.setUint16(off + 4, b.mh, true);
    off += 6;
    for (let i = 0; i < vertexCount; i++) {
      dv.setInt16(off, b.v[i * 2]!, true);
      dv.setInt16(off + 2, b.v[i * 2 + 1]!, true);
      off += 4;
    }
  }
  return buf;
}

// ── msgType=7 StaticElevation ─────────────────────────────────────────────────

/** header (12 B): msgType u8=7 | version u8=1 | reserved u16 | res u32 |
 *  reserved u32. Body: i16[res*res] real meters, little-endian, row-major
 *  (row 0 = north edge, same footprint as StaticMask over the world square).
 *
 *  A dedicated static heightfield channel, decoupled from the coarse sim
 *  `fields.terrain` (96²): elevation is real DEM data, static per city, and
 *  not gameplay-coupled, so it ships as its own hi-res raster in true meters
 *  rather than riding the normalized 0..1 sim field. Same additive/optional
 *  class as StaticMask/StaticBuildings — a client that ignores msgType=7
 *  simply falls back to the sim field's terrain, so this does NOT bump
 *  PROTOCOL_VERSION (mirrors the msgType=5 buildings precedent). */
export function encodeStaticElevation(res: number, elev: Int16Array): ArrayBuffer {
  const headerLen = 12;
  const buf = new ArrayBuffer(headerLen + res * res * 2);
  const dv = new DataView(buf);
  dv.setUint8(0, 7);
  dv.setUint8(1, 1);
  dv.setUint16(2, 0, true);
  dv.setUint32(4, res >>> 0, true);
  dv.setUint32(8, 0, true);
  let off = headerLen;
  for (let i = 0; i < res * res; i++) {
    dv.setInt16(off, elev[i] as number, true);
    off += 2;
  }
  return buf;
}

// ── msgType=6 HeatmapPayload (ridership analytics) ────────────────────────────
//
// Byte layout is owned by `@core/analytics` (encodeHeatmapPayload). Re-exported
// here so the sidecar wire surface stays the single import for binary frames.
// See src/core/analytics.ts file header for the precise 32-byte header layout.
export {
  encodeHeatmapPayload as encodeHeatmap,
  decodeHeatmapPayload as decodeHeatmap,
  HEATMAP_MSG_TYPE,
  HEATMAP_VERSION,
  HEATMAP_HEADER_BYTES,
  type HeatmapPayload,
} from '@core/analytics';
