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
            .add_systems(Startup, setup_egui_style_system)
            .add_systems(Update, collect_toasts_system)
            .add_systems(
                EguiPrimaryContextPass,
                (
                    main_menu_hud_system.run_if(in_state(AppState::MainMenu)),
                    loading_hud_system.run_if(in_state(AppState::Loading)),
                    in_game_hud_system.run_if(in_state(AppState::InGame)),
                    fatal_banner_system,
                ),
            );
    }
}

fn setup_egui_style_system(mut contexts: EguiContexts) {
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
    visuals.override_text_color = Some(TEXT_COLOR);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(0xe9, 0xea, 0xe5);
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

    egui::TopBottomPanel::top("hud_top").show(ctx, |ui| {
        ui.horizontal(|ui| {
            if let Some(state) = &ui_state.0 {
                ui.label(egui::RichText::new(format!("${:.0}", state.cash)).strong());
                ui.separator();
                ui.label(format!("Day {}", state.day));
                ui.separator();
                let approval_color = if state.approval >= 60.0 {
                    GOOD
                } else if state.approval >= 35.0 {
                    WARN
                } else {
                    BAD
                };
                ui.colored_label(approval_color, format!("Approval {:.0}%", state.approval));
                ui.separator();
                ui.label(format!("Pop {:.0}", state.population));
            } else {
                ui.label("Connecting to city...");
            }

            ui.separator();
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

            ui.separator();
            if ui
                .button(if subway.active {
                    "Surface view"
                } else {
                    "Subway view"
                })
                .clicked()
            {
                subway.toggle();
            }

            ui.separator();
            quality_selector(ui, &mut quality, &mut config);
        });
    });

    egui::TopBottomPanel::bottom("hud_toasts")
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
                        ui.separator();
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
