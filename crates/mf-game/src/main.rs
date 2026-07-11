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
mod crash_report;
mod design_system;
mod egui_idle;
mod goals;
mod hud;
mod input;
mod map_mode;
mod minimap;
mod overlays;
mod panels;
mod paths;
mod perf;
mod promo;
mod quality_boot;
mod report_ui;
mod reveal_input;
mod saves;
mod single_instance;
mod state;
mod theme_boot;
mod tools;
mod tutorial;
mod verify;
mod window_mgmt;

use bevy::prelude::*;
use bevy::window::WindowPlugin;
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
    // Before anything that might spawn the sidecar: a second Windows launch
    // focuses the existing window and exits instead of starting another sim.
    if !single_instance::ensure_single_instance() {
        return;
    }
    crash_report::install_panic_hook();

    // Load config before the window is created so size/position/fullscreen
    // apply on the first frame (boot_system used to load it too late).
    let config = config::MfConfig::load();
    let window = window_mgmt::window_from_config(&config);

    let mut app = App::new();
    app.insert_resource(ClearColor(SKY_DAY))
        .insert_resource(config)
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(window),
            ..default()
        }))
        .add_plugins((MfNetPlugin, MfStatePlugin, MfRenderPlugin))
        .add_plugins(app_icon::MfAppIconPlugin)
        .add_plugins(window_mgmt::MfWindowPlugin)
        .add_plugins((
            state::MfGameStatePlugin,
            camera::MfCameraPlugin,
            input::MfInputPlugin,
            reveal_input::MfRevealInputPlugin,
            hud::MfHudPlugin,
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
            minimap::MfMinimapPlugin,
            panels::MfPanelsPlugin,
            campaign::MfCampaignPlugin,
            report_ui::MfReportUiPlugin,
            attract::MfAttractPlugin,
            promo::MfPromoPlugin,
            tutorial::MfTutorialPlugin,
            goals::MfGoalsPlugin,
            perf::MfPerfPlugin,
            egui_idle::MfEguiIdlePlugin,
        ));
    // MF_PERF / MF_PERF_LOG: Bevy diagnostic plugins + spans. MF_PERF also
    // drives the 60s sample-then-exit harness in `perf.rs`.
    if std::env::var_os("MF_PERF").is_some() || std::env::var_os("MF_PERF_LOG").is_some() {
        app.add_plugins((
            bevy::diagnostic::FrameTimeDiagnosticsPlugin::default(),
            bevy::diagnostic::EntityCountDiagnosticsPlugin,
            bevy::diagnostic::LogDiagnosticsPlugin::default(),
        ));
        // Keep a longer frame-time history so MF_PERF percentiles are stable.
        app.insert_resource(perf::EguiPerfTimer::default());
    }
    app.run();
}
