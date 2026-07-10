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

// ---------------------------------------------------------------------
// Route line-diagram widget
// ---------------------------------------------------------------------
// A horizontal metro-map strip (ship-plan #25, v0.3 map mode wave): the
// route panel's per-route editor draws one of these under the vehicle/fare
// controls so the player sees the line's stations and where load is
// concentrated at a glance, without opening the 3D scene. Painter-drawn
// (no egui `Frame`/`Layout` children) for the same reason `icon()` above
// is: precise point math at a fixed strip height, no font-derived layout
// surprises.

/// Fixed strip height every `route_line_diagram` call allocates, regardless
/// of station count or whether labels are shown.
const ROUTE_DIAGRAM_HEIGHT: f32 = 48.0;
/// Stroke thickness range a segment's normalized load maps onto - the
/// mission's "2px..8px" so a crowded segment reads visibly fatter than an
/// empty one without needing a legend to interpret.
const ROUTE_DIAGRAM_MIN_THICKNESS: f32 = 2.0;
const ROUTE_DIAGRAM_MAX_THICKNESS: f32 = 8.0;
/// Above this many stations, per-station text labels stop fitting (they'd
/// overlap into illegible mush), so the diagram switches to unlabeled ticks
/// plus a single "N stops" caption instead.
const ROUTE_DIAGRAM_LABEL_CAP: usize = 12;
/// Horizontal inset from each edge of the strip so the first/last station
/// dot isn't clipped by the panel border.
const ROUTE_DIAGRAM_H_MARGIN: f32 = 10.0;
const ROUTE_DIAGRAM_DOT_RADIUS: f32 = 4.0;

/// Evenly-spaced tick x-offsets (measured from the strip's left edge, NOT
/// absolute screen space) for `station_count` stations across a strip of
/// `width` px inset by `margin` on each side. Pure function (no
/// `egui::Painter`/`Ui`) so the point math is unit-testable without a
/// headless `egui::Context`.
///
/// Degenerate guards: 0 stations returns no ticks; exactly 1 returns a
/// single centered tick rather than dividing by a zero station-gap count.
fn tick_offsets(width: f32, margin: f32, station_count: usize) -> Vec<f32> {
    if station_count == 0 {
        return Vec::new();
    }
    let usable = (width - margin * 2.0).max(0.0);
    if station_count == 1 {
        return vec![margin + usable * 0.5];
    }
    let step = usable / (station_count as f32 - 1.0);
    (0..station_count)
        .map(|i| margin + step * i as f32)
        .collect()
}

/// Maps each entry of `segment_loads` onto a stroke thickness in
/// `ROUTE_DIAGRAM_MIN_THICKNESS..=ROUTE_DIAGRAM_MAX_THICKNESS`, normalized
/// to the busiest segment on the route (`load / max_load`) so the fattest
/// line always marks the route's own worst crowding, independent of the
/// route's absolute ridership scale.
///
/// Degenerate guards: an empty slice returns no thicknesses (nothing to
/// draw a line between); an all-zero (or otherwise non-positive) max load
/// returns the minimum thickness for every segment rather than dividing by
/// zero or implying crowding that isn't there.
fn segment_thicknesses(segment_loads: &[f64]) -> Vec<f32> {
    if segment_loads.is_empty() {
        return Vec::new();
    }
    let max_load = segment_loads.iter().cloned().fold(0.0_f64, f64::max);
    if max_load <= 0.0 {
        return vec![ROUTE_DIAGRAM_MIN_THICKNESS; segment_loads.len()];
    }
    segment_loads
        .iter()
        .map(|&load| {
            let t = (load / max_load).clamp(0.0, 1.0) as f32;
            ROUTE_DIAGRAM_MIN_THICKNESS + t * (ROUTE_DIAGRAM_MAX_THICKNESS - ROUTE_DIAGRAM_MIN_THICKNESS)
// Sparkline (ship-plan #25, v0.3 finance panel)
// ---------------------------------------------------------------------
// A small inline line chart for a rolling numeric series - the finance
// panel's 7-day net history is the first caller, but the point-mapping
// math has nothing panel-specific in it, so it lives here rather than in
// `panels.rs` per this module's "reusable helper" bar.

/// Pure point-mapping for [`sparkline`]: places `values` left-to-right
/// across `rect` (oldest first) and top-to-bottom scaled to the series'
/// own min/max, so a single trace always uses the full height available
/// regardless of its absolute magnitude. Split out from `sparkline` itself
/// so the mapping math is unit-testable directly against plain `Rect`/
/// `Pos2` values, with no `egui::Ui`/`Context` (i.e. no painted frame)
/// required.
pub fn sparkline_points(values: &[f64], rect: egui::Rect) -> Vec<egui::Pos2> {
    if values.is_empty() {
        return Vec::new();
    }
    if values.len() == 1 {
        // Nothing to draw a line between; a single point in the middle
        // reads better than an arbitrary corner.
        return vec![rect.center()];
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = max - min;
    // A perfectly flat series (min == max, span == 0) has no range to scale
    // against; every point maps to the vertical center rather than dividing
    // by zero (or, worse, silently flooring to a near-zero span and
    // collapsing the whole trace onto the BOTTOM edge instead of drawing
    // the flat line the data actually represents).
    let flat = span.abs() < 1e-9;
    let last_idx = (values.len() - 1) as f32;
    values
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let t = i as f32 / last_idx;
            let x = rect.min.x + t * rect.width();
            let y = if flat {
                rect.center().y
            } else {
                let norm = ((v - min) / span) as f32; // 0.0 at min, 1.0 at max
                rect.max.y - norm * rect.height() // egui y grows downward
            };
            egui::pos2(x, y)
        })
        .collect()
}

/// Horizontal metro-map line diagram: a colored strip with one tick-dot per
/// station (evenly spaced) and, between consecutive ticks, a stroke whose
/// thickness reads the corresponding entry of `segment_loads` (fat = busy).
/// Height is fixed at [`ROUTE_DIAGRAM_HEIGHT`]; width fills whatever's
/// available in `ui` (`ui.available_width()`), same convention as egui's
/// other full-bleed widgets.
///
/// Station count above [`ROUTE_DIAGRAM_LABEL_CAP`] switches to unlabeled
/// ticks plus a trailing "N stops" caption - past that count, per-station
/// text labels would overlap into illegible mush at any reasonable panel
/// width.
///
/// `segment_loads` is expected to carry one entry per consecutive station
/// pair (`station_labels.len() - 1` entries); a mismatched length (stale
/// data, an off-by-one from the caller) falls back to the minimum
/// thickness for every segment rather than indexing out of bounds or
/// misattributing a load to the wrong segment.
///
/// Degenerate guards: 0 stations draws nothing (still allocates the fixed
/// height so the panel layout doesn't jump); 1 station draws a single dot
/// and no line; empty/all-zero `segment_loads` is handled by
/// [`segment_thicknesses`] above.
pub fn route_line_diagram(
    ui: &mut egui::Ui,
    color: egui::Color32,
    station_labels: &[String],
    segment_loads: &[f64],
) {
    let station_count = station_labels.len();
    let desired_size = egui::vec2(ui.available_width(), ROUTE_DIAGRAM_HEIGHT);
    let (rect, _response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    if station_count == 0 {
        return;
    }

    let painter = ui.painter_at(rect);
    let show_labels = station_count <= ROUTE_DIAGRAM_LABEL_CAP;
    // Line sits higher when labels are shown (room for text underneath);
    // dead center otherwise.
    let line_y = rect.top() + ROUTE_DIAGRAM_HEIGHT * if show_labels { 0.35 } else { 0.5 };
    let offsets = tick_offsets(rect.width(), ROUTE_DIAGRAM_H_MARGIN, station_count);

    if station_count > 1 {
        let thicknesses = segment_thicknesses(segment_loads);
        let aligned = thicknesses.len() == station_count - 1;
        for i in 0..station_count - 1 {
            let thickness = if aligned {
                thicknesses[i]
            } else {
                ROUTE_DIAGRAM_MIN_THICKNESS
            };
            let a = egui::pos2(rect.left() + offsets[i], line_y);
            let b = egui::pos2(rect.left() + offsets[i + 1], line_y);
            painter.line_segment([a, b], egui::Stroke::new(thickness, color));
        }
    }

    for (i, &off) in offsets.iter().enumerate() {
        let center = egui::pos2(rect.left() + off, line_y);
        painter.circle_filled(center, ROUTE_DIAGRAM_DOT_RADIUS, color);
        if show_labels {
            if let Some(label) = station_labels.get(i) {
                painter.text(
                    egui::pos2(center.x, rect.bottom() - 2.0),
                    egui::Align2::CENTER_BOTTOM,
                    label,
                    egui::FontId::proportional(TEXT_XS),
                    TEXT,
                );
            }
        }
    }

    if !show_labels {
        painter.text(
            egui::pos2(rect.left(), rect.bottom() - 2.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{station_count} stops"),
            egui::FontId::proportional(TEXT_XS),
            MUTED,
        );
    }
/// Paints `values` as a small polyline inside a `size`d rect the function
/// allocates itself (`Sense::hover()`: nothing here is interactive). Colored
/// by the sign of the MOST RECENT value (`values.last()`) rather than
/// per-segment - the question a player wants answered at a glance is "am I
/// in the red or the black right now," not a multi-color rainbow of every
/// individual day's sign. Draws a single dot (same sign rule) for a
/// one-value series, and just the empty background for a zero-value one.
pub fn sparkline(ui: &mut egui::Ui, values: &[f64], size: egui::Vec2) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CORNER_RADIUS, INACTIVE_BG);
    let last_is_good = values.last().is_none_or(|v| *v >= 0.0);
    let color = if last_is_good { GOOD } else { BAD };
    // A little inset so the trace doesn't touch the background's own edge.
    let inset = rect.shrink(3.0);
    let points = sparkline_points(values, inset);
    match points.as_slice() {
        [] => {}
        [only] => {
            painter.circle_filled(*only, 2.0, color);
        }
        _ => {
            painter.line(points, egui::Stroke::new(1.5, color));
        }
    }
    response
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

    // --- route_line_diagram: tick_offsets --------------------------------

    #[test]
    fn tick_offsets_zero_stations_is_empty() {
        assert!(tick_offsets(300.0, 10.0, 0).is_empty());
    }

    #[test]
    fn tick_offsets_one_station_is_centered() {
        let offsets = tick_offsets(300.0, 10.0, 1);
        assert_eq!(offsets.len(), 1);
        // usable = 300 - 20 = 280; centered = margin + 140 = 150.
        assert!((offsets[0] - 150.0).abs() < 0.001);
    }

    #[test]
    fn tick_offsets_span_the_full_usable_width_evenly() {
        let offsets = tick_offsets(210.0, 10.0, 4);
        assert_eq!(offsets.len(), 4);
        // First tick at the left margin, last at width - margin, evenly
        // stepped in between.
        assert!((offsets[0] - 10.0).abs() < 0.001);
        assert!((offsets[3] - 200.0).abs() < 0.001);
        let step = offsets[1] - offsets[0];
        for pair in offsets.windows(2) {
            assert!((pair[1] - pair[0] - step).abs() < 0.001);
    // --- sparkline_points --------------------------------------------------

    #[test]
    fn sparkline_points_empty_is_empty() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 40.0));
        assert!(sparkline_points(&[], rect).is_empty());
    }

    #[test]
    fn sparkline_points_single_value_is_the_center() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 40.0));
        let pts = sparkline_points(&[42.0], rect);
        assert_eq!(pts, vec![rect.center()]);
    }

    #[test]
    fn sparkline_points_spans_the_full_width_oldest_to_newest() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 0.0), egui::vec2(100.0, 40.0));
        let pts = sparkline_points(&[0.0, 1.0, 2.0], rect);
        assert_eq!(pts.len(), 3);
        assert!(
            (pts[0].x - rect.min.x).abs() < 0.001,
            "first point at left edge"
        );
        assert!(
            (pts[2].x - rect.max.x).abs() < 0.001,
            "last point at right edge"
        );
        assert!(
            (pts[1].x - rect.center().x).abs() < 0.001,
            "middle point centered"
        );
    }

    #[test]
    fn sparkline_points_min_value_touches_the_bottom_max_touches_the_top() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 40.0));
        let pts = sparkline_points(&[-5.0, 5.0], rect);
        // y grows downward in egui: the min value (index 0, -5.0) should sit
        // at the bottom (max.y), the max value (index 1, 5.0) at the top.
        assert!((pts[0].y - rect.max.y).abs() < 0.001, "min value at bottom");
        assert!((pts[1].y - rect.min.y).abs() < 0.001, "max value at top");
    }

    #[test]
    fn sparkline_points_flat_series_draws_a_flat_line_down_the_middle() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 40.0));
        let pts = sparkline_points(&[3.0, 3.0, 3.0], rect);
        for p in pts {
            assert!((p.y - rect.center().y).abs() < 0.001, "got {p:?}");
        }
    }

    #[test]
    fn tick_offsets_degenerate_width_does_not_go_negative_or_panic() {
        // Width narrower than 2x margin: usable clamps to 0 rather than
        // going negative and flipping the tick order.
        let offsets = tick_offsets(5.0, 10.0, 3);
        assert_eq!(offsets.len(), 3);
        for &o in &offsets {
            assert!(o.is_finite());
        }
    }

    // --- route_line_diagram: segment_thicknesses -------------------------

    #[test]
    fn segment_thicknesses_empty_loads_is_empty() {
        assert!(segment_thicknesses(&[]).is_empty());
    }

    #[test]
    fn segment_thicknesses_all_zero_falls_back_to_minimum() {
        let out = segment_thicknesses(&[0.0, 0.0, 0.0]);
        assert_eq!(out, vec![ROUTE_DIAGRAM_MIN_THICKNESS; 3]);
    }

    #[test]
    fn segment_thicknesses_normalizes_to_the_busiest_segment() {
        let out = segment_thicknesses(&[0.0, 50.0, 100.0]);
        assert_eq!(out.len(), 3);
        // Zero-load segment sits at the floor, the max-load segment at the
        // ceiling, and the halfway one lands exactly between.
        assert!((out[0] - ROUTE_DIAGRAM_MIN_THICKNESS).abs() < 0.001);
        assert!((out[2] - ROUTE_DIAGRAM_MAX_THICKNESS).abs() < 0.001);
        let mid = (ROUTE_DIAGRAM_MIN_THICKNESS + ROUTE_DIAGRAM_MAX_THICKNESS) / 2.0;
        assert!((out[1] - mid).abs() < 0.001);
    }

    #[test]
    fn segment_thicknesses_never_exceeds_the_declared_range() {
        let out = segment_thicknesses(&[3.0, 1.0, 9_000.0, 0.5]);
        for t in out {
            assert!((ROUTE_DIAGRAM_MIN_THICKNESS..=ROUTE_DIAGRAM_MAX_THICKNESS).contains(&t));
        }
    }

    // --- route_line_diagram: full paint (smoke + degenerate guards) ------

    /// `route_line_diagram` should paint without panicking for every
    /// degenerate case the mission calls out (0/1 station, empty loads,
    /// all-zero loads, a mismatched-length loads slice, and the >12
    /// station "N stops" caption path) against a plain headless
    /// `egui::Context` - same smoke-test shape as `every_icon_kind_paints_
    /// without_panicking` above.
    #[test]
    fn route_line_diagram_paints_without_panicking_for_every_degenerate_case() {
        let ctx = egui::Context::default();
        let cases: Vec<(Vec<String>, Vec<f64>)> = vec![
            (vec![], vec![]),
            (vec!["Only Stop".to_string()], vec![]),
            (
                vec!["A".to_string(), "B".to_string(), "C".to_string()],
                vec![],
            ),
            (
                vec!["A".to_string(), "B".to_string(), "C".to_string()],
                vec![0.0, 0.0],
            ),
            (
                vec!["A".to_string(), "B".to_string(), "C".to_string()],
                vec![1.0], // mismatched length vs. 2 expected segments
            ),
            (
                (0..15).map(|i| format!("S{i}")).collect(),
                (0..14).map(|i| i as f64).collect(),
            ),
        ];
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for (labels, loads) in &cases {
                    route_line_diagram(ui, ACCENT, labels, loads);
    fn sparkline_paints_without_panicking_for_various_series() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for values in [
                    vec![],
                    vec![10.0],
                    vec![-5.0, 3.0, -1.0, 8.0, 8.0, -2.0, 4.0],
                ] {
                    sparkline(ui, &values, egui::vec2(120.0, 32.0));
                }
            });
        });
    }
}
