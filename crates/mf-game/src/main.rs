//! MetroForge Native game shell (spec §3.4). Binary name: `metroforge`.
// Windows: GUI subsystem in release so no console window opens behind the game.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod build_ui;
mod camera;
mod campaign;
mod command_bus;
mod config;
mod design_system;
mod hud;
mod input;
mod map_mode;
mod overlays;
mod panels;
mod quality_boot;
mod report_ui;
mod reveal_input;
mod saves;
mod state;
mod tools;
mod verify;

use bevy::prelude::*;
use bevy::window::{PresentMode, Window, WindowPlugin};
use mf_net::MfNetPlugin;
use mf_render::MfRenderPlugin;
use mf_state::MfStatePlugin;
use quality_boot::MfQualityBootPlugin;

// Art-direction §1: SKY_DAY as the default clear color.
const SKY_DAY: Color = Color::srgb(
    0xdf as f32 / 255.0,
    0xe6 as f32 / 255.0,
    0xea as f32 / 255.0,
);

fn main() {
    let mut app = App::new();
    app.insert_resource(ClearColor(SKY_DAY))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "MetroForge".to_string(),
                resolution: (1440.0_f32, 900.0_f32).into(),
                present_mode: PresentMode::AutoVsync,
                ..default()
            }),
            ..default()
        }))
        .add_plugins((MfNetPlugin, MfStatePlugin, MfRenderPlugin))
        .add_plugins((
            state::MfGameStatePlugin,
            camera::MfCameraPlugin,
            input::MfInputPlugin,
            reveal_input::MfRevealInputPlugin,
            hud::MfHudPlugin,
            saves::MfSavesPlugin,
            verify::MfVerifyPlugin,
            MfQualityBootPlugin,
            audio::MfAudioPlugin,
            command_bus::MfCommandBusPlugin,
            tools::MfToolsPlugin,
            build_ui::MfBuildUiPlugin,
            overlays::MfOverlaysPlugin,
            map_mode::MfMapModePlugin,
            panels::MfPanelsPlugin,
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
