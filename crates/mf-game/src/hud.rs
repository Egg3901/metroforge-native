//! egui HUD (spec §3.4 `hud.rs`), styled per art-direction.md §8: off-white
//! panels, near-black text, vivid accents reserved for interactive/transit
//! elements, no gradients or rounded-corner excess, one embedded OFL font.

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use mf_net::{NetStatus, ReconnectState, SimEvent, SimLink};
use mf_protocol::envelope::FromSimJson;
use mf_protocol::{Difficulty, FromSimMsg, ToSim, ToastTone};
use mf_state::{LatestUi, QualityTier, SubwayView};

use crate::config::MfConfig;
use crate::state::{toggle_pause, AppState, PauseState, PendingInit, SimHello};

// Art-direction §1/§8 palette, in egui's 0..255 sRGB `Color32`.
const PANEL_BG: egui::Color32 = egui::Color32::from_rgb(0xf4, 0xf5, 0xf2); // near-white
const TEXT_COLOR: egui::Color32 = egui::Color32::from_rgb(0x17, 0x18, 0x1c); // rich black
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x00, 0x7a, 0xff); // metro blue
const GOOD: egui::Color32 = egui::Color32::from_rgb(0x34, 0xc7, 0x59);
const WARN: egui::Color32 = egui::Color32::from_rgb(0xff, 0x95, 0x00);
const BAD: egui::Color32 = egui::Color32::from_rgb(0xff, 0x3b, 0x30);

const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
// Muted secondary text (subtitle/labels/version) — art-direction reserves
// full rich-black for primary copy; this is the same de-emphasis egui's own
// `weak` text uses, picked to sit comfortably on the off-white panel.
const MUTED_TEXT: egui::Color32 = egui::Color32::from_rgb(0x6b, 0x6d, 0x72);

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
                    .chain(),
            );
    }
}

/// Guards [`setup_egui_style_system`] so it only does its (cheap but
/// non-trivial) font/visuals work once it actually succeeds. Deliberately
/// NOT a `Startup` system: at `Startup` the primary window's egui context
/// isn't guaranteed to exist yet (bevy_egui wires it up once the window
/// backend is ready), so a one-shot `Startup` system silently no-ops and
/// the HUD is stuck on bevy_egui's default dark theme forever — this bit
/// during initial implementation (art-direction §8's off-white panels
/// never appeared). Retrying every `EguiPrimaryContextPass` tick until it
/// succeeds fixes that with no observable per-frame cost once applied.
#[derive(Resource, Default)]
struct EguiStyleApplied(bool);

fn setup_egui_style_system(mut contexts: EguiContexts, mut applied: ResMut<EguiStyleApplied>) {
    if applied.0 {
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

    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = PANEL_BG;
    visuals.window_fill = PANEL_BG;
    visuals.extreme_bg_color = egui::Color32::from_rgb(0xe9, 0xea, 0xe5);
    visuals.faint_bg_color = egui::Color32::from_rgb(0xe9, 0xea, 0xe5);
    visuals.override_text_color = Some(TEXT_COLOR);
    visuals.widgets.noninteractive.bg_fill = PANEL_BG;
    visuals.widgets.noninteractive.weak_bg_fill = PANEL_BG;
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(0xe9, 0xea, 0xe5);
    visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(0xe9, 0xea, 0xe5);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(0xdc, 0xde, 0xd8);
    visuals.widgets.active.bg_fill = ACCENT;
    visuals.selection.bg_fill = ACCENT;
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
    applied.0 = true;
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
fn quality_options(ui: &mut egui::Ui, quality: &mut QualityTier, config: &mut MfConfig) {
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
        }
    }
}

fn quality_selector(ui: &mut egui::Ui, quality: &mut QualityTier, config: &mut MfConfig) {
    egui::ComboBox::from_label("Quality")
        .selected_text(format!("{quality:?}"))
        .show_ui(ui, |ui| quality_options(ui, quality, config));
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

fn main_menu_hud_system(
    mut contexts: EguiContexts,
    hello: Res<SimHello>,
    mut pending: ResMut<PendingInit>,
    mut quality: ResMut<QualityTier>,
    mut config: ResMut<MfConfig>,
    mut next_state: ResMut<NextState<AppState>>,
) -> Result {
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

    egui::CentralPanel::default().show(ctx, |ui| {
        ui.set_opacity(fade);
        ui.vertical_centered(|ui| {
            // Roughly centers the card vertically for typical window
            // heights without measuring the card first.
            ui.add_space((ui.available_height() * 0.22).max(24.0));

            ui.scope(|ui| {
                ui.set_width(360.0);
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
                    // Show the human label the player picked, not the raw
                    // preset key ("nyc") — the key is a wire-protocol
                    // identifier, not player-facing copy.
                    let selected_label = cities
                        .iter()
                        .find(|c| c.key == pending.preset_key)
                        .map(|c| c.label.clone())
                        .unwrap_or_else(|| {
                            if pending.preset_key.is_empty() || pending.preset_key == "nyc" {
                                "New York City".to_string()
                            } else {
                                pending.preset_key.clone()
                            }
                        });

                    field_label(ui, "City");
                    egui::ComboBox::from_id_salt("city_picker")
                        .selected_text(selected_label)
                        .width(300.0)
                        .show_ui(ui, |ui| {
                            if cities.is_empty() {
                                ui.selectable_value(
                                    &mut pending.preset_key,
                                    "nyc".to_string(),
                                    "New York City (default)",
                                );
                            }
                            for entry in cities {
                                ui.selectable_value(
                                    &mut pending.preset_key,
                                    entry.key.clone(),
                                    entry.label.clone(),
                                );
                            }
                        });

                    ui.add_space(12.0);
                    field_label(ui, "Difficulty");
                    egui::ComboBox::from_id_salt("difficulty_picker")
                        .selected_text(format!("{:?}", pending.difficulty))
                        .width(300.0)
                        .show_ui(ui, |ui| {
                            for d in [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard] {
                                ui.selectable_value(&mut pending.difficulty, d, format!("{d:?}"));
                            }
                        });

                    ui.add_space(12.0);
                    field_label(ui, "Quality");
                    egui::ComboBox::from_id_salt("quality_picker")
                        .selected_text(format!("{:?}", *quality))
                        .width(300.0)
                        .show_ui(ui, |ui| quality_options(ui, &mut quality, &mut config));

                    ui.add_space(28.0);
                    if ui
                        .add_sized(
                            [220.0, 44.0],
                            egui::Button::new(
                                egui::RichText::new("Start")
                                    .color(egui::Color32::WHITE)
                                    .size(16.0)
                                    .strong(),
                            )
                            .fill(ACCENT),
                        )
                        .clicked()
                    {
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

fn in_game_hud_system(
    mut contexts: EguiContexts,
    ui_state: Res<LatestUi>,
    link: Option<Res<SimLink>>,
    mut quality: ResMut<QualityTier>,
    mut config: ResMut<MfConfig>,
    mut subway: ResMut<SubwayView>,
    toasts: Res<ToastLog>,
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
                    if ui.add(button).clicked() {
                        if let Some(link) = &link {
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
                if ui.add(subway_button).clicked() {
                    subway.toggle();
                }

                thin_separator(ui);
                quality_selector(ui, &mut quality, &mut config);
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
fn pause_overlay_system(
    mut contexts: EguiContexts,
    mut pause: ResMut<PauseState>,
    ui_state: Res<LatestUi>,
    link: Option<Res<SimLink>>,
    mut quality: ResMut<QualityTier>,
    mut config: ResMut<MfConfig>,
    mut exit: EventWriter<AppExit>,
) -> Result {
    if !pause.active {
        return Ok(());
    }
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

                        if ui
                            .add_sized(
                                [220.0, 40.0],
                                egui::Button::new(
                                    egui::RichText::new("Resume")
                                        .color(egui::Color32::WHITE)
                                        .strong(),
                                )
                                .fill(ACCENT),
                            )
                            .clicked()
                        {
                            toggle_pause(&mut pause, &ui_state, link.as_deref());
                        }

                        ui.add_space(14.0);
                        field_label(ui, "Quality");
                        quality_selector(ui, &mut quality, &mut config);

                        ui.add_space(14.0);
                        if ui
                            .add_sized([220.0, 40.0], egui::Button::new("Quit to desktop"))
                            .clicked()
                        {
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
fn fatal_banner_system(mut contexts: EguiContexts, reconnect: Res<ReconnectState>) -> Result {
    let NetStatus::Fatal(msg) = &reconnect.status else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    egui::TopBottomPanel::bottom("fatal_banner").show(ctx, |ui| {
        ui.colored_label(BAD, format!("Lost connection to the sim: {msg}"));
    });
    Ok(())
}
