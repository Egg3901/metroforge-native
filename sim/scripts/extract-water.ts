/**
 * Extract pre-assembled OSM water polygons (OpenStreetMapData.com, simplified,
 * EPSG:3857) for each city bbox → small cached GeoJSON-ish per city. These are
 * topologically clean, coastline-assembled sea polygons — no flood fill, no
 * seeds, no guessing. build-cities.ts unions these with inland natural=water.
 *
 *   npx vite-node scripts/extract-water.ts
 *
 * One-time prerequisite (the 24MB source, gitignored under .cache/):
 *   curl -o .cache/osmdata/water.zip \
 *     https://osmdata.openstreetmap.de/download/simplified-water-polygons-split-3857.zip
 *   (cd .cache/osmdata && unzip -o water.zip)
 */
import * as shapefile from 'shapefile';
import { writeFileSync, mkdirSync, readFileSync } from 'node:fs';
import { expandBboxToSquare } from './geo-utils';

// full-resolution water polygons (exact shoreline detail)
const SHP = '.cache/osmdata/water-polygons-split-3857/water_polygons';

/** city bboxes: [south, west, north, east] — keep in sync with build-cities.ts.
 *  Expanded to a square (see geo-utils.ts) so the ocean polygons extracted
 *  here cover exactly the same area build-cities.ts fetches/projects. */
const RAW_CITIES: Record<string, [number, number, number, number]> = {
  nyc: [40.695, -74.02, 40.80, -73.93],
  boston: [42.33, -71.11, 42.40, -71.02],
  chicago: [41.83, -87.70, 41.95, -87.58],
  cleveland: [41.45, -81.75, 41.54, -81.63],
  la: [33.99, -118.30, 34.10, -118.18],
  atlanta: [33.72, -84.44, 33.82, -84.34],
  philly: [39.925, -75.20, 39.985, -75.12],
  sf: [37.74, -122.48, 37.82, -122.38],
  dc: [38.86, -77.07, 38.94, -76.97],
  seattle: [47.57, -122.38, 47.65, -122.28],
};
const CITIES: Record<string, [number, number, number, number]> = Object.fromEntries(
  Object.entries(RAW_CITIES).map(([k, bbox]) => [k, expandBboxToSquare(bbox)]),
);

const R = 20037508.342789244;
const toLL = (x: number, y: number): [number, number] => [
  (x / R) * 180, // lon
  (Math.atan(Math.exp((y / R) * Math.PI)) * 2 - Math.PI / 2) * (180 / Math.PI), // lat
];

type Ring = [number, number][]; // [lat, lon]
const out: Record<string, { outers: Ring[]; inners: Ring[] }> = {};
for (const k of Object.keys(CITIES)) out[k] = { outers: [], inners: [] };

const ringLL = (coords: number[][]): { ring: Ring; minLa: number; maxLa: number; minLo: number; maxLo: number } => {
  const ring: Ring = [];
  let minLa = 1e9, maxLa = -1e9, minLo = 1e9, maxLo = -1e9;
  for (const [x, y] of coords) {
    const [lon, lat] = toLL(x, y);
    ring.push([lat, lon]);
    minLa = Math.min(minLa, lat); maxLa = Math.max(maxLa, lat);
    minLo = Math.min(minLo, lon); maxLo = Math.max(maxLo, lon);
  }
  return { ring, minLa, maxLa, minLo, maxLo };
};
const intersects = (bb: [number, number, number, number], mnLa: number, mxLa: number, mnLo: number, mxLo: number): boolean =>
  !(mxLa < bb[0] || mnLa > bb[2] || mxLo < bb[1] || mnLo > bb[3]);

const shpBuf = readFileSync(`${SHP}.shp`);
const dbfBuf = readFileSync(`${SHP}.dbf`);
const source = await shapefile.open(
  shpBuf.buffer.slice(shpBuf.byteOffset, shpBuf.byteOffset + shpBuf.byteLength),
  dbfBuf.buffer.slice(dbfBuf.byteOffset, dbfBuf.byteOffset + dbfBuf.byteLength),
);
let rec: { done: boolean; value?: GeoJSON.Feature };
let scanned = 0;
for (rec = await source.read(); !rec.done; rec = await source.read()) {
  scanned++;
  const geom = rec.value?.geometry as GeoJSON.Polygon | GeoJSON.MultiPolygon | undefined;
  if (!geom) continue;
  const polys = geom.type === 'Polygon' ? [geom.coordinates] : geom.coordinates;
  for (const poly of polys) {
    // poly[0] = outer ring, poly[1..] = holes
    poly.forEach((coords, ri) => {
      const { ring, minLa, maxLa, minLo, maxLo } = ringLL(coords as number[][]);
      for (const [key, bb] of Object.entries(CITIES)) {
        if (intersects(bb, minLa, maxLa, minLo, maxLo)) {
          (ri === 0 ? out[key]!.outers : out[key]!.inners).push(ring);
        }
      }
    });
  }
}

mkdirSync('.cache/osmdata', { recursive: true });
for (const [key, data] of Object.entries(out)) {
  writeFileSync(`.cache/osmdata/${key}-ocean.json`, JSON.stringify(data));
  console.log(`${key}: ${data.outers.length} ocean outers / ${data.inners.length} holes`);
}
console.log(`scanned ${scanned} shapefile features`);
