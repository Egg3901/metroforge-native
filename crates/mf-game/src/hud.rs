//! egui HUD (spec §3.4 `hud.rs`), styled per art-direction.md §8: off-white
//! panels, near-black text, vivid accents reserved for interactive/transit
//! elements, no gradients or rounded-corner excess, one embedded OFL font.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use mf_net::{NetStatus, ReconnectState, SimEvent, SimLink};
use mf_protocol::envelope::FromSimJson;
use mf_protocol::{Difficulty, FromSimMsg, ToSim, ToastTone};
use mf_state::{LatestUi, QualityTier, SubwayView};

use crate::config::MfConfig;
use crate::state::{AppState, PendingInit, SimHello};

// Art-direction §1/§8 palette, in egui's 0..255 sRGB `Color32`.
const PANEL_BG: egui::Color32 = egui::Color32::from_rgb(0xf4, 0xf5, 0xf2); // near-white
const TEXT_COLOR: egui::Color32 = egui::Color32::from_rgb(0x17, 0x18, 0x1c); // rich black
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x00, 0x7a, 0xff); // metro blue
const GOOD: egui::Color32 = egui::Color32::from_rgb(0x34, 0xc7, 0x59);
const WARN: egui::Color32 = egui::Color32::from_rgb(0xff, 0x95, 0x00);
const BAD: egui::Color32 = egui::Color32::from_rgb(0xff, 0x3b, 0x30);

const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");

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

fn quality_selector(ui: &mut egui::Ui, quality: &mut QualityTier, config: &mut MfConfig) {
    egui::ComboBox::from_label("Quality")
        .selected_text(format!("{quality:?}"))
        .show_ui(ui, |ui| {
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
        });
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

fn main_menu_hud_system(
    mut contexts: EguiContexts,
    hello: Res<SimHello>,
    mut pending: ResMut<PendingInit>,
    mut quality: ResMut<QualityTier>,
    mut config: ResMut<MfConfig>,
    mut next_state: ResMut<NextState<AppState>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.heading("MetroForge");
        ui.add_space(12.0);

        ui.label("City");
        let cities = hello
            .0
            .as_ref()
            .map(|h| h.city_list.as_slice())
            .unwrap_or(&[]);
        egui::ComboBox::from_id_salt("city_picker")
            .selected_text(if pending.preset_key.is_empty() {
                "nyc".to_string()
            } else {
                pending.preset_key.clone()
            })
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

        ui.add_space(8.0);
        ui.label("Difficulty");
        egui::ComboBox::from_id_salt("difficulty_picker")
            .selected_text(format!("{:?}", pending.difficulty))
            .show_ui(ui, |ui| {
                for d in [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard] {
                    ui.selectable_value(&mut pending.difficulty, d, format!("{d:?}"));
                }
            });

        ui.add_space(8.0);
        quality_selector(ui, &mut quality, &mut config);

        ui.add_space(16.0);
        if ui
            .add(
                egui::Button::new(egui::RichText::new("Start").color(egui::Color32::WHITE))
                    .fill(ACCENT),
            )
            .clicked()
        {
            next_state.set(AppState::Loading);
        }
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
        ui.heading("Loading city...");
        ui.label(format!(
            "static city: {}",
            if city.static_city.is_some() {
                "ready"
            } else {
                "waiting"
            }
        ));
        ui.label(format!(
            "masks: {}",
            if city.masks_complete() {
                "ready"
            } else {
                "waiting"
            }
        ));
        ui.label(format!(
            "fields: {}",
            if fields.0.is_some() {
                "ready"
            } else {
                "waiting"
            }
        ));
        ui.label(format!(
            "ui: {}",
            if ui_state.0.is_some() {
                "ready"
            } else {
                "waiting"
            }
        ));
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
                    ui.label(
                        egui::RichText::new(format!("${:.0}", state.cash))
                            .strong()
                            .size(15.0),
                    );
                    thin_separator(ui);

                    const TICKS_PER_DAY: u64 = 1200;
                    let hour = (state.tick % TICKS_PER_DAY) as f64 / TICKS_PER_DAY as f64 * 24.0;
                    ui.label(format!(
                        "Day {}  {:02}:{:02}",
                        state.day,
                        hour as u32,
                        ((hour.fract()) * 60.0) as u32
                    ));
                    thin_separator(ui);

                    let approval_color = if state.approval >= 60.0 {
                        GOOD
                    } else if state.approval >= 35.0 {
                        WARN
                    } else {
                        BAD
                    };
                    ui.colored_label(approval_color, format!("Approval {:.0}%", state.approval));
                    thin_separator(ui);
                    ui.label(format!("Pop {:.0}", state.population));
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
