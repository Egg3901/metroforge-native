/**
 * Procedural city generation — tensor-field street networks (Chen et al. 2008,
 * see docs/research-city-gen.md).
 *
 * Order matters: terrain → river → CBD/subcenters → population → parks →
 * tensor field → arterial streamlines (with bridges) → local streamlines.
 * Population comes BEFORE streets; street density follows people.
 */
import { WORLD_SIZE } from '../constants';
import { presetByKey, type CityPreset } from './presets';
import { decodeB64Mask, decodeElevation, maskAt, type OsmCityData, type MapLabel } from './osmCity';
import { cellCenter, cellIndexAt, createFieldGrid } from '../fields';
import { Noise2D, clamp, makePolyline, vec } from '../geometry';
import type { Vec2 } from '../geometry';
import { Rng } from '../rng';
import type { District, Difficulty, FieldGrid, RoadEdge } from '../types';
import { traceStreamlines } from './streamlines';
import type { TensorField } from './tensor';
import { districtName, uniqueNames } from './names';

/**
 * Spatial hash over polyline segments for junction-snap nearest-segment queries.
 * Cell size = the snap radius; each segment is supercover-rasterized into every
 * cell it passes through, so a 5×5 neighborhood of any query point yields a
 * superset of every segment within the snap radius. Query returns candidate
 * (line, seg) refs sorted ascending, so iterating them with the same `d2 < best`
 * rule reproduces the linear scan's exact tie-breaking — bit-identical output.
 */
class SegmentGrid {
  private readonly cell: number;
  private readonly map = new Map<number, number[]>(); // cellKey -> encoded refs (line*1e6+seg)
  constructor(
    private readonly lines: Vec2[][],
    cell: number,
  ) {
    this.cell = cell;
    for (let li = 0; li < lines.length; li++) {
      const line = lines[li]!;
      for (let si = 0; si + 1 < line.length; si++) {
        this.rasterize(line[si]!, line[si + 1]!, li * 1_000_000 + si);
      }
    }
  }
  private key(cx: number, cy: number): number {
    return cx * 73856093 + cy * 19349663;
  }
  private rasterize(a: Vec2, b: Vec2, ref: number): void {
    const len = Math.hypot(b.x - a.x, b.y - a.y);
    const steps = Math.max(1, Math.ceil((len / this.cell) * 2)); // <= cell/2 spacing
    let lastKey = NaN;
    for (let s = 0; s <= steps; s++) {
      const t = s / steps;
      const cx = Math.floor((a.x + (b.x - a.x) * t) / this.cell);
      const cy = Math.floor((a.y + (b.y - a.y) * t) / this.cell);
      const k = this.key(cx, cy);
      if (k === lastKey) continue;
      lastKey = k;
      const arr = this.map.get(k);
      if (arr) arr.push(ref);
      else this.map.set(k, [ref]);
    }
  }
  /** Nearest projected point on any segment within `maxDist`; null if none.
   *  Equivalent to a full projectOnto scan (same tie-break) but O(local). */
  nearest(p: Vec2, maxDist: number): Vec2 | null {
    const cx = Math.floor(p.x / this.cell);
    const cy = Math.floor(p.y / this.cell);
    const refs: number[] = [];
    for (let oy = -2; oy <= 2; oy++) {
      for (let ox = -2; ox <= 2; ox++) {
        const arr = this.map.get(this.key(cx + ox, cy + oy));
        if (arr) for (const r of arr) refs.push(r);
      }
    }
    if (refs.length === 0) return null;
    refs.sort((x, y) => x - y);
    let best = maxDist * maxDist;
    let bestP: Vec2 | null = null;
    let prev = NaN;
    for (const ref of refs) {
      if (ref === prev) continue; // dedupe (a segment can land in several cells)
      prev = ref;
      const line = this.lines[Math.floor(ref / 1_000_000)]!;
      const si = ref % 1_000_000;
      const a = line[si]!;
      const b = line[si + 1]!;
      const dx = b.x - a.x;
      const dy = b.y - a.y;
      const L2 = dx * dx + dy * dy || 1;
      let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / L2;
      t = Math.max(0, Math.min(1, t));
      const qx = a.x + dx * t;
      const qy = a.y + dy * t;
      const d2 = (qx - p.x) * (qx - p.x) + (qy - p.y) * (qy - p.y);
      if (d2 < best) {
        best = d2;
        bestP = { x: qx, y: qy };
      }
    }
    return bestP;
  }
}

const DEFAULT_HALF = WORLD_SIZE / 2;
void DEFAULT_HALF;

/** Signed smallest angle from b to a. */
function angleDelta(a: number, b: number): number {
  let d = a - b;
  while (d > Math.PI) d -= Math.PI * 2;
  while (d < -Math.PI) d += Math.PI * 2;
  return d;
}

export interface GeneratedCity {
  fields: FieldGrid;
  roads: RoadEdge[];
  districts: District[];
  cbd: Vec2;
  /** high-res OSM water mask (1=water) for crisp coastline rendering, if a real
   *  city; sim still uses the coarser fields.water */
  waterMaskHi?: Uint8Array | undefined;
  parkMaskHi?: Uint8Array | undefined;
  buildingMaskHi?: Uint8Array | undefined;
  maskRes?: number | undefined;
  /** real-elevation heightfield (meters, elevRes²) for the static elevation
   *  channel, if a real city baked one */
  elevationHi?: Int16Array | undefined;
  elevRes?: number | undefined;
  labels?: MapLabel[] | undefined;
}

/** Drop collinear-ish points to keep polylines lean. */
function decimate(pts: Vec2[]): Vec2[] {
  if (pts.length <= 4) return pts.map((p) => vec(Math.round(p.x), Math.round(p.y)));
  const out: Vec2[] = [pts[0] as Vec2];
  let lastAngle: number | null = null;
  for (let i = 1; i < pts.length - 1; i++) {
    const a = out[out.length - 1] as Vec2;
    const b = pts[i] as Vec2;
    const c = pts[i + 1] as Vec2;
    const angAB = Math.atan2(b.y - a.y, b.x - a.x);
    const angBC = Math.atan2(c.y - b.y, c.x - b.x);
    const turn = Math.abs(angleDelta(angBC, angAB));
    const dist = Math.hypot(b.x - a.x, b.y - a.y);
    if (turn > 0.06 || dist > 220) {
      out.push(b);
      lastAngle = angAB;
    }
  }
  void lastAngle;
  out.push(pts[pts.length - 1] as Vec2);
  return out.map((p) => vec(Math.round(p.x), Math.round(p.y)));
}

export interface GenerateOptions {
  worldSize?: number | undefined;
  preset?: CityPreset | undefined;
  /** real-city OSM dataset — when present, real roads + water replace procgen */
  osm?: OsmCityData | undefined;
}

export function generateCity(seed: number, difficulty: Difficulty, opts: GenerateOptions = {}): GeneratedCity {
  const preset = opts.preset ?? presetByKey('generic');
  const osm = opts.osm;
  const rng = new Rng(seed);
  const terrainNoise = new Noise2D(() => rng.nextUint());
  const detailNoise = new Noise2D(() => rng.nextUint());
  const fields = createFieldGrid(osm ? osm.worldSize : opts.worldSize);
  const worldSize = fields.w * fields.cellSize;
  const HALF = worldSize / 2;

  // ── Terrain + water ──
  const w = preset.water;
  const waterAngle = w.coastAngleDeg !== null ? (w.coastAngleDeg * Math.PI) / 180 : rng.range(0, Math.PI * 2);
  const waterDir = vec(Math.cos(waterAngle), Math.sin(waterAngle));
  const waterOffset = w.coastInset * HALF;

  let osmElevationHi: Int16Array | undefined;
  let osmWaterHi: Uint8Array | undefined;
  let osmParkHi: Uint8Array | undefined;
  let osmBuildingHi: Uint8Array | undefined;
  if (osm) {
    // real-city land/water/parks from the baked OSM masks; gentle procedural relief
    const n = osm.maskRes * osm.maskRes;
    const packed = osm.maskPacked === true;
    const mask = decodeB64Mask(osm.waterMask, n, packed);
    const pmask = osm.parkMask ? decodeB64Mask(osm.parkMask, n, packed) : null;
    osmWaterHi = mask;
    osmParkHi = pmask ?? undefined;
    osmBuildingHi = osm.buildingMask ? decodeB64Mask(osm.buildingMask, n, packed) : undefined;
    if (osm.elevation && osm.elevRes) osmElevationHi = decodeElevation(osm.elevation, osm.elevRes);
    // The coarse 125 m field grid (fields.w×fields.h) is what the native
    // terrain mesh bilinearly samples + thresholds at water_frac>0.5. Taking a
    // single CENTER point-sample of the ~19 m OSM mask per field cell quantizes
    // the coast to the field grid and lets a cell whose center happens to land
    // in a river/pier read as fully water (or vice-versa) — which, after the
    // native bilinear spread, smears water blobs ~one field cell inland and
    // stands shoreline buildings in water. Instead take the AREA fraction of
    // the hi-res mask over each field cell footprint and majority-vote, so the
    // coarse land/water boundary tracks the real shoreline as closely as a
    // 125 m grid allows. (metroforge-native issue: eastern-Manhattan water.)
    const SUB = 7; // 7×7 sub-samples of the ~19 m mask per 125 m field cell
    const half = fields.cellSize / 2;
    const maskFrac = (m: Uint8Array, wx: number, wy: number): number => {
      let hit = 0;
      for (let sy = 0; sy < SUB; sy++) {
        const py = wy - half + ((sy + 0.5) / SUB) * fields.cellSize;
        for (let sx = 0; sx < SUB; sx++) {
          const px = wx - half + ((sx + 0.5) / SUB) * fields.cellSize;
          if (maskAt(m, osm.maskRes, worldSize, px, py)) hit++;
        }
      }
      return hit / (SUB * SUB);
    };
    for (let cy = 0; cy < fields.h; cy++) {
      for (let cx = 0; cx < fields.w; cx++) {
        const i = cy * fields.w + cx;
        const p = cellCenter(fields, i);
        const water = maskFrac(mask, p.x, p.y) > 0.5;
        fields.water[i] = water ? 1 : 0;
        if (pmask && !water && maskFrac(pmask, p.x, p.y) > 0.5) fields.parks[i] = 1;
      }
    }
    // Relief for real cities: the raw fbm amplitude reads as sand dunes on
    // flat urban islands (Roosevelt Island most visibly). Real relief here
    // is subtle, and shorelines sit near sea level — so damp the noise
    // overall AND fade it in with distance from water (multi-source BFS,
    // 4-neighborhood, distance in cells).
    const distToWater = new Float32Array(fields.w * fields.h).fill(Infinity);
    const queue: number[] = [];
    for (let i = 0; i < fields.w * fields.h; i++) {
      if (fields.water[i]) {
        distToWater[i] = 0;
        queue.push(i);
      }
    }
    for (let qi = 0; qi < queue.length; qi++) {
      const i = queue[qi] as number;
      const d = (distToWater[i] as number) + 1;
      const cx = i % fields.w;
      const cy = (i / fields.w) | 0;
      const neighbors = [
        cx > 0 ? i - 1 : -1,
        cx < fields.w - 1 ? i + 1 : -1,
        cy > 0 ? i - fields.w : -1,
        cy < fields.h - 1 ? i + fields.w : -1,
      ];
      for (const ni of neighbors) {
        if (ni >= 0 && d < (distToWater[ni] as number)) {
          distToWater[ni] = d;
          queue.push(ni);
        }
      }
    }
    const shoreFadeCells = (1200 / fields.cellSize) | 0 || 1; // full relief ~1.2km inland
    for (let cy = 0; cy < fields.h; cy++) {
      for (let cx = 0; cx < fields.w; cx++) {
        const i = cy * fields.w + cx;
        if (fields.water[i]) {
          fields.terrain[i] = 0.12;
          continue;
        }
        const p = cellCenter(fields, i);
        const elev = terrainNoise.fbm((p.x / worldSize) * 4 + 10, (p.y / worldSize) * 4 + 10, 4);
        const t = Math.min(1, (distToWater[i] as number) / shoreFadeCells);
        const fade = t * t * (3 - 2 * t); // smoothstep
        fields.terrain[i] = clamp(0.2 + elev * 0.12 * fade, 0, 1);
      }
    }
  } else {
    for (let cy = 0; cy < fields.h; cy++) {
      for (let cx = 0; cx < fields.w; cx++) {
        const i = cy * fields.w + cx;
        const p = cellCenter(fields, i);
        const nx = p.x / worldSize;
        const ny = p.y / worldSize;
        let elev = terrainNoise.fbm(nx * 4 + 10, ny * 4 + 10, 4);
        // landlocked presets keep the noise but never dip below the waterline
        if (!w.coast) {
          fields.terrain[i] = clamp(0.35 + elev * 0.5, 0, 1);
          fields.water[i] = 0;
          continue;
        }
        const coastDist = p.x * waterDir.x + p.y * waterDir.y - waterOffset;
        if (coastDist > 0) elev -= (coastDist / HALF) * 0.9;
        fields.terrain[i] = clamp(elev, 0, 1);
        fields.water[i] = elev < 0.22 ? 1 : 0;
      }
    }
  }

  // ── River: meanders from an inland edge downhill to the sea ──
  if (w.river && !osm) {
    const startAngle = waterAngle + Math.PI + rng.range(-0.5, 0.5);
    let px = Math.cos(startAngle) * HALF * 0.95;
    let py = Math.sin(startAngle) * HALF * 0.95;
    let dirAngle = Math.atan2(-py, -px);
    const meander = rng.range(2, 5);
    let reachedSea = false;
    let step = 0;
    for (; step < 400; step++) {
      const ci = cellIndexAt(fields, vec(px, py));
      const cx0 = ci % fields.w;
      const cy0 = Math.floor(ci / fields.w);
      for (let oy = -1; oy <= 1; oy++) {
        for (let ox = -1; ox <= 1; ox++) {
          if (Math.abs(ox) + Math.abs(oy) > 1 && !(rng.next() < 0.4)) continue;
          const nx = cx0 + ox;
          const ny = cy0 + oy;
          if (nx >= 0 && ny >= 0 && nx < fields.w && ny < fields.h) {
            fields.water[ny * fields.w + nx] = 1;
            fields.terrain[ny * fields.w + nx] = Math.min(fields.terrain[ny * fields.w + nx] as number, 0.2);
          }
        }
      }
      if ((fields.water[ci] as number) === 1 && step > 30) {
        const coastDist = px * waterDir.x + py * waterDir.y - waterOffset;
        if (coastDist > -600) {
          reachedSea = true;
          break;
        }
      }
      const toCoast = Math.atan2(waterDir.y, waterDir.x);
      const wiggle = Math.sin(step / 14) * 0.5 * Math.sin(meander + step / 40);
      dirAngle += angleDelta(toCoast, dirAngle) * 0.035 + wiggle * 0.14 + rng.range(-0.08, 0.08);
      px += Math.cos(dirAngle) * 95;
      py += Math.sin(dirAngle) * 95;
      if (Math.abs(px) > HALF || Math.abs(py) > HALF) {
        reachedSea = true; // flows off-map: fine
        break;
      }
    }
    if (!reachedSea) {
      // rivers should end somewhere: stamp a terminal lake
      const lakeR = rng.range(350, 600);
      for (let i = 0; i < fields.water.length; i++) {
        const c = cellCenter(fields, i);
        const d = Math.hypot(c.x - px, c.y - py);
        if (d < lakeR * (0.75 + 0.25 * detailNoise.at(c.x / 900 + 77, c.y / 900 + 77))) {
          fields.water[i] = 1;
          fields.terrain[i] = Math.min(fields.terrain[i] as number, 0.18);
        }
      }
    }
  }

  const isWaterAt = (p: Vec2): boolean => (fields.water[cellIndexAt(fields, p)] as number) === 1;

  // ── CBD: on land, biased toward the water (port cities) ──
  let cbd = vec(0, 0);
  {
    let best = -Infinity;
    for (let attempt = 0; attempt < 60; attempt++) {
      const cand = vec(rng.range(-HALF * 0.35, HALF * 0.35), rng.range(-HALF * 0.35, HALF * 0.35));
      if (isWaterAt(cand)) continue;
      const coastDist = Math.abs(cand.x * waterDir.x + cand.y * waterDir.y - waterOffset);
      const score = -coastDist / HALF - Math.hypot(cand.x, cand.y) / HALF + rng.range(0, 0.3);
      if (score > best) {
        best = score;
        cbd = cand;
      }
    }
  }

  // ── Employment subcenters (edge-city anchors) ──
  const subcenters: Vec2[] = [];
  for (let k = 0; k < rng.int(3, 5); k++) {
    for (let attempt = 0; attempt < 20; attempt++) {
      const ang = rng.range(0, Math.PI * 2);
      const cand = vec(cbd.x + Math.cos(ang) * rng.range(2000, 4200), cbd.y + Math.sin(ang) * rng.range(2000, 4200));
      if (Math.abs(cand.x) > HALF * 0.9 || Math.abs(cand.y) > HALF * 0.9 || isWaterAt(cand)) continue;
      if (subcenters.every((s) => Math.hypot(s.x - cand.x, s.y - cand.y) > 1800)) {
        subcenters.push(cand);
        break;
      }
    }
  }

  // ── Population & jobs (BEFORE streets — density drives the network) ──
  const popTarget: Record<Difficulty, number> = { easy: 220000, normal: 160000, hard: 110000 };
  const target = popTarget[difficulty];
  const rawPop = new Float32Array(fields.w * fields.h);
  const rawJobs = new Float32Array(fields.w * fields.h);
  let rawPopSum = 0;
  let rawJobsSum = 0;
  for (let i = 0; i < rawPop.length; i++) {
    if ((fields.water[i] as number) === 1) continue;
    const c = cellCenter(fields, i);
    const dCbd = Math.hypot(c.x - cbd.x, c.y - cbd.y);
    const noise = detailNoise.fbm(c.x / 3000 + 50, c.y / 3000 + 50, 3);
    // sprawl stretches the decay lengths: >1 spreads people out toward the edges
    const sp = preset.sprawl;
    let pop = Math.exp(-dCbd / (2600 * sp));
    for (const s of subcenters) {
      const dS = Math.hypot(c.x - s.x, c.y - s.y);
      pop += 0.45 * Math.exp(-dS / (1400 * sp));
    }
    pop *= 0.45 + noise;
    rawPop[i] = pop;
    rawPopSum += pop;
    let jobs = Math.exp(-dCbd / (1100 * sp)) * 3;
    for (const s of subcenters) {
      const dS = Math.hypot(c.x - s.x, c.y - s.y);
      jobs += Math.exp(-dS / (800 * sp)) * 0.8;
    }
    jobs *= 0.6 + noise;
    rawJobs[i] = jobs;
    rawJobsSum += jobs;
  }
  const jobsTarget = target * 0.45;
  for (let i = 0; i < rawPop.length; i++) {
    fields.population[i] = ((rawPop[i] as number) / rawPopSum) * target;
    fields.jobs[i] = ((rawJobs[i] as number) / rawJobsSum) * jobsTarget;
  }

  // ── Parks: real parks (OSM) already stamped above; else noise pockets +
  //    signature parks. Either way, parks displace residents. ──
  {
    if (!osm) {
      for (let i = 0; i < fields.parks.length; i++) {
        if ((fields.water[i] as number) === 1) continue;
        const c = cellCenter(fields, i);
        const n = detailNoise.fbm(c.x / 1400 + 300, c.y / 1400 + 300, 3);
        const dCbd = Math.hypot(c.x - cbd.x, c.y - cbd.y);
        if (n > 0.66 && dCbd > 700) fields.parks[i] = 1;
      }
      const bigParks = rng.int(1, 2);
      for (let k = 0; k < bigParks; k++) {
        const ang = rng.range(0, Math.PI * 2);
        const cx0 = cbd.x + Math.cos(ang) * rng.range(1200, 2400);
        const cy0 = cbd.y + Math.sin(ang) * rng.range(1200, 2400);
        const w = rng.range(500, 900);
        const h = rng.range(350, 650);
        for (let i = 0; i < fields.parks.length; i++) {
          const c = cellCenter(fields, i);
          if (Math.abs(c.x - cx0) < w / 2 && Math.abs(c.y - cy0) < h / 2 && (fields.water[i] as number) === 0) {
            fields.parks[i] = 1;
          }
        }
      }
    }
    for (let i = 0; i < fields.parks.length; i++) {
      if ((fields.parks[i] as number) === 1) {
        fields.population[i] = 0;
        fields.jobs[i] = 0;
      }
    }
  }

  const meanCellPop = target / (fields.w * fields.h);
  const densityAt = (p: Vec2): number => {
    if (Math.abs(p.x) > HALF || Math.abs(p.y) > HALF) return -1;
    const i = cellIndexAt(fields, p);
    if ((fields.water[i] as number) === 1 || (fields.parks[i] as number) === 1) return -1;
    return ((fields.population[i] as number) + (fields.jobs[i] as number)) / meanCellPop;
  };

  // ── Tensor field: grid patches + CBD radial + water-boundary alignment ──
  const baseAngle = (preset.grid.angleDeg * Math.PI) / 180;
  const field: TensorField = {
    grids: [],
    // rigid presets get a dominant citywide grid; the local patches + radial
    // then only perturb it, so the whole city reads as one oriented grid
    ...(preset.grid.rigid ? { globalGrid: { theta: baseAngle, weight: 3.2 } } : {}),
    radialCenter: cbd,
    radialWeight: preset.radialWeight,
    radialSigma: 2600,
    boundaries: [],
    boundarySigma: 550,
    boundaryWeight: 1.6,
    noise: (x, y) => detailNoise.at(x / 5200 + 400, y / 5200 + 400),
    noiseWeight: preset.grid.noiseWeight,
  };
  // grid patches: at subcenters + random populated points. Rigid presets lock
  // every patch to the preset bearing (Manhattan/Chicago grid); organic ones
  // let each patch drift off a noise-snapped angle.
  const gridSeeds: Vec2[] = [...subcenters];
  for (let k = 0; k < 6; k++) {
    gridSeeds.push(vec(rng.range(-HALF * 0.8, HALF * 0.8), rng.range(-HALF * 0.8, HALF * 0.8)));
  }
  for (const gcenter of gridSeeds) {
    const raw = detailNoise.at(gcenter.x / 4200 + 200, gcenter.y / 4200 + 200) * Math.PI;
    const theta = preset.grid.rigid
      ? baseAngle
      : baseAngle + Math.round(raw / (Math.PI / 12)) * (Math.PI / 12);
    field.grids.push({
      center: gcenter,
      theta,
      sigma: rng.range(1600, 2600),
      weight: preset.grid.weight,
    });
  }
  // boundary samples: shoreline cells with tangent from the water gradient
  for (let cy = 1; cy < fields.h - 1; cy++) {
    for (let cx = 1; cx < fields.w - 1; cx++) {
      const i = cy * fields.w + cx;
      if ((fields.water[i] as number) === 1) continue;
      // land cell adjacent to water = shoreline
      const wR = fields.water[cy * fields.w + cx + 1] as number;
      const wL = fields.water[cy * fields.w + cx - 1] as number;
      const wD = fields.water[(cy + 1) * fields.w + cx] as number;
      const wU = fields.water[(cy - 1) * fields.w + cx] as number;
      if (wR + wL + wD + wU === 0) continue;
      if ((cx + cy) % 2 !== 0) continue; // thin the samples
      const gx = wR - wL;
      const gy = wD - wU;
      field.boundaries.push({ pos: cellCenter(fields, i), theta: Math.atan2(gy, gx) + Math.PI / 2 });
    }
  }

  // ── Arterials: sparse streamlines, both eigen directions, may bridge water ──
  const roads: RoadEdge[] = [];
  let roadId = 1;
  if (osm) {
    // real street network straight from the OSM bundle
    for (const r of osm.roads) {
      if (r.pts.length < 4) continue;
      const pl: Vec2[] = [];
      for (let i = 0; i + 1 < r.pts.length; i += 2) pl.push(vec(r.pts[i] as number, r.pts[i + 1] as number));
      const cls: RoadEdge['cls'] = r.cls === 'arterial' || r.cls === 'collector' ? r.cls : 'local';
      roads.push({ id: roadId++, cls, polyline: makePolyline(pl) });
    }
  } else {
  const arterialSeeds: Vec2[] = [cbd, ...subcenters];
  for (let k = 0; k < 18; k++) {
    const cand = vec(rng.range(-HALF * 0.85, HALF * 0.85), rng.range(-HALF * 0.85, HALF * 0.85));
    if (densityAt(cand) > 0.4) arterialSeeds.push(cand);
  }
  const arterials = traceStreamlines(field, rng.fork(11), {
    separation: 620,
    inDomain: (p) => Math.abs(p.x) < HALF * 0.97 && Math.abs(p.y) < HALF * 0.97 && (densityAt(p) > 0.12 || Math.hypot(p.x - cbd.x, p.y - cbd.y) < 2800),
    bridgeMaxSteps: 9,
    blocked: isWaterAt,
    maxLength: 11000,
    minLength: 900,
    seeds: arterialSeeds,
    spawnSeeds: true,
    eigenDirs: [0, 1],
  });
  for (const line of arterials) {
    roads.push({ id: roadId++, cls: 'arterial', polyline: makePolyline(decimate(line)) });
  }

  // ── Locals: dense streamlines through populated land, spacing by density ──
  const localSeeds: Vec2[] = [cbd, ...subcenters];
  for (let k = 0; k < 140; k++) {
    const cand = vec(rng.range(-HALF * 0.9, HALF * 0.9), rng.range(-HALF * 0.9, HALF * 0.9));
    if (densityAt(cand) > 0.35) localSeeds.push(cand);
  }
  const arterialSamples: Vec2[] = [];
  for (const line of arterials) for (const p of line) arterialSamples.push(p);
  const locals = traceStreamlines(field, rng.fork(13), {
    separation: (p) => {
      const d = densityAt(p);
      return d > 2.5 ? 70 : d > 1.2 ? 95 : 130;
    },
    inDomain: (p) => densityAt(p) > 0.22,
    bridgeMaxSteps: 0,
    blocked: isWaterAt,
    maxLength: 2600,
    minLength: 330,
    seeds: localSeeds,
    snapTargets: arterialSamples,
    spawnSeeds: true,
    eigenDirs: [0, 1],
  });
  // Junction snap: pull each local street's dangling ends onto the nearest
  // arterial so small streets actually meet the main roads (T-intersections)
  // instead of stopping just short of them.
  // Project a point onto the nearest segment across a set of polylines (within
  // maxDist), skipping one line by reference (so a line never snaps to itself).
  const projectOnto = (p: Vec2, lines: Vec2[][], maxDist: number, skip: Vec2[] | null): Vec2 | null => {
    let best = maxDist * maxDist;
    let bestP: Vec2 | null = null;
    for (const line of lines) {
      if (line === skip) continue;
      for (let i = 0; i + 1 < line.length; i++) {
        const a = line[i] as Vec2;
        const b = line[i + 1] as Vec2;
        const dx = b.x - a.x;
        const dy = b.y - a.y;
        const L2 = dx * dx + dy * dy || 1;
        let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / L2;
        t = Math.max(0, Math.min(1, t));
        const qx = a.x + dx * t;
        const qy = a.y + dy * t;
        const d2 = (qx - p.x) * (qx - p.x) + (qy - p.y) * (qy - p.y);
        if (d2 < best) {
          best = d2;
          bestP = { x: qx, y: qy };
        }
      }
    }
    return bestP;
  };
  // Snap each local end to the nearest arterial (long reach, main-road junctions)
  // else to a nearby local street (short reach, closes grid stubs). This is what
  // makes small streets visibly meet other streets instead of dead-ending.
  const ARTERIAL_SNAP = 150;
  const LOCAL_SNAP = 80;
  // Spatial hash over arterial segments: replaces the O(locals × arterials)
  // nearest-arterial scan. Arterials are immutable through this loop, so the
  // grid can be built once. The locals fallback stays a linear scan because
  // `locals` is mutated (unshift/push) as we go.
  const arterialGrid = new SegmentGrid(arterials, ARTERIAL_SNAP);
  for (const line of locals) {
    if (line.length >= 2) {
      for (const endIdx of [0, line.length - 1] as const) {
        const end = line[endIdx] as Vec2;
        const q =
          arterialGrid.nearest(end, ARTERIAL_SNAP) ??
          projectOnto(end, locals, LOCAL_SNAP, line);
        if (q && Math.hypot(q.x - end.x, q.y - end.y) > 12 && !isWaterAt(q)) {
          if (endIdx === 0) line.unshift({ ...q });
          else line.push({ ...q });
        }
      }
    }
    roads.push({ id: roadId++, cls: 'local', polyline: makePolyline(decimate(line)) });
  }
  } // end procedural road generation

  // ── Land value: CBD proximity + waterfront + noise; NIMBY from wealth ──
  for (let i = 0; i < fields.landValue.length; i++) {
    if ((fields.water[i] as number) === 1) continue;
    const c = cellCenter(fields, i);
    const dCbd = Math.hypot(c.x - cbd.x, c.y - cbd.y);
    let nearWater = 0;
    const probe = 2;
    const cx = i % fields.w;
    const cy = Math.floor(i / fields.w);
    outer: for (let oy = -probe; oy <= probe; oy++) {
      for (let ox = -probe; ox <= probe; ox++) {
        const nx2 = cx + ox;
        const ny2 = cy + oy;
        if (nx2 < 0 || ny2 < 0 || nx2 >= fields.w || ny2 >= fields.h) continue;
        if ((fields.water[ny2 * fields.w + nx2] as number) === 1) {
          nearWater = 1;
          break outer;
        }
      }
    }
    const lv =
      Math.exp(-dCbd / 3500) * 1.2 +
      nearWater * 0.6 +
      detailNoise.fbm(c.x / 2500 + 90, c.y / 2500 + 90, 3) * 0.5;
    fields.landValue[i] = lv;
    const popNorm = (fields.population[i] as number) / meanCellPop;
    fields.nimby[i] = lv > 1.1 && popNorm < 1.2 ? clamp((lv - 1.0) * 55, 0, 90) : 0;
  }

  // ── Districts: 4×4-cell (500 m) blocks — must stay finer than walk radii ──
  const districts: District[] = [];
  const BLOCK = 4;
  let districtId = 0;
  for (let by = 0; by < fields.h; by += BLOCK) {
    for (let bx = 0; bx < fields.w; bx += BLOCK) {
      let pop = 0;
      let jobs = 0;
      let lvSum = 0;
      let landCells = 0;
      const cellIndices: number[] = [];
      let wx = 0;
      let wy = 0;
      let wSum = 0;
      for (let oy = 0; oy < BLOCK && by + oy < fields.h; oy++) {
        for (let ox = 0; ox < BLOCK && bx + ox < fields.w; ox++) {
          const i = (by + oy) * fields.w + (bx + ox);
          cellIndices.push(i);
          const cp = fields.population[i] as number;
          const cj = fields.jobs[i] as number;
          pop += cp;
          jobs += cj;
          if ((fields.water[i] as number) === 0) {
            lvSum += fields.landValue[i] as number;
            landCells++;
          }
          const w = cp + cj;
          if (w > 0) {
            const c = cellCenter(fields, i);
            wx += c.x * w;
            wy += c.y * w;
            wSum += w;
          }
        }
      }
      if (pop + jobs < 50) continue;
      districts.push({
        id: districtId++,
        name: '',
        centroid: wSum > 0 ? vec(wx / wSum, wy / wSum) : cellCenter(fields, cellIndices[Math.floor(cellIndices.length / 2)] as number),
        cellIndices,
        population: pop,
        jobs,
        landValue: landCells > 0 ? lvSum / landCells : 0,
      });
    }
  }

  // ── Name the neighborhoods (unique, seed-stable) ──
  const nameRng = rng.fork(0x0d15);
  const names = uniqueNames(nameRng, districts.length, districtName);
  for (let i = 0; i < districts.length; i++) {
    (districts[i] as District).name = names[i] as string;
  }

  return { fields, roads, districts, cbd, waterMaskHi: osmWaterHi, parkMaskHi: osmParkHi, buildingMaskHi: osmBuildingHi, maskRes: osm ? osm.maskRes : undefined, elevationHi: osmElevationHi, elevRes: osm && osm.elevRes ? osm.elevRes : undefined, labels: osm ? osm.labels : undefined };
}
