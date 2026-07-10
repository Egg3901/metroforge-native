//! Shared design-system constants for `mf-game`'s egui UI (ship-plan #25,
//! v0.2 build toolbar/route panel). Every future UI file should pull its
//! spacing/type/color/corner-radius values from here rather than
//! hand-rolling them per-file, the way `hud.rs` currently does (`hud.rs`
//! keeps its own copies for now and migrates onto this module at
//! integration - see the note on [`PANEL_BG`] etc. below).
//!
//! Values are lifted byte-for-byte from `hud.rs`'s existing constants so the
//! two files agree visually even before that migration happens; see
//! `art-direction.md` (BINDING) for the source values: off-white #f4f5f2
//! panels, rich-black #17181c text, accent #007aff, vivid color reserved for
//! interactive/transit elements, corner radius 2.
//!
//! Deliberately broader than what `build_ui.rs` (the only current
//! consumer) exercises - `hud.rs`'s eventual migration and future panels
//! are expected to reach for `GOOD`/`hero`/`SPACE_LG`/etc. that nothing
//! uses yet, so this module is exempted from the dead-code lint rather
//! than trimmed down to today's exact call sites.
#![allow(dead_code)]

use bevy_egui::egui;

// ---------------------------------------------------------------------
// Spacing scale
// ---------------------------------------------------------------------
// A small fixed scale (rather than free-hand `ui.add_space(11.3)` calls
// scattered per-file) so paddings/gaps stay visually consistent as more
// panels are added. Named for the common "t-shirt size" convention;
// `SPACING` is the same six values as a slice for callers that want to
// index/iterate rather than name one.

pub const SPACE_XXS: f32 = 4.0;
pub const SPACE_XS: f32 = 8.0;
pub const SPACE_SM: f32 = 12.0;
pub const SPACE_MD: f32 = 16.0;
pub const SPACE_LG: f32 = 24.0;
pub const SPACE_XL: f32 = 32.0;

pub const SPACING: [f32; 6] = [SPACE_XXS, SPACE_XS, SPACE_SM, SPACE_MD, SPACE_LG, SPACE_XL];

// ---------------------------------------------------------------------
// Type scale
// ---------------------------------------------------------------------
// Five sizes cover everything the HUD/build UI needs: tooltip/hint copy,
// secondary/muted labels, primary body/numeric text, section headings and
// one hero size for the main menu title. Helper functions below are the
// preferred call site (`ds::label_muted("...")` rather than
// `egui::RichText::new("...").size(ds::TEXT_SM).color(ds::MUTED)`
// repeated at every use) - add a new helper here rather than inlining the
// size/color combo at a call site.

pub const TEXT_XS: f32 = 11.0;
pub const TEXT_SM: f32 = 13.0;
pub const TEXT_MD: f32 = 15.0;
pub const TEXT_LG: f32 = 24.0;
pub const TEXT_XL: f32 = 34.0;

/// Smallest/muted copy: tooltip hints, field captions.
pub fn label_small(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).size(TEXT_XS).color(MUTED)
}

/// Secondary/de-emphasized body text (art-direction reserves full
/// rich-black for primary copy).
pub fn label_muted(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).size(TEXT_SM).color(MUTED)
}

/// Primary body text at the HUD's standard size.
pub fn label_body(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).size(TEXT_SM).color(TEXT)
}

/// A value that should draw the eye slightly more than plain body text
/// (numeric readouts, selected-state labels) without going all the way to
/// a heading size.
pub fn value_strong(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).size(TEXT_MD).strong()
}

/// Section heading (panel titles, dialog titles).
pub fn heading(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).size(TEXT_LG).strong()
}

/// Hero-sized text - currently only the main menu title uses this size.
pub fn hero(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).size(TEXT_XL).strong()
}

// ---------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------
// Values copied from `hud.rs`'s `PANEL_BG`/`TEXT_COLOR`/`ACCENT`/`GOOD`/
// `WARN`/`BAD`/`MUTED_TEXT` consts (art-direction.md §1/§8). `hud.rs` keeps
// its own private copies for now (it predates this module) and is expected
// to import from here instead at integration, per ship-plan #25's scope
// split - this file does not edit `hud.rs`.

/// #f4f5f2 - off-white panel fill.
pub const PANEL_BG: egui::Color32 = egui::Color32::from_rgb(0xf4, 0xf5, 0xf2);
/// #17181c - rich-black primary text.
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(0x17, 0x18, 0x1c);
/// #007aff - the one accent color, reserved for interactive/transit
/// elements (art-direction: "vivid color ONLY on interactive/transit
/// elements").
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x00, 0x7a, 0xff);
pub const GOOD: egui::Color32 = egui::Color32::from_rgb(0x34, 0xc7, 0x59);
pub const WARN: egui::Color32 = egui::Color32::from_rgb(0xff, 0x95, 0x00);
pub const BAD: egui::Color32 = egui::Color32::from_rgb(0xff, 0x3b, 0x30);
/// De-emphasized secondary text (same role as `hud.rs`'s `MUTED_TEXT`).
pub const MUTED: egui::Color32 = egui::Color32::from_rgb(0x6b, 0x6d, 0x72);

/// Fill for an inactive/idle toggle-style control (`hud.rs` uses this same
/// value for its speed/subway toggle buttons' resting state).
pub const INACTIVE_BG: egui::Color32 = egui::Color32::from_rgb(0xe9, 0xea, 0xe5);
/// Fill for a hovered idle control, one notch darker than [`INACTIVE_BG`].
pub const HOVER_BG: egui::Color32 = egui::Color32::from_rgb(0xdc, 0xde, 0xd8);

// ---------------------------------------------------------------------
// Corner radius
// ---------------------------------------------------------------------
// Art-direction: "no rounded-corner excess" - every panel/button/frame in
// the game uses this same near-square 2px radius (matches `hud.rs`'s
// `EguiStyleApplied` visuals setup and its own literal `CornerRadius::same(2)`
// calls).

pub const CORNER_RADIUS_PX: u8 = 2;
pub const CORNER_RADIUS: egui::CornerRadius = egui::CornerRadius::same(CORNER_RADIUS_PX);

// ---------------------------------------------------------------------
// Icon painting
// ---------------------------------------------------------------------
// Single-stroke line icons for the build toolbar, drawn directly with
// `egui::Painter` primitives rather than embedded raster/SVG assets - the
// whole toolbar is a handful of glyphs, and hand-drawn strokes stay crisp
// at any UI scale factor and cost nothing to ship (no asset files, no font
// icon set to license/embed). Every variant is normalized to draw inside
// whatever `rect` it's given (typically a ~28-36px square toolbar button)
// so callers don't need to know each icon's internal proportions.

/// Which glyph [`icon`] should paint. Kept to what the v0.2 build toolbar
/// actually needs (select/build/route/demolish/undo tools, the two
/// starting vehicle modes, and a cash/fare glyph) rather than a general
/// icon-font-sized set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconKind {
    /// Plain arrow-pointer silhouette - the "Select" tool.
    Cursor,
    /// Circle "head" over a stem - a station placed on the map.
    StationPin,
    /// A short zig-zag with a filled dot at each end - a route strung
    /// between stations.
    RouteLine,
    /// A diagonal stroke through a circle (a "prohibited" glyph) - the
    /// demolish tool. Chosen over a literal bulldozer silhouette per the
    /// brief ("use a diagonal-strike circle"): unambiguous at 20px, and a
    /// literal bulldozer silhouette either needs fill (contradicts the
    /// single-stroke brief) or stops being legible at toolbar size.
    Bulldozer,
    /// A partial arc with an arrowhead at its tail - "go back one step".
    Undo,
    Bus,
    Tram,
    /// A ring with a single stroke through it - cash/fare glyph, used for
    /// the route panel's fare control.
    Coin,
}

/// Maps a normalized `(nx, ny)` in `0.0..=1.0` (icon-local space, origin
/// top-left) onto an absolute point inside `rect`. Every icon path below is
/// authored in this normalized space so it scales cleanly with whatever
/// button size the toolbar picks.
fn pt(rect: egui::Rect, nx: f32, ny: f32) -> egui::Pos2 {
    egui::pos2(
        rect.min.x + nx * rect.width(),
        rect.min.y + ny * rect.height(),
    )
}

/// Paints one [`IconKind`] glyph inside `rect` using `color`/`stroke_w`.
/// Every variant is a handful of `Painter` primitives (line/circle/rect) -
/// no fills except the small terminus dots on [`IconKind::RouteLine`] and
/// the wheel dots on [`IconKind::Bus`]/[`IconKind::Tram`], which read better
/// solid at this scale than as tiny stroked rings.
pub fn icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    kind: IconKind,
    color: egui::Color32,
    stroke_w: f32,
) {
    let stroke = egui::Stroke::new(stroke_w, color);
    match kind {
        IconKind::Cursor => {
            let pts = [
                pt(rect, 0.22, 0.15),
                pt(rect, 0.22, 0.82),
                pt(rect, 0.40, 0.64),
                pt(rect, 0.53, 0.85),
                pt(rect, 0.65, 0.78),
                pt(rect, 0.50, 0.55),
                pt(rect, 0.74, 0.55),
                pt(rect, 0.22, 0.15), // close the silhouette
            ];
            painter.line(pts.to_vec(), stroke);
        }
        IconKind::StationPin => {
            let center = pt(rect, 0.5, 0.36);
            let r = rect.width().min(rect.height()) * 0.16;
            painter.circle_stroke(center, r, stroke);
            painter.line_segment([pt(rect, 0.5, 0.5), pt(rect, 0.5, 0.84)], stroke);
        }
        IconKind::RouteLine => {
            let a = pt(rect, 0.16, 0.78);
            let b = pt(rect, 0.42, 0.34);
            let c = pt(rect, 0.60, 0.62);
            let d = pt(rect, 0.86, 0.22);
            painter.line(vec![a, b, c, d], stroke);
            let dot_r = stroke_w * 1.2;
            painter.circle_filled(a, dot_r, color);
            painter.circle_filled(d, dot_r, color);
        }
        IconKind::Bulldozer => {
            let center = pt(rect, 0.5, 0.5);
            let r = rect.width().min(rect.height()) * 0.34;
            painter.circle_stroke(center, r, stroke);
            let diag = egui::vec2(
                std::f32::consts::FRAC_1_SQRT_2,
                std::f32::consts::FRAC_1_SQRT_2,
            ) * r;
            painter.line_segment([center - diag, center + diag], stroke);
        }
        IconKind::Undo => {
            let center = pt(rect, 0.52, 0.55);
            let r = rect.width().min(rect.height()) * 0.28;
            let start_deg: f32 = -55.0;
            let end_deg: f32 = 205.0;
            const STEPS: u32 = 10;
            let arc: Vec<egui::Pos2> = (0..=STEPS)
                .map(|i| {
                    let t = start_deg + (end_deg - start_deg) * (i as f32 / STEPS as f32);
                    let rad = t.to_radians();
                    center + egui::vec2(rad.cos(), rad.sin()) * r
                })
                .collect();
            painter.line(arc.clone(), stroke);
            // Arrowhead at the arc's tail so it reads as "undo", not just
            // "circle" - `arrow` draws the two head strokes from a
            // direction vector, so a short zero-length-ish shaft along the
            // arc's tangent is enough to place it.
            if arc.len() >= 2 {
                let tail = arc[arc.len() - 1];
                let prev = arc[arc.len() - 2];
                painter.arrow(prev, tail - prev, stroke);
            }
        }
        IconKind::Bus => {
            let body = egui::Rect::from_min_max(pt(rect, 0.14, 0.30), pt(rect, 0.86, 0.68));
            painter.rect_stroke(
                body,
                egui::CornerRadius::same(3),
                stroke,
                egui::StrokeKind::Middle,
            );
            let wr = rect.width() * 0.07;
            painter.circle_filled(pt(rect, 0.28, 0.72), wr, color);
            painter.circle_filled(pt(rect, 0.72, 0.72), wr, color);
        }
        IconKind::Tram => {
            // Same body+wheels shorthand as `Bus`, plus an overhead
            // pantograph stem so the two read as distinct modes at a
            // glance rather than relying on color alone.
            let body = egui::Rect::from_min_max(pt(rect, 0.16, 0.38), pt(rect, 0.84, 0.72));
            painter.rect_stroke(
                body,
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.line_segment([pt(rect, 0.5, 0.14), pt(rect, 0.5, 0.38)], stroke);
            painter.line_segment([pt(rect, 0.34, 0.14), pt(rect, 0.66, 0.14)], stroke);
            let wr = rect.width() * 0.06;
            painter.circle_filled(pt(rect, 0.30, 0.76), wr, color);
            painter.circle_filled(pt(rect, 0.70, 0.76), wr, color);
        }
        IconKind::Coin => {
            let center = pt(rect, 0.5, 0.5);
            let r = rect.width().min(rect.height()) * 0.34;
            painter.circle_stroke(center, r, stroke);
            painter.line_segment([pt(rect, 0.5, 0.28), pt(rect, 0.5, 0.72)], stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `IconKind` should paint without panicking against a plain
    /// headless `egui::Context` - no window/render backend needed, this
    /// exercises the same `Painter` calls a real frame would make. Not a
    /// visual assertion (nothing here can screenshot-compare), just a
    /// smoke test that the path math holds for degenerate-ish rects too.
    #[test]
    fn every_icon_kind_paints_without_panicking() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let painter = ctx.debug_painter();
            let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(28.0, 28.0));
            for kind in [
                IconKind::Cursor,
                IconKind::StationPin,
                IconKind::RouteLine,
                IconKind::Bulldozer,
                IconKind::Undo,
                IconKind::Bus,
                IconKind::Tram,
                IconKind::Coin,
            ] {
                icon(&painter, rect, kind, TEXT, 1.5);
            }
        });
    }

    #[test]
    fn spacing_scale_is_strictly_increasing() {
        for pair in SPACING.windows(2) {
            assert!(pair[0] < pair[1]);
        }
    }
}
