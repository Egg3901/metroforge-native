/**
 * Additive static-city wire extras that live on the baked OSM bundle but are
 * not part of the core `RoadEdge` / `GameState` types. Both `sim.worker.ts`
 * and `sidecar/simHost.ts` must apply the same mapping when building the
 * `ready.staticCity` payload.
 */
import type { RoadEdge } from '@core/types';

export type PoiAnchorWire = {
  id: string;
  kind: 'stadium' | 'airport' | 'university' | 'hospital' | 'museum';
  name: string;
  centroid: [number, number];
  area?: number;
};

type OsmRoadWire = { pts: number[]; name?: string; wikidata?: string };

type OsmBundleWire = { roads: OsmRoadWire[]; poiAnchors?: PoiAnchorWire[] };

/** Per-road name/wikidata aligned with `generateCity`'s OSM import (skips
 *  segments with fewer than four point scalars, same as generator.ts). */
export function roadMetaFromOsm(osm: OsmBundleWire | undefined): { name?: string; wikidata?: string }[] {
  if (!osm) return [];
  const out: { name?: string; wikidata?: string }[] = [];
  for (const r of osm.roads) {
    if (r.pts.length < 4) continue;
    const meta: { name?: string; wikidata?: string } = {};
    if (r.name) meta.name = r.name;
    if (r.wikidata) meta.wikidata = r.wikidata;
    out.push(meta);
  }
  return out;
}

export function poiAnchorsFromOsm(osm: OsmBundleWire | undefined): PoiAnchorWire[] | undefined {
  const anchors = osm?.poiAnchors;
  return anchors?.length ? anchors : undefined;
}

export type StaticRoadWire = {
  cls: string;
  points: number[];
  gradeLevel?: number;
  isBridge?: boolean;
  isTunnel?: boolean;
  name?: string;
  wikidata?: string;
};

/** Map a sim `RoadEdge` plus optional OSM meta into the static-city road DTO. */
export function staticRoadWire(r: RoadEdge, meta: { name?: string; wikidata?: string } | undefined): StaticRoadWire {
  const road: StaticRoadWire = {
    cls: r.cls,
    points: r.polyline.points.flatMap((p) => [p.x, p.y]),
    gradeLevel: r.gradeLevel ?? 0,
    isBridge: r.isBridge ?? false,
    isTunnel: r.isTunnel ?? false,
  };
  if (meta?.name) road.name = meta.name;
  if (meta?.wikidata) road.wikidata = meta.wikidata;
  return road;
}
