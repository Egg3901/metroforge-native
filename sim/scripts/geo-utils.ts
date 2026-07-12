/**
 * Shared geo helpers for the OSM city importer (build-cities.ts) and the
 * ocean-polygon extractor (extract-water.ts). Both scripts must query/clip
 * the SAME bbox per city or the ocean polygons and OSM roads/water won't
 * line up — see expandBboxToSquare.
 */

const LAT_M_PER_DEG = 110540; // meters per degree of latitude (~constant)
const LON_M_PER_DEG_AT_EQUATOR = 111320; // meters per degree of longitude at the equator

/**
 * Grow a [south, west, north, east] bbox so its lat/lon spans cover an EQUAL
 * area in meters (i.e. project to a square), centered on the original
 * bbox's centroid. The importer fits the bbox's longer axis to the 12km
 * world square; if the bbox itself isn't square, the shorter axis leaves
 * empty margin at the world edges. Expanding here (before any Overpass
 * queries or projection) means every downstream consumer — road/water
 * fetch, ocean extraction, and the projection math — works off one
 * consistent square area.
 */
export function expandBboxToSquare(bbox: [number, number, number, number]): [number, number, number, number] {
  const [s, w, n, e] = bbox;
  const lat0 = (s + n) / 2;
  const lon0 = (w + e) / 2;
  const lonMPerDeg = LON_M_PER_DEG_AT_EQUATOR * Math.cos((lat0 * Math.PI) / 180);

  const latSpanM = (n - s) * LAT_M_PER_DEG;
  const lonSpanM = (e - w) * lonMPerDeg;
  const target = Math.max(latSpanM, lonSpanM);

  const halfLatDeg = target / 2 / LAT_M_PER_DEG;
  const halfLonDeg = target / 2 / lonMPerDeg;

  return [lat0 - halfLatDeg, lon0 - halfLonDeg, lat0 + halfLatDeg, lon0 + halfLonDeg];
}
