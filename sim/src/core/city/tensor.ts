/**
 * Tensor field for street orientation — Chen et al., "Interactive Procedural
 * Street Modeling" (SIGGRAPH 2008). A 2D symmetric traceless tensor is stored
 * in angle-doubled form (cos 2θ, sin 2θ) so that θ and θ+π blend as the same
 * street direction. Basis fields (grid patches, one radial center, water
 * boundaries) are blended with distance falloffs; streets trace the major
 * eigenvector (θ) or the minor one (θ + π/2).
 */
import type { Vec2 } from '../geometry';

export interface GridBasis {
  center: Vec2;
  theta: number;
  /** Gaussian falloff radius, meters */
  sigma: number;
  weight: number;
}

export interface BoundarySample {
  pos: Vec2;
  /** tangent angle of the shoreline at this sample */
  theta: number;
}

export interface TensorField {
  grids: GridBasis[];
  /** citywide constant orientation (no falloff) — makes rigid grid cities read
   *  as one coherent grid instead of patches the radial field bends apart */
  globalGrid?: { theta: number; weight: number };
  radialCenter: Vec2;
  radialWeight: number;
  radialSigma: number;
  boundaries: BoundarySample[];
  boundarySigma: number;
  boundaryWeight: number;
  /** small angular noise to break perfect regularity */
  noise: (x: number, y: number) => number;
  noiseWeight: number;
}

/** Street direction (major eigenvector angle) at a point. */
export function sampleAngle(f: TensorField, p: Vec2): number {
  let c2 = 0;
  let s2 = 0;

  if (f.globalGrid) {
    c2 += f.globalGrid.weight * Math.cos(2 * f.globalGrid.theta);
    s2 += f.globalGrid.weight * Math.sin(2 * f.globalGrid.theta);
  }

  for (const g of f.grids) {
    const dx = p.x - g.center.x;
    const dy = p.y - g.center.y;
    const w = g.weight * Math.exp(-(dx * dx + dy * dy) / (2 * g.sigma * g.sigma));
    c2 += w * Math.cos(2 * g.theta);
    s2 += w * Math.sin(2 * g.theta);
  }

  {
    const dx = p.x - f.radialCenter.x;
    const dy = p.y - f.radialCenter.y;
    const d = Math.sqrt(dx * dx + dy * dy) + 1;
    const theta = Math.atan2(dy, dx); // radial: streets point at the center
    const w = f.radialWeight * Math.exp(-d / f.radialSigma);
    c2 += w * Math.cos(2 * theta);
    s2 += w * Math.sin(2 * theta);
  }

  for (const b of f.boundaries) {
    const dx = p.x - b.pos.x;
    const dy = p.y - b.pos.y;
    const d = Math.sqrt(dx * dx + dy * dy);
    if (d > f.boundarySigma * 3) continue;
    const w = f.boundaryWeight * Math.exp(-d / f.boundarySigma);
    c2 += w * Math.cos(2 * b.theta);
    s2 += w * Math.sin(2 * b.theta);
  }

  let theta = 0.5 * Math.atan2(s2, c2);
  theta += (f.noise(p.x, p.y) - 0.5) * f.noiseWeight;
  return theta;
}

/**
 * Direction to step in at p, following the field along `eigen` (0 = major,
 * 1 = minor/perpendicular), keeping continuity with the previous direction
 * (eigenvectors are sign-ambiguous).
 */
export function stepDirection(f: TensorField, p: Vec2, eigen: 0 | 1, prev: Vec2 | null): Vec2 {
  let theta = sampleAngle(f, p);
  if (eigen === 1) theta += Math.PI / 2;
  let dx = Math.cos(theta);
  let dy = Math.sin(theta);
  if (prev && dx * prev.x + dy * prev.y < 0) {
    dx = -dx;
    dy = -dy;
  }
  return { x: dx, y: dy };
}
