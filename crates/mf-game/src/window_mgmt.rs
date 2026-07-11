//! Window chrome for a good desktop citizen: DPI-aware sizing, borderless
//! fullscreen toggle (F11 / Alt+Enter) persisted to config, remember
//! size/position across sessions, and pause the render loop when the
//! window is unfocused/minimized so alt-tab doesn't burn the GPU.
//!
//! # DPI
//!
//! Bevy/`winit` already apply the OS HiDPI factor to the window's logical
//! size (`Window::scale_factor`). We deliberately do **not** call
//! `WindowResolution::with_scale_factor_override` — overriding to `1.0`
//! would make both the 3D viewport and egui render at physical pixels on
//! a 150%/200% display (tiny UI, wrong mouse mapping). `bevy_egui` reads
//! the same winit scale factor into `pixels_per_point`, so egui chrome
//! and the 3D surface stay in lockstep.

use std::time::Duration;

use bevy::prelude::*;
use bevy::window::{
    MonitorSelection, PrimaryWindow, Window, WindowMode, WindowPosition, WindowResolution,
};
use bevy::winit::{UpdateMode, WinitSettings};

use crate::config::MfConfig;

/// Default logical size used when config has no saved geometry and
/// `MF_RESOLUTION` is unset.
pub const DEFAULT_WIDTH: f32 = 1440.0;
pub const DEFAULT_HEIGHT: f32 = 900.0;

pub struct MfWindowPlugin;

impl Plugin for MfWindowPlugin {
    fn build(&self, app: &mut App) {
        // Continuous while focused (game); essentially pause when
        // alt-tabbed / minimized. `Duration::MAX` means "only wake on a
        // window event" — no periodic redraw, so the GPU idles.
        //
        // Harness/CI escape hatch: under Xvfb there is no window manager,
        // the window is never focused, and `Duration::MAX` parks the main
        // loop forever right after boot (v0.5.1 release-gate finding - the
        // in-city CI smoke froze at "Loading city"). Any of the harness env
        // vars forces Continuous regardless of focus.
        let harness = std::env::var_os("MF_AUTOSTART").is_some()
            || std::env::var_os("MF_VERIFY_DIR").is_some()
            || std::env::var_os("MF_QUALITY").is_some();
        let unfocused_mode = if harness {
            UpdateMode::Continuous
        } else {
            UpdateMode::reactive_low_power(Duration::MAX)
        };
        app.insert_resource(WinitSettings {
            focused_mode: UpdateMode::Continuous,
            unfocused_mode,
        })
        .add_systems(
            Update,
            (
                log_dpi_once_system,
                toggle_fullscreen_system,
                persist_window_geometry_system,
            ),
        );
    }
}

/// Build the primary [`Window`] from persisted config + optional
/// `MF_RESOLUTION=WxH` override (promo/screenshot runs).
pub fn window_from_config(config: &MfConfig) -> Window {
    let (width, height) = std::env::var("MF_RESOLUTION")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
        })
        .unwrap_or_else(|| {
            (
                config.window_width.unwrap_or(DEFAULT_WIDTH),
                config.window_height.unwrap_or(DEFAULT_HEIGHT),
            )
        });

    let position = match (config.window_x, config.window_y) {
        (Some(x), Some(y)) => WindowPosition::At(IVec2::new(x, y)),
        _ => WindowPosition::Automatic,
    };

    let mode = if config.borderless_fullscreen {
        WindowMode::BorderlessFullscreen(MonitorSelection::Current)
    } else {
        WindowMode::Windowed
    };

    Window {
        title: "MetroForge".to_string(),
        resolution: WindowResolution::new(width, height),
        position,
        mode,
        present_mode: bevy::window::PresentMode::AutoVsync,
        // Leave scale_factor_override unset so winit's OS HiDPI factor
        // drives both the 3D viewport and egui (see module docs).
        ..default()
    }
}

fn log_dpi_once_system(mut done: Local<bool>, windows: Query<&Window, With<PrimaryWindow>>) {
    if *done {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    // scale_factor is 1.0 until the backend reports the real HiDPI factor;
    // wait until it differs from the unset default *or* the window has a
    // non-zero physical size (backend has finished creating it).
    if window.physical_width() == 0 {
        return;
    }
    *done = true;
    tracing::info!(
        "mf-game: window dpi scale_factor={:.2} logical={}x{} physical={}x{}",
        window.scale_factor(),
        window.width(),
        window.height(),
        window.physical_width(),
        window.physical_height()
    );
}

fn toggle_fullscreen_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut config: ResMut<MfConfig>,
) {
    let f11 = keys.just_pressed(KeyCode::F11);
    let alt_enter = keys.just_pressed(KeyCode::Enter)
        && (keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight));
    if !f11 && !alt_enter {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };

    let going_fullscreen = !matches!(window.mode, WindowMode::BorderlessFullscreen(_));
    if going_fullscreen {
        // Snapshot windowed geometry before the mode switch overwrites
        // resolution with the monitor's physical size.
        snapshot_windowed_geometry(&window, &mut config);
        window.mode = WindowMode::BorderlessFullscreen(MonitorSelection::Current);
        config.set_borderless_fullscreen(true);
    } else {
        window.mode = WindowMode::Windowed;
        if let (Some(w), Some(h)) = (config.window_width, config.window_height) {
            window.resolution.set(w, h);
        }
        if let (Some(x), Some(y)) = (config.window_x, config.window_y) {
            window.position = WindowPosition::At(IVec2::new(x, y));
        }
        config.set_borderless_fullscreen(false);
    }
}

fn persist_window_geometry_system(
    mut config: ResMut<MfConfig>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut moved: EventReader<bevy::window::WindowMoved>,
    mut resized: EventReader<bevy::window::WindowResized>,
) {
    let moved_any = moved.read().count() > 0;
    let resized_any = resized.read().count() > 0;
    if !moved_any && !resized_any {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    // Don't clobber the saved windowed size with the fullscreen monitor
    // resolution while borderless.
    if matches!(window.mode, WindowMode::BorderlessFullscreen(_)) {
        return;
    }
    snapshot_windowed_geometry(window, &mut config);
    if let Err(e) = config.save() {
        tracing::warn!("mf-game: failed to persist window geometry: {e}");
    }
}

fn snapshot_windowed_geometry(window: &Window, config: &mut MfConfig) {
    config.window_width = Some(window.width());
    config.window_height = Some(window.height());
    if let WindowPosition::At(pos) = window.position {
        config.window_x = Some(pos.x);
        config.window_y = Some(pos.y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_from_config_defaults_without_saved_geometry() {
        let config = MfConfig::default();
        let window = window_from_config(&config);
        assert_eq!(window.title, "MetroForge");
        assert_eq!(window.width(), DEFAULT_WIDTH);
        assert_eq!(window.height(), DEFAULT_HEIGHT);
        assert!(matches!(window.mode, WindowMode::Windowed));
        assert!(window.resolution.scale_factor_override().is_none());
    }

    #[test]
    fn window_from_config_applies_saved_geometry_and_fullscreen() {
        let mut config = MfConfig::default();
        config.window_width = Some(1280.0);
        config.window_height = Some(720.0);
        config.window_x = Some(64);
        config.window_y = Some(48);
        config.borderless_fullscreen = true;
        let window = window_from_config(&config);
        assert_eq!(window.width(), 1280.0);
        assert_eq!(window.height(), 720.0);
        assert_eq!(window.position, WindowPosition::At(IVec2::new(64, 48)));
        assert!(matches!(
            window.mode,
            WindowMode::BorderlessFullscreen(MonitorSelection::Current)
        ));
    }
}
