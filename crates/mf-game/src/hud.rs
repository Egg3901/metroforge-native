//! egui HUD (spec §3.4 `hud.rs`). Visual chrome lives in
//! [`crate::design_system`]; this file owns layout and interaction only.

use std::time::{SystemTime, UNIX_EPOCH};

use bevy::app::AppExit;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use mf_net::{NetStatus, ReconnectState, SimEvent, SimLink};
use mf_protocol::envelope::FromSimJson;
use mf_protocol::{Difficulty, FromSimMsg, ToSim, ToastTone};
use mf_state::{LatestUi, QualityTier, SubwayView, Theme, WeatherEffects};

use crate::audio::{PlaySfx, Sfx};
use crate::campaign::{self, CampaignProgress};
use crate::config::MfConfig;
use crate::design_system as ds;
use crate::goals::GoalsPanelOpen;
use crate::saves::{self, PlaytimeTracker, SaveManager, SaveMeta, SaveSlot};
use crate::state::{toggle_pause, AppState, MenuScreen, PauseState, PendingInit, SimHello};

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
            .add_systems(
                EguiPrimaryContextPass,
                (
                    setup_egui_style_system,
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
                    fatal_banner_system.run_if(|| !ds::ui_gallery_enabled()),
                )
                    .chain()
                    .run_if(|| !ds::hud_hidden()),
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
    ds::install_fonts_and_visuals(ctx, *theme);
    applied.0 = Some(*theme);
}

fn ui_gallery_system(mut contexts: EguiContexts) -> Result {
    let ctx = contexts.ctx_mut()?;
    ds::show_gallery(ctx);
    Ok(())
}

fn collect_toasts_system(mut events: EventReader<SimEvent>, mut log: ResMut<ToastLog>) {
    for SimEvent(msg) in events.read() {
        if let FromSimMsg::Json(FromSimJson::Toast(toast)) = msg {
            log.push(toast.message.clone(), toast.tone);
        }
    }
}

fn thin_separator(ui: &mut egui::Ui) {
    ds::thin_separator(ui);
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

/// The tier rows shared by every quality combo box (Settings / pause).
/// Settings owns the only quality UI now — the in-game top bar no longer
/// duplicates these controls (design audit: play HUD is for play).
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
            .selectable_label(*quality == tier, tier.label())
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
            .selectable_label(*theme == candidate, candidate.label())
            .clicked()
        {
            *theme = candidate;
            config.set_theme_override(Some(candidate));
            sfx.write(PlaySfx(Sfx::Confirm));
        }
    }
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
}

/// ConnectingSim previously registered NO ui system at all, so a player whose
/// sidecar was slow (or repeatedly failing) stared at a bare ClearColor with
/// zero feedback until the fatal banner eventually appeared. Every app state
/// must draw *something*.
fn connecting_hud_system(mut contexts: EguiContexts, reconnect: Res<ReconnectState>) -> Result {
    let ctx = contexts.ctx_mut()?;
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(ds::menu_wash()))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.28).max(24.0));
                draw_logo(ui, 56.0);
                ui.add_space(ds::SPACE_MD);
                ui.label(ds::heading("MetroForge"));
                ui.add_space(ds::SPACE_SM);
                match &reconnect.status {
                    NetStatus::Fatal(msg) => {
                        ui.colored_label(BAD, format!("Could not start the simulation: {msg}"));
                    }
                    NetStatus::Reconnecting { attempt } => {
                        ui.label(
                            egui::RichText::new(format!(
                                "Starting the simulation (attempt {attempt} of 5)..."
                            ))
                            .color(muted_text()),
                        );
                    }
                    NetStatus::Connected => {
                        ui.label(
                            egui::RichText::new("Starting the simulation...").color(muted_text()),
                        );
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
fn field_label(ui: &mut egui::Ui, text: &str) {
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

fn format_playtime(secs: u64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
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
/// stars" caption. Returns `(clicked, double_clicked)` (always `(false,
/// false)` for a locked card - `Sense::hover()` only, so it isn't even
/// interactive). A double-click is the "actually select and play"
/// shortcut straight into Start (see `city_select_screen_ui`).
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
) -> (bool, bool) {
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
        card_hover_bg()
    } else {
        card_bg()
    };
    painter.rect_filled(rect, CARD_CORNER, bg);
    let border = if selected {
        egui::Stroke::new(2.5, accent())
    } else {
        egui::Stroke::new(1.0, card_border())
    };
    painter.rect_stroke(rect, CARD_CORNER, border, egui::StrokeKind::Inside);

    let content_rect = rect.shrink(10.0);
    let text_color = if unlocked { text_color() } else { muted_text() };
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
            muted_text()
        } else if filled {
            accent()
        } else {
            card_border()
        };
        draw_star(&star_painter, center, star_r, filled, color);
    }

    if !unlocked {
        child.add_space(6.0);
        child.horizontal(|ui| {
            let (lock_rect, _) =
                ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
            draw_lock(&ui.painter_at(lock_rect), lock_rect, muted_text());
            let plural = if stars_needed == 1 { "" } else { "s" };
            ui.label(
                egui::RichText::new(format!("Earn {stars_needed} more star{plural}"))
                    .size(11.0)
                    .color(muted_text()),
            );
        });
    }

    (
        unlocked && response.clicked(),
        unlocked && response.double_clicked(),
    )
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
                .unwrap_or_else(|| "Unknown city".to_string());
            format!(
                "{city} · Day {} · {} stops · {}",
                meta.day,
                meta.network_size,
                format_playtime(meta.playtime_secs)
            )
        }
        None => "Empty".to_string(),
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
    slots_cache: Local<Option<Vec<saves::SlotEntry>>>,
) -> Result {
    match *screen {
        MenuScreen::Title => title_screen_ui(contexts, screen, exit, sfx, hovered)?,
        MenuScreen::CitySelect => city_select_screen_ui(
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
            slots_cache,
        )?,
        MenuScreen::LoadGame => load_game_screen_ui(
            contexts,
            save_manager,
            toasts,
            next_state,
            screen,
            sfx,
            hovered,
            slots_cache,
        )?,
        MenuScreen::Settings => {
            let mut screen = screen;
            // Title-screen Settings: no live `TutorialState` needed — clearing
            // the persisted flag (the Replay button does that) re-arms the flow
            // on the next city load, which is the only way to reach `InGame`
            // from here anyway.
            if settings_screen_ui(contexts, settings, None, sfx, hovered)? {
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
fn draw_logo(ui: &mut egui::Ui, size: f32) {
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
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let fade = ds::animate(ctx, egui::Id::new("title_fade"), 1.0);

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(ds::menu_wash()))
        .show(ctx, |ui| {
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
                    ui.set_width(320.0);
                    ui.add_space(ui.available_height() * 0.18);
                    ui.horizontal(|ui| {
                        draw_logo(ui, 56.0);
                    });
                    ui.add_space(ds::SPACE_MD);
                    ui.label(ds::wordmark("MetroForge"));
                    ui.add_space(ds::SPACE_XS);
                    ui.label(
                        egui::RichText::new("Build the network. Move the city.")
                            .size(ds::TEXT_MD)
                            .color(muted_text()),
                    );
                    ui.add_space(ds::SPACE_XL);

                    let play = ds::menu_text_button(ui, "Play");
                    hover_tick(&play, &mut hovered, &mut sfx);
                    if play.clicked() {
                        sfx.write(PlaySfx(Sfx::Confirm));
                        *screen = MenuScreen::CitySelect;
                    }
                    let load = ds::menu_text_button(ui, "Load Game");
                    hover_tick(&load, &mut hovered, &mut sfx);
                    if load.clicked() {
                        sfx.write(PlaySfx(Sfx::Confirm));
                        *screen = MenuScreen::LoadGame;
                    }
                    let settings = ds::menu_text_button(ui, "Settings");
                    hover_tick(&settings, &mut hovered, &mut sfx);
                    if settings.clicked() {
                        sfx.write(PlaySfx(Sfx::Confirm));
                        *screen = MenuScreen::Settings;
                    }
                    let quit = ds::menu_text_button(ui, "Quit");
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

/// City select (owner feedback: "cant actually select and play" + the card
/// list truncating on 1080p). Root cause of both: the old single-screen
/// menu packed the city grid, continue slots, difficulty/quality/theme
/// pickers AND the Start button into one un-scrolled `CentralPanel` — on a
/// 1080p window that content is comfortably taller than the viewport, so
/// Start (the very last widget) rendered below the fold with no
/// `ScrollArea` to reach it. A city-card click landed fine (it only sets
/// `pending.preset_key`, no state transition), so the player saw their
/// click "do nothing" because the one control that actually starts the
/// game was invisible off-screen. Fixed by giving the scrollable content
/// its own `ScrollArea` and pinning Start (plus a Back button) in a fixed
/// bottom panel that's always on-screen regardless of list length.
/// Double-clicking an unlocked card is also wired as a shortcut straight
/// into Start, per the ask for "actually select and play".
#[allow(clippy::too_many_arguments)]
fn city_select_screen_ui(
    mut contexts: EguiContexts,
    hello: Res<SimHello>,
    progress: Res<CampaignProgress>,
    mut pending: ResMut<PendingInit>,
    mut save_manager: ResMut<SaveManager>,
    mut toasts: ResMut<ToastLog>,
    state: Res<State<AppState>>,
    mut next_state: ResMut<NextState<AppState>>,
    mut screen: ResMut<MenuScreen>,
    mut sfx: EventWriter<PlaySfx>,
    mut playtime: ResMut<PlaytimeTracker>,
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
    let fade = ds::animate(ctx, egui::Id::new("city_select_fade"), 1.0);

    let mut go_back = false;
    let mut start_pressed = false;

    egui::TopBottomPanel::top("city_select_top")
        .frame(
            egui::Frame::NONE
                .fill(panel_bg())
                .inner_margin(egui::Margin::symmetric(14, 10))
                .stroke(egui::Stroke::new(ds::ACCENT_EDGE_PX, accent())),
        )
        .show_separator_line(false)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            ui.horizontal(|ui| {
                let back = ds::button(ui, "Back", ds::ButtonKind::Ghost);
                hover_tick(&back, &mut hovered, &mut sfx);
                if back.clicked() {
                    go_back = true;
                }
                ui.add_space(ds::SPACE_SM);
                draw_logo(ui, 28.0);
                ui.add_space(ds::SPACE_XS);
                ui.label(
                    egui::RichText::new("MetroForge")
                        .font(ds::display_font(ds::TEXT_SM))
                        .color(text_color()),
                );
            });
        });

    // Start (and the version label) live in their own bottom panel outside
    // the ScrollArea below, so they stay reachable at any window height —
    // see this function's doc comment for why that's the actual fix for
    // "cant actually select and play".
    let selected_label = {
        let cities = hello
            .0
            .as_ref()
            .map(|h| h.city_list.as_slice())
            .unwrap_or(&[]);
        cities
            .iter()
            .find(|c| c.key == pending.preset_key)
            .map(|c| c.label.clone())
            .unwrap_or_else(|| capitalize(&pending.preset_key))
    };
    let start_caption = format!("Start {} ({})", selected_label, pending.difficulty.label());

    egui::TopBottomPanel::bottom("city_select_bottom")
        .frame(
            egui::Frame::NONE
                .fill(panel_bg())
                .inner_margin(egui::Margin::symmetric(14, 12))
                .stroke(egui::Stroke::new(ds::ACCENT_EDGE_PX, accent())),
        )
        .show_separator_line(false)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            ui.vertical_centered(|ui| {
                let start = ds::button_sized(
                    ui,
                    start_caption,
                    ds::ButtonKind::Primary,
                    Some(egui::vec2(280.0, 44.0)),
                );
                hover_tick(&start, &mut hovered, &mut sfx);
                if start.clicked() {
                    start_pressed = true;
                }
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .size(ds::TEXT_XS)
                        .color(muted_text()),
                );
            });
        });

    // Soft wash only — city cards carry their own fills so the diorama
    // still reads behind the grid (brand-first menu composition).
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(ds::menu_wash()))
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(16.0);
                        ui.scope(|ui| {
                            ui.set_width(460.0);
                            ui.vertical_centered(|ui| {
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
                                            let stars_needed =
                                                (2 * i as u32).saturating_sub(total_stars);
                                            let (clicked, double_clicked) = city_card(
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
                                            if double_clicked {
                                                pending.preset_key = key.to_string();
                                                start_pressed = true;
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
                                    .selected_text(pending.difficulty.label())
                                    .width(300.0)
                                    .show_ui(ui, |ui| {
                                        for d in
                                            [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard]
                                        {
                                            ui.selectable_value(
                                                &mut pending.difficulty,
                                                d,
                                                d.label(),
                                            );
                                        }
                                    });

                                // Bottom padding so the last widget doesn't sit flush
                                // against the pinned Start panel when scrolled all the
                                // way down.
                                ui.add_space(20.0);
                            });
                        });
                    });
                });
        });

    if start_pressed {
        sfx.write(PlaySfx(Sfx::Confirm));
        saves::reset_playtime(&mut playtime);
        next_state.set(AppState::Loading);
    }
    if go_back {
        sfx.write(PlaySfx(Sfx::Cancel));
        *screen = MenuScreen::Title;
    }
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
    mut slots_cache: Local<Option<Vec<saves::SlotEntry>>>,
) -> Result {
    if slots_cache.is_none() {
        *slots_cache = Some(saves::list());
    }
    // Refresh when re-entering this screen from Title.
    if screen.is_changed() {
        *slots_cache = Some(saves::list());
    }
    let slots = slots_cache.as_ref().expect("populated just above");

    let ctx = contexts.ctx_mut()?;
    let fade = ctx.animate_value_with_time(egui::Id::new("load_game_fade"), 1.0, 0.2);
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
                    egui::Button::new(egui::RichText::new("Back").size(14.0))
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
                ui.label(crate::design_system::heading("Load Game").color(text_color()));
                ui.add_space(crate::design_system::SPACE_XS);
                ui.label(
                    egui::RichText::new("Pick a slot to continue")
                        .size(crate::design_system::TEXT_SM)
                        .color(muted_text()),
                );
                ui.add_space(crate::design_system::SPACE_MD);

                egui::ScrollArea::vertical()
                    .max_height(ui.available_height() - 24.0)
                    .show(ui, |ui| {
                        ui.set_width(480.0);
                        field_label(ui, "Autosaves");
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
                        field_label(ui, "Manual slots");
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
        *slots_cache = None;
        *screen = MenuScreen::Title;
    }
    Ok(())
}

/// Settings screen: quality, theme, weather, autosave cadence — the
/// overrides `config.rs` persists to `config.toml`. Shared verbatim by the
/// title screen's Settings button and the in-game pause menu's Settings
/// button — both call sites own what "Back" means for them (return to
/// `MenuScreen::Title` vs. close the pause-menu settings panel) via this
/// function's `bool` return (true == Back was clicked this frame).
#[allow(clippy::too_many_arguments)]
fn settings_screen_ui(
    mut contexts: EguiContexts,
    mut settings: SettingsControls,
    // `None` on the title screen (no live flow to restart), `Some` from the
    // in-game pause menu (so Replay restarts the flow immediately).
    mut tutorial: Option<ResMut<crate::tutorial::TutorialState>>,
    mut sfx: EventWriter<PlaySfx>,
    mut hovered: Local<Option<egui::Id>>,
) -> Result<bool> {
    let mut back_clicked = false;
    let ctx = contexts.ctx_mut()?;
    let fade = ds::animate(ctx, egui::Id::new("settings_fade"), 1.0);

    ds::modal(ctx, egui::Id::new("settings_modal"), fade, |ui| {
        ui.set_width(320.0);
        ui.vertical_centered(|ui| {
            draw_logo(ui, 36.0);
            ui.add_space(6.0);
            ui.label(ds::heading("Settings"));
            ui.add_space(24.0);

            field_label(ui, "Quality");
            egui::ComboBox::from_id_salt("settings_quality")
                .selected_text(settings.quality.label())
                .width(300.0)
                .show_ui(ui, |ui| {
                    quality_options(ui, &mut settings.quality, &mut settings.config, &mut sfx)
                });

            ui.add_space(14.0);
            field_label(ui, "Theme");
            egui::ComboBox::from_id_salt("settings_theme")
                .selected_text(settings.theme.label())
                .width(300.0)
                .show_ui(ui, |ui| {
                    theme_options(ui, &mut settings.theme, &mut settings.config, &mut sfx)
                });

            ui.add_space(14.0);
            field_label(ui, "Weather");
            let tier_allows = settings.quality.knobs().atmosphere_enabled;
            ui.add_enabled_ui(tier_allows, |ui| {
                let mut enabled = settings.weather.enabled;
                let label = if tier_allows {
                    "Fog & clouds"
                } else {
                    "Fog & clouds (Medium+)"
                };
                if ui.checkbox(&mut enabled, label).changed() {
                    settings.weather.enabled = enabled;
                    settings.config.set_weather_effects(enabled);
                    sfx.write(PlaySfx(Sfx::Confirm));
                }
            });

            ui.add_space(14.0);
            field_label(ui, "Autosave");
            let interval_label = match settings.config.autosave_interval_days {
                0 => "Off".to_string(),
                n => format!("Every {n} sim-days"),
            };
            egui::ComboBox::from_id_salt("settings_autosave")
                .selected_text(interval_label)
                .width(300.0)
                .show_ui(ui, |ui| {
                    for days in [0_u32, 5, 10, 20, 30] {
                        let label = if days == 0 {
                            "Off".to_string()
                        } else {
                            format!("Every {days} sim-days")
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
                egui::RichText::new("Keeps a ring of 3 autosaves")
                    .size(ds::TEXT_XS)
                    .color(muted_text()),
            );

            ui.add_space(28.0);
            let replay = ds::button_sized(
                ui,
                "Replay tutorial",
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
                "Back",
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

fn loading_hud_system(
    mut contexts: EguiContexts,
    city: Res<mf_state::CurrentCity>,
    fields: Res<mf_state::LatestFields>,
    ui_state: Res<LatestUi>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(ds::menu_wash()))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.28).max(24.0));
                draw_logo(ui, 48.0);
                ui.add_space(ds::SPACE_MD);
                ui.label(ds::heading("Loading city"));
                ui.add_space(ds::SPACE_MD);

                let readiness = |label: &str, ready: bool| {
                    let status = if ready { "ready" } else { "waiting" };
                    egui::RichText::new(format!("{label}: {status}"))
                        .size(ds::TEXT_SM)
                        .color(muted_text())
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
    mut subway: ResMut<SubwayView>,
    toasts: Res<ToastLog>,
    mut goals_panel: ResMut<GoalsPanelOpen>,
    mut route_panel: ResMut<crate::build_ui::RoutePanelState>,
    mut sfx: EventWriter<PlaySfx>,
    mut hovered: Local<Option<egui::Id>>,
    mut egui_timer: Option<ResMut<crate::perf::EguiPerfTimer>>,
) -> Result {
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
                        egui::RichText::new(format!(
                            "Day {}  {:02}:{:02}",
                            state.day,
                            hour as u32,
                            ((hour.fract()) * 60.0) as u32
                        ))
                        .monospace(),
                        128.0,
                    );
                    thin_separator(ui);

                    // Art-direction: vivid color only on interactive/transit.
                    // Approval state is weight + glyph, not traffic-light hues.
                    let approval_text = if state.approval >= 60.0 {
                        format!("▲ Approval {:.0}%", state.approval)
                    } else if state.approval >= 35.0 {
                        format!("Approval {:.0}%", state.approval)
                    } else {
                        format!("▼ Approval {:.0}%", state.approval)
                    };
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
                        egui::RichText::new(format!("Pop {}", format_thousands(state.population)))
                            .monospace(),
                        130.0,
                    );

                    // Sim-depth (PR #31): a warning chip counting overcrowded
                    // routes. Clicking it opens the route panel focused on the
                    // first flagged route so the player can act on it. Only
                    // shown when the sidecar reports any (old ones send none).
                    let first_overcrowded = state
                        .overcrowded_routes
                        .iter()
                        .find(|id| state.routes.iter().any(|r| r.id == **id))
                        .copied();
                    if let Some(route_id) = first_overcrowded {
                        thin_separator(ui);
                        let count = state.overcrowded_routes.len();
                        let plural = if count == 1 { "" } else { "s" };
                        let chip = egui::Button::new(
                            egui::RichText::new(format!("{count} crowded route{plural}"))
                                .color(egui::Color32::WHITE)
                                .strong(),
                        )
                        .fill(WARN)
                        .corner_radius(crate::design_system::CORNER_RADIUS);
                        let resp = ui.add(chip).on_hover_text("Open the busiest crowded route");
                        hover_tick(&resp, &mut hovered, &mut sfx);
                        if resp.clicked() {
                            route_panel.open = true;
                            route_panel.selected = Some(route_id);
                            sfx.write(PlaySfx(Sfx::Confirm));
                        }
                    }
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
                        "Surface view"
                    } else {
                        "Subway view"
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
                let goals_resp = ds::button(ui, "Goals", ds::ButtonKind::Toggle(goals_panel.0));
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
) -> Result {
    if !pause.active {
        return Ok(());
    }
    // Owner ask: Settings must be reachable "from the in-game pause menu",
    // not just the title screen. Reuses `settings_screen_ui` verbatim
    // (same quality/theme/weather controls, same widget layout) rather than a
    // second copy of the ComboBoxes bolted onto the pause panel.
    if *pause_settings_open {
        if settings_screen_ui(contexts, settings, Some(tutorial), sfx, hovered)? {
            *pause_settings_open = false;
        }
        return Ok(());
    }
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
            ui.label(ds::heading("Paused"));
            ui.add_space(18.0);

            let resume = ds::button_sized(
                ui,
                "Resume",
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
                "Settings",
                ds::ButtonKind::Ghost,
                Some(egui::vec2(220.0, 36.0)),
            );
            hover_tick(&settings, &mut hovered, &mut sfx);
            if settings.clicked() {
                sfx.write(PlaySfx(Sfx::Confirm));
                settings_open_pressed = true;
            }

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
                        format!("Slot {n} empty")
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
                "Quit to desktop",
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
