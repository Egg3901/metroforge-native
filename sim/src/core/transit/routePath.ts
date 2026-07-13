/**
 * Derived route geometry: the out-and-back polyline a route's vehicles follow.
 * Cached per route; cache key covers the segment list so edits invalidate.
 */
import { makePolyline } from '../geometry';
import type { Polyline, Vec2 } from '../geometry';
import type { GameState, RouteDef } from '../types';

const cache = new Map<string, { key: string; path: Polyline }>();

export function clearRoutePathCache(): void {
  cache.clear();
}

export function getRoutePath(state: GameState, route: RouteDef): Polyline | null {
  // scope the memo to the game instance + route: entity ids reset per newGame, so a
  // bare route.id key would return another game's stale polyline when one process
  // serves multiple games (sidecar/replay).
  const cacheKey = `${state.instanceId}:${route.id}`;
  const key = route.segmentIds.join(',');
  const hit = cache.get(cacheKey);
  if (hit && hit.key === key) return hit.path;

  const outbound: Vec2[] = [];
  for (let i = 0; i < route.segmentIds.length; i++) {
    const seg = state.tracks.find((t) => t.id === route.segmentIds[i]);
    if (!seg) return null;
    const fromId = route.stationIds[i];
    let pts = seg.polyline.points;
    if (seg.toStationId === fromId) pts = [...pts].reverse();
    // skip duplicate joint point
    const startAt = outbound.length > 0 ? 1 : 0;
    for (let j = startAt; j < pts.length; j++) outbound.push(pts[j] as Vec2);
  }
  if (outbound.length < 2) return null;
  const back = [...outbound].reverse().slice(1);
  const path = makePolyline([...outbound, ...back]);
  cache.set(cacheKey, { key, path });
  return path;
}
