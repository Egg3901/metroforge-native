//! MetroForge Native game shell (spec §3.4). Binary name: `metroforge`.
// Windows: GUI subsystem in release so no console window opens behind the game.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_icon;
mod atmosphere_shots;
mod attract;
mod audio;
mod build_ui;
mod camera;
mod campaign;
mod city_catalog;
mod city_select;
mod command_bus;
mod config;
mod crash;
mod crash_report;
mod debug_overlay;
mod design_system;
mod egui_idle;
mod goals;
mod graphics_perf;
mod hud;
mod input;
mod map_mode;
mod map_paint;
mod minimap;
mod overlays;
mod panels;
mod paths;
mod perf;
mod promo;
mod quality_boot;
mod report_ui;
mod reveal_input;
mod routes_panel;
mod saves;
mod sidecar_kill_test;
mod single_instance;
mod soak;
mod state;
mod strings;
mod theme_boot;
mod tools;
mod tutorial;
mod verify;
mod window_mgmt;

use bevy::prelude::*;
use bevy::window::WindowPlugin;
use crash::{MfCrashPlugin, SafeMode};
use graphics_perf::MfGraphicsPerfPlugin;
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
    // Panic hooks before any Bevy/plugin work so boot-time panics still leave
    // reports. `crash` (safe-mode/log-ring report) installs first; the simpler
    // `crash_report` OS-native writer chains to it, so both run on a panic. The
    // log ring attaches via `LogPlugin::custom_layer` in the WindowPlugin set.
    crash::install_panic_hook();
    crash_report::install_panic_hook();
    let cli = crash::parse_cli(std::env::args());

    // Load config before the window is created so size/position/fullscreen
    // apply on the first frame. `window_from_config` also honors MF_RESOLUTION.
    let config = config::MfConfig::load();
    let window = window_mgmt::window_from_config(&config);

    let mut app = App::new();
    app.insert_resource(ClearColor(SKY_DAY))
        .insert_resource(SafeMode(cli.safe_mode))
        .insert_resource(config)
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(window),
                    ..default()
                })
                .set(crash::log_plugin_with_ring()),
        )
        .add_plugins((MfNetPlugin, MfStatePlugin, MfRenderPlugin))
        .add_plugins(app_icon::MfAppIconPlugin)
        .add_plugins(window_mgmt::MfWindowPlugin)
        .add_plugins((
            state::MfGameStatePlugin,
            camera::MfCameraPlugin,
            input::MfInputPlugin,
            reveal_input::MfRevealInputPlugin,
            hud::MfHudPlugin,
            MfCrashPlugin,
            saves::MfSavesPlugin,
            verify::MfVerifyPlugin,
            sidecar_kill_test::MfSidecarKillTestPlugin,
            MfQualityBootPlugin,
            MfThemeBootPlugin,
            audio::MfAudioPlugin,
            command_bus::MfCommandBusPlugin,
            tools::MfToolsPlugin,
            build_ui::MfBuildUiPlugin,
        ))
        .add_plugins(routes_panel::MfRoutesPanelPlugin)
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
            atmosphere_shots::MfAtmosphereShotsPlugin,
            tutorial::MfTutorialPlugin,
            goals::MfGoalsPlugin,
            debug_overlay::MfDebugOverlayPlugin,
            soak::MfSoakPlugin,
            perf::MfPerfPlugin,
            egui_idle::MfEguiIdlePlugin,
        ))
        .add_plugins(MfGraphicsPerfPlugin);
    // MF_PERF / MF_PERF_LOG: Bevy diagnostic plugins + spans. MF_PERF also
    // drives the 60s sample-then-exit harness in `perf.rs`.
    // (FrameTimeDiagnosticsPlugin is already registered by
    // MfGraphicsPerfPlugin for the FPS overlay + benchmark.)
    if std::env::var_os("MF_PERF").is_some() || std::env::var_os("MF_PERF_LOG").is_some() {
        app.add_plugins((
            bevy::diagnostic::EntityCountDiagnosticsPlugin,
            bevy::diagnostic::LogDiagnosticsPlugin::default(),
        ));
        // Keep a longer frame-time history so MF_PERF percentiles are stable.
        app.insert_resource(perf::EguiPerfTimer::default());
    }
    app.run();
}
