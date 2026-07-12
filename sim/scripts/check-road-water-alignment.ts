/**
 * Regression check: road-vertex ↔ water agreement per city.
 *
 * "Sometimes the actual land masses don't line up with roads" — roads running
 * into water. Root cause was a half-cell (½·125 m ≈ 62.5 m) offset in the
 * NATIVE renderer's `water_space` (crates/mf-render/src/terrain.rs): the coarse
 * 96² sim water field stores CELL-CENTER samples, but `water_space` was built
 * with the field's CORNER origin (`-HALF`) while `GridSpace::grid_coords`
 * assumes the origin is the first cell CENTER (as `height_space` correctly
 * bakes it: `-HALF + 0.5·cell`). That shifted the whole rendered shoreline
 * ~62.5 m south-east of the true coastline, so shoreline-hugging roads ran
 * into water.
 *
 * This script rebuilds the exact 96² water field the sim generates from the
 * committed 640² OSM water mask (same 7×7 area/majority sample as
 * src/core/city/generator.ts), then, for every committed road vertex, samples
 * the field the way the renderer does — under the BUGGY corner origin and the
 * FIXED cell-center origin — and reports the fraction of road vertices the
 * renderer would drop onto water (water_frac > 0.5).
 *
 * A well-aligned city has almost no road vertices in water. CI asserts the
 * FIXED rate stays below THRESHOLD.
 *
 *   bun run scripts/check-road-water-alignment.ts            # all cities, table
 *   bun run scripts/check-road-water-alignment.ts --assert   # exit 1 if over
 */
import { readFileSync, readdirSync } from 'node:fs';
import { decodeB64Mask, maskAt } from '../src/core/city/osmCity';

const FIELD_W = 96;
const FIELD_H = 96;
const THRESHOLD = 0.03; // ≤3% of road vertices may land on water (fixed sampling)

type CityJson = {
  key: string;
  worldSize: number;
  maskRes: number;
  waterMask: string;
  maskPacked?: boolean;
  roads: { cls: string; pts: number[] }[];
};

/** Rebuild the coarse 96² water field from the hi-res mask exactly as
 *  src/core/city/generator.ts does (7×7 area sample + majority vote at each
 *  field cell centre). */
function buildWaterField(mask: Uint8Array, maskRes: number, worldSize: number): Uint8Array {
  const cell = worldSize / FIELD_W;
  const half = cell / 2;
  const originX = -(FIELD_W * cell) / 2;
  const originY = -(FIELD_H * cell) / 2;
  const SUB = 7;
  const field = new Uint8Array(FIELD_W * FIELD_H);
  const maskFrac = (wx: number, wy: number): number => {
    let hit = 0;
    for (let sy = 0; sy < SUB; sy++) {
      const py = wy - half + ((sy + 0.5) / SUB) * cell;
      for (let sx = 0; sx < SUB; sx++) {
        const px = wx - half + ((sx + 0.5) / SUB) * cell;
        if (maskAt(mask, maskRes, worldSize, px, py)) hit++;
      }
    }
    return hit / (SUB * SUB);
  };
  for (let cy = 0; cy < FIELD_H; cy++) {
    const wy = originY + (cy + 0.5) * cell;
    for (let cx = 0; cx < FIELD_W; cx++) {
      const wx = originX + (cx + 0.5) * cell;
      field[cy * FIELD_W + cx] = maskFrac(wx, wy) > 0.5 ? 1 : 0;
    }
  }
  return field;
}

/** Bilinear water fraction at a world point, mirroring the Rust
 *  `GridSpace::bilinear_u8`. `originCenter` = the world position of grid
 *  sample (0,0): the FIXED renderer uses the first cell CENTRE, the BUGGY one
 *  used the field CORNER (`-HALF`). */
function waterFrac(field: Uint8Array, cell: number, originCenter: number, x: number, z: number): number {
  const gx = (x - originCenter) / cell;
  const gz = (z - originCenter) / cell;
  const x0 = Math.min(Math.max(Math.floor(gx), 0), FIELD_W - 1);
  const y0 = Math.min(Math.max(Math.floor(gz), 0), FIELD_H - 1);
  const x1 = Math.min(x0 + 1, FIELD_W - 1);
  const y1 = Math.min(y0 + 1, FIELD_H - 1);
  const tx = Math.min(Math.max(gx - x0, 0), 1);
  const ty = Math.min(Math.max(gz - y0, 0), 1);
  const v00 = field[y0 * FIELD_W + x0]!;
  const v10 = field[y0 * FIELD_W + x1]!;
  const v01 = field[y1 * FIELD_W + x0]!;
  const v11 = field[y1 * FIELD_W + x1]!;
  return (v00 * (1 - tx) + v10 * tx) * (1 - ty) + (v01 * (1 - tx) + v11 * tx) * ty;
}

export type AlignResult = { key: string; verts: number; buggyRate: number; fixedRate: number };

export function checkCity(json: CityJson): AlignResult {
  const { worldSize, maskRes } = json;
  const cell = worldSize / FIELD_W;
  const half = worldSize / 2;
  const cornerOrigin = -half; // BUGGY: field corner
  const centerOrigin = -half + cell * 0.5; // FIXED: first cell centre
  const n = FIELD_W * FIELD_H;
  const mask = decodeB64Mask(json.waterMask, maskRes * maskRes, json.maskPacked ?? true);
  void n;
  const field = buildWaterField(mask, maskRes, worldSize);
  let verts = 0;
  let buggyWet = 0;
  let fixedWet = 0;
  for (const road of json.roads) {
    for (let i = 0; i + 1 < road.pts.length; i += 2) {
      const x = road.pts[i]!;
      const z = road.pts[i + 1]!;
      verts++;
      if (waterFrac(field, cell, cornerOrigin, x, z) > 0.5) buggyWet++;
      if (waterFrac(field, cell, centerOrigin, x, z) > 0.5) fixedWet++;
    }
  }
  return { key: json.key, verts, buggyRate: buggyWet / verts, fixedRate: fixedWet / verts };
}

function main(): void {
  const assert = process.argv.includes('--assert');
  const dir = 'src/data/cities';
  const files = readdirSync(dir).filter((f) => f.endsWith('.json'));
  const results: AlignResult[] = [];
  for (const f of files) {
    const json = JSON.parse(readFileSync(`${dir}/${f}`, 'utf8')) as CityJson;
    if (!json.roads || !json.waterMask) continue;
    results.push(checkCity(json));
  }
  results.sort((a, b) => b.fixedRate - a.fixedRate);
  console.log('city        verts   buggy%(corner)  fixed%(center)');
  for (const r of results) {
    console.log(
      `${r.key.padEnd(10)} ${String(r.verts).padStart(6)}   ${(r.buggyRate * 100).toFixed(2).padStart(8)}   ${(r.fixedRate * 100).toFixed(2).padStart(12)}`,
    );
  }
  const worst = results.reduce((m, r) => Math.max(m, r.fixedRate), 0);
  console.log(`\nthreshold ${(THRESHOLD * 100).toFixed(1)}%  worst fixed ${(worst * 100).toFixed(2)}%`);
  if (assert) {
    const over = results.filter((r) => r.fixedRate > THRESHOLD);
    if (over.length) {
      console.error(`FAIL: ${over.map((r) => `${r.key} ${(r.fixedRate * 100).toFixed(2)}%`).join(', ')}`);
      process.exit(1);
    }
    console.log('PASS');
  }
}

if (import.meta.main) main();

export { THRESHOLD };
