//! egui HUD (spec §3.4 `hud.rs`), styled per art-direction.md §8: off-white
//! panels, near-black text, vivid accents reserved for interactive/transit
//! elements, no gradients or rounded-corner excess, one embedded OFL font.

use std::time::{SystemTime, UNIX_EPOCH};

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use mf_net::{NetStatus, ReconnectState, SimEvent, SimLink};
use mf_protocol::envelope::FromSimJson;
use mf_protocol::{Difficulty, FromSimMsg, ToSim, ToastTone};
use mf_state::{LatestUi, QualityTier, SubwayView, Theme};

use crate::audio::{PlaySfx, Sfx};
use crate::campaign::{self, CampaignProgress};
use crate::config::MfConfig;
use crate::saves::{self, SaveManager, SaveSlot};
use crate::state::{toggle_pause, AppState, PauseState, PendingInit, SimHello};

// Art-direction §1/§8 palette, in egui's 0..255 sRGB `Color32`. These are
// the `Theme::Light` values specifically — `hud_visuals_for` below is the
// theme-indexed source of truth for the egui chrome (panel/window fill,
// text, accent); these top-level consts stay fixed (badge/status colors
// like GOOD/WARN/BAD and card fills carry semantic meaning independent of
// theme, and changing every one of their many call sites throughout this
// file is out of scope for issue #32 — only the overall panel/text/accent
// chrome and the day/night/transit render palette are theme-indexed).
const PANEL_BG: egui::Color32 = egui::Color32::from_rgb(0xf4, 0xf5, 0xf2); // near-white
const TEXT_COLOR: egui::Color32 = egui::Color32::from_rgb(0x17, 0x18, 0x1c); // rich black
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x00, 0x7a, 0xff); // metro blue
const GOOD: egui::Color32 = egui::Color32::from_rgb(0x34, 0xc7, 0x59);
const WARN: egui::Color32 = egui::Color32::from_rgb(0xff, 0x95, 0x00);
const BAD: egui::Color32 = egui::Color32::from_rgb(0xff, 0x3b, 0x30);

// Theme-indexed egui chrome colors (panel/window fill, primary text, the
// accent, idle-widget backgrounds) live in `design_system::theme_colors` —
// see [`setup_egui_style_system`] below, the one call site.

const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
// Muted secondary text (subtitle/labels/version) — art-direction reserves
// full rich-black for primary copy; this is the same de-emphasis egui's own
// `weak` text uses, picked to sit comfortably on the off-white panel.
const MUTED_TEXT: egui::Color32 = egui::Color32::from_rgb(0x6b, 0x6d, 0x72);
// City-select / continue card fill + hover fill (same values as the top-bar
// speed/subway toggle buttons' resting/hover state — see `design_system::
// INACTIVE_BG`/`HOVER_BG`; kept as local copies rather than importing that
// module, matching this file's existing "hud.rs keeps its own copies for
// now" convention noted on that module's doc comment).
const CARD_BG: egui::Color32 = egui::Color32::from_rgb(0xe9, 0xea, 0xe5);
const CARD_HOVER_BG: egui::Color32 = egui::Color32::from_rgb(0xdc, 0xde, 0xd8);
const CARD_BORDER: egui::Color32 = egui::Color32::from_rgb(0xd8, 0xd9, 0xd4);
const CARD_CORNER: egui::CornerRadius = egui::CornerRadius::same(4);
/// `saves::SLOT_COUNT` as a `usize`, for fixed-size array types below (array
/// lengths need a `usize`; `saves::SLOT_COUNT` is a `u8` so callers stay
/// consistent with the rest of that module's slot-numbering type).
const SAVE_SLOT_COUNT: usize = saves::SLOT_COUNT as usize;

/// Rolling toast log (art-direction: HUD "toast log"). Capped so it can't
/// grow unbounded over a long session.
#[derive(Resource, Default)]
pub struct ToastLog(pub Vec<(String, ToastTone)>);

const TOAST_LOG_CAP: usize = 20;

pub struct MfHudPlugin;

impl Plugin for MfHudPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<ToastLog>()
            .init_resource::<EguiStyleApplied>()
            .add_systems(Update, collect_toasts_system)
            .add_systems(
                EguiPrimaryContextPass,
                (
                    setup_egui_style_system,
                    connecting_hud_system.run_if(in_state(AppState::ConnectingSim)),
                    main_menu_hud_system.run_if(in_state(AppState::MainMenu)),
                    loading_hud_system.run_if(in_state(AppState::Loading)),
                    in_game_hud_system.run_if(in_state(AppState::InGame)),
                    pause_overlay_system.run_if(in_state(AppState::InGame)),
                    fatal_banner_system,
                )
                    .chain()
                    .run_if(|| !crate::design_system::hud_hidden()),
            );
    }
}

/// Guards [`setup_egui_style_system`]'s (cheap but non-trivial) font/visuals
/// work: skips it once it has already applied the *current* theme.
/// Deliberately NOT a `Startup` system: at `Startup` the primary window's
/// egui context isn't guaranteed to exist yet (bevy_egui wires it up once
/// the window backend is ready), so a one-shot `Startup` system silently
/// no-ops and the HUD is stuck on bevy_egui's default dark theme forever —
/// this bit during initial implementation (art-direction §8's off-white
/// panels never appeared). Retrying every `EguiPrimaryContextPass` tick
/// until it succeeds fixes that with no observable per-frame cost once
/// applied — and re-applies whenever `applied_theme` no longer matches the
/// live `Theme` resource, so a HUD theme pick takes effect on the very next
/// frame instead of needing a restart.
#[derive(Resource, Default)]
struct EguiStyleApplied(Option<Theme>);

fn setup_egui_style_system(
    mut contexts: EguiContexts,
    theme: Res<Theme>,
    mut applied: ResMut<EguiStyleApplied>,
) {
    if applied.0 == Some(*theme) {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(INTER_REGULAR)),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".to_owned());
    ctx.set_fonts(fonts);

    let hv = crate::design_system::theme_colors(*theme);
    let mut visuals = if *theme == Theme::Light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    visuals.panel_fill = hv.panel_bg;
    visuals.window_fill = hv.panel_bg;
    visuals.extreme_bg_color = hv.extreme_bg;
    visuals.faint_bg_color = hv.extreme_bg;
    visuals.override_text_color = Some(hv.text);
    visuals.widgets.noninteractive.bg_fill = hv.panel_bg;
    visuals.widgets.noninteractive.weak_bg_fill = hv.panel_bg;
    visuals.widgets.inactive.bg_fill = hv.inactive_bg;
    visuals.widgets.inactive.weak_bg_fill = hv.inactive_bg;
    visuals.widgets.hovered.bg_fill = hv.hover_bg;
    visuals.widgets.active.bg_fill = hv.accent;
    visuals.selection.bg_fill = hv.accent;
    // Art-direction: "no rounded-corner excess" — keep corners near-square.
    visuals.window_corner_radius = egui::CornerRadius::same(2);
    visuals.menu_corner_radius = egui::CornerRadius::same(2);
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.noninteractive,
    ] {
        widget.corner_radius = egui::CornerRadius::same(2);
    }
    ctx.set_visuals(visuals);
    applied.0 = Some(*theme);
}

fn collect_toasts_system(mut events: EventReader<SimEvent>, mut log: ResMut<ToastLog>) {
    for SimEvent(msg) in events.read() {
        if let FromSimMsg::Json(FromSimJson::Toast(toast)) = msg {
            log.0.push((toast.message.clone(), toast.tone));
            if log.0.len() > TOAST_LOG_CAP {
                let excess = log.0.len() - TOAST_LOG_CAP;
                log.0.drain(0..excess);
            }
        }
    }
}

/// A muted, near-invisible group divider — art-direction §8 wants clean flat
/// separation, not egui's default heavy separator line.
fn thin_separator(ui: &mut egui::Ui) {
    ui.add(egui::Separator::default().shrink(6.0));
}

/// One hover tick the first frame the pointer lands on a widget; re-arms
/// when it leaves, so re-entering the same widget ticks again. `last` is
/// per-system `Local` state, so two systems can't fight over it.
fn hover_tick(resp: &egui::Response, last: &mut Option<egui::Id>, sfx: &mut EventWriter<PlaySfx>) {
    if resp.hovered() {
        if *last != Some(resp.id) {
            sfx.write(PlaySfx(Sfx::Hover));
            *last = Some(resp.id);
        }
    } else if *last == Some(resp.id) {
        *last = None;
    }
}

/// Comma-grouped integer (e.g. `146015` -> `"146,015"`). Plain
/// `{:.0}`-formatted cash/population numbers change width every tick they
/// cross a digit boundary, which visibly shifts every group to their right
/// in the top bar; grouping doesn't fix that alone (see
/// `fixed_width_label`) but keeps the number itself readable at a glance.
fn format_thousands(value: f64) -> String {
    let rounded = value.round().max(0.0) as u64;
    let digits = rounded.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped
}

fn format_cash(value: f64) -> String {
    format!("${}", format_thousands(value))
}

/// Reserves a fixed-width, left-aligned cell for `text` so a value growing
/// or shrinking a digit (cash crossing $1,000,000, population crossing
/// 100,000, etc.) can't nudge every group to its right — the top bar's
/// layout stays stable frame to frame. `width` should be sized for the
/// widest string the field will plausibly show.
fn fixed_width_label(ui: &mut egui::Ui, text: egui::RichText, width: f32) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(text);
        },
    );
}

/// The tier rows shared by every quality combo box (top bar, pause overlay,
/// main menu) — split out from [`quality_selector`] so the main menu can
/// pair it with its own stacked-above label instead of `from_label`'s
/// beside-the-box one, without a second copy of the tier list/persist call.
fn quality_options(
    ui: &mut egui::Ui,
    quality: &mut QualityTier,
    config: &mut MfConfig,
    sfx: &mut EventWriter<PlaySfx>,
) {
    for tier in [
        QualityTier::Potato,
        QualityTier::Low,
        QualityTier::Medium,
        QualityTier::High,
    ] {
        if ui
            .selectable_label(*quality == tier, format!("{tier:?}"))
            .clicked()
        {
            *quality = tier;
            config.set_quality_override(Some(tier));
            sfx.write(PlaySfx(Sfx::Confirm));
        }
    }
}

fn quality_selector(
    ui: &mut egui::Ui,
    quality: &mut QualityTier,
    config: &mut MfConfig,
    sfx: &mut EventWriter<PlaySfx>,
) {
    egui::ComboBox::from_label("Quality")
        .selected_text(format!("{quality:?}"))
        .show_ui(ui, |ui| quality_options(ui, quality, config, sfx));
}

/// The theme rows shared by every theme combo box (pause overlay, main
/// menu) — same split-out shape as [`quality_options`] above, issue #32.
fn theme_options(
    ui: &mut egui::Ui,
    theme: &mut Theme,
    config: &mut MfConfig,
    sfx: &mut EventWriter<PlaySfx>,
) {
    for candidate in Theme::ALL {
        if ui
            .selectable_label(*theme == candidate, candidate.label())
            .clicked()
        {
            *theme = candidate;
            config.set_theme_override(Some(candidate));
            sfx.write(PlaySfx(Sfx::Confirm));
        }
    }
}

fn theme_selector(
    ui: &mut egui::Ui,
    theme: &mut Theme,
    config: &mut MfConfig,
    sfx: &mut EventWriter<PlaySfx>,
) {
    egui::ComboBox::from_label("Theme")
        .selected_text(theme.label())
        .show_ui(ui, |ui| theme_options(ui, theme, config, sfx));
}

/// ConnectingSim previously registered NO ui system at all, so a player whose
/// sidecar was slow (or repeatedly failing) stared at a bare ClearColor with
/// zero feedback until the fatal banner eventually appeared. Every app state
/// must draw *something*.
fn connecting_hud_system(mut contexts: EguiContexts, reconnect: Res<ReconnectState>) -> Result {
    let ctx = contexts.ctx_mut()?;
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.heading("MetroForge");
        ui.add_space(12.0);
        match &reconnect.status {
            NetStatus::Fatal(msg) => {
                ui.colored_label(BAD, format!("Could not start the simulation: {msg}"));
            }
            NetStatus::Reconnecting { attempt } => {
                ui.label(format!(
                    "Starting the simulation (attempt {attempt} of 5)..."
                ));
            }
            NetStatus::Connected => {
                ui.label("Starting the simulation...");
            }
        }
    });
    Ok(())
}

/// Label for a field row: small and muted, stacked above its control
/// (chosen over right-aligned labels — with three rows of differing
/// natural label width, stacked keeps every control's left edge aligned
/// without hand-tuning a label column width).
fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(12.0).color(MUTED_TEXT));
}

/// A single 5-point star, filled via a triangle fan from its own center —
/// valid because a regular star polygon is star-shaped around that point
/// (every edge is visible from it), so the fan tiles it exactly with no
/// gaps or overlaps, unlike `egui::Shape::convex_polygon` which assumes
/// convexity a star doesn't have. `filled` picks a solid `color` fill
/// (earned) vs. a hollow outline in `color` (not yet earned).
///
/// Kept local rather than routed through `design_system::icon`: the menu
/// needs the filled-vs-hollow earned/unearned distinction, which the
/// stroke-style `IconKind::Star` does not model.
fn draw_star(
    painter: &egui::Painter,
    center: egui::Pos2,
    outer_r: f32,
    filled: bool,
    color: egui::Color32,
) {
    const POINTS: usize = 5;
    let inner_r = outer_r * 0.42;
    let verts: Vec<egui::Pos2> = (0..POINTS * 2)
        .map(|i| {
            let r = if i % 2 == 0 { outer_r } else { inner_r };
            let angle =
                -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / POINTS as f32;
            center + egui::vec2(angle.cos(), angle.sin()) * r
        })
        .collect();
    if filled {
        for i in 0..verts.len() {
            let a = verts[i];
            let b = verts[(i + 1) % verts.len()];
            painter.add(egui::Shape::convex_polygon(
                vec![center, a, b],
                color,
                egui::Stroke::NONE,
            ));
        }
    } else {
        let mut loop_pts = verts;
        loop_pts.push(loop_pts[0]);
        painter.line(loop_pts, egui::Stroke::new(1.2, color));
    }
}

/// A simple padlock silhouette (filled body + stroked shackle arc), sized
/// for a locked city card. Deliberately separate from `build_ui.rs`'s
/// smaller per-toolbar-button lock badge (that file isn't touched this
/// wave) rather than shared, since the two are drawn at different scales
/// for different contexts.
fn draw_lock(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let body = egui::Rect::from_min_size(
        egui::pos2(
            rect.min.x + rect.width() * 0.2,
            rect.min.y + rect.height() * 0.4,
        ),
        egui::vec2(rect.width() * 0.6, rect.height() * 0.6),
    );
    painter.rect_filled(body, egui::CornerRadius::same(1), color);
    let shackle_center = egui::pos2(body.center().x, body.min.y);
    painter.circle_stroke(
        shackle_center,
        rect.width() * 0.28,
        egui::Stroke::new(1.6, color),
    );
}

/// `"just now"` / `"Nm ago"` / `"Nh ago"` / `"Nd ago"` relative to now —
/// coarse on purpose, this is a save-slot caption, not a precise clock.
fn format_relative_time(saved_at_epoch_secs: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(saved_at_epoch_secs);
    let elapsed = now.saturating_sub(saved_at_epoch_secs);
    if elapsed < 60 {
        "just now".to_string()
    } else if elapsed < 3600 {
        format!("{}m ago", elapsed / 60)
    } else if elapsed < 86_400 {
        format!("{}h ago", elapsed / 3600)
    } else {
        format!("{}d ago", elapsed / 86_400)
    }
}

/// Fallback display label for a `CITY_ORDER` key that isn't (yet) present
/// in the sidecar's `hello.city_list` — capitalizes the raw key ("dc" ->
/// "Dc") rather than showing the wire-protocol identifier verbatim.
fn capitalize(key: &str) -> String {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// One city card in the main menu's select grid: label, up-to-3 star
/// glyphs, and (if locked) a dimmed treatment plus lock glyph + "Earn N more
/// stars" caption. Returns whether the card was clicked (always `false` for
/// a locked card — `Sense::hover()` only, so it isn't even interactive).
#[allow(clippy::too_many_arguments)]
fn city_card(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    label: &str,
    stars: u8,
    unlocked: bool,
    selected: bool,
    stars_needed: u32,
    hovered: &mut Option<egui::Id>,
    sfx: &mut EventWriter<PlaySfx>,
) -> bool {
    let sense = if unlocked {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    if unlocked {
        hover_tick(&response, hovered, sfx);
    }

    let painter = ui.painter_at(rect);
    let bg = if unlocked && response.hovered() {
        CARD_HOVER_BG
    } else {
        CARD_BG
    };
    painter.rect_filled(rect, CARD_CORNER, bg);
    let border = if selected {
        egui::Stroke::new(2.5, ACCENT)
    } else {
        egui::Stroke::new(1.0, CARD_BORDER)
    };
    painter.rect_stroke(rect, CARD_CORNER, border, egui::StrokeKind::Inside);

    let content_rect = rect.shrink(10.0);
    let text_color = if unlocked { TEXT_COLOR } else { MUTED_TEXT };
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.label(
        egui::RichText::new(label)
            .size(15.0)
            .strong()
            .color(text_color),
    );
    child.add_space(6.0);

    let star_size = egui::vec2(content_rect.width(), 16.0);
    let (star_rect, _) = child.allocate_exact_size(star_size, egui::Sense::hover());
    let star_painter = child.painter_at(star_rect);
    let star_r = 8.0;
    for i in 0..3u8 {
        let cx = star_rect.left() + star_r + i as f32 * (star_r * 2.5);
        let center = egui::pos2(cx, star_rect.center().y);
        let filled = i < stars.min(3);
        let color = if !unlocked {
            MUTED_TEXT
        } else if filled {
            ACCENT
        } else {
            CARD_BORDER
        };
        draw_star(&star_painter, center, star_r, filled, color);
    }

    if !unlocked {
        child.add_space(6.0);
        child.horizontal(|ui| {
            let (lock_rect, _) =
                ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
            draw_lock(&ui.painter_at(lock_rect), lock_rect, MUTED_TEXT);
            let plural = if stars_needed == 1 { "" } else { "s" };
            ui.label(
                egui::RichText::new(format!("Earn {stars_needed} more star{plural}"))
                    .size(11.0)
                    .color(MUTED_TEXT),
            );
        });
    }

    unlocked && response.clicked()
}

/// One row in the main menu's "Continue" section: an autosave or numbered
/// slot, showing its city/day/cash/timestamp if occupied or "Empty"
/// otherwise. Returns whether an occupied row was clicked (locked/empty
/// rows are hover-only, same convention as [`city_card`]).
fn continue_slot_row(
    ui: &mut egui::Ui,
    width: f32,
    slot: SaveSlot,
    meta: Option<&saves::SaveMeta>,
    hovered: &mut Option<egui::Id>,
    sfx: &mut EventWriter<PlaySfx>,
) -> bool {
    let title = match slot {
        SaveSlot::Autosave => "Autosave".to_string(),
        SaveSlot::Slot(n) => format!("Slot {n}"),
    };
    let occupied = meta.is_some();
    let sense = if occupied {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let size = egui::vec2(width, 40.0);
    let (rect, response) = ui.allocate_exact_size(size, sense);
    if occupied {
        hover_tick(&response, hovered, sfx);
    }

    let painter = ui.painter_at(rect);
    let bg = if occupied && response.hovered() {
        CARD_HOVER_BG
    } else {
        CARD_BG
    };
    painter.rect_filled(rect, CARD_CORNER, bg);
    painter.rect_stroke(
        rect,
        CARD_CORNER,
        egui::Stroke::new(1.0, CARD_BORDER),
        egui::StrokeKind::Inside,
    );

    let content_rect = rect.shrink2(egui::vec2(12.0, 6.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.horizontal(|ui| {
        ui.label(
            egui::RichText::new(&title)
                .size(13.0)
                .strong()
                .color(if occupied { TEXT_COLOR } else { MUTED_TEXT }),
        );
        if let Some(meta) = meta {
            ui.label(
                egui::RichText::new(format_relative_time(meta.saved_at_epoch_secs))
                    .size(11.0)
                    .color(MUTED_TEXT),
            );
        }
    });
    let subtitle = match meta {
        Some(meta) => {
            let city = meta
                .city_label
                .clone()
                .unwrap_or_else(|| "Unknown city".to_string());
            format!("{city} - Day {} - {}", meta.day, format_cash(meta.cash))
        }
        None => "Empty".to_string(),
    };
    child.label(egui::RichText::new(subtitle).size(11.0).color(MUTED_TEXT));

    occupied && response.clicked()
}

#[allow(clippy::too_many_arguments)]
fn main_menu_hud_system(
    mut contexts: EguiContexts,
    hello: Res<SimHello>,
    progress: Res<CampaignProgress>,
    mut pending: ResMut<PendingInit>,
    mut quality: ResMut<QualityTier>,
    mut theme: ResMut<Theme>,
    mut config: ResMut<MfConfig>,
    mut save_manager: ResMut<SaveManager>,
    mut toasts: ResMut<ToastLog>,
    state: Res<State<AppState>>,
    mut next_state: ResMut<NextState<AppState>>,
    mut sfx: EventWriter<PlaySfx>,
    mut hovered: Local<Option<egui::Id>>,
    mut slots_cache: Local<Option<Vec<saves::SlotEntry>>>,
) -> Result {
    // Reread the save slots from disk once on entry to `MainMenu` (or the
    // very first time this system runs) rather than every single frame —
    // the slot files only change from a pause-overlay save or an autosave,
    // both of which happen while `InGame`, so a menu-entry refresh is
    // enough to stay current without doing disk IO at 60 Hz for a screen
    // that just sits there between clicks.
    if state.is_changed() || slots_cache.is_none() {
        *slots_cache = Some(saves::list());
    }
    let slots = slots_cache.as_ref().expect("populated just above");

    let ctx = contexts.ctx_mut()?;
    // Fade in over ~200ms on entry. `set_opacity` (rather than fighting
    // egui for a per-widget alpha) multiplies the whole panel's painted
    // output, text included, so both the dim scrim of a prior state and
    // this menu's own controls ease in together.
    let fade = ctx.animate_value_with_time(egui::Id::new("main_menu_fade"), 1.0, 0.2);

    egui::TopBottomPanel::bottom("main_menu_version")
        .frame(
            egui::Frame::default()
                .fill(PANEL_BG)
                .inner_margin(egui::Margin::symmetric(12, 10)),
        )
        .show_separator_line(false)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .size(11.0)
                        .color(MUTED_TEXT),
                );
            });
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            ui.vertical_centered(|ui| {
                // Roughly centers the card vertically for typical window
                // heights without measuring the card first.
                ui.add_space((ui.available_height() * 0.22).max(24.0));

                ui.scope(|ui| {
                    ui.set_width(460.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("MetroForge").size(34.0).strong());
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Build the network. Move the city.")
                                .size(14.0)
                                .color(MUTED_TEXT),
                        );
                        ui.add_space(24.0);

                        let cities = hello
                            .0
                            .as_ref()
                            .map(|h| h.city_list.as_slice())
                            .unwrap_or(&[]);
                        let total_stars: u32 = campaign::CITY_ORDER
                            .iter()
                            .map(|&key| progress.stars(key) as u32)
                            .sum();

                        field_label(ui, "City");
                        ui.add_space(4.0);
                        const CARD_SIZE: egui::Vec2 = egui::vec2(216.0, 96.0);
                        const CARD_GAP: f32 = 12.0;
                        egui::Grid::new("city_grid")
                            .num_columns(2)
                            .spacing(egui::vec2(CARD_GAP, CARD_GAP))
                            .show(ui, |ui| {
                                for (i, &key) in campaign::CITY_ORDER.iter().enumerate() {
                                    let label = cities
                                        .iter()
                                        .find(|c| c.key == key)
                                        .map(|c| c.label.clone())
                                        .unwrap_or_else(|| capitalize(key));
                                    let stars = progress.stars(key);
                                    let unlocked = progress.city_unlocked(key);
                                    let selected = pending.preset_key == key;
                                    // Duplicates the unlock formula from
                                    // `campaign::CampaignProgress::city_unlocked`
                                    // purely to render the "earn N more" caption
                                    // - the real gate below always calls
                                    // `city_unlocked` itself rather than trusting
                                    // this local recomputation.
                                    let stars_needed = (2 * i as u32).saturating_sub(total_stars);
                                    let clicked = city_card(
                                        ui,
                                        CARD_SIZE,
                                        &label,
                                        stars,
                                        unlocked,
                                        selected,
                                        stars_needed,
                                        &mut hovered,
                                        &mut sfx,
                                    );
                                    if clicked {
                                        pending.preset_key = key.to_string();
                                        sfx.write(PlaySfx(Sfx::Confirm));
                                    }
                                    if i % 2 == 1 {
                                        ui.end_row();
                                    }
                                }
                                if !campaign::CITY_ORDER.len().is_multiple_of(2) {
                                    ui.end_row();
                                }
                            });

                        ui.add_space(20.0);
                        thin_separator(ui);
                        ui.add_space(8.0);
                        field_label(ui, "Continue");
                        ui.add_space(4.0);
                        for entry in slots {
                            let clicked = continue_slot_row(
                                ui,
                                460.0,
                                entry.slot,
                                entry.meta.as_ref(),
                                &mut hovered,
                                &mut sfx,
                            );
                            ui.add_space(6.0);
                            if clicked
                                && save_manager
                                    .load(entry.slot, &mut toasts, &mut sfx)
                                    .is_some()
                            {
                                next_state.set(AppState::Loading);
                            }
                        }

                        ui.add_space(12.0);
                        field_label(ui, "Difficulty");
                        egui::ComboBox::from_id_salt("difficulty_picker")
                            .selected_text(format!("{:?}", pending.difficulty))
                            .width(300.0)
                            .show_ui(ui, |ui| {
                                for d in [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard] {
                                    ui.selectable_value(
                                        &mut pending.difficulty,
                                        d,
                                        format!("{d:?}"),
                                    );
                                }
                            });

                        ui.add_space(12.0);
                        field_label(ui, "Quality");
                        egui::ComboBox::from_id_salt("quality_picker")
                            .selected_text(format!("{:?}", *quality))
                            .width(300.0)
                            .show_ui(ui, |ui| {
                                quality_options(ui, &mut quality, &mut config, &mut sfx)
                            });

                        ui.add_space(12.0);
                        field_label(ui, "Theme");
                        egui::ComboBox::from_id_salt("theme_picker")
                            .selected_text(theme.label())
                            .width(300.0)
                            .show_ui(ui, |ui| {
                                theme_options(ui, &mut theme, &mut config, &mut sfx)
                            });

                        ui.add_space(28.0);
                        let start = ui.add_sized(
                            [220.0, 44.0],
                            egui::Button::new(
                                egui::RichText::new("Start")
                                    .color(egui::Color32::WHITE)
                                    .size(16.0)
                                    .strong(),
                            )
                            .fill(ACCENT),
                        );
                        hover_tick(&start, &mut hovered, &mut sfx);
                        if start.clicked() {
                            sfx.write(PlaySfx(Sfx::Confirm));
                            next_state.set(AppState::Loading);
                        }
                    });
                });
            });
        });
    Ok(())
}

fn loading_hud_system(
    mut contexts: EguiContexts,
    city: Res<mf_state::CurrentCity>,
    fields: Res<mf_state::LatestFields>,
    ui_state: Res<LatestUi>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.3).max(24.0));
            ui.label(egui::RichText::new("Loading city").size(28.0).strong());
            ui.add_space(16.0);

            let readiness = |label: &str, ready: bool| {
                let status = if ready { "ready" } else { "waiting" };
                egui::RichText::new(format!("{label}: {status}"))
                    .size(13.0)
                    .color(MUTED_TEXT)
            };
            ui.label(readiness("Static city", city.static_city.is_some()));
            ui.label(readiness("Masks", city.masks_complete()));
            ui.label(readiness("Fields", fields.0.is_some()));
            ui.label(readiness("Interface", ui_state.0.is_some()));
        });
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn in_game_hud_system(
    mut contexts: EguiContexts,
    ui_state: Res<LatestUi>,
    link: Option<Res<SimLink>>,
    mut quality: ResMut<QualityTier>,
    mut theme: ResMut<Theme>,
    mut config: ResMut<MfConfig>,
    mut subway: ResMut<SubwayView>,
    toasts: Res<ToastLog>,
    mut sfx: EventWriter<PlaySfx>,
    mut hovered: Local<Option<egui::Id>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;

    // Art-direction §8: off-white panel, near-black text, consistent
    // spacing/padding, vivid accents reserved for interactive/transit
    // elements only. Budget | day+clock | approval | pop | speed | subway
    // toggle | quality, left-to-right, each group visually separated.
    egui::TopBottomPanel::top("hud_top")
        .frame(
            egui::Frame::default()
                .fill(PANEL_BG)
                .inner_margin(egui::Margin::symmetric(14, 10)),
        )
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(16.0, 0.0);
            ui.horizontal_centered(|ui| {
                if let Some(state) = &ui_state.0 {
                    // Monospace + a fixed-width cell per group: cash/pop/day
                    // digits change width every tick they cross a boundary
                    // (e.g. $999,999 -> $1,000,000), and with a proportional
                    // font in an auto-sized label that visibly shoves every
                    // group to its right. Widths below are sized for the
                    // widest value each field would plausibly show.
                    fixed_width_label(
                        ui,
                        egui::RichText::new(format_cash(state.cash))
                            .monospace()
                            .strong()
                            .size(15.0),
                        130.0,
                    );
                    thin_separator(ui);

                    const TICKS_PER_DAY: u64 = 1200;
                    let hour = (state.tick % TICKS_PER_DAY) as f64 / TICKS_PER_DAY as f64 * 24.0;
                    fixed_width_label(
                        ui,
                        egui::RichText::new(format!(
                            "Day {}  {:02}:{:02}",
                            state.day,
                            hour as u32,
                            ((hour.fract()) * 60.0) as u32
                        ))
                        .monospace(),
                        140.0,
                    );
                    thin_separator(ui);

                    let approval_color = if state.approval >= 60.0 {
                        GOOD
                    } else if state.approval >= 35.0 {
                        WARN
                    } else {
                        BAD
                    };
                    fixed_width_label(
                        ui,
                        egui::RichText::new(format!("Approval {:.0}%", state.approval))
                            .monospace()
                            .color(approval_color),
                        120.0,
                    );
                    thin_separator(ui);
                    fixed_width_label(
                        ui,
                        egui::RichText::new(format!("Pop {}", format_thousands(state.population)))
                            .monospace(),
                        130.0,
                    );
                } else {
                    ui.label("Connecting to city...");
                }

                thin_separator(ui);
                for (label, speed) in [("1x", 1.0), ("10x", 10.0), ("30x", 30.0), ("120x", 120.0)] {
                    let is_current = ui_state
                        .0
                        .as_ref()
                        .map(|s| (s.speed - speed).abs() < 0.01)
                        .unwrap_or(false);
                    let button = egui::Button::new(label).fill(if is_current {
                        ACCENT
                    } else {
                        egui::Color32::from_rgb(0xe9, 0xea, 0xe5)
                    });
                    let resp = ui.add(button);
                    hover_tick(&resp, &mut hovered, &mut sfx);
                    if resp.clicked() {
                        if let Some(link) = &link {
                            sfx.write(PlaySfx(Sfx::SpeedTick));
                            let _ = link
                                .transport
                                .send(ToSim::SetSpeed(mf_protocol::SetSpeedPayload { speed }));
                        }
                    }
                }

                thin_separator(ui);
                let subway_button = egui::Button::new(if subway.active {
                    "Surface view"
                } else {
                    "Subway view"
                })
                .fill(if subway.active {
                    ACCENT
                } else {
                    egui::Color32::from_rgb(0xe9, 0xea, 0xe5)
                });
                let subway_resp = ui.add(subway_button);
                hover_tick(&subway_resp, &mut hovered, &mut sfx);
                if subway_resp.clicked() {
                    subway.toggle();
                    sfx.write(PlaySfx(if subway.active {
                        Sfx::Confirm
                    } else {
                        Sfx::Cancel
                    }));
                }

                thin_separator(ui);
                quality_selector(ui, &mut quality, &mut config, &mut sfx);

                thin_separator(ui);
                theme_selector(ui, &mut theme, &mut config, &mut sfx);
            });
        });

    egui::TopBottomPanel::bottom("hud_toasts")
        .frame(
            egui::Frame::default()
                .fill(PANEL_BG)
                .inner_margin(egui::Margin::symmetric(14, 6)),
        )
        .min_height(0.0)
        .show(ctx, |ui| {
            if !toasts.0.is_empty() {
                ui.horizontal(|ui| {
                    for (msg, tone) in toasts.0.iter().rev().take(3) {
                        let color = match tone {
                            ToastTone::Info => TEXT_COLOR,
                            ToastTone::Warn => WARN,
                            ToastTone::Good => GOOD,
                        };
                        ui.colored_label(color, msg);
                        thin_separator(ui);
                    }
                });
            }
        });
    Ok(())
}

/// Pause overlay (`state::PauseState`, toggled by Esc in `input.rs`). Drawn
/// as its own pass after `in_game_hud_system` so it dims and sits on top of
/// the world *and* the top/bottom HUD bars, rather than only the space
/// between them. Uses `egui::Area`s at `Order::Foreground` rather than a
/// `CentralPanel`: panels paint at `Order::Background`, which the HUD bars
/// also occupy and would poke through the dim; a full-screen foreground
/// area also guarantees `wants_pointer_input()` is true everywhere on
/// screen (`egui::Context::is_pointer_over_area` matches on the topmost
/// layer at the cursor), so `camera.rs`'s existing egui-capture check keeps
/// world drag/zoom from leaking through while paused, with no change to
/// that file.
#[allow(clippy::too_many_arguments)]
fn pause_overlay_system(
    mut contexts: EguiContexts,
    mut pause: ResMut<PauseState>,
    ui_state: Res<LatestUi>,
    link: Option<Res<SimLink>>,
    mut quality: ResMut<QualityTier>,
    mut theme: ResMut<Theme>,
    mut config: ResMut<MfConfig>,
    mut save_manager: ResMut<SaveManager>,
    mut toasts: ResMut<ToastLog>,
    pending: Res<PendingInit>,
    mut exit: EventWriter<AppExit>,
    mut sfx: EventWriter<PlaySfx>,
    mut hovered: Local<Option<egui::Id>>,
    mut slot_occupied_cache: Local<Option<[bool; SAVE_SLOT_COUNT]>>,
) -> Result {
    if !pause.active {
        return Ok(());
    }
    // Refresh the occupied/empty cache once per pause session (on the
    // false->true transition, which flags `pause` as changed) rather than
    // re-reading 3 slot files from disk every single frame the pause panel
    // is open.
    if pause.is_changed() || slot_occupied_cache.is_none() {
        let mut occupied = [false; SAVE_SLOT_COUNT];
        for entry in saves::list() {
            if let SaveSlot::Slot(n) = entry.slot {
                let idx = (n - 1) as usize;
                if idx < occupied.len() {
                    occupied[idx] = entry.meta.is_some();
                }
            }
        }
        *slot_occupied_cache = Some(occupied);
    }
    let slot_occupied = slot_occupied_cache.expect("populated just above");

    let ctx = contexts.ctx_mut()?;

    egui::Area::new(egui::Id::new("pause_scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let screen = ui.ctx().screen_rect();
            ui.allocate_response(screen.size(), egui::Sense::hover());
            ui.painter().rect_filled(
                screen,
                egui::CornerRadius::ZERO,
                egui::Color32::from_rgba_unmultiplied(0x17, 0x18, 0x1c, 140),
            );
        });

    egui::Area::new(egui::Id::new("pause_panel"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            egui::Frame::default()
                .fill(PANEL_BG)
                .corner_radius(egui::CornerRadius::same(2))
                .inner_margin(egui::Margin::symmetric(28, 24))
                .show(ui, |ui| {
                    ui.set_width(260.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("Paused").size(24.0).strong());
                        ui.add_space(18.0);

                        let resume = ui.add_sized(
                            [220.0, 40.0],
                            egui::Button::new(
                                egui::RichText::new("Resume")
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                            )
                            .fill(ACCENT),
                        );
                        hover_tick(&resume, &mut hovered, &mut sfx);
                        if resume.clicked() && toggle_pause(&mut pause, &ui_state, link.as_deref())
                        {
                            sfx.write(PlaySfx(Sfx::Unpause));
                        }

                        ui.add_space(14.0);
                        field_label(ui, "Quality");
                        egui::ComboBox::from_id_salt("pause_quality")
                            .selected_text(format!("{:?}", *quality))
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                quality_options(ui, &mut quality, &mut config, &mut sfx)
                            });

                        ui.add_space(14.0);
                        field_label(ui, "Theme");
                        egui::ComboBox::from_id_salt("pause_theme")
                            .selected_text(theme.label())
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                theme_options(ui, &mut theme, &mut config, &mut sfx)
                            });

                        ui.add_space(14.0);
                        field_label(ui, "Save game");
                        ui.horizontal(|ui| {
                            for n in 1..=saves::SLOT_COUNT {
                                let occupied = slot_occupied
                                    .get((n - 1) as usize)
                                    .copied()
                                    .unwrap_or(false);
                                let label = if occupied {
                                    format!("Slot {n}")
                                } else {
                                    format!("Slot {n} (empty)")
                                };
                                let btn = ui.add_sized(
                                    [68.0, 32.0],
                                    egui::Button::new(egui::RichText::new(label).size(11.0)),
                                );
                                hover_tick(&btn, &mut hovered, &mut sfx);
                                if btn.clicked() {
                                    if let (Some(link), Some(state)) = (&link, &ui_state.0) {
                                        save_manager.request_save(
                                            SaveSlot::Slot(n),
                                            Some(pending.preset_key.clone()),
                                            state.day,
                                            state.cash,
                                            link,
                                            &mut toasts,
                                            &mut sfx,
                                        );
                                    }
                                }
                            }
                        });

                        ui.add_space(14.0);
                        let quit =
                            ui.add_sized([220.0, 40.0], egui::Button::new("Quit to desktop"));
                        hover_tick(&quit, &mut hovered, &mut sfx);
                        if quit.clicked() {
                            sfx.write(PlaySfx(Sfx::Cancel));
                            exit.write(AppExit::Success);
                        }
                    });
                });
        });

    Ok(())
}

/// Surfaces `mf-net`'s fatal reconnect failure as a banner rather than a
/// silent black screen (spec §3.2 reconnect: "5 attempts -> fatal error
/// screen"; `state.rs`'s watchdog already dropped us back to `MainMenu`).
fn fatal_banner_system(
    mut contexts: EguiContexts,
    reconnect: Res<ReconnectState>,
    mut sfx: EventWriter<PlaySfx>,
    mut error_played: Local<bool>,
) -> Result {
    let NetStatus::Fatal(msg) = &reconnect.status else {
        *error_played = false;
        return Ok(());
    };
    if !*error_played {
        sfx.write(PlaySfx(Sfx::Error));
        *error_played = true;
    }
    let ctx = contexts.ctx_mut()?;
    egui::TopBottomPanel::bottom("fatal_banner").show(ctx, |ui| {
        ui.colored_label(BAD, format!("Lost connection to the sim: {msg}"));
    });
    Ok(())
}
