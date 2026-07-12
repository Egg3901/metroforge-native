/**
 * Inject the real-elevation channel into an already-built city bundle.
 *
 * Full `build-cities.ts` re-derives masks/roads/buildings from OSM (and needs
 * the ocean-polygon cache to be present, or the water mask degrades). This
 * driver instead loads the committed `src/data/cities/<key>.json`, recomputes
 * ONLY the `elevRes`/`elevation` fields from the DEM using the exact same
 * projection math build-cities uses (deterministic from the city bbox), and
 * writes the bundle back — every other field preserved byte-for-byte.
 *
 *   bun run scripts/add-elevation.ts sf nyc
 */
import { readFileSync, writeFileSync } from 'node:fs';
import { expandBboxToSquare } from './geo-utils';
import { CITIES } from './build-cities';
import { DemSampler } from './dem';

const WORLD = 12000;
const HALF = WORLD / 2;
const ELEV_RES = 256;

function addElevation(key: string): void {
  const cfg = CITIES.find((c) => c.key === key);
  if (!cfg) throw new Error(`unknown city ${key}`);
  const path = `src/data/cities/${key}.json`;
  const bundle = JSON.parse(readFileSync(path, 'utf8')) as Record<string, unknown>;

  // Reproduce build-cities.ts's equirectangular projection exactly.
  const [s, w, n, e] = expandBboxToSquare(cfg.bbox);
  const lat0 = (s + n) / 2;
  const lon0 = (w + e) / 2;
  const cosLat0 = Math.cos((lat0 * Math.PI) / 180);
  const spanX = (e - w) * cosLat0 * 111320;
  const spanY = (n - s) * 110540;
  const scale = (WORLD * 0.94) / Math.max(spanX, spanY);
  const invLon = 1 / (cosLat0 * 111320 * scale);
  const invLat = 1 / (110540 * scale);

  const dem = new DemSampler(cfg.bbox, 30);
  const cellE = WORLD / ELEV_RES;
  const elev = new Int16Array(ELEV_RES * ELEV_RES);
  let eMin = Infinity;
  let eMax = -Infinity;
  for (let r = 0; r < ELEV_RES; r++) {
    const wy = -HALF + (r + 0.5) * cellE;
    const lat = lat0 - wy * invLat;
    for (let c = 0; c < ELEV_RES; c++) {
      const wx = -HALF + (c + 0.5) * cellE;
      const lon = lon0 + wx * invLon;
      const m = dem.sample(lon, lat);
      // Floor at sea level: terrarium encodes deep ocean bathymetry (down to
      // ~-850 m in NY Harbor). Water cells render flat via the water mask, but
      // a shoreline LAND vertex whose bilinear neighborhood touches an ocean
      // cell would otherwise get dragged sharply negative and spike the coast
      // downward. Real land here never sits meaningfully below sea level, so
      // clamp the floor to 0 (shorelines meet the water plane cleanly).
      elev[r * ELEV_RES + c] = Math.max(0, Math.min(32767, Math.round(m)));
      if (m < eMin) eMin = m;
      if (m > eMax) eMax = m;
    }
  }
  bundle.elevRes = ELEV_RES;
  bundle.elevation = Buffer.from(elev.buffer, elev.byteOffset, elev.byteLength).toString('base64');
  writeFileSync(path, JSON.stringify(bundle));
  console.log(
    `${key}: elevation ${ELEV_RES}² @ z${dem.zoom} → ${eMin.toFixed(0)}..${eMax.toFixed(0)} m → ${path}`,
  );
}

for (const key of process.argv.slice(2)) addElevation(key);
