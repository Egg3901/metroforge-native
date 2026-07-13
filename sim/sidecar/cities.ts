/**
 * Static registry of the 10 real-city OSM bundles. `bun build --compile`
 * cannot resolve a dynamic `import()` at binary boot (spec §2.1 / risk #2),
 * so every city JSON is imported statically here and embedded in the
 * executable (~6.4 MB total — acceptable). `resolveCity` replaces
 * `@core/city/osmRegistry`'s async `loadOsmCity`, which is the one thing in
 * the host layer that cannot be reused verbatim (its dynamic-import path).
 */
import { OSM_CITY_KEYS } from '@core/city/osmRegistry';
import { presetByKey } from '@core/city/presets';
import type { OsmCityData } from '@core/city/osmCity';

import atlanta from '../src/data/cities/atlanta.json';
import boston from '../src/data/cities/boston.json';
import chicago from '../src/data/cities/chicago.json';
import cleveland from '../src/data/cities/cleveland.json';
import dc from '../src/data/cities/dc.json';
import la from '../src/data/cities/la.json';
import nyc from '../src/data/cities/nyc.json';
import philly from '../src/data/cities/philly.json';
import seattle from '../src/data/cities/seattle.json';
import sf from '../src/data/cities/sf.json';

// Per-building footprint vectors (metroforge-native issue #6). Only cities
// with a generated `<key>.buildings.json` are imported — `bun build --compile`
// requires static imports (no dynamic `import()` at binary boot, same reason
// the OsmCityData bundles above are static), so cities without a file simply
// have no entry here and resolveBuildings returns undefined for them.
import clevelandBuildings from '../src/data/cities/cleveland.buildings.json';
import nycBuildings from '../src/data/cities/nyc.buildings.json';

export interface CityListEntry {
  key: string;
  label: string;
}

/** One real-OSM building footprint as produced by scripts/build-cities.ts:
 *  `v` flat [x0,y0,...] outer-ring vertices in integer half-meters, `h`
 *  top height in decimeters (0 = unknown), `mh` base (min) height in
 *  decimeters (0 = ground based or unknown; building:part sub-masses only). */
export interface BuildingsData {
  version: number;
  buildings: { h: number; mh: number; v: number[] }[];
}

const BUILDINGS_DATA: Partial<Record<string, BuildingsData>> = {
  cleveland: clevelandBuildings as unknown as BuildingsData,
  nyc: nycBuildings as unknown as BuildingsData,
};

const CITY_DATA: Record<string, OsmCityData> = {
  nyc: nyc as unknown as OsmCityData,
  boston: boston as unknown as OsmCityData,
  chicago: chicago as unknown as OsmCityData,
  cleveland: cleveland as unknown as OsmCityData,
  la: la as unknown as OsmCityData,
  atlanta: atlanta as unknown as OsmCityData,
  philly: philly as unknown as OsmCityData,
  sf: sf as unknown as OsmCityData,
  dc: dc as unknown as OsmCityData,
  seattle: seattle as unknown as OsmCityData,
};

/** `{key,label}` list for the `hello` handshake's `cityList`, in the same
 *  order `@core/city/osmRegistry` enumerates the OSM-backed presets. */
export const CITY_LIST: CityListEntry[] = OSM_CITY_KEYS.map((key) => ({ key, label: presetByKey(key).label }));

/** Synchronous replacement for `loadOsmCity(key)` — every dataset is already
 *  resident in the binary, so there is nothing to await. */
export function resolveCity(key: string | undefined): OsmCityData | undefined {
  if (key === undefined) return undefined;
  return CITY_DATA[key];
}

/** Building-vector data for `key`, or undefined if that city has none yet
 *  (NYC + Cleveland so far — every other city gracefully has no buildings frame). */
export function resolveBuildings(key: string | undefined): BuildingsData | undefined {
  if (key === undefined) return undefined;
  return BUILDINGS_DATA[key];
}
