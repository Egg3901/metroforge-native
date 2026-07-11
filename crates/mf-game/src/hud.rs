//! egui HUD (spec §3.4 `hud.rs`). Visual chrome lives in
//! [`crate::design_system`]; this file owns layout and interaction only.

use std::time::{SystemTime, UNIX_EPOCH};

use bevy::app::AppExit;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContextSettings, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use mf_net::{NetStatus, ReconnectPhase, ReconnectState, SimEvent, SimLink, MAX_ATTEMPTS};
use mf_protocol::envelope::FromSimJson;
use mf_protocol::{FromSimMsg, ToSim, ToastTone};
use mf_state::{
    ColorblindMode, DetectedQuality, EffectiveKnobs, LatestUi, QualityOverrides, QualityTier,
    ShadowQuality, SubwayView, Theme, WeatherEffects, DRAW_DISTANCE_MIN_M,
    DRAW_DISTANCE_UNLIMITED_M,
};

use crate::audio::{PlaySfx, Sfx};
use crate::camera::CameraRig;
use crate::campaign::CampaignProgress;
use crate::config::MfConfig;
use crate::design_system as ds;
use crate::goals::GoalsPanelOpen;
use crate::graphics_perf::{self, GraphicsBenchmark, ShowFps, BENCHMARK_DURATION_SECS};
use crate::saves::{self, PlaytimeTracker, SaveManager, SaveMeta, SaveSlot};
use crate::state::{toggle_pause, AppState, MenuScreen, PauseState, PendingInit, SimHello};

const GOOD: egui::Color32 = ds::GOOD;
const WARN: egui::Color32 = ds::WARN;
const BAD: egui::Color32 = ds::BAD;

fn panel_bg() -> egui::Color32 {
    ds::panel_bg()
}
fn text_color() -> egui::Color32 {
    ds::text()
}
fn accent() -> egui::Color32 {
    ds::accent()
}
fn muted_text() -> egui::Color32 {
    ds::muted()
}
fn card_bg() -> egui::Color32 {
    ds::inactive_bg()
}
fn card_hover_bg() -> egui::Color32 {
    ds::hover_bg()
}
fn card_border() -> egui::Color32 {
    ds::border()
}

fn quality_label(tier: QualityTier) -> &'static str {
    let s = crate::strings::current();
    match tier {
        QualityTier::Potato => s.quality_potato,
        QualityTier::Low => s.quality_low,
        QualityTier::Medium => s.quality_medium,
        QualityTier::High => s.quality_high,
    }
}

fn theme_label(theme: Theme) -> &'static str {
    let s = crate::strings::current();
    match theme {
        Theme::Light => s.theme_light,
        Theme::Dark => s.theme_dark,
        Theme::Purple => s.theme_purple,
    }
}

fn colorblind_label(mode: ColorblindMode) -> &'static str {
    let s = crate::strings::current();
    match mode {
        ColorblindMode::Off => s.colorblind_off,
        ColorblindMode::Deuteranopia => s.colorblind_deuteranopia,
        ColorblindMode::Protanopia => s.colorblind_protanopia,
        ColorblindMode::Tritanopia => s.colorblind_tritanopia,
    }
}

const CARD_CORNER: egui::CornerRadius = ds::CORNER_RADIUS;
/// `saves::SLOT_COUNT` as a `usize`, for fixed-size array types below (array
/// lengths need a `usize`; `saves::SLOT_COUNT` is a `u8` so callers stay
/// consistent with the rest of that module's slot-numbering type).
const SAVE_SLOT_COUNT: usize = saves::SLOT_COUNT as usize;

/// Rolling toast log (art-direction: HUD "toast log"). Capped so it can't
/// grow unbounded over a long session.
#[derive(Resource, Default)]
pub struct ToastLog(pub Vec<(String, ToastTone)>);

/// Hard cap on [`ToastLog`] length. Every push path must trim to this —
/// use [`ToastLog::push`] rather than writing `toasts.0` directly.
pub const TOAST_LOG_CAP: usize = 20;

impl ToastLog {
    /// Append a toast and drain from the front so the log never exceeds
    /// [`TOAST_LOG_CAP`]. The single entry point for every toast producer.
    pub fn push(&mut self, message: String, tone: ToastTone) {
        self.0.push((message, tone));
        if self.0.len() > TOAST_LOG_CAP {
            let excess = self.0.len() - TOAST_LOG_CAP;
            self.0.drain(0..excess);
        }
    }
}

pub struct MfHudPlugin;

impl Plugin for MfHudPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<ToastLog>()
            .init_resource::<EguiStyleApplied>()
            .add_systems(Update, collect_toasts_system)
            // Font/visuals install runs ungated every pass; it flips
            // `EguiStyleApplied::ready` only once the custom display family
            // has actually gone live (bevy_egui applies `set_fonts` on the
            // *next* frame, so painting a `FontFamily::Name("mf_display")`
            // heading on the install frame would panic). The HUD paint chain
            // below is gated on that readiness so the first rendered frame
            // never touches an unbound font family.
            .add_systems(EguiPrimaryContextPass, setup_egui_style_system)
            .add_systems(
                EguiPrimaryContextPass,
                (
                    // apply_ui_scale_system runs first so egui's
                    // pixels_per_point (driven by scale_factor) is correct
                    // before setup_egui_style_system and all widget paints.
                    apply_ui_scale_system,
                    ui_gallery_system.run_if(ds::ui_gallery_enabled),
                    connecting_hud_system
                        .run_if(in_state(AppState::ConnectingSim))
                        .run_if(|| !ds::ui_gallery_enabled()),
                    main_menu_hud_system
                        .run_if(in_state(AppState::MainMenu))
                        .run_if(|| !ds::ui_gallery_enabled()),
                    loading_hud_system
                        .run_if(in_state(AppState::Loading))
                        .run_if(|| !ds::ui_gallery_enabled()),
                    in_game_hud_system
                        .run_if(in_state(AppState::InGame))
                        .run_if(|| !ds::ui_gallery_enabled())
                        .run_if(crate::egui_idle::egui_content_active),
                    pause_overlay_system
                        .run_if(in_state(AppState::InGame))
                        .run_if(|| !ds::ui_gallery_enabled())
                        .run_if(crate::egui_idle::egui_content_active),
                    reconnect_overlay_system
                        .run_if(in_state(AppState::InGame))
                        .run_if(|| !ds::ui_gallery_enabled()),
                    sim_error_screen_system
                        .run_if(in_state(AppState::SimError))
                        .run_if(|| !ds::ui_gallery_enabled()),
                    fatal_banner_system.run_if(|| !ds::ui_gallery_enabled()),
                )
                    .chain()
                    .after(setup_egui_style_system)
                    .run_if(|| !ds::hud_hidden())
                    .run_if(chrome_ready),
            );
    }
}

/// True once [`setup_egui_style_system`] has installed the custom fonts and
/// they have gone live (one frame after the install). Gates the HUD paint
/// chain so no menu frame lays out text with an unbound font family.
fn chrome_ready(applied: Res<EguiStyleApplied>) -> bool {
    applied.ready
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
struct EguiStyleApplied {
    theme: Option<Theme>,
    /// Set true the first frame *after* the fonts were installed, when the
    /// custom display family is actually bound and safe to lay out.
    ready: bool,
}

fn setup_egui_style_system(
    mut contexts: EguiContexts,
    theme: Res<Theme>,
    mut applied: ResMut<EguiStyleApplied>,
) {
    if applied.theme == Some(*theme) {
        // Fonts installed on an earlier frame are now live. Once ready, it
        // stays ready across theme changes (the family remains bound), so a
        // later re-install never blacks out or panics the HUD.
        applied.ready = true;
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    ds::install_fonts_and_visuals(ctx, *theme);
    applied.theme = Some(*theme);
}

fn ui_gallery_system(mut contexts: EguiContexts) -> Result {
    let ctx = contexts.ctx_mut()?;
    ds::show_gallery(ctx);
    Ok(())
}

/// Applies `config.ui_scale` to `EguiContextSettings::scale_factor`, which
/// bevy_egui 0.36 multiplies into `pixels_per_point` before every frame.
/// Runs first in the EguiPrimaryContextPass chain so every widget paint
/// this frame uses the updated scale.
fn apply_ui_scale_system(
    config: Res<MfConfig>,
    mut egui_settings: Query<&mut EguiContextSettings>,
) {
    for mut settings in &mut egui_settings {
        if (settings.scale_factor - config.ui_scale).abs() > f32::EPSILON {
            settings.scale_factor = config.ui_scale;
        }
    }
}

fn collect_toasts_system(mut events: EventReader<SimEvent>, mut log: ResMut<ToastLog>) {
    for SimEvent(msg) in events.read() {
        if let FromSimMsg::Json(FromSimJson::Toast(toast)) = msg {
            log.push(toast.message.clone(), toast.tone);
        }
    }
}

/// A muted group divider between items in the horizontal top bar. Must be a
/// VERTICAL rule: the top bar lays out left-to-right, and the full-width
/// horizontal `ds::thin_separator` grabbed the whole remaining row width,
/// pushing the clock/speed/view/goals controls off-screen (they rendered as an
/// empty bar with only the cash readout). Uses `ds::vertical_divider`.
pub(crate) fn thin_separator(ui: &mut egui::Ui) {
    ds::vertical_divider(ui);
}

/// One hover tick the first frame the pointer lands on a widget; re-arms
/// when it leaves, so re-entering the same widget ticks again. `last` is
/// per-system `Local` state, so two systems can't fight over it.
pub(crate) fn hover_tick(
    resp: &egui::Response,
    last: &mut Option<egui::Id>,
    sfx: &mut EventWriter<PlaySfx>,
) {
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

pub(crate) fn format_cash(value: f64) -> String {
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

/// The tier rows shared by every quality combo box (Settings / pause).
/// Settings owns the only quality UI now — the in-game top bar no longer
/// duplicates these controls (design audit: play HUD is for play).
/// "Auto" clears `quality_override` and re-applies the boot-time GPU detect.
fn quality_options(
    ui: &mut egui::Ui,
    quality: &mut QualityTier,
    config: &mut MfConfig,
    detected: QualityTier,
    sfx: &mut EventWriter<PlaySfx>,
) {
    let auto_selected = config.quality_override.is_none();
    let auto_label = format!("Auto ({})", detected.label());
    if ui
        .selectable_label(auto_selected, auto_label)
        .on_hover_text("Use GPU auto-detect (default). Clears any saved quality override.")
        .clicked()
    {
        *quality = detected;
        config.set_quality_override(None);
        sfx.write(PlaySfx(Sfx::Confirm));
    }
    for tier in [
        QualityTier::Potato,
        QualityTier::Low,
        QualityTier::Medium,
        QualityTier::High,
    ] {
        if ui
            .selectable_label(!auto_selected && *quality == tier, quality_label(tier))
            .clicked()
        {
            *quality = tier;
            config.set_quality_override(Some(tier));
            sfx.write(PlaySfx(Sfx::Confirm));
        }
    }
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
            .selectable_label(*theme == candidate, theme_label(candidate))
            .clicked()
        {
            *theme = candidate;
            config.set_theme_override(Some(candidate));
            sfx.write(PlaySfx(Sfx::Confirm));
        }
    }
}

/// Persist Advanced deltas into both the live resource and config.toml.
fn apply_graphics_overrides(
    overrides: &mut QualityOverrides,
    config: &mut MfConfig,
    next: QualityOverrides,
) {
    *overrides = next;
    config.set_graphics_overrides(next);
}

/// Bundles the Settings-screen resources so callers stay under Bevy's
/// 16-param system limit (adding weather alone pushed `main_menu_hud_system`
/// over).
#[derive(SystemParam)]
struct SettingsControls<'w> {
    quality: ResMut<'w, QualityTier>,
    theme: ResMut<'w, Theme>,
    weather: ResMut<'w, WeatherEffects>,
    config: ResMut<'w, MfConfig>,
    overrides: ResMut<'w, QualityOverrides>,
    effective: Res<'w, EffectiveKnobs>,
    detected: Res<'w, DetectedQuality>,
    show_fps: ResMut<'w, ShowFps>,
    benchmark: ResMut<'w, GraphicsBenchmark>,
    colorblind: ResMut<'w, ColorblindMode>,
}

/// ConnectingSim previously registered NO ui system at all, so a player whose
/// sidecar was slow (or repeatedly failing) stared at a bare ClearColor with
/// zero feedback until the fatal banner eventually appeared. Every app state
/// must draw *something*.
fn connecting_hud_system(mut contexts: EguiContexts, reconnect: Res<ReconnectState>) -> Result {
    let s = crate::strings::current();
    let ctx = contexts.ctx_mut()?;
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::TRANSPARENT))
        .show(ctx, |ui| {
            crate::design_system::paint_menu_gradient_scrim(ui.painter(), ui.max_rect());
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.28).max(24.0));
                draw_logo(ui, 56.0);
                ui.add_space(ds::SPACE_MD);
                ui.label(ds::heading(s.brand));
                ui.add_space(ds::SPACE_SM);
                match &reconnect.status {
                    NetStatus::Fatal(diag) => {
                        ui.colored_label(BAD, s.could_not_start_sim(&diag.message));
                    }
                    NetStatus::Reconnecting { attempt, .. } => {
                        ui.label(
                            egui::RichText::new(
                                s.starting_simulation_attempt(*attempt, MAX_ATTEMPTS),
                            )
                            .color(muted_text()),
                        );
                    }
                    NetStatus::Connected => {
                        ui.label(egui::RichText::new(s.starting_simulation).color(muted_text()));
                    }
                }
            });
        });
    Ok(())
}

/// Label for a field row: small and muted, stacked above its control
/// (chosen over right-aligned labels — with three rows of differing
/// natural label width, stacked keeps every control's left edge aligned
/// without hand-tuning a label column width).
pub(crate) fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(crate::design_system::label_small(text));
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
pub(crate) fn draw_star(
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

/// A small sun (day) or crescent moon (night) glyph for the top-bar clock,
/// painted from primitives so it needs no font glyph the embedded Inter
/// subset might lack. Day: filled amber disc with four short rays. Night: a
/// crescent, cut by overpainting an offset disc in the panel fill color.
fn draw_day_night_icon(painter: &egui::Painter, rect: egui::Rect, is_night: bool) {
    let center = rect.center();
    let r = rect.width().min(rect.height()) * 0.32;
    if is_night {
        let moon = egui::Color32::from_rgb(0xc7, 0xcb, 0xd6);
        painter.circle_filled(center, r, moon);
        // Bite out an offset disc in the panel color to leave a crescent.
        painter.circle_filled(
            center + egui::vec2(r * 0.55, -r * 0.2),
            r * 0.95,
            panel_bg(),
        );
    } else {
        painter.circle_filled(center, r * 0.72, WARN);
        for i in 0..4 {
            let a = i as f32 * std::f32::consts::FRAC_PI_2 + std::f32::consts::FRAC_PI_4;
            let dir = egui::vec2(a.cos(), a.sin());
            painter.line_segment(
                [center + dir * (r * 0.95), center + dir * (r * 1.35)],
                egui::Stroke::new(1.4, WARN),
            );
        }
    }
}

/// True when `hour` (0..24) falls in the night band the day/night rig treats
/// as dark. Matches the sky's dusk/dawn feel: lamps on from 20:00 to 06:00.
fn is_night_hour(hour: f64) -> bool {
    !(6.0..20.0).contains(&hour)
}

/// A simple padlock silhouette (filled body + stroked shackle arc), sized
/// for a locked city card. Deliberately separate from `build_ui.rs`'s
/// smaller per-toolbar-button lock badge (that file isn't touched this
/// wave) rather than shared, since the two are drawn at different scales
/// for different contexts.
pub(crate) fn draw_lock(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
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
    let s = crate::strings::current();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(saved_at_epoch_secs);
    let elapsed = now.saturating_sub(saved_at_epoch_secs);
    if elapsed < 60 {
        s.just_now.to_string()
    } else if elapsed < 3600 {
        s.relative_minutes_ago(elapsed / 60)
    } else if elapsed < 86_400 {
        s.relative_hours_ago(elapsed / 3600)
    } else {
        s.relative_days_ago(elapsed / 86_400)
    }
}

pub(crate) fn format_playtime(secs: u64) -> String {
    let s = crate::strings::current();
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    if hours > 0 {
        s.playtime_hm(hours, mins)
    } else {
        s.playtime_m(mins)
    }
}

/// Fallback display label for a `CITY_ORDER` key that isn't (yet) present
/// in the sidecar's `hello.city_list` — capitalizes the raw key ("dc" ->
/// "Dc") rather than showing the wire-protocol identifier verbatim.
pub(crate) fn capitalize(key: &str) -> String {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// One row in the save browser / Continue section: an autosave or numbered
/// slot, showing city / sim day / network size / playtime / timestamp when
/// occupied, or "Empty" otherwise. Returns whether an occupied row was
/// clicked.
fn continue_slot_row(
    ui: &mut egui::Ui,
    width: f32,
    slot: SaveSlot,
    meta: Option<&saves::SaveMeta>,
    hovered: &mut Option<egui::Id>,
    sfx: &mut EventWriter<PlaySfx>,
) -> bool {
    let s = crate::strings::current();
    let title = slot.label();
    let occupied = meta.is_some();
    let sense = if occupied {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let row_height = if occupied { 56.0 } else { 40.0 };
    let size = egui::vec2(width, row_height);
    let (rect, response) = ui.allocate_exact_size(size, sense);
    if occupied {
        hover_tick(&response, hovered, sfx);
    }

    let painter = ui.painter_at(rect);
    let bg = if occupied && response.hovered() {
        card_hover_bg()
    } else {
        card_bg()
    };
    painter.rect_filled(rect, CARD_CORNER, bg);
    painter.rect_stroke(
        rect,
        CARD_CORNER,
        egui::Stroke::new(1.0, card_border()),
        egui::StrokeKind::Inside,
    );

    // Cheap thumbnail affordance: a small color chip when a PNG is present,
    // otherwise a muted placeholder block so the layout stays stable.
    let thumb_size = egui::vec2(28.0, 28.0);
    let thumb_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 10.0, rect.center().y - thumb_size.y * 0.5),
        thumb_size,
    );
    if let Some(meta) = meta {
        if meta.thumbnail_png_base64.is_some() {
            painter.rect_filled(thumb_rect, egui::CornerRadius::same(2), accent());
        } else {
            painter.rect_filled(
                thumb_rect,
                egui::CornerRadius::same(2),
                card_border().gamma_multiply(0.45),
            );
        }
    }

    let content_left = if occupied {
        thumb_rect.right() + 10.0
    } else {
        rect.left() + 12.0
    };
    let content_rect = egui::Rect::from_min_max(
        egui::pos2(content_left, rect.top() + 6.0),
        egui::pos2(rect.right() - 12.0, rect.bottom() - 6.0),
    );
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
                .color(if occupied { text_color() } else { muted_text() }),
        );
        if let Some(meta) = meta {
            ui.label(
                egui::RichText::new(format_relative_time(meta.saved_at_epoch_secs))
                    .size(11.0)
                    .color(muted_text()),
            );
        }
    });
    let subtitle = match meta {
        Some(meta) => {
            let city = meta
                .city_label
                .clone()
                .unwrap_or_else(|| s.unknown_city.to_string());
            s.save_subtitle(
                &city,
                meta.day,
                meta.network_size as usize,
                &format_playtime(meta.playtime_secs),
            )
        }
        None => s.empty.to_string(),
    };
    child.label(egui::RichText::new(subtitle).size(11.0).color(muted_text()));
    if let Some(meta) = meta {
        child.label(
            egui::RichText::new(format_cash(meta.cash))
                .size(11.0)
                .color(muted_text()),
        );
    }

    occupied && response.clicked()
}

#[allow(clippy::too_many_arguments)]
fn main_menu_hud_system(
    contexts: EguiContexts,
    hello: Res<SimHello>,
    progress: Res<CampaignProgress>,
    pending: ResMut<PendingInit>,
    settings: SettingsControls,
    save_manager: ResMut<SaveManager>,
    toasts: ResMut<ToastLog>,
    state: Res<State<AppState>>,
    next_state: ResMut<NextState<AppState>>,
    screen: ResMut<MenuScreen>,
    sfx: EventWriter<PlaySfx>,
    exit: EventWriter<AppExit>,
    playtime: ResMut<PlaytimeTracker>,
    hovered: Local<Option<egui::Id>>,
    city_select: Local<crate::city_select::CitySelectLocals>,
    rigs: Query<&mut CameraRig>,
) -> Result {
    // Capture before settings is consumed in the Settings branch.
    let reduce_motion = settings.config.reduce_motion;
    match *screen {
        MenuScreen::Title => title_screen_ui(contexts, screen, exit, sfx, hovered, reduce_motion)?,
        MenuScreen::CitySelect => crate::city_select::city_select_screen_ui(
            contexts,
            hello,
            progress,
            pending,
            save_manager,
            toasts,
            state,
            next_state,
            screen,
            sfx,
            playtime,
            hovered,
            city_select,
        )?,
        MenuScreen::LoadGame => load_game_screen_ui(
            contexts,
            save_manager,
            toasts,
            next_state,
            screen,
            sfx,
            hovered,
            city_select,
        )?,
        MenuScreen::Settings => {
            let mut screen = screen;
            // Title-screen Settings: no live `TutorialState` needed — clearing
            // the persisted flag (the Replay button does that) re-arms the flow
            // on the next city load, which is the only way to reach `InGame`
            // from here anyway.
            if settings_screen_ui(contexts, settings, None, rigs, sfx, hovered)? {
                *screen = MenuScreen::Title;
            }
        }
    }
    Ok(())
}

/// Website wordmark (`https://` storefront's `<link rel="icon">` inline
/// SVG: dark rounded-rect badge, four colored spokes meeting a ringed
/// center circle — see `metroforge/index.html`'s favicon data URI). Drawn
/// with the egui painter rather than shipping a rasterized PNG: it's four
/// strokes and a circle, so reproducing the exact same geometry/colors as
/// vector primitives is pixel-faithful to the source SVG without adding an
/// image-loading dependency (no `image`/`egui_extras` crate wired into
/// this workspace yet) just for one static mark.
pub(crate) fn draw_logo(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let bg = egui::Color32::from_rgb(0x0b, 0x0d, 0x10);
    painter.rect_filled(rect, egui::CornerRadius::same((size * 0.22) as u8), bg);

    let center = rect.center();
    let arm = size * 0.35;
    let stroke_w = size * 0.1;
    let corners = [
        (
            egui::vec2(-arm, -arm),
            egui::Color32::from_rgb(0x7e, 0xf2, 0x9a),
        ),
        (
            egui::vec2(arm, -arm),
            egui::Color32::from_rgb(0x54, 0xd0, 0xff),
        ),
        (
            egui::vec2(arm, arm),
            egui::Color32::from_rgb(0xff, 0xb6, 0x3d),
        ),
        (
            egui::vec2(-arm, arm),
            egui::Color32::from_rgb(0xff, 0x5d, 0x6c),
        ),
    ];
    for (offset, color) in corners {
        painter.line_segment(
            [center, center + offset],
            egui::Stroke::new(stroke_w, color),
        );
    }
    let hub_r = size * 0.15;
    painter.circle_filled(center, hub_r, bg);
    painter.circle_stroke(
        center,
        hub_r,
        egui::Stroke::new(size * 0.075, egui::Color32::from_rgb(0xf4, 0xf4, 0xf5)),
    );
}

/// Title screen: full-bleed diorama, large wordmark, left-anchored menu
/// column of borderless text buttons (Play / Load Game / Settings / Quit)
/// with accent-bar hover, version in the bottom corner. City picking, the
/// save browser, and options each live one click away.
fn title_screen_ui(
    mut contexts: EguiContexts,
    mut screen: ResMut<MenuScreen>,
    mut exit: EventWriter<AppExit>,
    mut sfx: EventWriter<PlaySfx>,
    mut hovered: Local<Option<egui::Id>>,
    reduce_motion: bool,
) -> Result {
    let s = crate::strings::current();
    let ctx = contexts.ctx_mut()?;
    let fade = if reduce_motion {
        1.0
    } else {
        ds::animate(ctx, egui::Id::new("title_fade"), 1.0)
    };

    // Transparent central panel: the attract-mode diorama is the brand
    // surface. A horizontal gradient scrim keeps the menu column readable
    // without milking out the city on the far side. Version is painted in
    // the bottom-right corner below rather than in its own bottom bar.
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::TRANSPARENT))
        .show(ctx, |ui| {
            crate::design_system::paint_menu_gradient_scrim(ui.painter(), ui.max_rect());
            ui.set_opacity(fade);
            let screen_rect = ui.max_rect();

            // Version, bottom-right corner.
            ui.painter().text(
                egui::pos2(
                    screen_rect.right() - ds::SPACE_MD,
                    screen_rect.bottom() - ds::SPACE_MD,
                ),
                egui::Align2::RIGHT_BOTTOM,
                format!("v{}", env!("CARGO_PKG_VERSION")),
                ds::body_font(ds::TEXT_XS),
                muted_text(),
            );

            // Left-anchored column: wordmark + menu.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                ui.add_space(ds::SPACE_XXL);
                ui.vertical(|ui| {
                    // Wide enough that the 72px Oswald wordmark stays on one
                    // line instead of wrapping ("MetroForg" / "e").
                    ui.set_width(460.0);
                    ui.add_space(ui.available_height() * 0.18);
                    ui.horizontal(|ui| {
                        draw_logo(ui, 56.0);
                    });
                    ui.add_space(ds::SPACE_MD);
                    ui.label(ds::wordmark(s.brand));
                    ui.add_space(ds::SPACE_XS);
                    ui.label(
                        egui::RichText::new(s.tagline)
                            .size(ds::TEXT_MD)
                            .color(muted_text()),
                    );
                    ui.add_space(ds::SPACE_XL);

                    let play = ds::menu_text_button(ui, s.play);
                    hover_tick(&play, &mut hovered, &mut sfx);
                    if play.clicked() {
                        sfx.write(PlaySfx(Sfx::Confirm));
                        *screen = MenuScreen::CitySelect;
                    }
                    let load = ds::menu_text_button(ui, s.load_game);
                    hover_tick(&load, &mut hovered, &mut sfx);
                    if load.clicked() {
                        sfx.write(PlaySfx(Sfx::Confirm));
                        *screen = MenuScreen::LoadGame;
                    }
                    let settings = ds::menu_text_button(ui, s.settings);
                    hover_tick(&settings, &mut hovered, &mut sfx);
                    if settings.clicked() {
                        sfx.write(PlaySfx(Sfx::Confirm));
                        *screen = MenuScreen::Settings;
                    }
                    let quit = ds::menu_text_button(ui, s.quit);
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

/// Title-screen save browser: every autosave ring entry + numbered slot
/// with city / sim day / network size / playtime / relative timestamp.
#[allow(clippy::too_many_arguments)]
fn load_game_screen_ui(
    mut contexts: EguiContexts,
    mut save_manager: ResMut<SaveManager>,
    mut toasts: ResMut<ToastLog>,
    mut next_state: ResMut<NextState<AppState>>,
    mut screen: ResMut<MenuScreen>,
    mut sfx: EventWriter<PlaySfx>,
    mut hovered: Local<Option<egui::Id>>,
    mut city_select: Local<crate::city_select::CitySelectLocals>,
) -> Result {
    let s = crate::strings::current();
    if city_select.slots_cache.is_none() {
        city_select.slots_cache = Some(saves::list());
    }
    // Refresh when re-entering this screen from Title.
    if screen.is_changed() {
        city_select.slots_cache = Some(saves::list());
    }
    let slots = city_select
        .slots_cache
        .as_ref()
        .expect("populated just above");

    let ctx = contexts.ctx_mut()?;
    let fade = ds::animate(ctx, egui::Id::new("load_game_fade"), 1.0);
    let mut go_back = false;
    let mut load_slot: Option<SaveSlot> = None;

    egui::TopBottomPanel::bottom("load_game_back")
        .frame(
            egui::Frame::default()
                .fill(panel_bg())
                .inner_margin(egui::Margin::symmetric(16, 14)),
        )
        .show_separator_line(false)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            ui.vertical_centered(|ui| {
                let back = ui.add_sized(
                    [220.0, 40.0],
                    egui::Button::new(egui::RichText::new(s.back).size(14.0))
                        .corner_radius(crate::design_system::CORNER_RADIUS),
                );
                hover_tick(&back, &mut hovered, &mut sfx);
                if back.clicked() {
                    go_back = true;
                }
            });
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(crate::design_system::menu_wash()))
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            ui.vertical_centered(|ui| {
                ui.add_space(crate::design_system::SPACE_LG);
                draw_logo(ui, 48.0);
                ui.add_space(crate::design_system::SPACE_SM);
                ui.label(crate::design_system::heading(s.load_game).color(text_color()));
                ui.add_space(crate::design_system::SPACE_XS);
                ui.label(
                    egui::RichText::new(s.pick_slot_to_continue)
                        .size(crate::design_system::TEXT_SM)
                        .color(muted_text()),
                );
                ui.add_space(crate::design_system::SPACE_MD);

                egui::ScrollArea::vertical()
                    .max_height(ui.available_height() - 24.0)
                    .show(ui, |ui| {
                        ui.set_width(480.0);
                        field_label(ui, s.autosaves);
                        ui.add_space(4.0);
                        for entry in slots
                            .iter()
                            .filter(|e| matches!(e.slot, SaveSlot::Autosave(_)))
                        {
                            let clicked = continue_slot_row(
                                ui,
                                460.0,
                                entry.slot,
                                entry.meta.as_ref(),
                                &mut hovered,
                                &mut sfx,
                            );
                            ui.add_space(6.0);
                            if clicked {
                                load_slot = Some(entry.slot);
                            }
                        }
                        ui.add_space(12.0);
                        field_label(ui, s.manual_slots);
                        ui.add_space(4.0);
                        for entry in slots.iter().filter(|e| matches!(e.slot, SaveSlot::Slot(_))) {
                            let clicked = continue_slot_row(
                                ui,
                                460.0,
                                entry.slot,
                                entry.meta.as_ref(),
                                &mut hovered,
                                &mut sfx,
                            );
                            ui.add_space(6.0);
                            if clicked {
                                load_slot = Some(entry.slot);
                            }
                        }
                        ui.add_space(20.0);
                    });
            });
        });

    if let Some(slot) = load_slot {
        if save_manager.load(slot, &mut toasts, &mut sfx).is_some() {
            next_state.set(AppState::Loading);
        }
    }
    if go_back {
        sfx.write(PlaySfx(Sfx::Cancel));
        city_select.slots_cache = None;
        *screen = MenuScreen::Title;
    }
    Ok(())
}

/// Settings screen: quality, theme, weather, autosave cadence, UI scale,
/// colorblind mode, reduce motion — the overrides `config.rs` persists to
/// `config.toml`. Shared verbatim by the title screen's Settings button and
/// the in-game pause menu's Settings button — both call sites own what
/// "Back" means for them (return to `MenuScreen::Title` vs. close the
/// pause-menu settings panel) via this function's `bool` return (true ==
/// Back was clicked this frame).
#[allow(clippy::too_many_arguments)]
fn settings_screen_ui(
    mut contexts: EguiContexts,
    mut settings: SettingsControls,
    // `None` on the title screen (no live flow to restart), `Some` from the
    // in-game pause menu (so Replay restarts the flow immediately).
    mut tutorial: Option<ResMut<crate::tutorial::TutorialState>>,
    mut rigs: Query<&mut CameraRig>,
    mut sfx: EventWriter<PlaySfx>,
    mut hovered: Local<Option<egui::Id>>,
) -> Result<bool> {
    let s = crate::strings::current();
    let mut back_clicked = false;
    let ctx = contexts.ctx_mut()?;
    let fade = if settings.config.reduce_motion {
        1.0
    } else {
        ds::animate(ctx, egui::Id::new("settings_fade"), 1.0)
    };

    ds::modal(ctx, egui::Id::new("settings_modal"), fade, |ui| {
        ui.set_width(320.0);
        ui.vertical_centered(|ui| {
            draw_logo(ui, 36.0);
            ui.add_space(6.0);
            ui.label(ds::heading(s.settings));
            ui.add_space(24.0);

            field_label(ui, s.quality);
            egui::ComboBox::from_id_salt("settings_quality")
                .selected_text(settings.quality.label())
                .width(300.0)
                .show_ui(ui, |ui| {
                    quality_options(
                        ui,
                        &mut settings.quality,
                        &mut settings.config,
                        settings.detected.0,
                        &mut sfx,
                    )
                });

            ui.add_space(14.0);
            field_label(ui, s.theme);
            egui::ComboBox::from_id_salt("settings_theme")
                .selected_text(settings.theme.label())
                .width(300.0)
                .show_ui(ui, |ui| {
                    theme_options(ui, &mut settings.theme, &mut settings.config, &mut sfx)
                });

            ui.add_space(14.0);
            field_label(ui, s.ui_scale);
            let mut scale = settings.config.ui_scale;
            let scale_text = format!("{scale:.2}x");
            if ui
                .add(
                    egui::Slider::new(
                        &mut scale,
                        crate::config::UI_SCALE_MIN..=crate::config::UI_SCALE_MAX,
                    )
                    .text(scale_text),
                )
                .changed()
            {
                settings.config.set_ui_scale(scale);
                sfx.write(PlaySfx(Sfx::Confirm));
            }

            field_label(ui, s.camera_sensitivity);
            let mut cam = settings.config.camera_sensitivity;
            let cam_text = format!("{cam:.2}x");
            if ui
                .add(
                    egui::Slider::new(
                        &mut cam,
                        crate::config::CAMERA_SENS_MIN..=crate::config::CAMERA_SENS_MAX,
                    )
                    .text(cam_text),
                )
                .changed()
            {
                settings.config.set_camera_sensitivity(cam);
                sfx.write(PlaySfx(Sfx::Confirm));
            }

            ui.add_space(14.0);
            field_label(ui, s.colorblind);
            let cur_mode = *settings.colorblind;
            egui::ComboBox::from_id_salt("settings_colorblind")
                .selected_text(colorblind_label(cur_mode))
                .width(300.0)
                .show_ui(ui, |ui| {
                    for mode in ColorblindMode::ALL {
                        if ui
                            .selectable_label(cur_mode == mode, colorblind_label(mode))
                            .clicked()
                            && cur_mode != mode
                        {
                            settings.config.set_colorblind(mode);
                            *settings.colorblind = mode;
                            sfx.write(PlaySfx(Sfx::Confirm));
                        }
                    }
                });

            ui.add_space(14.0);
            let mut rm = settings.config.reduce_motion;
            if ui.checkbox(&mut rm, s.reduce_motion).changed() {
                settings.config.set_reduce_motion(rm);
                sfx.write(PlaySfx(Sfx::Confirm));
            }
            ui.label(
                egui::RichText::new(s.reduce_motion_hint)
                    .size(ds::TEXT_XS)
                    .color(muted_text()),
            );

            ui.add_space(14.0);
            field_label(ui, s.weather);
            let tier_allows = settings.quality.knobs().atmosphere_enabled;
            ui.add_enabled_ui(tier_allows, |ui| {
                let mut enabled = settings.weather.enabled;
                let label = if tier_allows {
                    s.fog_and_clouds
                } else {
                    s.fog_and_clouds_gated
                };
                if ui.checkbox(&mut enabled, label).changed() {
                    settings.weather.enabled = enabled;
                    settings.config.set_weather_effects(enabled);
                    sfx.write(PlaySfx(Sfx::Confirm));
                }
            });

            ui.add_space(14.0);
            field_label(ui, s.autosave);
            let interval_label = match settings.config.autosave_interval_days {
                0 => s.off.to_string(),
                n => s.every_n_sim_days(n),
            };
            egui::ComboBox::from_id_salt("settings_autosave")
                .selected_text(interval_label)
                .width(300.0)
                .show_ui(ui, |ui| {
                    for days in [0_u32, 5, 10, 20, 30] {
                        let label = if days == 0 {
                            s.off.to_string()
                        } else {
                            s.every_n_sim_days(days)
                        };
                        let selected = settings.config.autosave_interval_days == days;
                        if ui.selectable_label(selected, label).clicked()
                            && settings.config.autosave_interval_days != days
                        {
                            settings.config.set_autosave_interval_days(days);
                            sfx.write(PlaySfx(Sfx::Confirm));
                        }
                    }
                });
            ui.label(
                egui::RichText::new(s.autosave_ring_hint)
                    .size(ds::TEXT_XS)
                    .color(muted_text()),
            );

            ui.add_space(14.0);
            field_label(ui, "Audio");
            let mut mute = settings.config.mute;
            if ui.checkbox(&mut mute, "Mute").changed() {
                settings.config.set_mute(mute);
                if !mute {
                    sfx.write(PlaySfx(Sfx::Confirm));
                }
            }
            ui.add_enabled_ui(!settings.config.mute, |ui| {
                let mut volume = settings.config.master_volume;
                let slider = ui.add(
                    egui::Slider::new(&mut volume, 0.0..=1.0)
                        .text("Master volume")
                        .show_value(true),
                );
                if slider.changed() {
                    settings.config.set_master_volume(volume);
                }
                if slider.drag_stopped() && !settings.config.mute {
                    sfx.write(PlaySfx(Sfx::Confirm));
                }
            });

            ui.add_space(14.0);
            egui::ScrollArea::vertical()
                .max_height(ui.ctx().screen_rect().height() * 0.45)
                .show(ui, |ui| {
                    ui.set_width(320.0);
                    advanced_settings_ui(ui, &mut settings, &mut rigs, &mut sfx, &mut hovered);
                });

            ui.add_space(28.0);
            let replay = ds::button_sized(
                ui,
                s.replay_tutorial,
                ds::ButtonKind::Ghost,
                Some(egui::vec2(220.0, 36.0)),
            );
            hover_tick(&replay, &mut hovered, &mut sfx);
            if replay.clicked() {
                sfx.write(PlaySfx(Sfx::Confirm));
                settings.config.set_tutorial_completed(false);
                if let Some(tutorial) = tutorial.as_mut() {
                    tutorial.request_replay();
                }
            }

            ui.add_space(10.0);
            let back = ds::button_sized(
                ui,
                s.back,
                ds::ButtonKind::Ghost,
                Some(egui::vec2(220.0, 40.0)),
            );
            hover_tick(&back, &mut hovered, &mut sfx);
            if back.clicked() {
                sfx.write(PlaySfx(Sfx::Cancel));
                back_clicked = true;
            }
        });
    });
    Ok(back_clicked)
}

/// Advanced graphics deltas + performance (FPS counter / benchmark) section
/// of the Settings screen. Quality/Theme/etc live in `settings_screen_ui`.
fn advanced_settings_ui(
    ui: &mut egui::Ui,
    settings: &mut SettingsControls,
    rigs: &mut Query<&mut CameraRig>,
    sfx: &mut EventWriter<PlaySfx>,
    hovered: &mut Option<egui::Id>,
) {
    let preset = *settings.quality;
    let preset_knobs = preset.knobs();
    let effective = settings.effective.0;

    ui.add_space(4.0);
    ui.add_space(18.0);
    ui.label(
        egui::RichText::new("Advanced")
            .size(15.0)
            .color(text_color())
            .strong(),
    );
    ui.label(
        egui::RichText::new("Overrides apply instantly on top of the selected preset.")
            .size(11.0)
            .color(muted_text()),
    );
    ui.add_space(8.0);

    // --- Shadows ---
    field_label(ui, "Shadows");
    let shadow_eff = ShadowQuality::from_map_size(effective.shadow_map_size);
    egui::ComboBox::from_id_salt("settings_shadows")
        .selected_text(shadow_eff.label())
        .width(320.0)
        .show_ui(ui, |ui| {
            for q in [
                ShadowQuality::Off,
                ShadowQuality::Medium,
                ShadowQuality::High,
            ] {
                let selected = shadow_eff == q;
                if ui.selectable_label(selected, q.label()).clicked() && !selected {
                    let mut next = *settings.overrides;
                    next.shadows =
                        if q == ShadowQuality::from_map_size(preset_knobs.shadow_map_size) {
                            None
                        } else {
                            Some(q)
                        };
                    apply_graphics_overrides(&mut settings.overrides, &mut settings.config, next);
                    sfx.write(PlaySfx(Sfx::Confirm));
                }
            }
        });

    ui.add_space(10.0);
    // --- Draw distance ---
    field_label(ui, "Draw distance");
    let mut draw_m = effective
        .building_draw_distance_m
        .unwrap_or(DRAW_DISTANCE_UNLIMITED_M);
    let draw_label = if effective.building_draw_distance_m.is_none() {
        "Unlimited".to_string()
    } else {
        format!("{:.0} m", draw_m)
    };
    ui.label(
        egui::RichText::new(draw_label)
            .size(12.0)
            .color(muted_text()),
    );
    let draw_response = ui.add(egui::Slider::new(
        &mut draw_m,
        DRAW_DISTANCE_MIN_M..=DRAW_DISTANCE_UNLIMITED_M,
    ));
    if draw_response.changed() {
        let mut next = *settings.overrides;
        let preset_m = preset_knobs
            .building_draw_distance_m
            .unwrap_or(DRAW_DISTANCE_UNLIMITED_M);
        next.draw_distance_m = if (draw_m - preset_m).abs() < 1.0 {
            None
        } else {
            Some(draw_m)
        };
        // Live apply every drag frame; persist only when the gesture ends
        // (or on a non-drag click) so we don't rewrite config.toml per pixel.
        *settings.overrides = next;
        if !draw_response.dragged() {
            settings.config.set_graphics_overrides(next);
        }
    }
    if draw_response.drag_stopped() {
        settings.config.set_graphics_overrides(*settings.overrides);
    }

    ui.add_space(10.0);
    // --- Trees (rebuilds park tree meshes) ---
    let mut trees = effective.tree_enabled;
    if ui
        .checkbox(&mut trees, "Trees")
        .on_hover_text("Toggling rebuilds park tree meshes (one frame).")
        .changed()
    {
        let mut next = *settings.overrides;
        next.trees = if trees == preset_knobs.tree_enabled {
            None
        } else {
            Some(trees)
        };
        apply_graphics_overrides(&mut settings.overrides, &mut settings.config, next);
        sfx.write(PlaySfx(Sfx::Confirm));
    }

    // --- Distance fog ---
    let mut fog_on = effective.fog.is_some();
    if ui
        .checkbox(&mut fog_on, "Distance fog")
        .on_hover_text("Masks draw-distance pop-in on weaker presets.")
        .changed()
    {
        let mut next = *settings.overrides;
        let preset_fog = preset_knobs.fog.is_some();
        next.fog = if fog_on == preset_fog {
            None
        } else {
            Some(fog_on)
        };
        apply_graphics_overrides(&mut settings.overrides, &mut settings.config, next);
        sfx.write(PlaySfx(Sfx::Confirm));
    }

    // --- Volumetric clouds ---
    let mut clouds = settings
        .overrides
        .volumetric_clouds
        .unwrap_or(preset_knobs.atmosphere_enabled && settings.weather.enabled);
    if ui
        .checkbox(&mut clouds, "Volumetric clouds")
        .on_hover_text(
            "Scrolling fog/cloud volumes. Enabling on weak presets also turns on Medium shadows (required).",
        )
        .changed()
    {
        let mut next = *settings.overrides;
        next.volumetric_clouds = if clouds == preset_knobs.atmosphere_enabled {
            None
        } else {
            Some(clouds)
        };
        if clouds {
            settings.weather.enabled = true;
            settings.config.set_weather_effects(true);
            if effective.shadow_map_size.is_none()
                && next.shadows.unwrap_or(ShadowQuality::from_map_size(
                    preset_knobs.shadow_map_size,
                )) == ShadowQuality::Off
            {
                next.shadows = Some(ShadowQuality::Medium);
            }
        } else {
            settings.weather.enabled = false;
            settings.config.set_weather_effects(false);
        }
        apply_graphics_overrides(&mut settings.overrides, &mut settings.config, next);
        sfx.write(PlaySfx(Sfx::Confirm));
    }

    // --- Outlines ---
    let mut outlines = effective.outline_enabled;
    if ui
        .checkbox(&mut outlines, "Building outlines")
        .on_hover_text("Cel-shading outline on the dense urban-core building chunk.")
        .changed()
    {
        let mut next = *settings.overrides;
        next.outlines = if outlines == preset_knobs.outline_enabled {
            None
        } else {
            Some(outlines)
        };
        apply_graphics_overrides(&mut settings.overrides, &mut settings.config, next);
        sfx.write(PlaySfx(Sfx::Confirm));
    }

    // --- VSync ---
    let mut vsync = effective.vsync;
    if ui.checkbox(&mut vsync, "VSync").changed() {
        let mut next = *settings.overrides;
        next.vsync = if vsync == preset_knobs.vsync {
            None
        } else {
            Some(vsync)
        };
        apply_graphics_overrides(&mut settings.overrides, &mut settings.config, next);
        sfx.write(PlaySfx(Sfx::Confirm));
    }

    ui.add_space(8.0);
    let has_deltas = !settings.overrides.is_empty();
    ui.add_enabled_ui(has_deltas, |ui| {
        let reset = ui.add_sized(
            [280.0, 32.0],
            egui::Button::new(egui::RichText::new("Reset to preset").size(13.0)),
        );
        hover_tick(&reset, hovered, sfx);
        if reset.clicked() {
            apply_graphics_overrides(
                &mut settings.overrides,
                &mut settings.config,
                QualityOverrides::default(),
            );
            settings.weather.enabled = settings.config.weather_effects;
            sfx.write(PlaySfx(Sfx::Confirm));
        }
    });

    ui.add_space(16.0);
    ui.label(
        egui::RichText::new("Performance")
            .size(15.0)
            .color(text_color())
            .strong(),
    );
    ui.add_space(6.0);

    let mut show_fps = settings.show_fps.0;
    if ui.checkbox(&mut show_fps, "Show FPS counter").changed() {
        settings.show_fps.0 = show_fps;
        settings.config.set_show_fps(show_fps);
        sfx.write(PlaySfx(Sfx::Confirm));
    }

    ui.add_space(8.0);
    let can_bench = rigs.single().is_ok() && !settings.benchmark.is_running();
    ui.add_enabled_ui(can_bench, |ui| {
        let label = if settings.benchmark.is_running() {
            format!("Benchmarking… ({BENCHMARK_DURATION_SECS:.0}s)")
        } else {
            format!("Run {BENCHMARK_DURATION_SECS:.0}s benchmark")
        };
        let bench_btn = ui.add_sized(
            [280.0, 34.0],
            egui::Button::new(egui::RichText::new(label).size(13.0)),
        );
        hover_tick(&bench_btn, hovered, sfx);
        if bench_btn.clicked() {
            graphics_perf::begin_benchmark(&mut settings.benchmark, rigs);
            sfx.write(PlaySfx(Sfx::Confirm));
        }
    });
    if rigs.single().is_err() {
        ui.label(
            egui::RichText::new("Benchmark needs a loaded city (pause → Settings).")
                .size(11.0)
                .color(muted_text()),
        );
    }

    if let Some(result) = settings.benchmark.result() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(format!(
                "Avg {:.1} ms · 1% low {:.1} ms",
                result.avg_ms, result.low_1pct_ms
            ))
            .size(12.0)
            .color(text_color()),
        );
        ui.label(
            egui::RichText::new(format!("Recommended: {}", result.recommended.label()))
                .size(13.0)
                .color(accent()),
        );
        let apply = ui.add_sized(
            [280.0, 34.0],
            egui::Button::new(
                egui::RichText::new(format!("Apply {}", result.recommended.label())).size(13.0),
            ),
        );
        hover_tick(&apply, hovered, sfx);
        if apply.clicked() {
            let tier = result.recommended;
            *settings.quality = tier;
            settings.config.set_quality_override(Some(tier));
            apply_graphics_overrides(
                &mut settings.overrides,
                &mut settings.config,
                QualityOverrides::default(),
            );
            settings.benchmark.clear_result();
            sfx.write(PlaySfx(Sfx::Confirm));
        }
    }
}

fn loading_hud_system(
    mut contexts: EguiContexts,
    city: Res<mf_state::CurrentCity>,
    fields: Res<mf_state::LatestFields>,
    ui_state: Res<LatestUi>,
) -> Result {
    let s = crate::strings::current();
    let ctx = contexts.ctx_mut()?;
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::TRANSPARENT))
        .show(ctx, |ui| {
            crate::design_system::paint_menu_gradient_scrim(ui.painter(), ui.max_rect());
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.28).max(24.0));
                draw_logo(ui, 48.0);
                ui.add_space(ds::SPACE_MD);
                ui.label(ds::heading(s.loading_city));
                ui.add_space(ds::SPACE_MD);

                let readiness = |label: &str, ready: bool| {
                    egui::RichText::new(s.loading_status(label, ready))
                        .size(ds::TEXT_SM)
                        .color(muted_text())
                };
                ui.label(readiness(s.loading_static_city, city.static_city.is_some()));
                ui.label(readiness(s.loading_masks, city.masks_complete()));
                ui.label(readiness(s.loading_fields, fields.0.is_some()));
                ui.label(readiness(s.loading_interface, ui_state.0.is_some()));
            });
        });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn in_game_hud_system(
    mut contexts: EguiContexts,
    ui_state: Res<LatestUi>,
    link: Option<Res<SimLink>>,
    mut subway: ResMut<SubwayView>,
    toasts: Res<ToastLog>,
    mut goals_panel: ResMut<GoalsPanelOpen>,
    mut route_panel: ResMut<crate::build_ui::RoutePanelState>,
    mut sfx: EventWriter<PlaySfx>,
    mut hovered: Local<Option<egui::Id>>,
    mut egui_timer: Option<ResMut<crate::perf::EguiPerfTimer>>,
) -> Result {
    let s = crate::strings::current();
    let t0 = egui_timer.as_ref().map(|_| std::time::Instant::now());
    let ctx = contexts.ctx_mut()?;

    egui::TopBottomPanel::top("hud_top")
        .frame(
            egui::Frame::NONE
                .fill(panel_bg())
                .inner_margin(egui::Margin::symmetric(14, 10))
                .stroke(egui::Stroke::new(ds::ACCENT_EDGE_PX, accent())),
        )
        .show_separator_line(false)
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

                    // Prefer the sidecar's sim `hourOfDay` (sim-depth, PR #31)
                    // over the tick-derived clock so this readout stays
                    // consistent with the day/night rig, which now reads the
                    // same field. A small sun/moon glyph precedes the time.
                    let hour = state.display_hour();
                    let (icon_rect, _) =
                        ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                    draw_day_night_icon(ui.painter(), icon_rect, is_night_hour(hour));
                    fixed_width_label(
                        ui,
                        egui::RichText::new(s.day_clock(
                            state.day,
                            hour as u32,
                            (hour.fract() * 60.0) as u32,
                        ))
                        .monospace(),
                        128.0,
                    );
                    thin_separator(ui);

                    // Art-direction: vivid color only on interactive/transit.
                    // Approval state is weight + glyph, not traffic-light hues.
                    let trend: i8 = if state.approval >= 60.0 {
                        1
                    } else if state.approval >= 35.0 {
                        0
                    } else {
                        -1
                    };
                    let approval_text = s.approval_pct(state.approval, trend);
                    let approval_style = if state.approval < 35.0 {
                        egui::RichText::new(approval_text)
                            .monospace()
                            .strong()
                            .color(text_color())
                    } else {
                        egui::RichText::new(approval_text)
                            .monospace()
                            .color(text_color())
                    };
                    fixed_width_label(ui, approval_style, 130.0);
                    thin_separator(ui);
                    fixed_width_label(
                        ui,
                        egui::RichText::new(s.pop(&format_thousands(state.population))).monospace(),
                        130.0,
                    );

                    // Sim-depth (PR #31): a warning chip counting overcrowded
                    // routes. Clicking it opens the route panel focused on the
                    // first flagged route so the player can act on it. Only
                    // shown when the sidecar reports any (old ones send none).
                    // The sidecar reports a scalar count; which route is
                    // busiest comes from per-route live_crowding.
                    let count = state.overcrowded_routes.unwrap_or(0) as usize;
                    let busiest = state
                        .routes
                        .iter()
                        .filter(|r| r.live_crowding.unwrap_or(0.0) > 1.0)
                        .max_by(|a, b| {
                            a.live_crowding
                                .unwrap_or(0.0)
                                .total_cmp(&b.live_crowding.unwrap_or(0.0))
                        })
                        .map(|r| r.id);
                    if let (true, Some(route_id)) = (count > 0, busiest) {
                        thin_separator(ui);
                        let chip = egui::Button::new(
                            egui::RichText::new(s.crowded_routes_chip(count))
                                .color(egui::Color32::WHITE)
                                .strong(),
                        )
                        .fill(WARN)
                        .corner_radius(crate::design_system::CORNER_RADIUS);
                        let resp = ui.add(chip).on_hover_text(s.open_busiest_crowded_route);
                        hover_tick(&resp, &mut hovered, &mut sfx);
                        if resp.clicked() {
                            route_panel.open = true;
                            route_panel.selected = Some(route_id);
                            sfx.write(PlaySfx(Sfx::Confirm));
                        }
                    }
                } else {
                    ui.label(s.connecting_to_city);
                }

                thin_separator(ui);
                for (label, speed) in [
                    (s.speed_1x, 1.0_f64),
                    (s.speed_10x, 10.0),
                    (s.speed_30x, 30.0),
                    (s.speed_120x, 120.0),
                ] {
                    let is_current = ui_state
                        .0
                        .as_ref()
                        .map(|s| (s.speed - speed).abs() < 0.01)
                        .unwrap_or(false);
                    let resp = ds::button(ui, label, ds::ButtonKind::Toggle(is_current));
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
                let subway_resp = ds::button(
                    ui,
                    if subway.active {
                        s.surface_view
                    } else {
                        s.subway_view
                    },
                    ds::ButtonKind::Toggle(subway.active),
                );
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
                let goals_resp = ds::button(ui, s.goals, ds::ButtonKind::Toggle(goals_panel.0));
                hover_tick(&goals_resp, &mut hovered, &mut sfx);
                if goals_resp.clicked() {
                    goals_panel.0 = !goals_panel.0;
                    sfx.write(PlaySfx(if goals_panel.0 {
                        Sfx::Confirm
                    } else {
                        Sfx::Cancel
                    }));
                }
            });
        });

    egui::TopBottomPanel::bottom("hud_toasts")
        .frame(
            egui::Frame::NONE
                .fill(panel_bg())
                .inner_margin(egui::Margin::symmetric(14, 6)),
        )
        .show_separator_line(false)
        .min_height(0.0)
        .show(ctx, |ui| {
            if !toasts.0.is_empty() {
                ui.horizontal(|ui| {
                    for (msg, tone) in toasts.0.iter().rev().take(3) {
                        ds::toast(ui, msg, *tone);
                        ui.add_space(ds::SPACE_XS);
                    }
                });
            }
        });
    if let (Some(t0), Some(timer)) = (t0, egui_timer.as_mut()) {
        timer.0 = timer.0.saturating_add(t0.elapsed().as_micros() as u64);
    }
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
    contexts: EguiContexts,
    mut pause: ResMut<PauseState>,
    ui_state: Res<LatestUi>,
    link: Option<Res<SimLink>>,
    settings: SettingsControls,
    tutorial: ResMut<crate::tutorial::TutorialState>,
    mut save_manager: ResMut<SaveManager>,
    mut toasts: ResMut<ToastLog>,
    pending: Res<PendingInit>,
    playtime: Res<PlaytimeTracker>,
    mut exit: EventWriter<AppExit>,
    mut sfx: EventWriter<PlaySfx>,
    mut hovered: Local<Option<egui::Id>>,
    mut slot_occupied_cache: Local<Option<[bool; SAVE_SLOT_COUNT]>>,
    mut pause_settings_open: Local<bool>,
    rigs: Query<&mut CameraRig>,
) -> Result {
    if !pause.active {
        return Ok(());
    }
    // Owner ask: Settings must be reachable "from the in-game pause menu",
    // not just the title screen. Reuses `settings_screen_ui` verbatim
    // (same quality/theme/Advanced controls) rather than a second copy
    // bolted onto the pause panel.
    if *pause_settings_open {
        if settings_screen_ui(contexts, settings, Some(tutorial), rigs, sfx, hovered)? {
            *pause_settings_open = false;
        }
        return Ok(());
    }
    let s = crate::strings::current();
    let mut contexts = contexts;
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

    let mut settings_open_pressed = false;
    let ctx = contexts.ctx_mut()?;
    let fade = ds::animate_bool(ctx, egui::Id::new("pause_fade"), true);

    ds::modal(ctx, egui::Id::new("pause_modal"), fade, |ui| {
        ui.set_width(260.0);
        ui.vertical_centered(|ui| {
            draw_logo(ui, 36.0);
            ui.add_space(6.0);
            ui.label(ds::heading(s.paused));
            ui.add_space(18.0);

            let resume = ds::button_sized(
                ui,
                s.resume,
                ds::ButtonKind::Primary,
                Some(egui::vec2(220.0, 40.0)),
            );
            hover_tick(&resume, &mut hovered, &mut sfx);
            if resume.clicked() && toggle_pause(&mut pause, &ui_state, link.as_deref()) {
                sfx.write(PlaySfx(Sfx::Unpause));
            }

            ui.add_space(10.0);
            let settings = ds::button_sized(
                ui,
                s.settings,
                ds::ButtonKind::Ghost,
                Some(egui::vec2(220.0, 36.0)),
            );
            hover_tick(&settings, &mut hovered, &mut sfx);
            if settings.clicked() {
                sfx.write(PlaySfx(Sfx::Confirm));
                settings_open_pressed = true;
            }

            ui.add_space(14.0);
            field_label(ui, s.save_game);
            ui.horizontal(|ui| {
                for n in 1..=saves::SLOT_COUNT {
                    let occupied = slot_occupied
                        .get((n - 1) as usize)
                        .copied()
                        .unwrap_or(false);
                    let label = if occupied {
                        s.slot_label(n)
                    } else {
                        s.slot_empty_label(n)
                    };
                    let btn = ds::button_sized(
                        ui,
                        label,
                        ds::ButtonKind::Ghost,
                        Some(egui::vec2(68.0, 32.0)),
                    );
                    hover_tick(&btn, &mut hovered, &mut sfx);
                    if btn.clicked() {
                        if let (Some(link), Some(state)) = (&link, &ui_state.0) {
                            let meta = SaveMeta::from_ui(
                                Some(pending.preset_key.clone()),
                                state,
                                playtime.whole_secs(),
                            );
                            save_manager.request_save(
                                SaveSlot::Slot(n),
                                meta,
                                link,
                                &mut toasts,
                                &mut sfx,
                            );
                        }
                    }
                }
            });

            ui.add_space(14.0);
            let quit = ds::button_sized(
                ui,
                s.quit_to_desktop,
                ds::ButtonKind::Danger,
                Some(egui::vec2(220.0, 40.0)),
            );
            hover_tick(&quit, &mut hovered, &mut sfx);
            if quit.clicked() {
                sfx.write(PlaySfx(Sfx::Cancel));
                exit.write(AppExit::Success);
            }
        });
    });

    if settings_open_pressed {
        *pause_settings_open = true;
    }
    Ok(())
}

/// Brief full-screen overlay while a mid-game sidecar reconnect is in
/// flight. Stays on `InGame` underneath so the world doesn't tear down.
fn reconnect_overlay_system(mut contexts: EguiContexts, reconnect: Res<ReconnectState>) -> Result {
    let NetStatus::Reconnecting {
        attempt,
        reason,
        phase,
    } = &reconnect.status
    else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    egui::Area::new(egui::Id::new("reconnect_scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let screen = ui.ctx().screen_rect();
            ui.allocate_response(screen.size(), egui::Sense::hover());
            ui.painter().rect_filled(
                screen,
                egui::CornerRadius::ZERO,
                egui::Color32::from_rgba_unmultiplied(8, 10, 14, 200),
            );
        });
    egui::Area::new(egui::Id::new("reconnect_panel"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            egui::Frame::default()
                .fill(panel_bg())
                .corner_radius(egui::CornerRadius::same(2))
                .inner_margin(egui::Margin::symmetric(28, 22))
                .show(ui, |ui| {
                    ui.set_width(360.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("Reconnecting to simulation")
                                .size(20.0)
                                .strong()
                                .color(text_color()),
                        );
                        ui.add_space(crate::design_system::SPACE_SM);
                        let phase_label = match phase {
                            ReconnectPhase::Respawning => "Restarting sidecar…",
                            ReconnectPhase::Handshaking => "Re-handshaking…",
                            ReconnectPhase::Reloading => "Restoring city…",
                        };
                        ui.label(
                            egui::RichText::new(format!(
                                "{phase_label} (attempt {attempt} of {MAX_ATTEMPTS})"
                            ))
                            .color(muted_text()),
                        );
                        ui.label(
                            egui::RichText::new(reason.detail())
                                .size(12.0)
                                .color(muted_text()),
                        );
                    });
                });
        });
    Ok(())
}

/// Full-screen fatal diagnostics after 3 failed reconnects: reason, sidecar
/// log tail, and a one-click copy-diagnostics button. Never a silent freeze.
fn sim_error_screen_system(
    mut contexts: EguiContexts,
    mut reconnect: ResMut<ReconnectState>,
    mut next_state: ResMut<NextState<AppState>>,
    mut exit: EventWriter<AppExit>,
    mut sfx: EventWriter<PlaySfx>,
    mut error_played: Local<bool>,
    mut copied_flash: Local<Option<std::time::Instant>>,
) -> Result {
    let NetStatus::Fatal(diag) = reconnect.status.clone() else {
        *error_played = false;
        return Ok(());
    };
    if !*error_played {
        sfx.write(PlaySfx(Sfx::Error));
        *error_played = true;
    }
    let clipboard = diag.clipboard_text();
    let ctx = contexts.ctx_mut()?;
    let mut go_menu = false;
    let mut go_quit = false;
    let mut did_copy = false;
    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(crate::design_system::menu_wash()))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.08).max(16.0));
                draw_logo(ui, 48.0);
                ui.add_space(crate::design_system::SPACE_MD);
                ui.label(
                    egui::RichText::new("Simulation disconnected")
                        .size(26.0)
                        .strong()
                        .color(text_color()),
                );
                ui.add_space(crate::design_system::SPACE_SM);
                ui.label(egui::RichText::new(&diag.message).size(14.0).color(BAD));
                ui.label(
                    egui::RichText::new(format!(
                        "Cause: {} — {}",
                        diag.reason.label(),
                        diag.reason.detail()
                    ))
                    .size(13.0)
                    .color(muted_text()),
                );
                ui.add_space(crate::design_system::SPACE_MD);
                ui.label(
                    egui::RichText::new("Sidecar log (tail)")
                        .size(13.0)
                        .strong()
                        .color(text_color()),
                );
                ui.add_space(4.0);
                let mut log = if diag.log_tail.trim().is_empty() {
                    "(no stderr captured)".to_string()
                } else {
                    diag.log_tail.clone()
                };
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut log)
                                .desired_width(520.0)
                                .font(egui::TextStyle::Monospace)
                                .interactive(false),
                        );
                    });
                ui.add_space(crate::design_system::SPACE_MD);
                ui.horizontal(|ui| {
                    let copy = ui.add_sized(
                        [180.0, 36.0],
                        egui::Button::new(
                            egui::RichText::new("Copy diagnostics")
                                .color(egui::Color32::WHITE)
                                .size(14.0),
                        )
                        .fill(accent()),
                    );
                    if copy.clicked() {
                        ui.ctx().copy_text(clipboard.clone());
                        did_copy = true;
                    }
                    if ui
                        .add_sized(
                            [140.0, 36.0],
                            egui::Button::new(egui::RichText::new("Back to menu").size(14.0)),
                        )
                        .clicked()
                    {
                        go_menu = true;
                    }
                    if ui
                        .add_sized(
                            [100.0, 36.0],
                            egui::Button::new(egui::RichText::new("Quit").size(14.0)),
                        )
                        .clicked()
                    {
                        go_quit = true;
                    }
                });
                if copied_flash.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(2)) {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("Copied to clipboard.")
                            .size(12.0)
                            .color(GOOD),
                    );
                }
            });
        });
    if did_copy {
        *copied_flash = Some(std::time::Instant::now());
        sfx.write(PlaySfx(Sfx::Confirm));
    }
    if go_menu {
        sfx.write(PlaySfx(Sfx::Cancel));
        reconnect.clear_fatal();
        next_state.set(AppState::Boot);
    }
    if go_quit {
        sfx.write(PlaySfx(Sfx::Cancel));
        exit.write(AppExit::Success);
    }
    Ok(())
}

/// Surfaces a boot-time fatal reconnect failure on the main menu as a banner
/// (in-session fatals use [`sim_error_screen_system`] instead).
fn fatal_banner_system(
    mut contexts: EguiContexts,
    reconnect: Res<ReconnectState>,
    mut sfx: EventWriter<PlaySfx>,
    mut error_played: Local<bool>,
) -> Result {
    let NetStatus::Fatal(diag) = &reconnect.status else {
        *error_played = false;
        return Ok(());
    };
    if !*error_played {
        sfx.write(PlaySfx(Sfx::Error));
        *error_played = true;
    }
    let s = crate::strings::current();
    let ctx = contexts.ctx_mut()?;
    egui::TopBottomPanel::bottom("fatal_banner").show(ctx, |ui| {
        ui.colored_label(BAD, s.lost_connection(&diag.message));
    });
    Ok(())
}
