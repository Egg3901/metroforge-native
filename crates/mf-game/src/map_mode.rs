//! Top-down map mode (ship-plan #25, v0.3): `KeyM` toggles between the
//! normal RTS camera angle and a near-vertical, north-up overview.
//!
//! Keybind ownership: `M` is claimed HERE this wave (contention avoidance -
//! `input.rs` already owns Tab/Esc, `tools.rs` owns the build-tool hotkeys;
//! `M` was unclaimed by either, verified by grep before wiring this up).
//!
//! Drives the EXISTING `camera::CameraRig` (owned by `camera.rs`, spec
//! §3.4) rather than introducing a second camera path - this module only
//! reads `CameraRig`'s public fields and writes its `_goal` twins, never
//! `camera.rs` internals. Per `camera.rs`'s own module doc: every input
//! system except an active drag writes the `_goal` fields and lets
//! `camera_smoothing_system` ease the real value toward them each frame;
//! only `zoom_to_fit_on_enter` and the verify harness assign the raw
//! `target`/`yaw`/`pitch`/`distance` directly (screenshots can't wait out a
//! smoothing curve), and `camera_smoothing_system`'s external-write
//! detection (`advance_smoothing`, comparing against `RigLastOutput`) exists
//! specifically to snap the goal to match those direct writes without
//! fighting them. This module deliberately takes the FIRST path (goal-only
//! writes) both entering and exiting map mode, so the transition eases in
//! over the normal orbit-smoothing curve (`ORBIT_SMOOTH_RATE`/
//! `DOLLY_SMOOTH_RATE`) instead of snapping like a verify-harness frame
//! would - confirmed by reading `camera.rs` before wiring this, not
//! guessed: writing the raw fields here would just get treated as yet
//! another external write and re-smoothed from THAT value, which is not
//! what a deliberate "ease to map view" toggle should look like.
//!
//! `camera_input_system` (orbit/pan/dolly) still runs unmodified while map
//! mode is active. In particular, right-drag orbit writes `yaw`/`pitch`
//! directly as part of its "active drag" 1:1 path (by design, so a drag
//! never feels laggy) and would un-level the north-up/near-vertical framing
//! this module just eased into. ACCEPTABLE for v0.3 (explicitly not fixed
//! here, per mission scope): a player who orbits while map mode is active
//! simply drifts away from the intended framing rather than getting stuck
//! or crashing anything - deciding whether a future wave suppresses orbit
//! input while active, or treats any manual orbit as an implicit "exit map
//! mode", is left as a follow-up.
//!
//! Reveal (cursor-driven fog-of-war) needs no special handling while active:
//! it's already purely cursor-position-driven (`reveal_input.rs`, out of
//! this module's scope) and reads correctly from directly overhead the same
//! as from any other camera angle.
//!
//! Map intelligence overlay: while active, `map_mode_overlay_system` paints
//! bridge casing, dashed tunnels, POI anchor glyphs, and named-bridge labels
//! (at high zoom) over the 3D view via egui — shared helpers in `map_paint.rs`.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use mf_render::palette;
use mf_state::{CurrentCity, LatestFields, Theme};

use crate::camera::CameraRig;
use crate::design_system;
use crate::map_paint::{
    build_water_land_image, map_roads_from_dtos, map_zoom_t, paint_bridge_labels, paint_map_roads,
    paint_poi_anchors, square_map_rect, MapRoadColors, BASE_IMAGE_RES,
};
use crate::state::AppState;

/// Pitch (radians) map mode eases the camera toward. Deliberately
/// near-vertical rather than exactly `FRAC_PI_2` (straight down): a
/// perfectly overhead pitch flattens `camera_transform_system`'s horizontal
/// offset (`distance * pitch.cos()`) to zero, which is fine for the ribbon
/// geometry but leaves no sense of "forward" for a future north-indicator/
/// compass overlay to hang off of.
pub const MAP_MODE_PITCH: f32 = 1.52;
/// Yaw map mode eases toward. `0.0` is already north-up in
/// `camera_transform_system`'s convention (yaw=0 sits the camera on `+Z`
/// looking back toward `-Z`), so this is a plain reset to that existing
/// convention, not a new one.
pub const MAP_MODE_YAW: f32 = 0.0;
/// Floor on the distance map mode eases toward. A `max(current, ..)`, never
/// a hard set: this only ever pushes the camera OUT to at least an
/// overview-friendly range, it never zooms a player who's already further
/// out than this back in.
pub const MAP_MODE_MIN_DISTANCE: f32 = 2500.0;

/// Rig state captured the instant map mode is entered, restored on exit.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SavedRig {
    target: Vec2,
    yaw: f32,
    pitch: f32,
    distance: f32,
}

/// Map-mode on/off + the rig snapshot to restore on exit. `saved.is_some()`
/// IS the "active" flag (see [`MapModeState::is_active`]) rather than a
/// separate `bool` alongside it, so there is no representable state where
/// "active" and "have a rig to restore" disagree.
#[derive(Resource, Default)]
pub struct MapModeState {
    saved: Option<SavedRig>,
}

impl MapModeState {
    /// Whether map mode is currently active. Exposed for any future system
    /// (a HUD "MAP" badge, `reveal_input.rs`, etc.) that wants to react to
    /// the mode without reaching into this module's private fields.
    pub fn is_active(&self) -> bool {
        self.saved.is_some()
    }
}

/// Pure goal computation for ENTERING map mode, given the rig's current
/// (pre-toggle) distance. Split out from the system so the distance-floor
/// math is unit-testable without spinning up a Bevy `App`/`Query`, same
/// convention `camera.rs` uses for `apply_wheel_dolly`/`pan_release_step`.
/// Returns `(yaw_goal, pitch_goal, distance_goal)`.
fn enter_goals(current_distance: f32) -> (f32, f32, f32) {
    (
        MAP_MODE_YAW,
        MAP_MODE_PITCH,
        current_distance.max(MAP_MODE_MIN_DISTANCE),
    )
}

pub struct MfMapModePlugin;

impl Plugin for MfMapModePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapModeState>()
            .init_resource::<MapModeOverlayCache>()
            .add_systems(
                Update,
                (
                    map_mode_toggle_system,
                    rebuild_map_overlay_base_system,
                    map_mode_overlay_system,
                )
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

#[derive(Resource, Default)]
struct MapModeOverlayCache {
    base_key: Option<(u32, Theme)>,
    base_texture: Option<egui::TextureHandle>,
    roads_key: Option<usize>,
    roads: Vec<crate::map_paint::MapRoadSegment>,
}

fn map_mode_toggle_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut map_mode: ResMut<MapModeState>,
    mut rigs: Query<&mut CameraRig>,
) {
    if !keys.just_pressed(KeyCode::KeyM) {
        return;
    }
    let Ok(mut rig) = rigs.single_mut() else {
        return;
    };

    match map_mode.saved {
        None => {
            map_mode.saved = Some(SavedRig {
                target: rig.target,
                yaw: rig.yaw,
                pitch: rig.pitch,
                distance: rig.distance,
            });
            let (yaw_goal, pitch_goal, distance_goal) = enter_goals(rig.distance);
            rig.yaw_goal = yaw_goal;
            rig.pitch_goal = pitch_goal;
            rig.distance_goal = distance_goal;
        }
        Some(saved) => {
            rig.target_goal = saved.target;
            rig.yaw_goal = saved.yaw;
            rig.pitch_goal = saved.pitch;
            rig.distance_goal = saved.distance;
            map_mode.saved = None;
        }
    }
}

fn rebuild_map_overlay_base_system(
    map_mode: Res<MapModeState>,
    mut contexts: EguiContexts,
    fields: Res<LatestFields>,
    city: Res<CurrentCity>,
    theme: Res<Theme>,
    mut cache: ResMut<MapModeOverlayCache>,
) {
    if !map_mode.is_active() {
        return;
    }
    let Some(fields) = fields.0.as_ref() else {
        return;
    };
    let Some(static_city) = city.static_city.as_ref() else {
        return;
    };
    let key = (fields.version, *theme);
    if cache.base_key == Some(key) {
        return;
    }
    let field_w = static_city.field_w as usize;
    let field_h = static_city.field_h as usize;
    if field_w == 0 || field_h == 0 || fields.water.len() != field_w * field_h {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let image = build_water_land_image(
        &fields.water,
        field_w,
        field_h,
        BASE_IMAGE_RES,
        color32_from(palette::ground()),
        color32_from(palette::water()),
    );
    let handle = ctx.load_texture("map_mode_base", image, egui::TextureOptions::NEAREST);
    cache.base_texture = Some(handle);
    cache.base_key = Some(key);
}

fn map_mode_overlay_system(
    map_mode: Res<MapModeState>,
    mut contexts: EguiContexts,
    city: Res<CurrentCity>,
    mut cache: ResMut<MapModeOverlayCache>,
    rigs: Query<&CameraRig>,
) -> Result {
    if !map_mode.is_active() {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let Some(static_city) = city.static_city.as_ref() else {
        return Ok(());
    };
    let Ok(rig) = rigs.single() else {
        return Ok(());
    };

    let world_half = (static_city.world_size as f32 / 2.0).max(1.0);
    let zoom_t = map_zoom_t(rig.distance, world_half);
    let s = crate::strings::current();

    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("map_mode_overlay"))
        .order(egui::Order::Background)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.set_width(screen.width());
            ui.set_height(screen.height());
            let (rect, _response) = ui.allocate_exact_size(screen.size(), egui::Sense::hover());
            let map_rect = square_map_rect(rect);
            let painter = ui.painter_at(rect);

            if let Some(tex) = cache.base_texture.as_ref() {
                painter.image(
                    tex.id(),
                    map_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                painter.rect_filled(map_rect, 0.0, color32_from(palette::ground()));
            }

            map_overlay_roads(&mut cache, static_city);
            let roads = &cache.roads;
            let colors = map_road_colors();
            paint_map_roads(&painter, map_rect, roads, world_half, colors, 1.0);
            if let Some(anchors) = static_city.poi_anchors.as_deref() {
                paint_poi_anchors(&painter, ui, map_rect, anchors, world_half, zoom_t);
            }
            paint_bridge_labels(
                &painter,
                ui,
                map_rect,
                roads,
                world_half,
                zoom_t,
                design_system::WHITE,
            );

            let badge = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 12.0, rect.top() + 12.0),
                egui::vec2(52.0, 24.0),
            );
            painter.rect_filled(badge, 0.0, design_system::SURFACE);
            painter.rect_stroke(
                badge,
                egui::CornerRadius::ZERO,
                egui::Stroke::new(design_system::ACCENT_EDGE_PX, design_system::ACCENT),
                egui::StrokeKind::Inside,
            );
            painter.text(
                badge.center(),
                egui::Align2::CENTER_CENTER,
                s.map_mode_badge,
                design_system::body_font(design_system::TEXT_XS),
                design_system::WHITE,
            );
        });
    Ok(())
}

fn map_overlay_roads(cache: &mut MapModeOverlayCache, static_city: &mf_protocol::StaticCityJson) {
    let fingerprint = static_city.roads.len()
        ^ (static_city.field_w as usize).wrapping_mul(31)
        ^ (static_city.field_h as usize).wrapping_mul(97);
    if cache.roads_key != Some(fingerprint) {
        cache.roads = map_roads_from_dtos(&static_city.roads);
        cache.roads_key = Some(fingerprint);
    }
}

fn map_road_colors() -> MapRoadColors {
    let road = color32_from(palette::road()).gamma_multiply(0.55);
    let edge = color32_from(palette::road_edge());
    MapRoadColors {
        ground: color32_from(palette::ground()),
        road,
        bridge_fill: road,
        bridge_casing: edge,
        tunnel: road.gamma_multiply(0.45),
    }
}

fn color32_from(color: Color) -> egui::Color32 {
    let srgba = color.to_srgba();
    egui::Color32::from_rgba_unmultiplied(
        (srgba.red * 255.0).round() as u8,
        (srgba.green * 255.0).round() as u8,
        (srgba.blue * 255.0).round() as u8,
        (srgba.alpha * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_goals_raises_distance_to_floor_when_zoomed_in() {
        let (yaw, pitch, distance) = enter_goals(800.0);
        assert_eq!(yaw, MAP_MODE_YAW);
        assert_eq!(pitch, MAP_MODE_PITCH);
        assert_eq!(distance, MAP_MODE_MIN_DISTANCE);
    }

    #[test]
    fn enter_goals_keeps_distance_when_already_zoomed_out_further() {
        let (_, _, distance) = enter_goals(9_000.0);
        assert_eq!(distance, 9_000.0);
    }

    #[test]
    fn enter_goals_at_exactly_the_floor_is_a_no_op() {
        let (_, _, distance) = enter_goals(MAP_MODE_MIN_DISTANCE);
        assert_eq!(distance, MAP_MODE_MIN_DISTANCE);
    }

    #[test]
    fn map_mode_state_defaults_to_inactive() {
        assert!(!MapModeState::default().is_active());
    }
}
