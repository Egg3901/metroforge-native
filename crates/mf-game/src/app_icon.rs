//! Window/taskbar icon (owner feedback: "we dont use our logo from
//! website as app logo... anywhere in app"). Sets the OS window icon from
//! the same wordmark geometry `hud.rs`'s `draw_logo` paints on the title
//! screen (`metroforge/index.html`'s favicon SVG: dark badge, four
//! colored spokes, ringed hub) — built as a raw RGBA buffer at startup
//! rather than shipping a bitmap asset, for the same "no image-loading
//! dependency yet" reason `draw_logo` gives.
//!
//! Packaged distributables (`.ico`/`.icns`/`.desktop` icons baked into the
//! Windows/macOS/Linux bundles themselves, referenced from
//! `scripts/package.sh`/`release.yml`) are a separate, larger piece of
//! work — those pipelines don't reference any icon file today, so wiring
//! them up is left as a follow-up (see the tracking issue) rather than
//! bolted on here; this module only covers the *running* window/taskbar
//! icon, which every platform picks up from `winit` at runtime with no
//! packaging changes.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy::winit::WinitWindows;

const ICON_SIZE: u32 = 64;

pub struct MfAppIconPlugin;

impl Plugin for MfAppIconPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, set_window_icon_once);
    }
}

/// Runs every frame but only actually does anything once — `WinitWindows`
/// doesn't populate its entity map until sometime after `Startup` (the
/// window is created by the winit backend asynchronously), so a plain
/// `Startup` system is too early on at least some platforms. Local<bool>
/// latches it done the first frame a primary window is found.
fn set_window_icon_once(
    windows: NonSend<WinitWindows>,
    primary: Query<Entity, With<PrimaryWindow>>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    let Ok(entity) = primary.single() else {
        return;
    };
    let Some(winit_window) = windows.get_window(entity) else {
        return;
    };
    *done = true;
    let rgba = render_icon_rgba(ICON_SIZE);
    match winit::window::Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE) {
        Ok(icon) => winit_window.set_window_icon(Some(icon)),
        Err(e) => tracing::warn!("mf-game: failed to build window icon: {e}"),
    }
}

/// Same geometry as `hud.rs::draw_logo`, rasterized into a `size x size`
/// RGBA8 buffer: dark rounded-square badge, four colored spokes from
/// center to each corner, a ringed hub circle. Kept in lockstep with that
/// function by eye (both trace the same source SVG) — a shared helper
/// would need to abstract over "egui painter" vs "raw pixel buffer",
/// which isn't worth it for four line segments and a circle.
fn render_icon_rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let bg = [0x0b, 0x0d, 0x10, 0xff];
    let corners: [([u8; 4], (f32, f32)); 4] = [
        ([0x7e, 0xf2, 0x9a, 0xff], (-1.0, -1.0)),
        ([0x54, 0xd0, 0xff, 0xff], (1.0, -1.0)),
        ([0xff, 0xb6, 0x3d, 0xff], (1.0, 1.0)),
        ([0xff, 0x5d, 0x6c, 0xff], (-1.0, 1.0)),
    ];
    let center = (s / 2.0, s / 2.0);
    let arm = s * 0.35;
    let half_stroke = s * 0.05;
    let hub_r = s * 0.15;
    let hub_ring_inner = hub_r - s * 0.075;

    let mut buf = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let idx = ((y * size + x) * 4) as usize;

            // Ring hub: painted last (on top), so check first and continue.
            let dx = px - center.0;
            let dy = py - center.1;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= hub_r && dist >= hub_ring_inner {
                buf[idx..idx + 4].copy_from_slice(&[0xf4, 0xf4, 0xf5, 0xff]);
                continue;
            }
            if dist < hub_ring_inner {
                buf[idx..idx + 4].copy_from_slice(&bg);
                continue;
            }

            let mut pixel = bg;
            for (color, (dir_x, dir_y)) in corners {
                if point_near_segment(
                    px,
                    py,
                    center.0,
                    center.1,
                    center.0 + dir_x * arm,
                    center.1 + dir_y * arm,
                    half_stroke,
                ) {
                    pixel = color;
                    break;
                }
            }
            buf[idx..idx + 4].copy_from_slice(&pixel);
        }
    }
    buf
}

/// Point-to-segment distance test used to rasterize each of the four
/// spoke strokes without pulling in a 2D vector-graphics crate for one
/// icon.
#[allow(clippy::too_many_arguments)]
fn point_near_segment(px: f32, py: f32, x1: f32, y1: f32, x2: f32, y2: f32, half_w: f32) -> bool {
    let (dx, dy) = (x2 - x1, y2 - y1);
    let len_sq = dx * dx + dy * dy;
    let t = if len_sq > 0.0 {
        (((px - x1) * dx + (py - y1) * dy) / len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (cx, cy) = (x1 + t * dx, y1 + t * dy);
    let (ex, ey) = (px - cx, py - cy);
    (ex * ex + ey * ey).sqrt() <= half_w
}
