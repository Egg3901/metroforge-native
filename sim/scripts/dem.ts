/**
 * DEM (digital elevation model) ingest for the city importer.
 *
 * Source: AWS Terrain Tiles — terrarium-encoded PNG tiles, public,
 * unauthenticated: https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png
 * (data derived from SRTM/ASTER/NED etc., public-domain-ish; attribution:
 * "Elevation data: Mapzen / AWS Terrain Tiles"). Terrarium decode:
 *   elevation_m = R*256 + G + B/256 - 32768
 *
 * Tiles are cached under sim/.cache/dem/{z}/{x}/{y}.png (gitignored) so
 * re-runs are offline+fast. A slippy-map (Web Mercator) tile pyramid: this
 * module fetches every tile overlapping a lat/lon window at a chosen zoom,
 * decodes it to a per-pixel elevation grid, and offers a bilinear sampler
 * keyed on (lat, lon).
 */
import { mkdirSync, existsSync, readFileSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { inflateSync } from 'node:zlib';

const TILE = 256; // terrarium tile side in pixels
const CACHE_DIR = '.cache/dem';

/** Web-Mercator meters/pixel at a given zoom + latitude (for zoom picking). */
export function metersPerPixel(zoom: number, latDeg: number): number {
  return (156543.03392 * Math.cos((latDeg * Math.PI) / 180)) / 2 ** zoom;
}

/** Pick the slippy zoom whose pixel size is closest to `targetMeters` at
 *  `latDeg` (clamped to a sane tile-count range). */
export function pickZoom(latDeg: number, targetMeters: number): number {
  let best = 12;
  let bestErr = Infinity;
  for (let z = 8; z <= 14; z++) {
    const err = Math.abs(metersPerPixel(z, latDeg) - targetMeters);
    if (err < bestErr) {
      bestErr = err;
      best = z;
    }
  }
  return best;
}

/** lon/lat (deg) → fractional slippy tile coords at zoom `z`. */
function lonLatToTile(lonDeg: number, latDeg: number, z: number): { xf: number; yf: number } {
  const latRad = (latDeg * Math.PI) / 180;
  const n = 2 ** z;
  const xf = ((lonDeg + 180) / 360) * n;
  const yf = ((1 - Math.log(Math.tan(latRad) + 1 / Math.cos(latRad)) / Math.PI) / 2) * n;
  return { xf, yf };
}

// ── minimal truecolor (8-bit RGB / RGBA) PNG decoder ────────────────────────
// Terrarium tiles are non-interlaced 8-bit color-type-2 (RGB). We still
// handle color-type-6 (RGBA) defensively. Returns a tightly packed RGB
// Uint8Array (3 bytes/pixel), width*height.
function decodePngRgb(buf: Buffer): { w: number; h: number; rgb: Uint8Array } {
  // signature
  const SIG = [137, 80, 78, 71, 13, 10, 26, 10];
  for (let i = 0; i < 8; i++) if (buf[i] !== SIG[i]) throw new Error('dem: not a PNG');
  let off = 8;
  let w = 0;
  let h = 0;
  let bitDepth = 0;
  let colorType = 0;
  const idat: Buffer[] = [];
  while (off < buf.length) {
    const len = buf.readUInt32BE(off);
    const type = buf.toString('ascii', off + 4, off + 8);
    const data = buf.subarray(off + 8, off + 8 + len);
    if (type === 'IHDR') {
      w = data.readUInt32BE(0);
      h = data.readUInt32BE(4);
      bitDepth = data[8]!;
      colorType = data[9]!;
      const interlace = data[12]!;
      if (bitDepth !== 8 || (colorType !== 2 && colorType !== 6) || interlace !== 0) {
        throw new Error(`dem: unsupported PNG (bitDepth=${bitDepth} colorType=${colorType} interlace=${interlace})`);
      }
    } else if (type === 'IDAT') {
      idat.push(Buffer.from(data));
    } else if (type === 'IEND') {
      break;
    }
    off += 12 + len; // len + type(4) + data + crc(4)
  }
  const channels = colorType === 6 ? 4 : 3;
  const raw = inflateSync(Buffer.concat(idat));
  const stride = w * channels;
  const rgb = new Uint8Array(w * h * 3);
  const prev = new Uint8Array(stride);
  const cur = new Uint8Array(stride);
  let p = 0;
  const paeth = (a: number, b: number, c: number): number => {
    const pp = a + b - c;
    const pa = Math.abs(pp - a);
    const pb = Math.abs(pp - b);
    const pc = Math.abs(pp - c);
    return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
  };
  for (let y = 0; y < h; y++) {
    const filter = raw[p++]!;
    for (let x = 0; x < stride; x++) {
      const rawB = raw[p++]!;
      const a = x >= channels ? cur[x - channels]! : 0;
      const b = prev[x]!;
      const c = x >= channels ? prev[x - channels]! : 0;
      let val: number;
      switch (filter) {
        case 0: val = rawB; break;
        case 1: val = rawB + a; break;
        case 2: val = rawB + b; break;
        case 3: val = rawB + ((a + b) >> 1); break;
        case 4: val = rawB + paeth(a, b, c); break;
        default: throw new Error(`dem: bad PNG filter ${filter}`);
      }
      cur[x] = val & 0xff;
    }
    for (let x = 0; x < w; x++) {
      const si = x * channels;
      const di = (y * w + x) * 3;
      rgb[di] = cur[si]!;
      rgb[di + 1] = cur[si + 1]!;
      rgb[di + 2] = cur[si + 2]!;
    }
    prev.set(cur);
  }
  return { w, h, rgb };
}

/** Fetch (cached) one terrarium tile and return its per-pixel elevation grid
 *  (meters, row-major TILE×TILE). Throws on network failure (never fakes). */
function loadTile(z: number, x: number, y: number): Float32Array {
  const dir = `${CACHE_DIR}/${z}/${x}`;
  const path = `${dir}/${y}.png`;
  let png: Buffer | undefined;
  if (existsSync(path)) {
    const b = readFileSync(path);
    if (b.length > 8 && b[0] === 137 && b[1] === 80) png = b;
  }
  if (!png) {
    mkdirSync(dir, { recursive: true });
    const url = `https://s3.amazonaws.com/elevation-tiles-prod/terrarium/${z}/${x}/${y}.png`;
    let ok = false;
    for (let attempt = 0; attempt < 4 && !ok; attempt++) {
      try {
        execFileSync('curl', ['-sSfL', '--max-time', '60', '-o', path, url], { stdio: 'ignore' });
        const b = readFileSync(path);
        if (b.length > 8 && b[0] === 137 && b[1] === 80) {
          png = b;
          ok = true;
        }
      } catch {
        // retry with backoff
      }
      if (!ok) execFileSync('sleep', ['2']);
    }
    if (!png) throw new Error(`dem: failed to fetch terrarium tile ${z}/${x}/${y} (network blocked?)`);
  }
  const { w, h, rgb } = decodePngRgb(png);
  if (w !== TILE || h !== TILE) throw new Error(`dem: tile ${z}/${x}/${y} is ${w}x${h}, expected ${TILE}`);
  const elev = new Float32Array(TILE * TILE);
  for (let i = 0; i < TILE * TILE; i++) {
    const r = rgb[i * 3]!;
    const g = rgb[i * 3 + 1]!;
    const b = rgb[i * 3 + 2]!;
    elev[i] = r * 256 + g + b / 256 - 32768;
  }
  return elev;
}

/**
 * An elevation sampler over a lat/lon window at a fixed zoom. Loads (and
 * caches) all covering tiles up front, then answers bilinear `sample(lon,
 * lat)` queries in meters.
 */
export class DemSampler {
  private readonly z: number;
  private readonly tiles = new Map<string, Float32Array>();
  readonly zoom: number;

  constructor(bbox: [number, number, number, number], targetMeters = 30) {
    const [s, w, n, e] = bbox;
    const latMid = (s + n) / 2;
    this.z = pickZoom(latMid, targetMeters);
    this.zoom = this.z;
    const a = lonLatToTile(w, n, this.z); // NW corner (min tile x, min tile y)
    const b = lonLatToTile(e, s, this.z); // SE corner (max tile x, max tile y)
    const tx0 = Math.floor(a.xf);
    const ty0 = Math.floor(a.yf);
    const tx1 = Math.floor(b.xf);
    const ty1 = Math.floor(b.yf);
    for (let ty = ty0; ty <= ty1; ty++) {
      for (let tx = tx0; tx <= tx1; tx++) {
        this.tiles.set(`${tx}/${ty}`, loadTile(this.z, tx, ty));
      }
    }
  }

  /** Bilinear-sampled elevation (meters) at a geographic point. */
  sample(lonDeg: number, latDeg: number): number {
    const { xf, yf } = lonLatToTile(lonDeg, latDeg, this.z);
    // fractional pixel coordinates within the global tile pyramid
    const px = xf * TILE;
    const py = yf * TILE;
    const gx = Math.floor(px);
    const gy = Math.floor(py);
    const fx = px - gx;
    const fy = py - gy;
    const at = (gxi: number, gyi: number): number => {
      const tx = Math.floor(gxi / TILE);
      const ty = Math.floor(gyi / TILE);
      const tile = this.tiles.get(`${tx}/${ty}`);
      if (!tile) return 0;
      const lx = gxi - tx * TILE;
      const ly = gyi - ty * TILE;
      return tile[ly * TILE + lx]!;
    };
    const v00 = at(gx, gy);
    const v10 = at(gx + 1, gy);
    const v01 = at(gx, gy + 1);
    const v11 = at(gx + 1, gy + 1);
    return (v00 * (1 - fx) + v10 * fx) * (1 - fy) + (v01 * (1 - fx) + v11 * fx) * fy;
  }
}
