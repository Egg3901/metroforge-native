//! Single source of truth for MetroForge Native's Mirror's-Edge art
//! direction (`art-direction.md`, BINDING — overrides any palette guidance
//! in the base spec). Every other `mf-render` module pulls its colors from
//! here rather than hard-coding hex values.
//!
//! Theme system (issue #32): every function below is theme-indexed. The
//! active [`mf_state::Theme`] is tracked in a small process-global atomic
//! (`set_theme`/`current_theme`) rather than threaded as a `Res<Theme>`
//! parameter through every call site — nearly all of these functions are
//! called from plain material-build helpers deep inside `terrain.rs`/
//! `buildings.rs`/`roads.rs`/`transit.rs`/`vehicles.rs` that have no ECS
//! access of their own, only their caller systems do. A theme change is rare
//! (a menu click), so a system that watches `Res<Theme>` and calls
//! `set_theme` once per change (see `lib.rs`'s `sync_theme_system`) is far
//! simpler than rewriting every helper's signature. `Theme::Light` (value
//! `0`) is both the atomic's initial value and `Theme::default()`, so any
//! color read before that sync system has run for the first time still
//! matches the Light table exactly — no visual glitch on startup.
//!
//! `Theme::Light`'s table is byte-for-byte the original (pre-theme) values;
//! selecting it must never change a single pixel.

use bevy::color::LinearRgba;
use bevy::prelude::Color;
use mf_state::{ColorblindMode, Theme};
use std::sync::atomic::{AtomicU8, Ordering};

fn hex(r: u8, g: u8, b: u8) -> Color {
    Color::srgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

fn hexa(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::srgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a)
}

type Rgb = (u8, u8, u8);

/// One theme's full color table. Every field mirrors a public function
/// below; see each function's doc comment for what the role means visually.
struct Palette {
    building_top: Rgb,
    building_side: Rgb,
    building_base: Rgb,
    building_night: Rgb,
    ground: Rgb,
    road: Rgb,
    road_edge: Rgb,
    water: Rgb,
    park: Rgb,
    sky_day: Rgb,
    sky_night: Rgb,
    vignette_edge: Rgb,
    route_colors: [Rgb; 8],
    mode_bus: Rgb,
    mode_tram: Rgb,
    mode_metro: Rgb,
    mode_rail: Rgb,
}

/// The original Mirror's-Edge white-city palette — every value here is
/// unchanged from the pre-theme-system `palette.rs`.
const LIGHT: Palette = Palette {
    building_top: (0xf4, 0xf5, 0xf2),
    building_side: (0xe2, 0xe5, 0xe3),
    building_base: (0xd6, 0xda, 0xd8),
    building_night: (0xb9, 0xbe, 0xc4),
    ground: (0xe9, 0xea, 0xe5),
    road: (0x17, 0x18, 0x1c),
    road_edge: (0x2a, 0x2c, 0x32),
    water: (0x74, 0xb6, 0xe2),
    park: (0x8c, 0xce, 0x8e),
    sky_day: (0xdf, 0xe6, 0xea),
    sky_night: (0x0a, 0x0f, 0x1c),
    vignette_edge: (0x06, 0x08, 0x0e),
    route_colors: [
        (0xff, 0x3b, 0x30),
        (0x00, 0x7a, 0xff),
        (0xff, 0xcc, 0x00),
        (0x34, 0xc7, 0x59),
        (0xff, 0x95, 0x00),
        (0xaf, 0x52, 0xde),
        (0x00, 0xc7, 0xbe),
        (0xff, 0x2d, 0x95),
    ],
    mode_bus: (0xff, 0x95, 0x00),
    mode_tram: (0x34, 0xc7, 0x59),
    mode_metro: (0x00, 0x7a, 0xff),
    mode_rail: (0xaf, 0x52, 0xde),
};

/// "Near-black buildings/ground, light route colors, glowing transit" — the
/// existing night rig's values promoted to a standing theme rather than a
/// time-of-day state. `sky_day == sky_night` and `building_night ==
/// building_top` deliberately: with the theme already dark, the day/night
/// cycle (`daynight.rs`) has nothing left to animate toward, so it becomes a
/// no-op instead of needing a separate "disable day/night" branch.
const DARK: Palette = Palette {
    building_top: (0x1c, 0x1e, 0x22),
    building_side: (0x15, 0x16, 0x1a),
    building_base: (0x0d, 0x0e, 0x11),
    building_night: (0x1c, 0x1e, 0x22),
    // Ground lifted a notch so rich-black roads still read as streets at
    // overview zoom (was ~2:1 road/ground; Light keeps ~15:1).
    ground: (0x14, 0x16, 0x1a),
    road: (0x08, 0x09, 0x0c),
    road_edge: (0x4a, 0x4e, 0x56),
    water: (0x3f, 0xa9, 0xe0),
    park: (0x4f, 0xd9, 0x7a),
    sky_day: (0x05, 0x07, 0x0c),
    sky_night: (0x05, 0x07, 0x0c),
    vignette_edge: (0x04, 0x05, 0x0a),
    route_colors: [
        (0xff, 0x8a, 0x82),
        (0x6b, 0xbb, 0xff),
        (0xff, 0xe1, 0x66),
        (0x86, 0xe0, 0x9c),
        (0xff, 0xc0, 0x66),
        (0xd3, 0x9a, 0xf0),
        (0x66, 0xe4, 0xdd),
        (0xff, 0x8a, 0xc6),
    ],
    mode_bus: (0xff, 0xc0, 0x66),
    mode_tram: (0x86, 0xe0, 0x9c),
    mode_metro: (0x6b, 0xbb, 0xff),
    mode_rail: (0xd3, 0x9a, 0xf0),
};

/// Violet/vaporwave palette variant: deep purple city, hot-pink road
/// hairlines, neon teal/magenta transit.
const PURPLE: Palette = Palette {
    building_top: (0x2b, 0x1b, 0x4e),
    building_side: (0x24, 0x1a, 0x44),
    building_base: (0x1a, 0x12, 0x36),
    building_night: (0x2b, 0x1b, 0x4e),
    // Slightly lighter ground + deeper road so the street grid separates
    // from the violet city mass without relying only on pink hairlines.
    ground: (0x22, 0x14, 0x3c),
    road: (0x12, 0x08, 0x22),
    road_edge: (0xff, 0x2e, 0xc4),
    water: (0x29, 0xe6, 0xd8),
    park: (0x2d, 0xe2, 0xc9),
    sky_day: (0x15, 0x0a, 0x2e),
    sky_night: (0x15, 0x0a, 0x2e),
    vignette_edge: (0x10, 0x08, 0x1f),
    route_colors: [
        (0xff, 0x2e, 0xc4),
        (0x29, 0xe6, 0xff),
        (0xff, 0xe1, 0x4d),
        (0x39, 0xff, 0x9d),
        (0xff, 0x8a, 0x2e),
        (0xb0, 0x5c, 0xff),
        (0x2d, 0xe2, 0xc9),
        (0xff, 0x5c, 0xe8),
    ],
    mode_bus: (0xff, 0x8a, 0x2e),
    mode_tram: (0x39, 0xff, 0x9d),
    mode_metro: (0x29, 0xe6, 0xff),
    mode_rail: (0xb0, 0x5c, 0xff),
};

fn palette_for(theme: Theme) -> &'static Palette {
    match theme {
        Theme::Light => &LIGHT,
        Theme::Dark => &DARK,
        Theme::Purple => &PURPLE,
    }
}

// ---------------------------------------------------------------------------
// Colorblind remaps (accessibility). Only the vivid route table and the four
// mode accents shift; city massing colors stay theme-authored.
//
// Chosen values (Okabe-Ito / Paul Tol inspired) and why they stay
// distinguishable under each deficiency:
//
// Deuteranopia (green-blind): avoid red/green pairs. Table uses orange,
// sky-blue, yellow, blue, vermillion, purple, olive (not green), magenta —
// pairwise RGB + deutan-sim distance stays above the test floor.
//   routes: E69F00, 56B4E9, F0E442, 0072B2, D55E00, CC79A7, 999933, 882255
//   accents bus/tram/metro/rail: E69F00 / F0E442 / 0072B2 / CC79A7
//
// Protanopia (red-blind): similar red/green issue but reds darken further.
// Prefer brighter oranges and cooler blues; drop dark vermillion.
//   routes: F0E442, 56B4E9, 009E73, 0072B2, E69F00, CC79A7, 4DB6AC, 882255
//   accents: E69F00 / 009E73 / 56B4E9 / CC79A7
//
// Tritanopia (blue-yellow blind): avoid blue/yellow pairs. Lean on red,
// green, orange, purple, brown, pink, gray, cyan-green.
//   routes: E41A1C, 4DAF4A, FF7F00, 984EA3, A65628, F781BF, 1A1A1A, 66C2A5
//   accents: FF7F00 / 4DAF4A / E41A1C / 984EA3
// ---------------------------------------------------------------------------

struct AccentRemap {
    route_colors: [Rgb; 8],
    mode_bus: Rgb,
    mode_tram: Rgb,
    mode_metro: Rgb,
    mode_rail: Rgb,
}

const DEUTERANOPIA: AccentRemap = AccentRemap {
    route_colors: [
        (0xe6, 0x9f, 0x00),
        (0x56, 0xb4, 0xe9),
        (0xf0, 0xe4, 0x42),
        (0x00, 0x72, 0xb2),
        (0xd5, 0x5e, 0x00),
        (0xcc, 0x79, 0xa7),
        // Olive (yellow-leaning) instead of bluish-green: under deutan
        // simulation, #009E73 collapses toward #0072B2.
        (0x99, 0x99, 0x33),
        (0x88, 0x22, 0x55),
    ],
    mode_bus: (0xe6, 0x9f, 0x00),
    mode_tram: (0xf0, 0xe4, 0x42),
    mode_metro: (0x00, 0x72, 0xb2),
    mode_rail: (0xcc, 0x79, 0xa7),
};

const PROTANOPIA: AccentRemap = AccentRemap {
    route_colors: [
        (0xf0, 0xe4, 0x42),
        (0x56, 0xb4, 0xe9),
        (0x00, 0x9e, 0x73),
        (0x00, 0x72, 0xb2),
        (0xe6, 0x9f, 0x00),
        (0xcc, 0x79, 0xa7),
        (0x4d, 0xb6, 0xac),
        // Magenta-brown instead of deep indigo: indigo collapses toward
        // bluish-green under protan simulation.
        (0x88, 0x22, 0x55),
    ],
    mode_bus: (0xe6, 0x9f, 0x00),
    mode_tram: (0x00, 0x9e, 0x73),
    mode_metro: (0x56, 0xb4, 0xe9),
    mode_rail: (0xcc, 0x79, 0xa7),
};

const TRITANOPIA: AccentRemap = AccentRemap {
    route_colors: [
        (0xe4, 0x1a, 0x1c),
        (0x4d, 0xaf, 0x4a),
        (0xff, 0x7f, 0x00),
        (0x98, 0x4e, 0xa3),
        (0xa6, 0x56, 0x28),
        (0xf7, 0x81, 0xbf),
        // Near-black instead of mid-gray: gray collapses toward purple under
        // tritan simulation.
        (0x1a, 0x1a, 0x1a),
        (0x66, 0xc2, 0xa5),
    ],
    mode_bus: (0xff, 0x7f, 0x00),
    mode_tram: (0x4d, 0xaf, 0x4a),
    mode_metro: (0xe4, 0x1a, 0x1c),
    mode_rail: (0x98, 0x4e, 0xa3),
};

fn accent_remap(mode: ColorblindMode) -> Option<&'static AccentRemap> {
    match mode {
        ColorblindMode::Off => None,
        ColorblindMode::Deuteranopia => Some(&DEUTERANOPIA),
        ColorblindMode::Protanopia => Some(&PROTANOPIA),
        ColorblindMode::Tritanopia => Some(&TRITANOPIA),
    }
}

/// Process-global "which theme's colors should `palette.rs` return right
/// now" — see the module doc comment for why this isn't a `Res<Theme>`
/// parameter. `0 == Theme::Light`, matching both the atomic's initial value
/// and `Theme::default()`.
static CURRENT_THEME: AtomicU8 = AtomicU8::new(0);

/// Process-global colorblind shift. `0 == ColorblindMode::Off`.
static CURRENT_COLORBLIND: AtomicU8 = AtomicU8::new(0);

/// Called by `lib.rs`'s `sync_theme_system` whenever `Res<Theme>` changes
/// (and once at startup) — every `palette::` color function reads back
/// through [`current_theme`] the next time it's called.
pub fn set_theme(theme: Theme) {
    CURRENT_THEME.store(theme as u8, Ordering::Relaxed);
}

/// Publish the active [`ColorblindMode`] (mirrors [`set_theme`]).
pub fn set_colorblind_mode(mode: ColorblindMode) {
    let v = match mode {
        ColorblindMode::Off => 0,
        ColorblindMode::Deuteranopia => 1,
        ColorblindMode::Protanopia => 2,
        ColorblindMode::Tritanopia => 3,
    };
    CURRENT_COLORBLIND.store(v, Ordering::Relaxed);
}

/// The theme every `palette::` color function below currently paints with.
pub fn current_theme() -> Theme {
    match CURRENT_THEME.load(Ordering::Relaxed) {
        1 => Theme::Dark,
        2 => Theme::Purple,
        _ => Theme::Light,
    }
}

/// Active colorblind remapping (defaults to [`ColorblindMode::Off`]).
pub fn current_colorblind_mode() -> ColorblindMode {
    match CURRENT_COLORBLIND.load(Ordering::Relaxed) {
        1 => ColorblindMode::Deuteranopia,
        2 => ColorblindMode::Protanopia,
        3 => ColorblindMode::Tritanopia,
        _ => ColorblindMode::Off,
    }
}

fn active() -> &'static Palette {
    palette_for(current_theme())
}

/// Building roof/top faces.
pub fn building_top() -> Color {
    let (r, g, b) = active().building_top;
    hex(r, g, b)
}

/// Cooler side faces, fake edge definition without lighting.
pub fn building_side() -> Color {
    let (r, g, b) = active().building_side;
    hex(r, g, b)
}

/// Bottom skirt rows.
pub fn building_base() -> Color {
    let (r, g, b) = active().building_base;
    hex(r, g, b)
}

/// Night-dimmed building tone (art-direction §6). Equal to
/// [`building_top`] on `Dark`/`Purple` — those themes are already dark, so
/// the day/night dim has nothing further to do.
pub fn building_night() -> Color {
    let (r, g, b) = active().building_night;
    hex(r, g, b)
}

/// Everything not road/water/park.
pub fn ground() -> Color {
    let (r, g, b) = active().ground;
    hex(r, g, b)
}

/// ALL road classes; differentiate by width only.
pub fn road() -> Color {
    let (r, g, b) = active().road;
    hex(r, g, b)
}

/// 1m hairline edge stripe on arterials, medium/high tier only.
pub fn road_edge() -> Color {
    let (r, g, b) = active().road_edge;
    hex(r, g, b)
}

pub fn water() -> Color {
    let (r, g, b) = active().water;
    hex(r, g, b)
}

/// Painted park green (owner: parks stay green, with trees).
pub fn park() -> Color {
    let (r, g, b) = active().park;
    hex(r, g, b)
}

pub fn sky_day() -> Color {
    let (r, g, b) = active().sky_day;
    hex(r, g, b)
}

pub fn sky_night() -> Color {
    let (r, g, b) = active().sky_night;
    hex(r, g, b)
}

/// Subway-view vignette edge tone.
pub fn vignette_edge(alpha: f32) -> Color {
    let (r, g, b) = active().vignette_edge;
    hexa(r, g, b, alpha)
}

/// Bright brick + stripe colors, assigned in this fixed order; beyond the
/// eighth route, extend with a golden-angle hue rotation (see
/// [`vivid_route_color`]).
///
/// The wire's `colorTable` carries the *web* palette's hex values — per
/// art-direction.md, the native client ignores those and keeps its own
/// theme-indexed table indexed by `routeColorIdx` (same index = same color
/// everywhere, for a given theme).
pub fn vivid_route_color(idx: usize) -> Color {
    let table = if let Some(remap) = accent_remap(current_colorblind_mode()) {
        &remap.route_colors
    } else {
        &active().route_colors
    };
    if let Some(&(r, g, b)) = table.get(idx) {
        return hex(r, g, b);
    }
    // Golden-angle hue rotation for indices beyond the fixed eight.
    let extra = (idx - table.len()) as f32;
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

// ---------------------------------------------------------------------------
// Diorama strata (v0.8 Underground / Captain Toad art-direction amendment).
// The floating-slab cut sides and the cutaway cut-face display clean banded
// flat-color geology. Per the BINDING art direction these muted earth tones
// are "the only non-transit color family besides water/parks", so — unlike
// route colors — they are NOT theme-indexed or colorblind-remapped: a single
// authored earth palette reads correctly on the white Light city and the dark
// themes alike (the bands sit on the slab's own shaded sides, not the bright
// tabletop). Warm tan fill → muted ochre clay → grey-brown rock → darkest
// bedrock, plus a thin vivid blue water-table line.
// ---------------------------------------------------------------------------

/// Fill / soil band — warm tan (topmost).
pub fn strata_fill() -> Color {
    hex(0xc9, 0xa9, 0x76)
}

/// Clay / sand band — muted ochre.
pub fn strata_clay() -> Color {
    hex(0xb0, 0x8b, 0x55)
}

/// Competent-rock band — grey-brown.
pub fn strata_rock() -> Color {
    hex(0x7a, 0x70, 0x62)
}

/// Deep basement rock — darkest.
pub fn strata_bedrock() -> Color {
    hex(0x45, 0x40, 0x3a)
}

/// Thin water-table line drawn across the strata (vivid blue; animated on
/// Medium+). Shares the water hue family so it reads as "groundwater".
pub fn strata_water_table() -> Color {
    hex(0x3f, 0xa9, 0xe0)
}

/// The band color for a reconstructed [`crate::geology::Stratum`].
pub fn strata_color(kind: crate::geology::Stratum) -> Color {
    use crate::geology::Stratum;
    match kind {
        Stratum::Fill => strata_fill(),
        Stratum::Clay => strata_clay(),
        Stratum::Rock => strata_rock(),
        Stratum::Bedrock => strata_bedrock(),
    }
}

/// Soft dark quad drawn on the backdrop under the floating slab so the diorama
/// reads as a model hovering in the void (art-direction amendment: "gentle
/// contact shadow under the slab so it floats"). Translucent near-black.
pub fn contact_shadow() -> Color {
    hexa(0x00, 0x00, 0x00, 0.28)
}

/// Mode accent tints for station marker rings (body stays white).
pub fn mode_accent(mode: mf_protocol::TransitMode) -> Color {
    let (r, g, b) = if let Some(remap) = accent_remap(current_colorblind_mode()) {
        match mode {
            mf_protocol::TransitMode::Bus => remap.mode_bus,
            mf_protocol::TransitMode::Tram => remap.mode_tram,
            mf_protocol::TransitMode::Metro => remap.mode_metro,
            mf_protocol::TransitMode::Rail => remap.mode_rail,
        }
    } else {
        let p = active();
        match mode {
            mf_protocol::TransitMode::Bus => p.mode_bus,
            mf_protocol::TransitMode::Tram => p.mode_tram,
            mf_protocol::TransitMode::Metro => p.mode_metro,
            mf_protocol::TransitMode::Rail => p.mode_rail,
        }
    };
    hex(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `CURRENT_THEME` is a single process-global atomic, and `cargo test`
    /// runs tests in this module concurrently by default — without
    /// serializing, one test's `set_theme` call could flip the atomic out
    /// from under another test mid-assertion. A single mutex held for each
    /// test's whole body (rather than per-`set_theme`-call) keeps every
    /// test's read-after-write sequence atomic relative to its neighbors.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Resets the atomic to `Light` (tests should leave it however they
    /// like otherwise, since the lock guard already serializes them against
    /// each other) so each assertion's expectations don't depend on
    /// execution order.
    fn reset() {
        set_theme(Theme::Light);
        set_colorblind_mode(ColorblindMode::Off);
    }

    #[test]
    fn defaults_to_light_theme() {
        let _guard = lock();
        reset();
        assert_eq!(current_theme(), Theme::Light);
    }

    #[test]
    fn light_theme_first_eight_routes_are_the_fixed_bricks() {
        let _guard = lock();
        reset();
        for (i, &(r, g, b)) in LIGHT.route_colors.iter().enumerate() {
            assert_eq!(vivid_route_color(i), hex(r, g, b));
        }
    }

    #[test]
    fn ninth_route_extends_via_golden_angle_and_differs_from_first() {
        let _guard = lock();
        reset();
        assert_ne!(vivid_route_color(8), vivid_route_color(0));
    }

    #[test]
    fn switching_theme_changes_building_top() {
        let _guard = lock();
        reset();
        let light = building_top();
        set_theme(Theme::Dark);
        let dark = building_top();
        set_theme(Theme::Purple);
        let purple = building_top();
        assert_ne!(light, dark);
        assert_ne!(light, purple);
        assert_ne!(dark, purple);
        reset();
    }

    #[test]
    fn dark_and_purple_pin_sky_day_to_sky_night() {
        let _guard = lock();
        reset();
        set_theme(Theme::Dark);
        assert_eq!(sky_day(), sky_night());
        set_theme(Theme::Purple);
        assert_eq!(sky_day(), sky_night());
        reset();
    }

    #[test]
    fn every_theme_route_table_has_eight_distinct_colors() {
        let _guard = lock();
        for theme in Theme::ALL {
            let table = palette_for(theme).route_colors;
            for i in 0..table.len() {
                for j in (i + 1)..table.len() {
                    assert_ne!(table[i], table[j], "{theme:?} route {i} vs {j}");
                }
            }
        }
    }

    fn rgb_dist(a: Rgb, b: Rgb) -> f32 {
        let dr = a.0 as f32 - b.0 as f32;
        let dg = a.1 as f32 - b.1 as f32;
        let db = a.2 as f32 - b.2 as f32;
        (dr * dr + dg * dg + db * db).sqrt()
    }

    /// Crude CVD simulation matrices (Machado et al. approximations) used
    /// only to assert remapped route sets stay separable under the target
    /// deficiency — not for runtime rendering.
    fn simulate_cvd(rgb: Rgb, mode: ColorblindMode) -> (f32, f32, f32) {
        let r = rgb.0 as f32 / 255.0;
        let g = rgb.1 as f32 / 255.0;
        let b = rgb.2 as f32 / 255.0;
        let (m00, m01, m02, m10, m11, m12, m20, m21, m22) = match mode {
            ColorblindMode::Deuteranopia => (0.625, 0.375, 0.0, 0.7, 0.3, 0.0, 0.0, 0.3, 0.7),
            ColorblindMode::Protanopia => (0.567, 0.433, 0.0, 0.558, 0.442, 0.0, 0.0, 0.242, 0.758),
            ColorblindMode::Tritanopia => (0.95, 0.05, 0.0, 0.0, 0.433, 0.567, 0.0, 0.475, 0.525),
            ColorblindMode::Off => (1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0),
        };
        (
            m00 * r + m01 * g + m02 * b,
            m10 * r + m11 * g + m12 * b,
            m20 * r + m21 * g + m22 * b,
        )
    }

    fn sim_dist(a: Rgb, b: Rgb, mode: ColorblindMode) -> f32 {
        let (ar, ag, ab) = simulate_cvd(a, mode);
        let (br, bg, bb) = simulate_cvd(b, mode);
        let dr = ar - br;
        let dg = ag - bg;
        let db = ab - bb;
        (dr * dr + dg * dg + db * db).sqrt()
    }

    #[test]
    fn colorblind_route_tables_are_pairwise_distinguishable() {
        let _guard = lock();
        // Minimum simulated distance (0..sqrt(3) in linear RGB). Empirically
        // Okabe-Ito pairs clear ~0.25; keep a conservative floor.
        const MIN_SIM: f32 = 0.18;
        const MIN_RGB: f32 = 60.0;
        for mode in [
            ColorblindMode::Deuteranopia,
            ColorblindMode::Protanopia,
            ColorblindMode::Tritanopia,
        ] {
            let table = accent_remap(mode).unwrap().route_colors;
            for i in 0..table.len() {
                for j in (i + 1)..table.len() {
                    assert!(
                        rgb_dist(table[i], table[j]) >= MIN_RGB,
                        "{mode:?} route {i} vs {j} RGB too close"
                    );
                    assert!(
                        sim_dist(table[i], table[j], mode) >= MIN_SIM,
                        "{mode:?} route {i} vs {j} CVD-sim too close ({})",
                        sim_dist(table[i], table[j], mode)
                    );
                }
            }
            let accents = [
                accent_remap(mode).unwrap().mode_bus,
                accent_remap(mode).unwrap().mode_tram,
                accent_remap(mode).unwrap().mode_metro,
                accent_remap(mode).unwrap().mode_rail,
            ];
            for i in 0..accents.len() {
                for j in (i + 1)..accents.len() {
                    assert!(
                        sim_dist(accents[i], accents[j], mode) >= MIN_SIM,
                        "{mode:?} accent {i} vs {j} CVD-sim too close"
                    );
                }
            }
        }
    }

    #[test]
    fn colorblind_mode_changes_first_route_color() {
        let _guard = lock();
        reset();
        set_colorblind_mode(ColorblindMode::Off);
        let off = vivid_route_color(0);
        set_colorblind_mode(ColorblindMode::Deuteranopia);
        let deutan = vivid_route_color(0);
        assert_ne!(off, deutan);
        set_colorblind_mode(ColorblindMode::Off);
        reset();
    }
}
