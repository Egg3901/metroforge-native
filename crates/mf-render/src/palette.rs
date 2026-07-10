//! Single source of truth for MetroForge Native's Mirror's-Edge art
//! direction (`art-direction.md`, BINDING — overrides any palette guidance
//! in the base spec). Every other `mf-render` module pulls its colors from
//! here rather than hard-coding hex values.

use bevy::color::LinearRgba;
use bevy::prelude::Color;

fn hex(r: u8, g: u8, b: u8) -> Color {
    Color::srgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

fn hexa(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::srgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a)
}

/// #f4f5f2 — near-white.
pub fn building_top() -> Color {
    hex(0xf4, 0xf5, 0xf2)
}

/// #e2e5e3 — cooler side faces, fake edge definition without lighting.
pub fn building_side() -> Color {
    hex(0xe2, 0xe5, 0xe3)
}

/// #d6dad8 — bottom skirt rows.
pub fn building_base() -> Color {
    hex(0xd6, 0xda, 0xd8)
}

/// #b9bec4 — night-dimmed building tone (art-direction §6).
pub fn building_night() -> Color {
    hex(0xb9, 0xbe, 0xc4)
}

/// #e9eae5 — everything not road/water/park.
pub fn ground() -> Color {
    hex(0xe9, 0xea, 0xe5)
}

/// #17181c — rich black, ALL road classes; differentiate by width only.
pub fn road() -> Color {
    hex(0x17, 0x18, 0x1c)
}

/// #2a2c32 — 1m hairline edge stripe on arterials, medium/high tier only.
pub fn road_edge() -> Color {
    hex(0x2a, 0x2c, 0x32)
}

/// #74b6e2 — light blue, saturated enough to survive the high key.
pub fn water() -> Color {
    hex(0x74, 0xb6, 0xe2)
}

/// #8cce8e — painted park green (owner: parks stay green, with trees).
pub fn park() -> Color {
    hex(0x8c, 0xce, 0x8e)
}

pub fn sky_day() -> Color {
    hex(0xdf, 0xe6, 0xea)
}

pub fn sky_night() -> Color {
    hex(0x0a, 0x0f, 0x1c)
}

/// Subway-view vignette edge tone: rgba(6,8,14,0.55).
pub fn vignette_edge(alpha: f32) -> Color {
    hexa(0x06, 0x08, 0x0e, alpha)
}

/// Bright brick + stripe colors, assigned in this fixed order; beyond the
/// eighth route, extend with a golden-angle hue rotation (see
/// [`vivid_route_color`]).
const ROUTE_COLORS_HEX: [(u8, u8, u8); 8] = [
    (0xff, 0x3b, 0x30),
    (0x00, 0x7a, 0xff),
    (0xff, 0xcc, 0x00),
    (0x34, 0xc7, 0x59),
    (0xff, 0x95, 0x00),
    (0xaf, 0x52, 0xde),
    (0x00, 0xc7, 0xbe),
    (0xff, 0x2d, 0x95),
];

/// The wire's `colorTable` carries the *web* palette's hex values — per
/// art-direction.md, the native client ignores those and keeps its own vivid
/// table indexed by `routeColorIdx` (same index = same color everywhere).
pub fn vivid_route_color(idx: usize) -> Color {
    if let Some(&(r, g, b)) = ROUTE_COLORS_HEX.get(idx) {
        return hex(r, g, b);
    }
    // Golden-angle hue rotation for indices beyond the fixed eight.
    let extra = (idx - ROUTE_COLORS_HEX.len()) as f32;
    let hue = (extra * 137.508) % 360.0;
    Color::hsl(hue, 0.85, 0.55)
}

/// A brighter variant of a route color, used for chevron arrows painted on
/// route stripes ("same color, 20% brighter" — art-direction §3).
pub fn brighten(color: Color, amount: f32) -> Color {
    let srgba = color.to_srgba();
    Color::srgba(
        (srgba.red * (1.0 + amount)).min(1.0),
        (srgba.green * (1.0 + amount)).min(1.0),
        (srgba.blue * (1.0 + amount)).min(1.0),
        srgba.alpha,
    )
}

/// Scale a `Color`'s linear RGB by `strength` for use as a `StandardMaterial`
/// `emissive` value. `bevy_color::LinearRgba` has no `Mul<f32>` impl, so this
/// is a small manual field-scale helper used throughout the vivid transit
/// materials (stations/routes/vehicles) to make them "glow" at night /
/// in subway view.
pub fn emissive(color: Color, strength: f32) -> LinearRgba {
    let l = color.to_linear();
    LinearRgba::rgb(l.red * strength, l.green * strength, l.blue * strength)
}

/// Mode accent tints for station marker rings (body stays white).
pub fn mode_accent(mode: mf_protocol::TransitMode) -> Color {
    match mode {
        mf_protocol::TransitMode::Bus => hex(0xff, 0x95, 0x00),
        mf_protocol::TransitMode::Tram => hex(0x34, 0xc7, 0x59),
        mf_protocol::TransitMode::Metro => hex(0x00, 0x7a, 0xff),
        mf_protocol::TransitMode::Rail => hex(0xaf, 0x52, 0xde),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_eight_routes_are_the_fixed_bricks() {
        for (i, &(r, g, b)) in ROUTE_COLORS_HEX.iter().enumerate() {
            assert_eq!(vivid_route_color(i), hex(r, g, b));
        }
    }

    #[test]
    fn ninth_route_extends_via_golden_angle_and_differs_from_first() {
        assert_ne!(vivid_route_color(8), vivid_route_color(0));
    }
}
