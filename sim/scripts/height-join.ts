/**
 * Height-join pass: fills in missing `height` values on an already-exported
 * city buildings bundle (src/data/cities/<key>.buildings.json) from an
 * external footprint+height dataset, WITHOUT touching footprint geometry.
 *
 * Winner from the 3-source evaluation (Overture / Microsoft Global ML
 * Building Footprints / Open City Model) for Cleveland: see
 * scripts/height-join-cleveland-report.md (or the PR body) for the full
 * comparison table.
 *
 * Usage:
 *   npx vite-node scripts/height-join.ts cleveland --source=ms
 *   npx vite-node scripts/height-join.ts cleveland --source=overture
 *
 * Raw source downloads are cached under .cache/ (gitignored) so re-runs are
 * fast and don't re-hit S3/Overpass-adjacent hosts.
 */
import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { gunzipSync } from 'node:zlib';
import { CITIES } from './build-cities';
import { expandBboxToSquare } from './geo-utils';

const WORLD = 12000;

interface BuildingRecord {
  h: number; // decimeters, 0 = unknown
  mh: number;
  v: number[]; // flat [x0,y0,x1,y1,...] world half-meters (meters*2, rounded)
}

interface SourcePoint {
  lat: number;
  lon: number;
  h: number; // meters, source-reported height (may be -1/absent for "no data")
}

/** Same equirectangular projection build-cities.ts uses, parameterized on the
 *  SQUARE bbox (must match: buildings.json coordinates were baked with this
 *  exact projection). Returns projector + inverse for a city bbox. */
function makeProjection(sqBbox: [number, number, number, number]) {
  const [s, w, n, e] = sqBbox;
  const lat0 = (s + n) / 2;
  const lon0 = (w + e) / 2;
  const cosLat0 = Math.cos((lat0 * Math.PI) / 180);
  const mx = (lon: number) => (lon - lon0) * cosLat0 * 111320;
  const my = (lat: number) => (lat - lat0) * 110540;
  const spanX = (e - w) * cosLat0 * 111320;
  const spanY = (n - s) * 110540;
  const scale = (WORLD * 0.94) / Math.max(spanX, spanY);
  const project = (lat: number, lon: number): [number, number] => [mx(lon) * scale, -my(lat) * scale];
  return { project };
}

/** Point-in-polygon (ray casting) against a flat [x0,y0,x1,y1,...] ring in
 *  the SAME half-meter integer units buildings.json stores (v is meters*2). */
function pointInRing(px: number, py: number, v: number[]): boolean {
  let inside = false;
  const n = v.length / 2;
  for (let i = 0, j = n - 1; i < n; j = i++) {
    const xi = v[i * 2]!, yi = v[i * 2 + 1]!;
    const xj = v[j * 2]!, yj = v[j * 2 + 1]!;
    const intersect = yi > py !== yj > py && px < ((xj - xi) * (py - yi)) / (yj - yi) + xi;
    if (intersect) inside = !inside;
  }
  return inside;
}

function ringCentroid(v: number[]): [number, number] {
  let sx = 0, sy = 0;
  const n = v.length / 2;
  for (let i = 0; i < n; i++) { sx += v[i * 2]!; sy += v[i * 2 + 1]!; }
  return [sx / n, sy / n];
}

function ringBBox(v: number[]): [number, number, number, number] {
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (let i = 0; i < v.length; i += 2) {
    const x = v[i]!, y = v[i + 1]!;
    if (x < minX) minX = x;
    if (x > maxX) maxX = x;
    if (y < minY) minY = y;
    if (y > maxY) maxY = y;
  }
  return [minX, minY, maxX, maxY];
}

const CELL = 100; // half-meter units grid cell (~50m) for the spatial index

/** Grid-bucket footprint indices by bbox for fast point→candidate lookup.
 *  Buildings register in every cell their bbox overlaps. */
function buildGrid(buildings: BuildingRecord[]) {
  const grid = new Map<string, number[]>();
  const bboxes: [number, number, number, number][] = [];
  for (let i = 0; i < buildings.length; i++) {
    const bb = ringBBox(buildings[i]!.v);
    bboxes.push(bb);
    const [minX, minY, maxX, maxY] = bb;
    const cx0 = Math.floor(minX / CELL), cx1 = Math.floor(maxX / CELL);
    const cy0 = Math.floor(minY / CELL), cy1 = Math.floor(maxY / CELL);
    for (let cx = cx0; cx <= cx1; cx++) {
      for (let cy = cy0; cy <= cy1; cy++) {
        const key = `${cx},${cy}`;
        let arr = grid.get(key);
        if (!arr) grid.set(key, (arr = []));
        arr.push(i);
      }
    }
  }
  return { grid, bboxes };
}

/** Find the footprint whose polygon contains (px,py) in half-meter units,
 *  falling back to nearest centroid within maxDistM meters if no polygon
 *  contains the point (handles source-point/footprint edge jitter). */
function findMatch(
  px: number,
  py: number,
  buildings: BuildingRecord[],
  grid: Map<string, number[]>,
  maxDistM: number,
): number {
  const cx = Math.floor(px / CELL), cy = Math.floor(py / CELL);
  let nearestIdx = -1;
  let nearestD2 = (maxDistM * 2) * (maxDistM * 2); // half-meter units
  for (let dx = -1; dx <= 1; dx++) {
    for (let dy = -1; dy <= 1; dy++) {
      const arr = grid.get(`${cx + dx},${cy + dy}`);
      if (!arr) continue;
      for (const idx of arr) {
        const b = buildings[idx]!;
        if (pointInRing(px, py, b.v)) return idx;
        const [ccx, ccy] = ringCentroid(b.v);
        const d2 = (ccx - px) ** 2 + (ccy - py) ** 2;
        if (d2 < nearestD2) { nearestD2 = d2; nearestIdx = idx; }
      }
    }
  }
  return nearestIdx;
}

// ── source loaders ──────────────────────────────────────────────────────

/** Microsoft Global ML Building Footprints (with height). Downloads the z9
 *  quadkey tile(s) covering the bbox from dataset-links.csv, caches under
 *  .cache/, filters to bbox by ring centroid. */
function loadMsSource(sqBbox: [number, number, number, number]): SourcePoint[] {
  const [s, w, n, e] = sqBbox;
  const cacheDir = '.cache';
  if (!existsSync(cacheDir)) mkdirSync(cacheDir);
  const linksPath = `${cacheDir}/ms-dataset-links.csv`;
  if (!existsSync(linksPath)) {
    execFileSync('curl', ['-s', '-m', '60', '-o', linksPath,
      'https://minedbuildings.z5.web.core.windows.net/global-buildings/dataset-links.csv']);
  }
  const csv = readFileSync(linksPath, 'utf8');
  const quads = new Set(quadkeysForBbox(s, w, n, e, 9));
  const urls: string[] = [];
  for (const line of csv.split('\n')) {
    if (!line.startsWith('UnitedStates,')) continue;
    const parts = line.split(',');
    const qk = parts[1]!;
    if (quads.has(qk)) urls.push(parts[2]!);
  }
  const points: SourcePoint[] = [];
  for (const url of urls) {
    const fname = `${cacheDir}/ms-${url.split('quadkey=')[1]!.split('/')[0]}.csv.gz`;
    if (!existsSync(fname)) {
      execFileSync('curl', ['-s', '-m', '180', '-o', fname, url]);
    }
    const gz = readFileSync(fname);
    const text = gunzipSync(gz).toString('utf8');
    for (const line of text.split('\n')) {
      if (!line.trim()) continue;
      let rec: any;
      try { rec = JSON.parse(line); } catch { continue; }
      const coords: [number, number][] = rec.geometry.coordinates[0];
      let clon = 0, clat = 0;
      for (const [lo, la] of coords) { clon += lo; clat += la; }
      clon /= coords.length; clat /= coords.length;
      if (clat < s || clat > n || clon < w || clon > e) continue;
      points.push({ lat: clat, lon: clon, h: rec.properties?.height ?? -1 });
    }
  }
  return points;
}

function quadkeysForBbox(s: number, w: number, n: number, e: number, zoom: number): string[] {
  const toQuad = (lat: number, lon: number): string => {
    const latRad = (lat * Math.PI) / 180;
    const nn = 2 ** zoom;
    const xtile = Math.floor(((lon + 180) / 360) * nn);
    const ytile = Math.floor(
      ((1 - Math.log(Math.tan(latRad) + 1 / Math.cos(latRad)) / Math.PI) / 2) * nn,
    );
    let q = '';
    for (let i = zoom; i > 0; i--) {
      let digit = 0;
      const mask = 1 << (i - 1);
      if (xtile & mask) digit += 1;
      if (ytile & mask) digit += 2;
      q += digit;
    }
    return q;
  };
  const set = new Set<string>();
  for (const [lat, lon] of [[s, w], [s, e], [n, w], [n, e], [(s + n) / 2, (w + e) / 2]] as [number, number][]) {
    set.add(toQuad(lat, lon));
  }
  return [...set];
}

/** Overture Maps buildings theme (S3 GeoParquet, anonymous access). Requires
 *  `duckdb` python package (pip install duckdb) on PATH via `python3`; shells
 *  out to a tiny inline query and reads back a cached ndjson dump. This is
 *  the evaluated-but-NOT-wired-as-winner path for Cleveland — see the PR
 *  comparison table. Kept here so a future city run can pick Overture if a
 *  faster S3 path (e.g. bbox-partitioned release) becomes available. */
function loadOvertureSource(sqBbox: [number, number, number, number]): SourcePoint[] {
  const [s, w, n, e] = sqBbox;
  const cacheDir = '.cache';
  const ndjsonPath = `${cacheDir}/overture-buildings.ndjson`;
  if (!existsSync(ndjsonPath)) {
    const py = `
import duckdb, json
con = duckdb.connect()
con.execute("INSTALL httpfs; LOAD httpfs; INSTALL spatial; LOAD spatial;")
con.execute("SET s3_region='us-west-2';")
q = """
  SELECT height, num_floors,
         ST_X(ST_Centroid(geometry)) AS lon,
         ST_Y(ST_Centroid(geometry)) AS lat
  FROM read_parquet('s3://overturemaps-us-west-2/release/2026-05-20.0/theme=buildings/type=building/*.parquet', hive_partitioning=1)
  WHERE bbox.xmin BETWEEN ${w} AND ${e} AND bbox.ymin BETWEEN ${s} AND ${n}
"""
for row in con.execute(q).fetchall():
    print(json.dumps({"h": row[0], "f": row[1], "lon": row[2], "lat": row[3]}))
`;
    const out = execFileSync('python3', ['-c', py], { maxBuffer: 1024 * 1024 * 512 });
    writeFileSync(ndjsonPath, out);
  }
  const points: SourcePoint[] = [];
  for (const line of readFileSync(ndjsonPath, 'utf8').split('\n')) {
    if (!line.trim()) continue;
    const rec = JSON.parse(line);
    if (rec.lat < s || rec.lat > n || rec.lon < w || rec.lon > e) continue;
    points.push({ lat: rec.lat, lon: rec.lon, h: rec.h ?? -1 });
  }
  return points;
}

// ── main join ────────────────────────────────────────────────────────────

export function heightJoin(cityKey: string, source: 'ms' | 'overture'): void {
  const cfg = CITIES.find((c) => c.key === cityKey);
  if (!cfg) throw new Error(`unknown city ${cityKey}`);
  const sqBbox = expandBboxToSquare(cfg.bbox);
  const { project } = makeProjection(sqBbox);

  const bundlePath = `src/data/cities/${cityKey}.buildings.json`;
  const bundle = JSON.parse(readFileSync(bundlePath, 'utf8')) as { version: number; buildings: BuildingRecord[] };
  const before = bundle.buildings.filter((b) => b.h > 0).length;

  // snapshot footprint geometry so we can prove it's untouched after the join
  const geomBefore = bundle.buildings.map((b) => b.v.join(','));

  const points = source === 'ms' ? loadMsSource(sqBbox) : loadOvertureSource(sqBbox);
  const { grid } = buildGrid(bundle.buildings);

  let matched = 0;
  let newHeights = 0;
  for (const pt of points) {
    if (!(pt.h > 0)) continue; // source has no usable height for this footprint
    const [px, py] = project(pt.lat, pt.lon);
    // findMatch operates in half-meter units (v is meters*2)
    const idx = findMatch(px * 2, py * 2, bundle.buildings, grid, 15);
    if (idx < 0) continue;
    matched++;
    const b = bundle.buildings[idx]!;
    if (b.h === 0) {
      b.h = Math.round(Math.min(500, Math.max(3, pt.h)) * 10); // decimeters, same convention as build-cities.ts
      newHeights++;
    }
  }

  // verify footprint geometry untouched
  const geomAfter = bundle.buildings.map((b) => b.v.join(','));
  for (let i = 0; i < geomBefore.length; i++) {
    if (geomBefore[i] !== geomAfter[i]) throw new Error(`footprint geometry mutated at index ${i}`);
  }

  const after = bundle.buildings.filter((b) => b.h > 0).length;
  writeFileSync(bundlePath, JSON.stringify(bundle));
  console.log(
    `${cityKey}: height-join(${source}) matched ${matched} source points, filled ${newHeights} new heights. ` +
      `Coverage ${before}/${bundle.buildings.length} (${((before / bundle.buildings.length) * 100).toFixed(1)}%) -> ` +
      `${after}/${bundle.buildings.length} (${((after / bundle.buildings.length) * 100).toFixed(1)}%)`,
  );
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const cityKey = process.argv[2];
  const sourceArg = process.argv.find((a) => a.startsWith('--source='));
  const source = (sourceArg?.split('=')[1] as 'ms' | 'overture' | undefined) ?? 'ms';
  if (!cityKey) {
    console.error('usage: npx vite-node scripts/height-join.ts <city> [--source=ms|overture]');
    process.exit(1);
  }
  heightJoin(cityKey, source);
}
