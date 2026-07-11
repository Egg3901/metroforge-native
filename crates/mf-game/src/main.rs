//! MetroForge Native game shell (spec §3.4). Binary name: `metroforge`.
// Windows: GUI subsystem in release so no console window opens behind the game.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_icon;
mod attract;
mod audio;
mod build_ui;
mod camera;
mod campaign;
mod command_bus;
mod config;
mod crash;
mod design_system;
mod goals;
mod hud;
mod input;
mod map_mode;
mod overlays;
mod panels;
mod promo;
mod quality_boot;
mod report_ui;
mod reveal_input;
mod saves;
mod state;
mod theme_boot;
mod tools;
mod tutorial;
mod verify;

use bevy::prelude::*;
use bevy::window::{PresentMode, Window, WindowPlugin};
use crash::{MfCrashPlugin, SafeMode};
use mf_net::MfNetPlugin;
use mf_render::MfRenderPlugin;
use mf_state::MfStatePlugin;
use quality_boot::MfQualityBootPlugin;
use theme_boot::MfThemeBootPlugin;

// Art-direction §1: SKY_DAY as the default clear color.
const SKY_DAY: Color = Color::srgb(
    0xdf as f32 / 255.0,
    0xe6 as f32 / 255.0,
    0xea as f32 / 255.0,
);

fn main() {
    // Panic hook before any Bevy/plugin work so boot-time panics still leave
    // a local report. Log ring attaches via LogPlugin::custom_layer below.
    crash::install_panic_hook();
    let cli = crash::parse_cli(std::env::args());

    let mut app = App::new();
    app.insert_resource(ClearColor(SKY_DAY))
        .insert_resource(SafeMode(cli.safe_mode))
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "MetroForge".to_string(),
                        // MF_RESOLUTION=WxH overrides for promo/screenshot runs.
                        resolution: std::env::var("MF_RESOLUTION")
                            .ok()
                            .and_then(|v| {
                                let (w, h) = v.split_once('x')?;
                                Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
                            })
                            .unwrap_or((1440.0, 900.0))
                            .into(),
                        present_mode: PresentMode::AutoVsync,
                        ..default()
                    }),
                    ..default()
                })
                .set(crash::log_plugin_with_ring()),
        )
        .add_plugins((MfNetPlugin, MfStatePlugin, MfRenderPlugin))
        .add_plugins(app_icon::MfAppIconPlugin)
        .add_plugins((
            state::MfGameStatePlugin,
            camera::MfCameraPlugin,
            input::MfInputPlugin,
            reveal_input::MfRevealInputPlugin,
            hud::MfHudPlugin,
            MfCrashPlugin,
            saves::MfSavesPlugin,
            verify::MfVerifyPlugin,
            MfQualityBootPlugin,
            MfThemeBootPlugin,
            audio::MfAudioPlugin,
            command_bus::MfCommandBusPlugin,
            tools::MfToolsPlugin,
            build_ui::MfBuildUiPlugin,
        ))
        // Bevy's Plugins tuple impl caps at 15 elements; second batch.
        .add_plugins((
            overlays::MfOverlaysPlugin,
            map_mode::MfMapModePlugin,
            panels::MfPanelsPlugin,
            campaign::MfCampaignPlugin,
            report_ui::MfReportUiPlugin,
            attract::MfAttractPlugin,
            promo::MfPromoPlugin,
            tutorial::MfTutorialPlugin,
            goals::MfGoalsPlugin,
        ));
    // MF_PERF_LOG=1: log frame-time diagnostics (avg/FPS) once per second.
    // Costs nothing when unset; gives players and CI a zero-setup way to
    // capture before/after numbers for performance work.
    if std::env::var_os("MF_PERF_LOG").is_some() {
        app.add_plugins((
            bevy::diagnostic::FrameTimeDiagnosticsPlugin::default(),
            bevy::diagnostic::LogDiagnosticsPlugin::default(),
        ));
    }
    app.run();
}
