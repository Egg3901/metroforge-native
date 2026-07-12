/**
 * Real-city datasets imported from OpenStreetMap (see scripts/build-cities.ts).
 * The bundle carries the real road network + a baked water mask; the generator
 * lays procedural population/jobs/districts on top of the real land.
 */

export interface OsmCityData {
  key: string;
  label: string;
  worldSize: number;
  maskRes: number;
  /** base64 of a maskRes×maskRes Uint8 grid, 1 = water, row-major over the
   *  world square [-worldSize/2, worldSize/2] */
  waterMask: string;
  /** same grid, 1 = park/green (Central Park, Boston Common, …) */
  parkMask?: string;
  /** same grid, 1 = real OSM building footprint coverage */
  buildingMask?: string;
  /** masks are 1-bit-per-cell packed (vs legacy 1-byte-per-cell) */
  maskPacked?: boolean;
  /** real-elevation heightfield side length (elevRes×elevRes), if baked */
  elevRes?: number;
  /** base64 little-endian Int16 grid of real meters, row-major over the world
   *  square (row 0 = north edge, same convention as the masks) */
  elevation?: string;
  roads: { cls: string; pts: number[] }[];
  /** real OSM place names for map labels */
  labels?: MapLabel[];
}

export interface MapLabel {
  kind: 'road' | 'water' | 'park';
  name: string;
  x: number;
  y: number;
  /** road labels: baseline angle in radians */
  angle?: number;
  /** importance 1..~5 → drives zoom-gated visibility + size */
  imp: number;
}

/** Decode a base64 mask to a Uint8Array of `n` cells (1 = set). Packed = 1 bit
 *  per cell (current format); otherwise 1 byte per cell (legacy). */
export function decodeB64Mask(b64: string, n?: number, packed = true): Uint8Array {
  const bin = atob(b64);
  if (!packed) {
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }
  const count = n ?? bin.length * 8;
  const out = new Uint8Array(count);
  for (let i = 0; i < count; i++) out[i] = (bin.charCodeAt(i >> 3) >> (i & 7)) & 1;
  return out;
}

/** Decode a base64 little-endian Int16 elevation grid to an Int16Array of
 *  `res*res` meters (row-major, row 0 = north edge). */
export function decodeElevation(b64: string, res: number): Int16Array {
  const bin = atob(b64);
  const out = new Int16Array(res * res);
  for (let i = 0; i < out.length; i++) {
    const lo = bin.charCodeAt(i * 2) & 0xff;
    const hi = bin.charCodeAt(i * 2 + 1) & 0xff;
    out[i] = ((hi << 8) | lo) << 16 >> 16; // sign-extend
  }
  return out;
}

/** Sample a mask at a world point → true if the cell is set. */
export function maskAt(mask: Uint8Array, res: number, worldSize: number, x: number, y: number): boolean {
  const half = worldSize / 2;
  const c = Math.floor(((x + half) / worldSize) * res);
  const r = Math.floor(((y + half) / worldSize) * res);
  if (c < 0 || r < 0 || c >= res || r >= res) return false;
  return mask[r * res + c] === 1;
}
