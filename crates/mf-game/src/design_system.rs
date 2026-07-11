//! MetroForge UI design system: Mirror's Edge-adjacent chrome.
//!
//! Near-black `#0b0d10`, white type, and the four spoke accents
//! (green / blue / orange / red). Every visible surface in `mf-game`
//! should go through the helpers here (`panel`, `button`, `modal`,
//! `window`, `stat_tile`, `progress_bar`, `toast`, …) so nothing paints
//! stock egui bevels or window chrome. Call sites must not construct
//! raw `egui::Button` / `egui::Window` themselves.
//!
//! Motion: 120–180 ms ease-out via [`animate`] / [`animate_bool`] wrapping
//! `ctx.animate_value_with_time` / `animate_bool_with_time`.
#![allow(dead_code)]

use bevy_egui::egui;
use mf_protocol::ToastTone;
use mf_state::Theme;
use std::sync::Arc;

// ---------------------------------------------------------------------
// Spacing scale
// ---------------------------------------------------------------------

pub const SPACE_XXS: f32 = 4.0;
pub const SPACE_XS: f32 = 8.0;
pub const SPACE_SM: f32 = 12.0;
pub const SPACE_MD: f32 = 16.0;
pub const SPACE_LG: f32 = 24.0;
pub const SPACE_XL: f32 = 32.0;
pub const SPACE_XXL: f32 = 48.0;

pub const SPACING: [f32; 7] = [
    SPACE_XXS, SPACE_XS, SPACE_SM, SPACE_MD, SPACE_LG, SPACE_XL, SPACE_XXL,
];

// ---------------------------------------------------------------------
// Type scale + font families
// ---------------------------------------------------------------------
// Body: Inter (proportional). Headings / wordmark: Oswald display face
// registered as FontFamily::Name("mf_display") by `install_fonts`.

pub const TEXT_XS: f32 = 11.0;
pub const TEXT_SM: f32 = 13.0;
pub const TEXT_MD: f32 = 15.0;
pub const TEXT_LG: f32 = 28.0;
pub const TEXT_XL: f32 = 56.0;
pub const TEXT_WORDMARK: f32 = 72.0;

/// Display face family name installed by [`install_fonts`].
pub fn display_family() -> egui::FontFamily {
    egui::FontFamily::Name(Arc::from("mf_display"))
}

pub fn display_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, display_family())
}

pub fn body_font(size: f32) -> egui::FontId {
    egui::FontId::proportional(size)
}

pub fn label_small(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .size(TEXT_XS)
        .color(muted())
}

pub fn label_muted(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .size(TEXT_SM)
        .color(muted())
}

pub fn label_body(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .size(TEXT_SM)
        .color(current_colors().text)
}

pub fn value_strong(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .size(TEXT_MD)
        .strong()
        .color(current_colors().text)
}

pub fn heading(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .font(display_font(TEXT_LG))
        .color(current_colors().text)
}

pub fn hero(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .font(display_font(TEXT_XL))
        .color(current_colors().text)
}

pub fn wordmark(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .font(display_font(TEXT_WORDMARK))
        .color(egui::Color32::WHITE)
}

// ---------------------------------------------------------------------
// Palette (Mirror's Edge / spoke identity)
// ---------------------------------------------------------------------

/// Near-black surface fill — logo badge / UI chrome base.
pub const SURFACE: egui::Color32 = egui::Color32::from_rgb(0x0b, 0x0d, 0x10);
/// Slightly lifted surface for nested panels / idle controls.
pub const SURFACE_RAISED: egui::Color32 = egui::Color32::from_rgb(0x14, 0x17, 0x1c);
/// Hover fill over raised surfaces.
pub const SURFACE_HOVER: egui::Color32 = egui::Color32::from_rgb(0x1c, 0x21, 0x28);

pub const WHITE: egui::Color32 = egui::Color32::from_rgb(0xf4, 0xf4, 0xf5);
pub const MUTED_FIXED: egui::Color32 = egui::Color32::from_rgb(0x8a, 0x8e, 0x96);

/// Four spoke accents from the wordmark / app icon.
pub const SPOKE_GREEN: egui::Color32 = egui::Color32::from_rgb(0x7e, 0xf2, 0x9a);
pub const SPOKE_BLUE: egui::Color32 = egui::Color32::from_rgb(0x54, 0xd0, 0xff);
pub const SPOKE_ORANGE: egui::Color32 = egui::Color32::from_rgb(0xff, 0xb6, 0x3d);
pub const SPOKE_RED: egui::Color32 = egui::Color32::from_rgb(0xff, 0x5d, 0x6c);

/// Semantic aliases (same spoke hues).
pub const GOOD: egui::Color32 = SPOKE_GREEN;
pub const WARN: egui::Color32 = SPOKE_ORANGE;
pub const BAD: egui::Color32 = SPOKE_RED;
pub const ACCENT: egui::Color32 = SPOKE_BLUE;

// Legacy const names kept so older call sites / tests still compile.
pub const PANEL_BG: egui::Color32 = SURFACE;
pub const TEXT: egui::Color32 = WHITE;
pub const MUTED: egui::Color32 = MUTED_FIXED;
pub const INACTIVE_BG: egui::Color32 = SURFACE_RAISED;
pub const HOVER_BG: egui::Color32 = SURFACE_HOVER;

/// Hard 2px accent edge (no rounded-gray-blob panels).
pub const ACCENT_EDGE_PX: f32 = 2.0;
pub const CORNER_RADIUS_PX: u8 = 0;
pub const CORNER_RADIUS: egui::CornerRadius = egui::CornerRadius::ZERO;

/// Open / close / hover motion window (seconds).
pub const ANIM_SECS: f32 = 0.15;

// ---------------------------------------------------------------------
// Z-order policy (salvaged from the HUD-unification pass, PR #75)
// ---------------------------------------------------------------------
// egui `Order` is a coarse stack; within a layer later-drawn areas win.
// Modals sit at `Tooltip` so a pause / settings / report card always
// paints above panels, toasts, and world-anchored tutorial hints — the
// hint and the modal used to share `Foreground`, so a hint could bleed
// over the dimmed card. Policy (low to high): hints < panels < toasts < modal.

/// World-anchored tutorial hint cards (below floating panels and toasts).
pub const ORDER_HINT: egui::Order = egui::Order::Middle;
/// Floating panels (goals / finance / station windows).
pub const ORDER_PANEL: egui::Order = egui::Order::Middle;
/// Toast strip (above panels, below modal).
pub const ORDER_TOAST: egui::Order = egui::Order::Foreground;
/// Pause / settings / report scrim + card. Nothing may render above this.
pub const ORDER_MODAL: egui::Order = egui::Order::Tooltip;

/// Semantic crowding ramp for a `0.0..1.0` live-crowding value (sim-depth,
/// PR #31): interpolates [`GOOD`] (empty) -> [`WARN`] (filling) -> [`BAD`]
/// (packed) so a route stripe/row reads its load at a glance. Values are
/// clamped, so out-of-range inputs saturate at the endpoints rather than
/// wrapping. Kept as a plain color helper (no theme lookup) since
/// `GOOD`/`WARN`/`BAD` are the fixed status colors that read on every theme.
pub fn crowding_color(crowding: f64) -> egui::Color32 {
    let t = crowding.clamp(0.0, 1.0) as f32;
    let lerp = |a: u8, b: u8, t: f32| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    let (from, to, seg_t) = if t < 0.5 {
        (GOOD, WARN, t / 0.5)
    } else {
        (WARN, BAD, (t - 0.5) / 0.5)
    };
    egui::Color32::from_rgb(
        lerp(from.r(), to.r(), seg_t),
        lerp(from.g(), to.g(), seg_t),
        lerp(from.b(), to.b(), seg_t),
    )
}

// ---------------------------------------------------------------------
// Theme-indexed chrome
// ---------------------------------------------------------------------
// UI chrome commits to the near-black ME surface on every theme; only the
// interactive accent shifts so Theme::Purple still reads as a pick.

pub struct ThemeColors {
    pub panel_bg: egui::Color32,
    pub text: egui::Color32,
    pub accent: egui::Color32,
    pub muted: egui::Color32,
    pub extreme_bg: egui::Color32,
    pub inactive_bg: egui::Color32,
    pub hover_bg: egui::Color32,
    pub border: egui::Color32,
}

pub fn theme_colors(theme: Theme) -> ThemeColors {
    let accent = match theme {
        Theme::Light => SPOKE_BLUE,
        Theme::Dark => SPOKE_BLUE,
        Theme::Purple => egui::Color32::from_rgb(0xd3, 0x9a, 0xf0),
    };
    ThemeColors {
        panel_bg: SURFACE,
        text: WHITE,
        accent,
        muted: MUTED_FIXED,
        extreme_bg: egui::Color32::from_rgb(0x05, 0x06, 0x08),
        inactive_bg: SURFACE_RAISED,
        hover_bg: SURFACE_HOVER,
        border: egui::Color32::from_rgb(0x2a, 0x2e, 0x36),
    }
}

static CURRENT_THEME: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set_current_theme(theme: Theme) {
    CURRENT_THEME.store(theme as u8, std::sync::atomic::Ordering::Relaxed);
}

fn current_theme() -> Theme {
    match CURRENT_THEME.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Theme::Dark,
        2 => Theme::Purple,
        _ => Theme::Light,
    }
}

pub fn current_colors() -> ThemeColors {
    theme_colors(current_theme())
}

pub fn panel_bg() -> egui::Color32 {
    current_colors().panel_bg
}
pub fn text() -> egui::Color32 {
    current_colors().text
}
pub fn accent() -> egui::Color32 {
    current_colors().accent
}
pub fn muted() -> egui::Color32 {
    current_colors().muted
}
pub fn inactive_bg() -> egui::Color32 {
    current_colors().inactive_bg
}
pub fn hover_bg() -> egui::Color32 {
    current_colors().hover_bg
}
pub fn border() -> egui::Color32 {
    current_colors().border
}

/// Single-layer dim (legacy callers). Prefer [`paint_scrim`] for modals.
pub fn scrim() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(0x05, 0x06, 0x08, 160)
}

/// Soft wash over the attract diorama on title / city-select.
pub fn menu_wash() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(0x0b, 0x0d, 0x10, 72)
}

pub fn hud_hidden() -> bool {
    static ONCE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| std::env::var_os("MF_HIDE_HUD").is_some())
}

pub fn ui_gallery_enabled() -> bool {
    static ONCE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| matches!(std::env::var("MF_UI_GALLERY").as_deref(), Ok("1")))
}

// ---------------------------------------------------------------------
// Fonts + egui visuals (no stock bevels)
// ---------------------------------------------------------------------

const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
const OSWALD_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Oswald-SemiBold.ttf");

/// Install Inter (body) + Oswald (display) and strip stock egui chrome.
pub fn install_fonts_and_visuals(ctx: &egui::Context, theme: Theme) {
    set_current_theme(theme);

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(INTER_REGULAR)),
    );
    fonts.font_data.insert(
        "oswald".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(OSWALD_SEMIBOLD)),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".to_owned());
    fonts
        .families
        .insert(display_family(), vec!["oswald".to_owned()]);
    ctx.set_fonts(fonts);

    let hv = theme_colors(theme);
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = hv.panel_bg;
    visuals.window_fill = hv.panel_bg;
    visuals.extreme_bg_color = hv.extreme_bg;
    visuals.faint_bg_color = hv.extreme_bg;
    visuals.override_text_color = Some(hv.text);
    visuals.window_stroke = egui::Stroke::NONE;
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.popup_shadow = egui::epaint::Shadow::NONE;
    visuals.window_corner_radius = CORNER_RADIUS;
    visuals.menu_corner_radius = CORNER_RADIUS;
    visuals.widgets.noninteractive.bg_fill = hv.panel_bg;
    visuals.widgets.noninteractive.weak_bg_fill = hv.panel_bg;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.inactive.bg_fill = hv.inactive_bg;
    visuals.widgets.inactive.weak_bg_fill = hv.inactive_bg;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.hovered.bg_fill = hv.hover_bg;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.active.bg_fill = hv.accent;
    visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
    visuals.selection.bg_fill = hv.accent;
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = CORNER_RADIUS;
        widget.expansion = 0.0;
    }
    ctx.set_visuals(visuals.clone());

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(SPACE_SM, SPACE_XS);
    style.spacing.button_padding = egui::vec2(SPACE_MD, SPACE_SM);
    style.spacing.window_margin = egui::Margin::same(0);
    style.visuals = visuals;
    ctx.set_style(style);
}

// ---------------------------------------------------------------------
// Motion helpers (ease-out cubic over ANIM_SECS)
// ---------------------------------------------------------------------

fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Animate a float toward `target` over [`ANIM_SECS`] with ease-out.
pub fn animate(ctx: &egui::Context, id: egui::Id, target: f32) -> f32 {
    let raw = ctx.animate_value_with_time(id, target, ANIM_SECS);
    // animate_value_with_time already eases; we still expose a stable API.
    raw
}

/// Animate a bool open/close; returns eased 0..1.
pub fn animate_bool(ctx: &egui::Context, id: egui::Id, open: bool) -> f32 {
    ease_out_cubic(ctx.animate_bool_with_time(id, open, ANIM_SECS))
}

/// Hover glow factor (0..1) for a response, eased.
pub fn hover_t(ctx: &egui::Context, response: &egui::Response) -> f32 {
    ease_out_cubic(ctx.animate_bool_with_time(
        response.id.with("mf_hover"),
        response.hovered(),
        ANIM_SECS,
    ))
}

// ---------------------------------------------------------------------
// Paint primitives
// ---------------------------------------------------------------------

fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
    egui::Color32::from_rgba_unmultiplied(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
        lerp(a.a(), b.a()),
    )
}

fn with_alpha(c: egui::Color32, a: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Flat fill + 2px accent underline + optional hover glow. No bevels.
pub fn paint_surface(
    painter: &egui::Painter,
    rect: egui::Rect,
    fill: egui::Color32,
    accent_color: egui::Color32,
    hover: f32,
) {
    let glow = with_alpha(accent_color, (28.0 * hover) as u8);
    if hover > 0.01 {
        painter.rect_filled(rect.expand(2.0), CORNER_RADIUS, glow);
    }
    painter.rect_filled(rect, CORNER_RADIUS, fill);
    let edge = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.bottom() - ACCENT_EDGE_PX),
        rect.max,
    );
    painter.rect_filled(edge, CORNER_RADIUS, accent_color);
}

/// Two-layer scrim: deep wash + lighter inner veil (blurred-feel stand-in).
pub fn paint_scrim(painter: &egui::Painter, screen: egui::Rect) {
    painter.rect_filled(
        screen,
        CORNER_RADIUS,
        egui::Color32::from_rgba_unmultiplied(0x05, 0x06, 0x08, 180),
    );
    painter.rect_filled(
        screen.shrink(0.0),
        CORNER_RADIUS,
        egui::Color32::from_rgba_unmultiplied(0x0b, 0x0d, 0x10, 90),
    );
}

/// Panel frame: flat near-black fill, 2px left accent edge, generous pad.
pub fn panel_frame(accent_color: egui::Color32) -> egui::Frame {
    egui::Frame::NONE
        .fill(panel_bg())
        .inner_margin(egui::Margin::symmetric(SPACE_LG as i8, SPACE_MD as i8))
        .stroke(egui::Stroke::new(ACCENT_EDGE_PX, accent_color))
}

/// Content panel helper: allocates a framed region and runs `add_contents`.
pub fn panel<R>(
    ui: &mut egui::Ui,
    accent_color: egui::Color32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let open_t = animate(ui.ctx(), ui.id().with("mf_panel_open"), 1.0);
    panel_frame(accent_color).show(ui, |ui| {
        ui.set_opacity(open_t.clamp(0.15, 1.0));
        add_contents(ui)
    })
}

// ---------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Ghost,
    Danger,
    /// Toggle / selected chrome (speed, subway, routes, …).
    Toggle(bool),
}

/// Custom-painted button. Primary / ghost / danger / toggle.
pub fn button(ui: &mut egui::Ui, label: impl Into<String>, kind: ButtonKind) -> egui::Response {
    button_sized(ui, label, kind, None)
}

pub fn button_sized(
    ui: &mut egui::Ui,
    label: impl Into<String>,
    kind: ButtonKind,
    size: Option<egui::Vec2>,
) -> egui::Response {
    let label = label.into();
    let padding = egui::vec2(SPACE_MD, SPACE_SM);
    let font = body_font(TEXT_SM);
    let galley = ui.fonts(|f| f.layout_no_wrap(label.clone(), font.clone(), text()));
    let desired = size.unwrap_or_else(|| {
        egui::vec2(
            (galley.size().x + padding.x * 2.0).max(120.0),
            (galley.size().y + padding.y * 2.0).max(36.0),
        )
    });
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());
    let h = hover_t(ui.ctx(), &response);
    let (fill, text_color, edge) = match kind {
        ButtonKind::Primary => {
            let base = accent();
            let fill = lerp_color(base, WHITE, 0.08 * h);
            (fill, SURFACE, base)
        }
        ButtonKind::Ghost => {
            let fill = lerp_color(inactive_bg(), hover_bg(), h);
            (fill, text(), accent())
        }
        ButtonKind::Danger => {
            let fill = lerp_color(BAD, WHITE, 0.08 * h);
            (fill, WHITE, BAD)
        }
        ButtonKind::Toggle(on) => {
            if on {
                (lerp_color(accent(), WHITE, 0.06 * h), SURFACE, accent())
            } else {
                (lerp_color(inactive_bg(), hover_bg(), h), text(), border())
            }
        }
    };
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        paint_surface(painter, rect, fill, edge, h);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            font,
            text_color,
        );
    }
    response
}

/// Borderless title-menu text button with a left accent bar on hover.
pub fn menu_text_button(ui: &mut egui::Ui, label: impl Into<String>) -> egui::Response {
    let label = label.into();
    let font = display_font(22.0);
    let galley = ui.fonts(|f| f.layout_no_wrap(label.clone(), font.clone(), text()));
    let height = (galley.size().y + SPACE_SM).max(40.0);
    let width = ui.available_width().max(galley.size().x + SPACE_LG);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let h = hover_t(ui.ctx(), &response);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if h > 0.01 {
            painter.rect_filled(
                rect,
                CORNER_RADIUS,
                with_alpha(accent(), (18.0 + 40.0 * h) as u8),
            );
            let bar = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top() + 6.0),
                egui::pos2(rect.left() + ACCENT_EDGE_PX, rect.bottom() - 6.0),
            );
            painter.rect_filled(bar, CORNER_RADIUS, with_alpha(accent(), (255.0 * h) as u8));
        }
        let text_pos = egui::pos2(rect.left() + SPACE_MD, rect.center().y);
        painter.text(
            text_pos,
            egui::Align2::LEFT_CENTER,
            label,
            font,
            lerp_color(muted(), WHITE, 0.35 + 0.65 * h),
        );
    }
    response
}

// ---------------------------------------------------------------------
// Stat tile / progress / toast
// ---------------------------------------------------------------------

/// Compact label-over-value tile for HUD / finance readouts.
pub fn stat_tile(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
    let value = value.into();
    let width = ui.available_width().max(96.0);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(width.min(160.0), 52.0), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    paint_surface(painter, rect, inactive_bg(), accent(), 0.0);
    painter.text(
        egui::pos2(rect.left() + SPACE_SM, rect.top() + SPACE_XS),
        egui::Align2::LEFT_TOP,
        label,
        body_font(TEXT_XS),
        muted(),
    );
    painter.text(
        egui::pos2(rect.left() + SPACE_SM, rect.bottom() - SPACE_XS),
        egui::Align2::LEFT_BOTTOM,
        value,
        body_font(TEXT_MD),
        text(),
    );
}

/// Flat progress bar with accent fill (no egui ProgressBar chrome).
pub fn progress_bar(ui: &mut egui::Ui, frac: f32, width: f32) -> egui::Response {
    let height = 8.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, CORNER_RADIUS, inactive_bg());
        let filled = rect.with_max_x(rect.left() + rect.width() * frac.clamp(0.0, 1.0));
        painter.rect_filled(filled, CORNER_RADIUS, accent());
    }
    response
}

/// Single toast chip (tone-colored left edge).
pub fn toast(ui: &mut egui::Ui, message: &str, tone: ToastTone) {
    let edge = match tone {
        ToastTone::Info => accent(),
        ToastTone::Warn => WARN,
        ToastTone::Good => GOOD,
    };
    let font = body_font(TEXT_SM);
    let galley = ui.fonts(|f| f.layout_no_wrap(message.to_owned(), font.clone(), text()));
    let size = egui::vec2(
        (galley.size().x + SPACE_MD * 2.0 + ACCENT_EDGE_PX).min(ui.available_width()),
        galley.size().y + SPACE_XS,
    );
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    painter.rect_filled(rect, CORNER_RADIUS, inactive_bg());
    let bar = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.left() + ACCENT_EDGE_PX, rect.bottom()),
    );
    painter.rect_filled(bar, CORNER_RADIUS, edge);
    painter.text(
        egui::pos2(rect.left() + SPACE_SM + ACCENT_EDGE_PX, rect.center().y),
        egui::Align2::LEFT_CENTER,
        message,
        font,
        text(),
    );
}

// ---------------------------------------------------------------------
// Modal + floating window (only place that may call egui::Window)
// ---------------------------------------------------------------------

/// Centered modal over a two-layer scrim. `open_t` is an eased 0..1.
pub fn modal<R>(
    ctx: &egui::Context,
    id: egui::Id,
    open_t: f32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    if open_t <= 0.001 {
        return None;
    }

    egui::Area::new(id.with("scrim"))
        .order(ORDER_MODAL)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let screen = ui.ctx().screen_rect();
            ui.allocate_response(screen.size(), egui::Sense::click());
            let mut painter = ui.painter().clone();
            painter.set_opacity(open_t);
            paint_scrim(&painter, screen);
        });

    let inner = egui::Area::new(id.with("panel"))
        .order(ORDER_MODAL)
        .anchor(
            egui::Align2::CENTER_CENTER,
            egui::vec2(0.0, (1.0 - open_t) * 12.0),
        )
        .show(ctx, |ui| {
            ui.set_opacity(open_t);
            panel_frame(accent()).show(ui, add_contents).inner
        });
    Some(inner.inner)
}

/// Options for [`window`] — the sole wrapper around `egui::Window`.
pub struct WindowOpts<'a> {
    pub title: &'a str,
    pub id: egui::Id,
    pub open: Option<&'a mut bool>,
    pub collapsible: bool,
    pub resizable: bool,
    pub default_pos: Option<egui::Pos2>,
    pub default_width: Option<f32>,
    pub anchor: Option<(egui::Align2, egui::Vec2)>,
}

impl<'a> WindowOpts<'a> {
    pub fn new(title: &'a str, id: egui::Id) -> Self {
        Self {
            title,
            id,
            open: None,
            collapsible: false,
            resizable: false,
            default_pos: None,
            default_width: None,
            anchor: None,
        }
    }
}

/// Floating panel. Only design-system entry that constructs `egui::Window`.
pub fn window<R>(
    ctx: &egui::Context,
    mut opts: WindowOpts<'_>,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<egui::InnerResponse<Option<R>>> {
    let open_t = if let Some(open) = opts.open.as_ref() {
        animate_bool(ctx, opts.id.with("win_open"), **open)
    } else {
        animate(ctx, opts.id.with("win_open"), 1.0)
    };
    if open_t <= 0.001 {
        return None;
    }

    let frame = egui::Frame::NONE
        .fill(panel_bg())
        .inner_margin(egui::Margin::symmetric(SPACE_SM as i8, SPACE_SM as i8))
        .stroke(egui::Stroke::new(ACCENT_EDGE_PX, accent()));

    let mut w = egui::Window::new(
        egui::RichText::new(opts.title)
            .font(display_font(TEXT_MD))
            .color(text()),
    )
    .id(opts.id)
    .collapsible(opts.collapsible)
    .resizable(opts.resizable)
    .frame(frame)
    .title_bar(true);

    if let Some(pos) = opts.default_pos {
        w = w.default_pos(pos);
    }
    if let Some(width) = opts.default_width {
        w = w.default_width(width);
    }
    if let Some((align, offset)) = opts.anchor {
        w = w.anchor(align, offset);
    }
    if let Some(open) = opts.open.as_mut() {
        w = w.open(open);
    }

    w.show(ctx, |ui| {
        ui.set_opacity(open_t.clamp(0.2, 1.0));
        add_contents(ui)
    })
}

/// Thin flat separator (no egui heavy rule).
pub fn thin_separator(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, CORNER_RADIUS, with_alpha(border(), 180));
}

// ---------------------------------------------------------------------
// UI gallery (MF_UI_GALLERY=1)
// ---------------------------------------------------------------------

/// Full-bleed component gallery for one-screenshot visual review.
pub fn show_gallery(ctx: &egui::Context) {
    let fade = animate(ctx, egui::Id::new("mf_gallery_fade"), 1.0);
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(SURFACE).inner_margin(SPACE_XL))
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(wordmark("MetroForge"));
                ui.label(label_muted("UI gallery  MF_UI_GALLERY=1"));
                ui.add_space(SPACE_LG);

                ui.label(heading("Buttons"));
                ui.add_space(SPACE_SM);
                ui.horizontal(|ui| {
                    let _ = button(ui, "Primary", ButtonKind::Primary);
                    let _ = button(ui, "Ghost", ButtonKind::Ghost);
                    let _ = button(ui, "Danger", ButtonKind::Danger);
                    let _ = button(ui, "On", ButtonKind::Toggle(true));
                    let _ = button(ui, "Off", ButtonKind::Toggle(false));
                });
                ui.add_space(SPACE_MD);

                ui.label(heading("Menu text"));
                ui.add_space(SPACE_SM);
                ui.allocate_ui_with_layout(
                    egui::vec2(280.0, 160.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        let _ = menu_text_button(ui, "Play");
                        let _ = menu_text_button(ui, "Settings");
                        let _ = menu_text_button(ui, "Quit");
                    },
                );
                ui.add_space(SPACE_MD);

                ui.label(heading("Panel / tiles / progress / toast"));
                ui.add_space(SPACE_SM);
                panel(ui, accent(), |ui| {
                    ui.set_width(360.0);
                    ui.label(label_body("Panel with accent edge."));
                    ui.add_space(SPACE_SM);
                    ui.horizontal(|ui| {
                        stat_tile(ui, "Cash", "$1,240,000");
                        ui.add_space(SPACE_SM);
                        stat_tile(ui, "Approval", "72%");
                    });
                    ui.add_space(SPACE_SM);
                    let _ = progress_bar(ui, 0.62, 320.0);
                    ui.add_space(SPACE_SM);
                    toast(ui, "Route opened", ToastTone::Good);
                    ui.add_space(SPACE_XXS);
                    toast(ui, "Budget warning", ToastTone::Warn);
                    ui.add_space(SPACE_XXS);
                    toast(ui, "System notice", ToastTone::Info);
                });
                ui.add_space(SPACE_MD);

                ui.label(heading("Spokes"));
                ui.add_space(SPACE_SM);
                ui.horizontal(|ui| {
                    for (name, color) in [
                        ("Green", SPOKE_GREEN),
                        ("Blue", SPOKE_BLUE),
                        ("Orange", SPOKE_ORANGE),
                        ("Red", SPOKE_RED),
                    ] {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(72.0, 40.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, CORNER_RADIUS, color);
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            name,
                            body_font(TEXT_XS),
                            SURFACE,
                        );
                    }
                });
                ui.add_space(SPACE_XL);
            });
        });
}

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
    /// A solid 5-point star - the v0.4 end-of-scenario report's star
    /// rating. Unlike every other glyph above (stroke-only, per this
    /// module's "single-stroke" brief), this one is a filled polygon: the
    /// report draws three of these per city, and the "earned vs not"
    /// distinction is carried by `color` alone (e.g. `GOOD` vs `MUTED`) -
    /// a filled shape reads unambiguously at a glance in both states,
    /// where an outline-only star would read as "not earned" in both.
    Star,
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
        IconKind::Star => {
            let center = pt(rect, 0.5, 0.52);
            let outer_r = rect.width().min(rect.height()) * 0.46;
            // Classic 5-point star inner/outer radius ratio.
            let inner_r = outer_r * 0.382;
            let points: Vec<egui::Pos2> = (0..10)
                .map(|i| {
                    let angle =
                        -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                    let r = if i % 2 == 0 { outer_r } else { inner_r };
                    center + egui::vec2(angle.cos(), angle.sin()) * r
                })
                .collect();
            painter.add(egui::Shape::convex_polygon(
                points,
                color,
                egui::Stroke::NONE,
            ));
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
            ROUTE_DIAGRAM_MIN_THICKNESS
                + t * (ROUTE_DIAGRAM_MAX_THICKNESS - ROUTE_DIAGRAM_MIN_THICKNESS)
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
                    text(),
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
            muted(),
        );
    }
}

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
    painter.rect_filled(rect, CORNER_RADIUS, inactive_bg());
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

    #[test]
    fn z_order_policy_is_strictly_increasing() {
        // Middle < Foreground < Tooltip (egui Order discriminant order).
        assert!(ORDER_HINT < ORDER_TOAST);
        assert!(ORDER_TOAST < ORDER_MODAL);
        assert_eq!(ORDER_HINT, ORDER_PANEL);
    }

    #[test]
    fn crowding_color_hits_the_semantic_endpoints() {
        assert_eq!(crowding_color(0.0), GOOD);
        assert_eq!(crowding_color(0.5), WARN);
        assert_eq!(crowding_color(1.0), BAD);
    }

    #[test]
    fn crowding_color_clamps_out_of_range_inputs() {
        assert_eq!(crowding_color(-3.0), GOOD);
        assert_eq!(crowding_color(9.0), BAD);
    }

    #[test]
    fn crowding_color_interpolates_between_endpoints() {
        // A quarter of the way is between GOOD and WARN, distinct from both.
        let mid = crowding_color(0.25);
        assert_ne!(mid, GOOD);
        assert_ne!(mid, WARN);
        // Red channel rises monotonically as crowding climbs (GOOD..BAD).
        assert!(crowding_color(0.2).r() <= crowding_color(0.8).r());
    }

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
                IconKind::Star,
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
                }
            });
        });
    }
}

#[cfg(test)]
mod sparkline_tests {
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
                IconKind::Star,
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
