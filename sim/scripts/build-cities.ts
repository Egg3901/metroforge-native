/**
 * Real-city importer. Pulls OSM geometry (roads + coastline + inland water) for
 * a city, projects it into the game's world square, simplifies, bakes a water
 * mask, and writes a compact bundle to src/data/cities/<key>.json — plus a
 * preview PNG so we can eyeball recognizability without the engine.
 *
 *   npx vite-node scripts/build-cities.ts          # all configured cities
 *   npx vite-node scripts/build-cities.ts nyc      # one city
 *
 * Raw Overpass responses are cached in /tmp so re-runs are fast.
 */
import { mkdirSync, writeFileSync, existsSync, readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { encodePng } from './png';
import { expandBboxToSquare } from './geo-utils';
import { DemSampler } from './dem';

const WORLD = 12000; // fit each city into a 12km square (matches medium map)
const HALF = WORLD / 2;
const MASK_RES = 640; // water/park bitmask resolution over the world square (~19m/cell)
/** Real-elevation heightfield resolution over the world square (~47m/cell).
 *  Shipped as a dedicated i16-meters channel decoupled from the coarse 96²
 *  sim field — see the `elevation` bundle field / scripts/dem.ts. */
const ELEV_RES = 256;

/** Pack a 0/1 mask to 1 bit per cell, base64. */
function packMask(bits: Uint8Array): string {
  const packed = new Uint8Array(Math.ceil(bits.length / 8));
  for (let i = 0; i < bits.length; i++) if (bits[i]) packed[i >> 3]! |= 1 << (i & 7);
  return Buffer.from(packed).toString('base64');
}

export interface CityCfg {
  key: string;
  label: string;
  /** OSM bbox: south, west, north, east */
  bbox: [number, number, number, number];
}

// bboxes must match scripts/extract-water.ts
export const CITIES: CityCfg[] = [
  { key: 'nyc', label: 'New York', bbox: [40.695, -74.02, 40.80, -73.93] },
  { key: 'boston', label: 'Boston', bbox: [42.33, -71.11, 42.40, -71.02] },
  { key: 'chicago', label: 'Chicago', bbox: [41.83, -87.70, 41.95, -87.58] },
  { key: 'cleveland', label: 'Cleveland', bbox: [41.45, -81.75, 41.54, -81.63] },
  { key: 'la', label: 'Los Angeles', bbox: [33.99, -118.30, 34.10, -118.18] },
  { key: 'atlanta', label: 'Atlanta', bbox: [33.72, -84.44, 33.82, -84.34] },
  // ~12 km downtown cores with transit-history hooks
  { key: 'philly', label: 'Philadelphia', bbox: [39.925, -75.20, 39.985, -75.12] },
  { key: 'sf', label: 'San Francisco', bbox: [37.74, -122.48, 37.82, -122.38] },
  { key: 'dc', label: 'Washington', bbox: [38.86, -77.07, 38.94, -76.97] },
  { key: 'seattle', label: 'Seattle', bbox: [47.57, -122.38, 47.65, -122.28] },
];

type LL = { lat: number; lon: number };
type Way = { tags: Record<string, string>; geometry: LL[] };

const ENDPOINTS = [
  'https://overpass-api.de/api/interpreter',
  'https://overpass.kumi.systems/api/interpreter',
  'https://maps.mail.ru/osm/tools/overpass/api/interpreter',
];
function sleep(ms: number): void {
  execFileSync('sleep', [String(ms / 1000)]);
}
type OsmEl = {
  type: string;
  tags?: Record<string, string>;
  geometry?: LL[];
  members?: { type: string; role: string; geometry?: LL[] }[];
};
/** Overpass replies HTTP 200 with a JSON body that still starts with `{` even
 *  when the server-side query timed out or errored — the partial/empty result
 *  carries a top-level `remark` like "runtime error: Query timed out ...". Such
 *  a response must NEVER be cached as success, or a silently-truncated road/
 *  water fetch (exactly what leaves square-expanded margins like NJ / outer
 *  Brooklyn empty) gets frozen into the bundle. */
function isTruncated(json: string): boolean {
  try {
    const remark = (JSON.parse(json) as { remark?: string }).remark;
    return typeof remark === 'string' && /timed out|runtime error|out of memory/i.test(remark);
  } catch {
    return true; // unparseable => treat as truncated/failed
  }
}

/** FNV-1a hex fingerprint of the fully-expanded Overpass query. Folded into the
 *  cache filename so a bbox change (e.g. the square-bbox expansion) — or any
 *  query edit — invalidates the /tmp cache instead of silently reusing a
 *  response fetched for the OLD area. The bare `${key}-roads` key did not, which
 *  is how stale old-bbox roads could survive a re-run. */
function queryFingerprint(query: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < query.length; i++) {
    h ^= query.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16).padStart(8, '0');
}

function fetchRaw(query: string, cacheKey: string): OsmEl[] {
  const cache = `/tmp/osm-${cacheKey}-${queryFingerprint(query)}.json`;
  let json = '';
  if (existsSync(cache)) {
    const cached = readFileSync(cache, 'utf8');
    if (cached.trimStart().startsWith('{') && !isTruncated(cached)) json = cached;
  }
  if (!json) {
    // per-key query file: a single shared /tmp/q.overpass gets clobbered when
    // two importer runs overlap, silently caching the WRONG query's response
    const qFile = `/tmp/q-${cacheKey}.overpass`;
    writeFileSync(qFile, query);
    for (let attempt = 0; attempt < 6 && (!json.trimStart().startsWith('{') || isTruncated(json)); attempt++) {
      const url = ENDPOINTS[attempt % ENDPOINTS.length]!;
      try {
        json = execFileSync(
          'curl',
          ['-s', '--max-time', '420', '-G', url, '--data-urlencode', `data@${qFile}`],
          { encoding: 'utf8', maxBuffer: 256 * 1024 * 1024 },
        );
      } catch {
        json = '';
      }
      if (!json.trimStart().startsWith('{') || isTruncated(json)) {
        const why = json.trimStart().startsWith('{') ? 'server-side timeout/partial' : 'endpoint busy';
        console.log(`  ${cacheKey}: retry ${attempt + 1} (${why}), backing off…`);
        json = '';
        sleep(6000);
      }
    }
    if (!json.trimStart().startsWith('{') || isTruncated(json)) throw new Error(`Overpass failed for ${cacheKey}`);
    writeFileSync(cache, json);
  }
  return (JSON.parse(json) as { elements: OsmEl[] }).elements;
}

/** Ways with usable geometry (roads, coastline). */
function waysOf(els: OsmEl[]): Way[] {
  return els
    .filter((e) => e.type === 'way' && e.geometry && e.geometry.length >= 2)
    .map((e) => ({ tags: e.tags ?? {}, geometry: e.geometry as LL[] }));
}
/** Closed rings from ways + relation outer members (water/park polygons). */
function ringsOf(els: OsmEl[]): LL[][] {
  const rings: LL[][] = [];
  for (const e of els) {
    if (e.type === 'way' && e.geometry && e.geometry.length >= 3) rings.push(e.geometry);
    else if (e.type === 'relation' && e.members) {
      for (const m of e.members) {
        if (m.type === 'way' && m.role !== 'inner' && m.geometry && m.geometry.length >= 3) rings.push(m.geometry);
      }
    }
  }
  return rings;
}

/** Stitch member ways into closed rings by connecting shared endpoints — an OSM
 *  multipolygon's outer/inner boundary is often split across several ways. */
function assembleRings(ways: LL[][]): LL[][] {
  const key = (p: LL): string => `${p.lat.toFixed(7)},${p.lon.toFixed(7)}`;
  const rings: LL[][] = [];
  const rem = ways.filter((w) => w.length >= 2).map((w) => w.slice());
  while (rem.length) {
    const ring = rem.shift()!.slice();
    let go = true;
    while (go && key(ring[0]!) !== key(ring[ring.length - 1]!)) {
      go = false;
      const end = key(ring[ring.length - 1]!);
      const start = key(ring[0]!);
      for (let i = 0; i < rem.length; i++) {
        const s = rem[i]!;
        const a = key(s[0]!), b = key(s[s.length - 1]!);
        if (a === end) { ring.push(...s.slice(1)); }
        else if (b === end) { ring.push(...s.slice(0, -1).reverse()); }
        else if (b === start) { ring.unshift(...s.slice(0, -1)); }
        else if (a === start) { ring.unshift(...s.slice(1).reverse()); }
        else continue;
        rem.splice(i, 1); go = true; break;
      }
    }
    if (ring.length >= 3) rings.push(ring);
  }
  return rings;
}

/** Outer rings of building ways/relations WITH the tags that carry height info
 *  (a bare `classifyRings` throws tags away, which is fine for the raster mask
 *  but not for the vector export below). Inner (hole) rings are intentionally
 *  skipped for v1 — see buildBuildingsExport. */
function classifyBuildingRingsTagged(els: OsmEl[]): { ring: LL[]; tags: Record<string, string> }[] {
  const outers: { ring: LL[]; tags: Record<string, string> }[] = [];
  for (const e of els) {
    if (e.type === 'way' && e.geometry && e.geometry.length >= 3) {
      outers.push({ ring: e.geometry, tags: e.tags ?? {} });
    } else if (e.type === 'relation' && e.members) {
      const tags = e.tags ?? {};
      const ow: LL[][] = [];
      for (const m of e.members) {
        if (m.type !== 'way' || !m.geometry || m.geometry.length < 2) continue;
        if (m.role !== 'inner') ow.push(m.geometry);
      }
      for (const r of assembleRings(ow)) outers.push({ ring: r, tags });
    }
  }
  return outers;
}

/** Parse the leading number out of an OSM numeric tag value, tolerating unit
 *  suffixes like " m" (e.g. "48" or "48 m" both parse to 48). Undefined if
 *  the tag is absent or doesn't start with a number. */
function leadingFloat(s: string | undefined): number | undefined {
  if (s === undefined) return undefined;
  const m = /-?\d+(\.\d+)?/.exec(s.trim());
  if (!m) return undefined;
  const v = Number(m[0]);
  return Number.isFinite(v) ? v : undefined;
}

/** Parse a building height in meters from OSM tags, priority `height` (meters,
 *  ignoring unit suffixes like " m") > `building:height` > `building:levels`
 *  (× 3.2 m/level). Returns 0 (unknown) if none parse. */
function buildingHeightMeters(tags: Record<string, string>): number {
  const h = leadingFloat(tags.height);
  if (h !== undefined && h > 0) return h;
  const bh = leadingFloat(tags['building:height']);
  if (bh !== undefined && bh > 0) return bh;
  const lv = leadingFloat(tags['building:levels']);
  if (lv !== undefined && lv > 0) return lv * 3.2;
  return 0;
}

/** A building:part's own height in meters, priority height tag then
 *  building:levels (times 3.2 m per level); no building:height fallback
 *  here since that tag is an outline convention, not a part one. Returns 0
 *  (unknown, meaning use the containing outline's height instead) if
 *  neither parses. */
function partOwnHeightMeters(tags: Record<string, string>): number {
  const h = leadingFloat(tags.height);
  if (h !== undefined && h > 0) return h;
  const lv = leadingFloat(tags['building:levels']);
  if (lv !== undefined && lv > 0) return lv * 3.2;
  return 0;
}

interface MinHeight {
  meters: number;
  /** True only when a min_height/building:min_level tag was actually
   *  present and parsed; distinguishes "explicitly ground level" from "no
   *  data" so the min>=height skip rule only fires on real conflicting tag
   *  data, never on an implicit ground-level default. */
  explicit: boolean;
}

/** A building:part's base height in meters, priority min_height tag then
 *  building:min_level (times 3.2 m per level). Defaults to ground level
 *  (0, not explicit) when neither tag is present. */
function partMinHeightMeters(tags: Record<string, string>): MinHeight {
  const mh = leadingFloat(tags.min_height);
  if (mh !== undefined) return { meters: Math.max(0, mh), explicit: true };
  const ml = leadingFloat(tags['building:min_level']);
  if (ml !== undefined) return { meters: Math.max(0, ml * 3.2), explicit: true };
  return { meters: 0, explicit: false };
}

/** Shoelace signed area, computed directly on the exported (x, y-down)
 *  coordinates without any axis flip: Σ(x_i·y_{i+1} - x_{i+1}·y_i) / 2. We
 *  define "counter-clockwise" as POSITIVE by this formula, applied verbatim
 *  to the stored axes (x right, y down) — i.e. the same shoelace convention
 *  used for a normal x-right/y-up plane, just applied to whatever axes we
 *  actually stored. Because y is down here, a ring wound this way appears
 *  clockwise if you mentally flip to y-up/north-up viewing; that's expected
 *  and consumers should use this same formula (not a "visual" CCW check) to
 *  stay consistent. */
function signedArea2D(pts: [number, number][]): number {
  let a = 0;
  for (let i = 0; i < pts.length; i++) {
    const [x0, y0] = pts[i]!;
    const [x1, y1] = pts[(i + 1) % pts.length]!;
    a += x0 * y1 - x1 * y0;
  }
  return a / 2;
}

interface BuildingRecord {
  h: number; // height in decimeters, 0 = unknown
  mh: number; // min height (base of the mass above ground) in decimeters, 0 = ground based or unknown
  v: number[]; // flat [x0,y0,x1,y1,...] integer half-meters (world meters * 2, rounded)
}

/** Ring -> simplified, capped, CCW-normalized point list in projected meters,
 *  or null if it should be dropped (degenerate after simplification, or too
 *  small). `P` is the same world-space projection used for roads/water
 *  (already ~1 world unit = 1 m, y down, north up, origin-centered), so the
 *  1.2 m / 2.5 m tolerances below are literal meters in that space. Shared by
 *  outline and building:part processing so both go through one simplify/cap/
 *  winding pipeline. */
function simplifyRingPoints(ringLL: LL[], P: (ll: LL) => [number, number]): [number, number][] | null {
  let ring = ringLL;
  // OSM closed ways repeat the first point as the last; drop the duplicate so
  // rings are stored open (implicit closing edge), matching road polylines.
  if (ring.length > 1) {
    const first = ring[0]!, last = ring[ring.length - 1]!;
    if (first.lat === last.lat && first.lon === last.lon) ring = ring.slice(0, -1);
  }
  if (ring.length < 3) return null;
  const pts = ring.map(P);

  let simp = simplify(pts, 1.2);
  if (simp.length > 64) simp = simplify(pts, 2.5);
  let tol = 2.5;
  while (simp.length > 64) {
    tol *= 2;
    simp = simplify(pts, tol);
  }
  if (simp.length < 3) return null;

  const area = signedArea2D(simp);
  if (Math.abs(area) < 15) return null; // < 15 m^2 after simplification
  if (area < 0) simp = simp.slice().reverse(); // normalize to CCW (see signedArea2D doc)
  return simp;
}

/** Flat [x0,y0,x1,y1,...] integer half-meters (world meters * 2, rounded). */
function pointsToVertexInts(pts: [number, number][]): number[] {
  const v: number[] = new Array(pts.length * 2);
  for (let i = 0; i < pts.length; i++) {
    const [x, y] = pts[i]!;
    v[i * 2] = Math.round(x * 2);
    v[i * 2 + 1] = Math.round(y * 2);
  }
  return v;
}

/** Area-weighted polygon centroid (Green's theorem formula, matching the
 *  signedArea2D convention). Falls back to a plain vertex average for the
 *  (rare, near-degenerate) case where the signed area is ~0. */
function polygonCentroid(pts: [number, number][]): [number, number] {
  let a = 0, cx = 0, cy = 0;
  for (let i = 0; i < pts.length; i++) {
    const [x0, y0] = pts[i]!;
    const [x1, y1] = pts[(i + 1) % pts.length]!;
    const cross = x0 * y1 - x1 * y0;
    a += cross;
    cx += (x0 + x1) * cross;
    cy += (y0 + y1) * cross;
  }
  a /= 2;
  if (Math.abs(a) < 1e-6) {
    let sx = 0, sy = 0;
    for (const [x, y] of pts) { sx += x; sy += y; }
    return [sx / pts.length, sy / pts.length];
  }
  return [cx / (6 * a), cy / (6 * a)];
}

/** Ray-casting point-in-polygon test over a point-array ring (as opposed to
 *  the flat-array version used by the raster mask code below). */
function pointInPolygonPts(x: number, y: number, poly: [number, number][]): boolean {
  let inside = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i, i++) {
    const [xi, yi] = poly[i]!;
    const [xj, yj] = poly[j]!;
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

/** One outer ring -> one exported outline footprint, or null if it should be
 *  dropped. Outlines are always ground based (mh=0); see processPartFinal for
 *  building:part height/minHeight resolution. */
function processBuildingRing(ringLL: LL[], tags: Record<string, string>, P: (ll: LL) => [number, number]): BuildingRecord | null {
  const simp = simplifyRingPoints(ringLL, P);
  if (!simp) return null;
  const meters = buildingHeightMeters(tags);
  const h = meters > 0 ? Math.round(Math.min(500, Math.max(3, meters)) * 10) : 0;
  return { h, mh: 0, v: pointsToVertexInts(simp) };
}

/** Resolve a building:part's final {h, mh, v} once its simplified points and
 *  the meters-height of its containing outline (0 if it has none) are known,
 *  or null if the part should be dropped: a mapper-specified min_height/
 *  building:min_level at or above the resolved top height describes a part
 *  with no visible vertical extent. */
function processPartFinal(pts: [number, number][], tags: Record<string, string>, parentHeightMeters: number): BuildingRecord | null {
  const ownMeters = partOwnHeightMeters(tags);
  const heightMeters = ownMeters > 0 ? ownMeters : parentHeightMeters;
  const min = partMinHeightMeters(tags);
  if (heightMeters > 0 && min.explicit && min.meters >= heightMeters) return null;

  const h = heightMeters > 0 ? Math.round(Math.min(500, Math.max(3, heightMeters)) * 10) : 0;
  let mh = h > 0 ? Math.round(min.meters * 10) : 0;
  // independent rounding of h (clamped to a 3..500 m band) and mh (unclamped)
  // can occasionally push mh to or past h even when the pre-round meters
  // compared cleanly above; keep the mh < h invariant unconditionally.
  if (mh >= h) mh = Math.max(0, h - 1);
  return { h, mh, v: pointsToVertexInts(pts) };
}

interface OutlineCandidate {
  tags: Record<string, string>;
  pts: [number, number][];
  area: number; // abs m^2, same units as part areas below
  rec: BuildingRecord;
}

interface BuildingsExportStats {
  path: string;
  bytes: number;
  count: number;
  withHeight: number;
  outlinesKept: number;
  outlinesSuppressed: number;
  partsExported: number;
  partsSkipped: number;
  partsWithMinHeight: number;
}

/** Build and write the per-building vector export (real footprint polygons +
 *  heights), separate from the coverage-bitmask baked into the main bundle.
 *  `building:part` outer rings are simplified through the same pipeline as
 *  outlines, associated with a containing outline via a centroid-in-polygon
 *  test, and heights fall back to that outline's height when the part has
 *  none of its own. An outline whose contained parts cover 30% or more of
 *  its own footprint area is suppressed (the parts replace it); outlines
 *  with less coverage keep both, since the mapper only detailed a fragment. */
function buildBuildingsExport(
  key: string,
  buildingEls: OsmEl[],
  buildingPartEls: OsmEl[],
  P: (ll: LL) => [number, number],
): BuildingsExportStats {
  const outlineTagged = classifyBuildingRingsTagged(buildingEls);
  const outlines: OutlineCandidate[] = [];
  for (const { ring, tags } of outlineTagged) {
    const pts = simplifyRingPoints(ring, P);
    if (!pts) continue;
    const area = Math.abs(signedArea2D(pts));
    const meters = buildingHeightMeters(tags);
    const h = meters > 0 ? Math.round(Math.min(500, Math.max(3, meters)) * 10) : 0;
    outlines.push({ tags, pts, area, rec: { h, mh: 0, v: pointsToVertexInts(pts) } });
  }

  const partTagged = classifyBuildingRingsTagged(buildingPartEls);
  const partAreaByOutline = new Array<number>(outlines.length).fill(0);
  const parts: BuildingRecord[] = [];
  let partsSkipped = 0;
  let partsWithMinHeight = 0;
  for (const { ring, tags } of partTagged) {
    const pts = simplifyRingPoints(ring, P);
    if (!pts) { partsSkipped++; continue; }
    const area = Math.abs(signedArea2D(pts));
    const centroid = polygonCentroid(pts);
    let parentIdx = -1;
    for (let i = 0; i < outlines.length; i++) {
      if (pointInPolygonPts(centroid[0]!, centroid[1]!, outlines[i]!.pts)) { parentIdx = i; break; }
    }
    const parentHeightMeters = parentIdx >= 0 ? buildingHeightMeters(outlines[parentIdx]!.tags) : 0;
    const rec = processPartFinal(pts, tags, parentHeightMeters);
    if (!rec) { partsSkipped++; continue; }
    parts.push(rec);
    if (rec.mh > 0) partsWithMinHeight++;
    if (parentIdx >= 0) partAreaByOutline[parentIdx]! += area;
  }

  const suppressed = outlines.map((oc, i) => oc.area > 0 && partAreaByOutline[i]! / oc.area >= 0.3);
  const keptOutlines = outlines.filter((_, i) => !suppressed[i]);
  const outlinesSuppressed = outlines.length - keptOutlines.length;

  const buildings: BuildingRecord[] = [...keptOutlines.map((o) => o.rec), ...parts];
  const bundle = { version: 2, buildings };
  const path = `src/data/cities/${key}.buildings.json`;
  const json = JSON.stringify(bundle);
  writeFileSync(path, json);
  return {
    path,
    bytes: json.length,
    count: buildings.length,
    withHeight: buildings.filter((b) => b.h > 0).length,
    outlinesKept: keptOutlines.length,
    outlinesSuppressed,
    partsExported: parts.length,
    partsSkipped,
    partsWithMinHeight,
  };
}

/** Outer + inner (hole) rings, so multipolygon water with land holes is correct. */
function classifyRings(els: OsmEl[]): { outers: LL[][]; inners: LL[][] } {
  const outers: LL[][] = [];
  const inners: LL[][] = [];
  for (const e of els) {
    if (e.type === 'way' && e.geometry && e.geometry.length >= 3) outers.push(e.geometry);
    else if (e.type === 'relation' && e.members) {
      const ow: LL[][] = [], iw: LL[][] = [];
      for (const m of e.members) {
        if (m.type !== 'way' || !m.geometry || m.geometry.length < 2) continue;
        (m.role === 'inner' ? iw : ow).push(m.geometry);
      }
      for (const r of assembleRings(ow)) outers.push(r);
      for (const r of assembleRings(iw)) inners.push(r);
    }
  }
  return { outers, inners };
}

const CLASS: Record<string, 'arterial' | 'collector' | 'local'> = {
  motorway: 'arterial', trunk: 'arterial', primary: 'arterial',
  motorway_link: 'arterial', trunk_link: 'arterial', primary_link: 'arterial',
  secondary: 'collector', tertiary: 'collector', secondary_link: 'collector', tertiary_link: 'collector',
  residential: 'local', living_street: 'local', unclassified: 'local',
};

function build(cfg: CityCfg): void {
  // expand the configured bbox to a square (in meters) around its centroid
  // before doing anything else — Overpass queries, ocean-polygon lookup, and
  // the projection below all need to agree on the same square area, or the
  // shorter axis leaves the world square's east/west (or north/south) thirds
  // empty. See scripts/geo-utils.ts.
  const [s, w, n, e] = expandBboxToSquare(cfg.bbox);
  const bb = `${s},${w},${n},${e}`;
  const roads = waysOf(fetchRaw(
    `[out:json][timeout:180];way["highway"~"^(motorway|trunk|primary|secondary|tertiary|residential|living_street|unclassified)(_link)?$"](${bb});out geom;`,
    `${cfg.key}-roads`,
  ));
  // pre-assembled sea polygons (coastline-derived) from OpenStreetMapData.com,
  // extracted per city by scripts/extract-water.ts — topologically clean, no
  // flood/seed reconstruction. Inland water (lakes/rivers) still from OSM below.
  let ocean: { outers: LL[][]; inners: LL[][] } = { outers: [], inners: [] };
  const ocPath = `.cache/osmdata/${cfg.key}-ocean.json`;
  if (existsSync(ocPath)) {
    const raw = JSON.parse(readFileSync(ocPath, 'utf8')) as { outers: [number, number][][]; inners: [number, number][][] };
    const conv = (rings: [number, number][][]): LL[][] => rings.map((r) => r.map(([lat, lon]) => ({ lat, lon })));
    ocean = { outers: conv(raw.outers), inners: conv(raw.inners) };
  }
  // water: ways AND relations (lakes, rivers like the Charles, Chicago River)
  const waterEls = fetchRaw(
    `[out:json][timeout:180];(way["natural"="water"](${bb});way["waterway"="riverbank"](${bb});relation["natural"="water"](${bb});relation["waterway"="riverbank"](${bb}););out geom;`,
    `${cfg.key}-water2`,
  );
  const waterR = classifyRings(waterEls);
  // named river/bay centerlines for labels (Hudson, East River, Charles, ...)
  const waterwayEls = fetchRaw(
    `[out:json][timeout:180];(way["waterway"="river"]["name"](${bb});relation["natural"="water"]["name"](${bb}););out geom;`,
    `${cfg.key}-waterways`,
  );
  // parks/greens: ways AND relations (Boston Common, Central Park, ...)
  const parkEls = fetchRaw(
    `[out:json][timeout:180];(way["leisure"~"^(park|garden)$"](${bb});way["natural"="wood"](${bb});relation["leisure"="park"](${bb}););out geom;`,
    `${cfg.key}-parks`,
  );
  const parkRings = ringsOf(parkEls);
  // real building footprints (rasterized to a coverage mask, not stored as vectors)
  const buildingEls = fetchRaw(
    `[out:json][timeout:180];(way["building"](${bb});relation["building"](${bb}););out geom;`,
    `${cfg.key}-buildings`,
  );
  // building:part sub-masses (Empire State tiers, podium+tower splits, ...)
  // exported as extra stacked footprints alongside (or in place of) their
  // containing outline; see buildBuildingsExport.
  const buildingPartEls = fetchRaw(
    `[out:json][timeout:180];(way["building:part"](${bb});relation["building:part"](${bb}););out geom;`,
    `${cfg.key}-buildingparts`,
  );

  // equirectangular projection around bbox center, north-up, fit to world square
  const lat0 = (s + n) / 2;
  const lon0 = (w + e) / 2;
  const mx = (ll: LL): number => (ll.lon - lon0) * Math.cos((lat0 * Math.PI) / 180) * 111320;
  const my = (ll: LL): number => (ll.lat - lat0) * 110540; // north positive
  const spanX = (e - w) * Math.cos((lat0 * Math.PI) / 180) * 111320;
  const spanY = (n - s) * 110540;
  const scale = (WORLD * 0.94) / Math.max(spanX, spanY);
  const P = (ll: LL): [number, number] => [mx(ll) * scale, -my(ll) * scale]; // world: y down, north up

  // ── roads: classify, project, simplify ──
  const outRoads: { cls: string; pts: number[] }[] = [];
  for (const way of roads) {
    const cls = CLASS[way.tags.highway ?? ''];
    if (!cls) continue;
    const pts = way.geometry.map(P);
    const simp = simplify(pts, 5);
    if (simp.length < 2) continue;
    // OSM ways crossing the bbox edge keep their full geometry beyond the
    // fetched area; clip to the world square so no road point/segment can
    // render off the map edge, splitting into sub-polylines as needed.
    for (const seg of clipPolylineToBox(simp, HALF)) {
      outRoads.push({ cls, pts: seg.flatMap(([x, y]) => [Math.round(x), Math.round(y)]) });
    }
  }

  // union: pre-assembled sea polygons + inland OSM water (both with holes)
  const waterOuter: number[][] = [...ocean.outers, ...waterR.outers].map((ring) => ring.map(P).flat());
  const waterInner: number[][] = [...ocean.inners, ...waterR.inners].map((ring) => ring.map(P).flat());
  const parkPolys: number[][] = parkRings.map((ring) => ring.map(P).flat());

  // ── labels: real OSM names for roads / water / parks ──
  type Label = { kind: 'road' | 'water' | 'park'; name: string; x: number; y: number; angle?: number; imp: number };
  const labels: Label[] = [];
  const geomOf = (e: OsmEl): LL[] | null => e.geometry ?? e.members?.find((m) => m.geometry)?.geometry ?? null;
  const addAreaLabels = (els: OsmEl[], kind: 'water' | 'park'): void => {
    const seen = new Set<string>();
    for (const e of els) {
      const name = e.tags?.name;
      const geom = geomOf(e);
      if (!name || !geom || geom.length < 3 || seen.has(name)) continue;
      seen.add(name);
      let sx = 0, sy = 0, minx = 1e9, miny = 1e9, maxx = -1e9, maxy = -1e9;
      for (const ll of geom) { const [x, y] = P(ll); sx += x; sy += y; minx = Math.min(minx, x); miny = Math.min(miny, y); maxx = Math.max(maxx, x); maxy = Math.max(maxy, y); }
      const area = (maxx - minx) * (maxy - miny);
      if (area < 40000) continue; // skip tiny features
      labels.push({ kind, name, x: Math.round(sx / geom.length), y: Math.round(sy / geom.length), imp: 1 + Math.min(4, area / 1_500_000) });
    }
  };
  addAreaLabels([...waterEls, ...waterwayEls], 'water');
  addAreaLabels(parkEls, 'park');
  // roads: one label per named major road, at its longest member's midpoint
  const bestByName = new Map<string, typeof roads[number]>();
  for (const way of roads) {
    const name = way.tags.name;
    const cls = CLASS[way.tags.highway ?? ''];
    if (!name || !cls || cls === 'local') continue;
    const prev = bestByName.get(name);
    if (!prev || way.geometry.length > prev.geometry.length) bestByName.set(name, way);
  }
  for (const [name, way] of bestByName) {
    const g0 = way.geometry;
    const m = Math.floor(g0.length / 2);
    const [x, y] = P(g0[m]!);
    const [ax, ay] = P(g0[Math.max(0, m - 1)]!);
    const [bx, by] = P(g0[Math.min(g0.length - 1, m + 1)]!);
    let angle = Math.atan2(by - ay, bx - ax);
    if (angle > Math.PI / 2) angle -= Math.PI;
    if (angle < -Math.PI / 2) angle += Math.PI;
    labels.push({ kind: 'road', name, x: Math.round(x), y: Math.round(y), angle: Math.round(angle * 1000) / 1000, imp: CLASS[way.tags.highway ?? ''] === 'arterial' ? 2.5 : 1.6 });
  }
  // clamp every label anchor into the world square — a feature centroid can
  // sit slightly outside the fetched bbox, and roads/labels must never
  // render off the visible map (see clipPolylineToBox for the road-line
  // analog, applied above).
  const clampToWorld = (v: number): number => Math.max(-HALF, Math.min(HALF, v));
  for (const l of labels) { l.x = clampToWorld(l.x); l.y = clampToWorld(l.y); }

  const pointInPoly = (x: number, y: number, poly: number[]): boolean => {
    let inside = false;
    for (let i = 0, j = poly.length - 2; i < poly.length; j = i, i += 2) {
      const xi = poly[i]!, yi = poly[i + 1]!, xj = poly[j]!, yj = poly[j + 1]!;
      if ((yi > y) !== (yj > y) && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
    }
    return inside;
  };
  const N = MASK_RES;
  const cellW = WORLD / N;
  const cellOf = (x: number, y: number): number => {
    const c = Math.floor((x + HALF) / cellW);
    const r = Math.floor((y + HALF) / cellW);
    if (c < 0 || r < 0 || c >= N || r >= N) return -1;
    return r * N + c;
  };

  // ── Land/water by EXACT polygon rasterization (scanline). Fill every water
  // outer ring (pre-assembled sea + inland OSM water), then punch out the
  // inner-ring holes (land inside water — islands, the land side of a bay).
  // Fully deterministic: no coastline side-test, no flood, no seeds. ──
  void cellOf;
  const water = new Uint8Array(N * N);
  const fillInto = (grid: Uint8Array, poly: number[], val: number): void => {
    let minY = 1e9, maxY = -1e9;
    for (let k = 1; k < poly.length; k += 2) { minY = Math.min(minY, poly[k]!); maxY = Math.max(maxY, poly[k]!); }
    const r0 = Math.max(0, Math.floor((minY + HALF) / cellW));
    const r1 = Math.min(N - 1, Math.ceil((maxY + HALF) / cellW));
    for (let r = r0; r <= r1; r++) {
      const y = -HALF + (r + 0.5) * cellW;
      const xs: number[] = [];
      for (let k = 0, j = poly.length - 2; k < poly.length; j = k, k += 2) {
        const yi = poly[k + 1]!, yj = poly[j + 1]!;
        if ((yi > y) !== (yj > y)) xs.push(poly[k]! + ((poly[j]! - poly[k]!) * (y - yi)) / (yj - yi));
      }
      xs.sort((a, b) => a - b);
      for (let m = 0; m + 1 < xs.length; m += 2) {
        const c0 = Math.max(0, Math.ceil((xs[m]! + HALF) / cellW - 0.5));
        const c1 = Math.min(N - 1, Math.floor((xs[m + 1]! + HALF) / cellW - 0.5));
        for (let c = c0; c <= c1; c++) grid[r * N + c] = val;
      }
    }
  };
  for (const p of waterOuter) fillInto(water, p, 1); // water areas
  for (const p of waterInner) fillInto(water, p, 0); // land holes inside them

  // building coverage mask: rasterize real OSM footprints (40k+ per city) into
  // the grid — cheap regardless of count, renders as elegant flat city blocks
  const buildingBits = new Uint8Array(N * N);
  const buildingR = classifyRings(buildingEls);
  for (const ring of buildingR.outers) fillInto(buildingBits, ring.map(P).flat(), 1);
  for (const ring of buildingR.inners) fillInto(buildingBits, ring.map(P).flat(), 0);
  for (let i = 0; i < buildingBits.length; i++) if (water[i]) buildingBits[i] = 0; // never on water

  // real per-building footprint polygons + heights, written alongside the
  // rasterized coverage mask above (metroforge-native issue #6 data half,
  // building:part stacked-mass support per issue #16 item 1)
  mkdirSync('src/data/cities', { recursive: true });
  const buildingsExport = buildBuildingsExport(cfg.key, buildingEls, buildingPartEls, P);
  const kbB = (buildingsExport.bytes / 1024) | 0;
  console.log(
    `${cfg.key}: buildings vectors: ${buildingsExport.count} (${buildingsExport.withHeight} with real height, ` +
      `${buildingsExport.count - buildingsExport.withHeight} unknown) → ${buildingsExport.path} (${kbB} KB)`,
  );
  console.log(
    `${cfg.key}: outlines kept ${buildingsExport.outlinesKept}, outlines suppressed by parts ${buildingsExport.outlinesSuppressed}, ` +
      `parts exported ${buildingsExport.partsExported} (${buildingsExport.partsWithMinHeight} with minHeight>0), parts skipped ${buildingsExport.partsSkipped}`,
  );

  // bake final masks
  const bits = new Uint8Array(N * N);
  const parkBits = new Uint8Array(N * N);
  for (let r = 0; r < N; r++) {
    for (let c = 0; c < N; c++) {
      const i = r * N + c;
      bits[i] = water[i];
      if (!water[i]) {
        const x = -HALF + (c + 0.5) * cellW;
        const y = -HALF + (r + 0.5) * cellW;
        for (const poly of parkPolys) if (pointInPoly(x, y, poly)) { parkBits[i] = 1; break; }
      }
    }
  }

  // ── preview PNG ──
  writePreview(cfg.key, outRoads, bits, parkBits);

  // ── real elevation (terrarium DEM) ──
  // Bake an ELEV_RES² grid of real meters over the world square. Each cell
  // center is inverse-projected (undo the equirectangular P() above) back to
  // lon/lat, then sampled from the AWS Terrain Tiles DEM (scripts/dem.ts).
  // This is a STATIC per-city channel, decoupled from the coarse sim field —
  // identical across runs (baked into committed JSON), so it never perturbs
  // determinism/stateHash. Water cells keep their sampled seabed/shore value;
  // the renderer clamps them to sea level via the authoritative water mask.
  const dem = new DemSampler(cfg.bbox, 30);
  const cosLat0 = Math.cos((lat0 * Math.PI) / 180);
  const invLon = 1 / (cosLat0 * 111320 * scale);
  const invLat = 1 / (110540 * scale);
  const cellE = WORLD / ELEV_RES;
  const elev = new Int16Array(ELEV_RES * ELEV_RES);
  let eMin = Infinity;
  let eMax = -Infinity;
  for (let r = 0; r < ELEV_RES; r++) {
    const wy = -HALF + (r + 0.5) * cellE; // world y (south positive, y-down)
    const lat = lat0 - wy * invLat;
    for (let c = 0; c < ELEV_RES; c++) {
      const wx = -HALF + (c + 0.5) * cellE;
      const lon = lon0 + wx * invLon;
      const m = dem.sample(lon, lat);
      const clamped = Math.max(-32768, Math.min(32767, Math.round(m)));
      elev[r * ELEV_RES + c] = clamped;
      if (m < eMin) eMin = m;
      if (m > eMax) eMax = m;
    }
  }
  const elevB64 = Buffer.from(elev.buffer, elev.byteOffset, elev.byteLength).toString('base64');
  console.log(
    `${cfg.key}: elevation ${ELEV_RES}² @ z${dem.zoom} → ${eMin.toFixed(0)}..${eMax.toFixed(0)} m (${(elevB64.length / 1024) | 0} KB b64)`,
  );

  // ── bundle ──
  const bundle = {
    key: cfg.key,
    label: cfg.label,
    worldSize: WORLD,
    maskRes: MASK_RES,
    waterMask: packMask(bits),
    parkMask: packMask(parkBits),
    buildingMask: packMask(buildingBits),
    maskPacked: true,
    elevRes: ELEV_RES,
    /** base64 little-endian Int16 grid of real meters, row-major over the
     *  world square (row 0 = north edge, matching the mask convention). */
    elevation: elevB64,
    roads: outRoads,
    labels,
  };
  mkdirSync('src/data/cities', { recursive: true });
  const path = `src/data/cities/${cfg.key}.json`;
  writeFileSync(path, JSON.stringify(bundle));
  const kb = (JSON.stringify(bundle).length / 1024) | 0;
  console.log(`${cfg.key}: ${outRoads.length} roads, ${ocean.outers.length} sea, ${waterOuter.length} water, ${parkPolys.length} parks, ${buildingR.outers.length} buildings → ${path} (${kb} KB)`);
}

/** Clip a single segment [x0,y0]-[x1,y1] against the box [-half,half]^2 using
 *  Liang-Barsky; returns the clipped segment, or null if it lies entirely
 *  outside the box. */
function clipSegmentToBox(
  x0: number, y0: number, x1: number, y1: number, half: number,
): [number, number, number, number] | null {
  const dx = x1 - x0;
  const dy = y1 - y0;
  let t0 = 0;
  let t1 = 1;
  const p = [-dx, dx, -dy, dy];
  const q = [x0 - -half, half - x0, y0 - -half, half - y0];
  for (let i = 0; i < 4; i++) {
    if (p[i] === 0) {
      if (q[i]! < 0) return null; // parallel and outside
    } else {
      const r = q[i]! / p[i]!;
      if (p[i]! < 0) {
        if (r > t1) return null;
        if (r > t0) t0 = r;
      } else {
        if (r < t0) return null;
        if (r < t1) t1 = r;
      }
    }
  }
  return [x0 + t0 * dx, y0 + t0 * dy, x0 + t1 * dx, y0 + t1 * dy];
}

/** Clip a polyline against the world square [-half,half]^2, splitting it into
 *  one or more sub-polylines wherever it exits/re-enters the box. Any
 *  resulting sub-polyline with fewer than 2 points is dropped. Points are
 *  compared with a tiny epsilon so consecutive clipped segments that share an
 *  endpoint merge into one continuous polyline instead of fragmenting. */
function clipPolylineToBox(pts: [number, number][], half: number): [number, number][][] {
  const out: [number, number][][] = [];
  let cur: [number, number][] = [];
  const EPS = 1e-6;
  const last = (): [number, number] | undefined => cur[cur.length - 1];
  for (let i = 0; i + 1 < pts.length; i++) {
    const [ax, ay] = pts[i]!;
    const [bx, by] = pts[i + 1]!;
    const clipped = clipSegmentToBox(ax, ay, bx, by, half);
    if (!clipped) {
      if (cur.length >= 2) out.push(cur);
      cur = [];
      continue;
    }
    const [cx0, cy0, cx1, cy1] = clipped;
    const p = last();
    if (!p || Math.abs(p[0] - cx0) > EPS || Math.abs(p[1] - cy0) > EPS) {
      if (cur.length >= 2) out.push(cur);
      cur = [[cx0, cy0]];
    }
    cur.push([cx1, cy1]);
  }
  if (cur.length >= 2) out.push(cur);
  return out;
}

// Ramer–Douglas–Peucker
function simplify(pts: [number, number][], eps: number): [number, number][] {
  if (pts.length < 3) return pts;
  let maxD = 0;
  let idx = 0;
  const [ax, ay] = pts[0]!;
  const [bx, by] = pts[pts.length - 1]!;
  const dx = bx - ax, dy = by - ay;
  const L = Math.hypot(dx, dy) || 1;
  for (let i = 1; i < pts.length - 1; i++) {
    const [px, py] = pts[i]!;
    const d = Math.abs((px - ax) * dy - (py - ay) * dx) / L;
    if (d > maxD) { maxD = d; idx = i; }
  }
  if (maxD <= eps) return [pts[0]!, pts[pts.length - 1]!];
  return [...simplify(pts.slice(0, idx + 1), eps).slice(0, -1), ...simplify(pts.slice(idx), eps)];
}

function writePreview(key: string, roads: { cls: string; pts: number[] }[], bits: Uint8Array, parkBits: Uint8Array): void {
  const S = 3;
  const W = MASK_RES * S;
  const rgb = new Uint8Array(W * W * 3);
  const put = (px: number, py: number, r: number, g: number, b: number): void => {
    if (px < 0 || py < 0 || px >= W || py >= W) return;
    const o = (py * W + px) * 3;
    rgb[o] = r; rgb[o + 1] = g; rgb[o + 2] = b;
  };
  for (let r = 0; r < MASK_RES; r++) for (let c = 0; c < MASK_RES; c++) {
    const wtr = bits[r * MASK_RES + c] === 1;
    const park = parkBits[r * MASK_RES + c] === 1;
    const [rr, gg, bb] = wtr ? [26, 52, 82] : park ? [46, 82, 50] : [58, 66, 52];
    for (let sy = 0; sy < S; sy++) for (let sx = 0; sx < S; sx++) put(c * S + sx, r * S + sy, rr, gg, bb);
  }
  const toPx = (x: number, y: number): [number, number] => [
    Math.round(((x + HALF) / WORLD) * W),
    Math.round(((y + HALF) / WORLD) * W),
  ];
  const drawRoads = (cls: string, thick: number, col: [number, number, number]): void => {
    for (const road of roads) {
      if (road.cls !== cls) continue;
      const p = road.pts;
      for (let i = 0; i + 3 < p.length; i += 2) {
        const [ax, ay] = toPx(p[i]!, p[i + 1]!);
        const [bx, by] = toPx(p[i + 2]!, p[i + 3]!);
        const steps = Math.max(1, Math.ceil(Math.hypot(bx - ax, by - ay)));
        for (let s = 0; s <= steps; s++) {
          const x = Math.round(ax + ((bx - ax) * s) / steps);
          const y = Math.round(ay + ((by - ay) * s) / steps);
          for (let oy = -thick; oy <= thick; oy++) for (let ox = -thick; ox <= thick; ox++) put(x + ox, y + oy, ...col);
        }
      }
    }
  };
  drawRoads('local', 0, [120, 118, 110]);
  drawRoads('collector', 1, [180, 178, 168]);
  drawRoads('arterial', 1, [220, 210, 180]);
  mkdirSync('grader', { recursive: true });
  writeFileSync(`grader/city-${key}.png`, encodePng(W, W, rgb));
}

// Guarded so importing CITIES/build() from another script (e.g.
// scripts/height-join.ts, which needs the bbox table but must NOT trigger a
// full re-fetch/rebuild of every city as a side effect of import) is safe.
if (import.meta.url === `file://${process.argv[1]}`) {
  const only = process.argv[2];
  for (const c of CITIES) {
    if (only && c.key !== only) continue;
    build(c);
  }
  console.log('done. previews: grader/city-*.png');
}
