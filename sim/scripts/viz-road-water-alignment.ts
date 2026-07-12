/**
 * Before/after visual of the road↔water half-cell fix, rendered exactly the
 * way the native terrain shader samples water (bilinear over the coarse 96²
 * field). Water fraction is drawn as a blue field; committed roads are drawn
 * on top in the SAME world→pixel mapping. BEFORE uses the buggy corner origin
 * (water_space = field corner), AFTER uses the fixed cell-centre origin.
 *
 *   bun run scripts/viz-road-water-alignment.ts nyc
 *
 * Writes /tmp/<key>-align-before.png and /tmp/<key>-align-after.png cropped to
 * the reported offender (NYC East River / lower-Manhattan east edge).
 */
import { readFileSync, writeFileSync } from 'node:fs';
import { decodeB64Mask, maskAt } from '../src/core/city/osmCity';
import { encodePng } from './png';

const FIELD_W = 96;
const FIELD_H = 96;
const WORLD = 12000;
const HALF = WORLD / 2;
const CELL = WORLD / FIELD_W;

function buildWaterField(mask: Uint8Array, maskRes: number): Uint8Array {
  const half = CELL / 2;
  const originX = -HALF;
  const SUB = 7;
  const field = new Uint8Array(FIELD_W * FIELD_H);
  const frac = (wx: number, wy: number): number => {
    let hit = 0;
    for (let sy = 0; sy < SUB; sy++) {
      const py = wy - half + ((sy + 0.5) / SUB) * CELL;
      for (let sx = 0; sx < SUB; sx++) {
        const px = wx - half + ((sx + 0.5) / SUB) * CELL;
        if (maskAt(mask, maskRes, WORLD, px, py)) hit++;
      }
    }
    return hit / (SUB * SUB);
  };
  for (let cy = 0; cy < FIELD_H; cy++) {
    const wy = originX + (cy + 0.5) * CELL;
    for (let cx = 0; cx < FIELD_W; cx++) {
      const wx = originX + (cx + 0.5) * CELL;
      field[cy * FIELD_W + cx] = frac(wx, wy) > 0.5 ? 1 : 0;
    }
  }
  return field;
}

function waterFrac(field: Uint8Array, origin: number, x: number, z: number): number {
  const gx = (x - origin) / CELL;
  const gz = (z - origin) / CELL;
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

// Crop window in world coords (lower-Manhattan / East River), world y down.
const CROP = { x0: 200, x1: 3400, y0: -600, y1: 2600 };
const SCALE = 0.28; // px per world unit
const IMG_W = Math.round((CROP.x1 - CROP.x0) * SCALE);
const IMG_H = Math.round((CROP.y1 - CROP.y0) * SCALE);
const w2px = (x: number): number => Math.round((x - CROP.x0) * SCALE);
const w2py = (y: number): number => Math.round((y - CROP.y0) * SCALE);

function render(key: string, field: Uint8Array, origin: number, roads: { pts: number[] }[], out: string): void {
  const rgb = new Uint8Array(IMG_W * IMG_H * 3);
  // water field background
  for (let py = 0; py < IMG_H; py++) {
    const wy = CROP.y0 + py / SCALE;
    for (let px = 0; px < IMG_W; px++) {
      const wx = CROP.x0 + px / SCALE;
      const wf = waterFrac(field, origin, wx, wy);
      const i = (py * IMG_W + px) * 3;
      // land = warm grey, water = blue; blend by fraction (renderer look)
      rgb[i] = Math.round(214 * (1 - wf) + 40 * wf);
      rgb[i + 1] = Math.round(210 * (1 - wf) + 90 * wf);
      rgb[i + 2] = Math.round(198 * (1 - wf) + 170 * wf);
    }
  }
  // roads on top (black)
  const plot = (px: number, py: number): void => {
    if (px < 0 || py < 0 || px >= IMG_W || py >= IMG_H) return;
    const i = (py * IMG_W + px) * 3;
    rgb[i] = 20;
    rgb[i + 1] = 20;
    rgb[i + 2] = 20;
  };
  for (const r of roads) {
    for (let k = 0; k + 3 < r.pts.length; k += 2) {
      const ax = w2px(r.pts[k]!);
      const ay = w2py(r.pts[k + 1]!);
      const bx = w2px(r.pts[k + 2]!);
      const by = w2py(r.pts[k + 3]!);
      const steps = Math.max(Math.abs(bx - ax), Math.abs(by - ay), 1);
      for (let s = 0; s <= steps; s++) {
        const t = s / steps;
        plot(Math.round(ax + (bx - ax) * t), Math.round(ay + (by - ay) * t));
      }
    }
  }
  writeFileSync(out, encodePng(IMG_W, IMG_H, rgb));
  console.log(`${key}: ${out} (${IMG_W}x${IMG_H})`);
}

for (const key of process.argv.slice(2)) {
  const json = JSON.parse(readFileSync(`src/data/cities/${key}.json`, 'utf8'));
  const mask = decodeB64Mask(json.waterMask, json.maskRes * json.maskRes, json.maskPacked ?? true);
  const field = buildWaterField(mask, json.maskRes);
  render(key, field, -HALF, json.roads, `/tmp/${key}-align-before.png`); // buggy corner origin
  render(key, field, -HALF + CELL * 0.5, json.roads, `/tmp/${key}-align-after.png`); // fixed centre origin
}
