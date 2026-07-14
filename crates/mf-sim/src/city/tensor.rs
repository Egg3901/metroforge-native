//! Tensor field for street orientation. Port of `sim/src/core/city/tensor.ts`.
//!
//! Chen et al., "Interactive Procedural Street Modeling" (SIGGRAPH 2008). A 2D
//! symmetric traceless tensor is stored in angle-doubled form so that theta and
//! theta+pi blend as the same street direction. Basis fields (grid patches, one
//! radial center, water boundaries) blend with distance falloffs; streets trace
//! the major eigenvector (theta) or the minor one (theta + pi/2).

use crate::geometry::Vec2;

/// A grid-orientation basis patch. Mirrors `GridBasis`.
#[derive(Clone, Copy, Debug)]
pub struct GridBasis {
    /// Patch center.
    pub center: Vec2,
    /// Orientation angle (radians).
    pub theta: f64,
    /// Gaussian falloff radius, meters.
    pub sigma: f64,
    /// Blend weight.
    pub weight: f64,
}

/// A shoreline tangent sample. Mirrors `BoundarySample`.
#[derive(Clone, Copy, Debug)]
pub struct BoundarySample {
    /// Sample position.
    pub pos: Vec2,
    /// Tangent angle of the shoreline at this sample.
    pub theta: f64,
}

/// The blended tensor field. Mirrors `TensorField`. The angular `noise` source
/// is a boxed closure (the TS field is a function); it borrows the detail
/// noise, so the field carries a lifetime.
pub struct TensorField<'a> {
    /// Grid patches.
    pub grids: Vec<GridBasis>,
    /// Citywide constant orientation (theta, weight); no falloff.
    pub global_grid: Option<(f64, f64)>,
    /// Radial convergence center.
    pub radial_center: Vec2,
    /// Radial blend weight.
    pub radial_weight: f64,
    /// Radial falloff length, meters.
    pub radial_sigma: f64,
    /// Shoreline boundary samples.
    pub boundaries: Vec<BoundarySample>,
    /// Boundary falloff length, meters.
    pub boundary_sigma: f64,
    /// Boundary blend weight.
    pub boundary_weight: f64,
    /// Small angular noise source.
    pub noise: Box<dyn Fn(f64, f64) -> f64 + 'a>,
    /// Noise weight.
    pub noise_weight: f64,
}

/// Street direction (major eigenvector angle) at a point. Mirrors `sampleAngle`.
pub fn sample_angle(f: &TensorField, p: Vec2) -> f64 {
    let mut c2 = 0.0;
    let mut s2 = 0.0;

    if let Some((theta, weight)) = f.global_grid {
        c2 += weight * (2.0 * theta).cos();
        s2 += weight * (2.0 * theta).sin();
    }

    for g in &f.grids {
        let dx = p.x - g.center.x;
        let dy = p.y - g.center.y;
        let w = g.weight * (-(dx * dx + dy * dy) / (2.0 * g.sigma * g.sigma)).exp();
        c2 += w * (2.0 * g.theta).cos();
        s2 += w * (2.0 * g.theta).sin();
    }

    {
        let dx = p.x - f.radial_center.x;
        let dy = p.y - f.radial_center.y;
        let d = (dx * dx + dy * dy).sqrt() + 1.0;
        let theta = dy.atan2(dx); // radial: streets point at the center
        let w = f.radial_weight * (-d / f.radial_sigma).exp();
        c2 += w * (2.0 * theta).cos();
        s2 += w * (2.0 * theta).sin();
    }

    for b in &f.boundaries {
        let dx = p.x - b.pos.x;
        let dy = p.y - b.pos.y;
        let d = (dx * dx + dy * dy).sqrt();
        if d > f.boundary_sigma * 3.0 {
            continue;
        }
        let w = f.boundary_weight * (-d / f.boundary_sigma).exp();
        c2 += w * (2.0 * b.theta).cos();
        s2 += w * (2.0 * b.theta).sin();
    }

    let mut theta = 0.5 * s2.atan2(c2);
    theta += ((f.noise)(p.x, p.y) - 0.5) * f.noise_weight;
    theta
}

/// Direction to step in at `p`, following `eigen` (0 = major, 1 = minor),
/// keeping continuity with `prev`. Mirrors `stepDirection`.
pub fn step_direction(f: &TensorField, p: Vec2, eigen: u8, prev: Option<Vec2>) -> Vec2 {
    let mut theta = sample_angle(f, p);
    if eigen == 1 {
        theta += std::f64::consts::FRAC_PI_2;
    }
    let mut dx = theta.cos();
    let mut dy = theta.sin();
    if let Some(pv) = prev {
        if dx * pv.x + dy * pv.y < 0.0 {
            dx = -dx;
            dy = -dy;
        }
    }
    Vec2 { x: dx, y: dy }
}
