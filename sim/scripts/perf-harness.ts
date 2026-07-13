#!/usr/bin/env bun
/**
 * Perf + determinism harness for the sim-core refactor (Refs #140).
 * - Builds an NYC-scale network, runs N ticks, times them (performance.now).
 * - Prints stateHash at checkpoints so pre/post refactor can be compared bit-for-bit.
 * - Also prints a generator-output hash for a fixed seed (item 3 grade guard).
 */
import { newGame } from '../src/core/newGame';
import { loadOsmCity } from '../src/core/city/osmRegistry';
import { applyCommand } from '../src/core/commands';
import { simTick } from '../src/core/sim';
import { stateHash } from '../src/core/save';
import { generateCity } from '../src/core/city/generator';
import { presetByKey } from '../src/core/city/presets';
import type { GameState } from '../src/core/types';
import type { Vec2 } from '../src/core/geometry';

const SEED = 12345;
const TICKS = Number(process.env.TICKS ?? 2000);

function genHash(seed: number, presetKey?: string): number {
  const city = generateCity(seed, 'normal', { preset: presetByKey(presetKey) });
  let h = 2166136261 >>> 0;
  const mix = (v: number): void => {
    const x = Math.round(v * 100);
    h = Math.imul(h ^ (x & 0xffff), 16777619) >>> 0;
    h = Math.imul(h ^ ((x >> 16) & 0xffff), 16777619) >>> 0;
  };
  mix(city.roads.length);
  for (const r of city.roads) {
    mix(r.id);
    mix(r.polyline.points.length);
    for (const p of r.polyline.points) { mix(p.x); mix(p.y); }
  }
  mix(city.districts.length);
  return h >>> 0;
}

function buildNetwork(state: GameState): number {
  // pick long road candidates like the smoke test does
  const cands: { a: Vec2; b: Vec2; len: number }[] = [];
  for (const r of state.roads) {
    const pts = r.polyline.points;
    if (pts.length < 2) continue;
    const a = pts[0]!;
    const b = pts[pts.length - 1]!;
    const len = Math.hypot(b.x - a.x, b.y - a.y);
    if (len >= 250) cands.push({ a, b, len });
  }
  cands.sort((x, y) => y.len - x.len);
  let routes = 0;
  for (const { a, b } of cands.slice(0, 40)) {
    const sa = applyCommand(state, { kind: 'buildStation', mode: 'bus', pos: a });
    if (!sa.ok) continue;
    const sb = applyCommand(state, { kind: 'buildStation', mode: 'bus', pos: b });
    if (!sb.ok) continue;
    const tr = applyCommand(state, { kind: 'buildTrack', mode: 'bus', grade: 'surface', fromStationId: sa.createdId!, toStationId: sb.createdId!, waypoints: [] });
    if (!tr.ok) continue;
    const ro = applyCommand(state, { kind: 'createRoute', mode: 'bus', stationIds: [sa.createdId!, sb.createdId!] });
    if (!ro.ok) continue;
    applyCommand(state, { kind: 'editRoute', routeId: ro.createdId!, vehicleCount: 10, headwaySeconds: 300 });
    routes++;
    if (routes >= 30) break;
  }
  return routes;
}

async function main(): Promise<void> {
  const gh = genHash(SEED, 'nyc');
  console.log(`genHash(nyc,${SEED}) = ${gh}`);
  const ghProc = genHash(999, undefined);
  console.log(`genHash(procedural,999) = ${ghProc}`);

  const osm = await loadOsmCity('nyc');
  const state = newGame(SEED, 'normal', { presetKey: 'nyc', osm });
  const routes = buildNetwork(state);
  console.log(`built ${routes} routes, ${state.stations.length} stations, ${state.vehicles.length} vehicles, ${state.fields.w}x${state.fields.h} grid`);

  const t0 = performance.now();
  for (let i = 0; i < TICKS; i++) {
    simTick(state);
    if ((i + 1) % 500 === 0) {
      const el = performance.now() - t0;
      console.log(`  tick ${i + 1}: hash=${stateHash(state)} elapsed=${el.toFixed(1)}ms`);
    }
  }
  const total = performance.now() - t0;
  console.log(`TICKS=${TICKS} total=${total.toFixed(1)}ms  per-tick=${(total / TICKS).toFixed(3)}ms  finalHash=${stateHash(state)}`);
}

main().catch((e) => { console.error(e); process.exit(1); });
