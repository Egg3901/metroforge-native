//! Bottom-right HUD minimap: a self-contained, cheap-to-run overview of the
//! city, drawn entirely with the egui painter (no render-to-texture) inside
//! a collapsible ~220px window.
//!
//! Layers, cheapest-to-draw-first:
//! 1. A cached low-res water/land image, rebuilt only when `LatestFields`'
//!    version or the active `Theme` changes (`rebuild_base_image_system`).
//! 2. Faint arterial road hairlines, cached once per city load
//!    (`rebuild_roads_cache_system`) since `StaticCityJson.roads` never
//!    changes after a city loads.
//! 3. Transit routes (colored polylines, same vivid table `mf_render`'s
//!    world view uses) and station dots, rebuilt only when `LatestUi`
//!    changes (2 Hz network cadence, cheap to just redo every tick it
//!    ticks).
//! 4. A camera viewport indicator quad, plus click/drag-to-pan.
//!
//! `N` toggles the minimap collapsed/expanded (persisted via `MfConfig`,
//! see its doc comment for why not `M`). Everything above is skipped
//! outright when collapsed, so a collapsed minimap costs one `if` per
//! frame beyond the toggle-key check.
//!
//! ## World <-> minimap coordinate convention
//!
//! The minimap always draws a SQUARE sub-area (matching `camera.rs`'s
//! symmetric `[-world_half, world_half]` pan-clamp bounds) centered in
//! whatever rect egui hands the drawing closure, letterboxed if that rect
//! isn't square itself. World +X maps to minimap +X (right); world +Y
//! (Bevy's +Z, "north" in the top-down `map_mode.rs` framing) maps to
//! minimap UP, i.e. minimap -Y, matching a conventional north-up map.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use mf_protocol::TransitMode;
use mf_render::palette;
use mf_state::{CurrentCity, LatestFields, LatestUi, Theme};

use crate::camera::CameraRig;
use crate::config::MfConfig;
use crate::state::AppState;

/// Minimap window side length in egui points. Small and fixed, per the
/// mission's "~220px" target.
const MINIMAP_SIZE: f32 = 220.0;
/// Low-res base image side length in texels. City shape reads fine at this
/// resolution and it keeps the per-rebuild cost (and the uploaded texture)
/// tiny; rebuilds are already gated to fields-version/theme changes, not
/// per-frame, so this isn't even on the hot path.
const BASE_IMAGE_RES: usize = 96;

pub struct MfMinimapPlugin;

impl Plugin for MfMinimapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MinimapCache>().add_systems(
            Update,
            (
                minimap_toggle_system,
                rebuild_base_image_system,
                rebuild_roads_cache_system,
                rebuild_transit_cache_system,
                minimap_ui_system,
            )
                .chain()
                .run_if(in_state(AppState::InGame)),
        );
    }
}

/// One cached arterial road polyline in world space (points only; color is
/// resolved from the active theme at draw time so a theme switch doesn't
/// need a rebuild).
type WorldPolyline = Vec<Vec2>;

/// One cached transit route: pre-resolved color and world-space station
/// path, both already vivid-palette-indexed so `minimap_ui_system` doesn't
/// touch `LatestUi` at all when nothing changed.
struct CachedRoute {
    color: egui::Color32,
    points: Vec<Vec2>,
}

struct CachedStation {
    pos: Vec2,
    mode: TransitMode,
}

#[derive(Resource, Default)]
struct MinimapCache {
    /// `(fields_version, theme)` the current `base_texture` was built for.
    base_key: Option<(u32, Theme)>,
    base_texture: Option<egui::TextureHandle>,
    /// Arterial roads never change after a city loads, so this is built
    /// once (`roads_built_for` guards against rebuilding every frame) and
    /// left alone until the city itself changes.
    roads_built_for: Option<usize>,
    roads: Vec<WorldPolyline>,
    /// Bumped on every `LatestUi` change; compared against `LatestUi`'s own
    /// change tick isn't available across systems cheaply, so this just
    /// mirrors the resource's `is_changed()` each frame it runs.
    routes: Vec<CachedRoute>,
    stations: Vec<CachedStation>,
}

fn minimap_toggle_system(keys: Res<ButtonInput<KeyCode>>, mut config: ResMut<MfConfig>) {
    if !keys.just_pressed(KeyCode::KeyN) {
        return;
    }
    let open = !config.minimap_open;
    config.set_minimap_open(open);
}

/// Rebuilds the cached water/land base image (and uploads it as an egui
/// texture) only when the fields version or the active theme has moved
/// since the last build.
fn rebuild_base_image_system(
    mut contexts: EguiContexts,
    fields: Res<LatestFields>,
    city: Res<CurrentCity>,
    theme: Res<Theme>,
    config: Res<MfConfig>,
    mut cache: ResMut<MinimapCache>,
) {
    if !config.minimap_open {
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

    let ground = color32_from(palette::ground());
    let water = color32_from(palette::water());
    let mut pixels = Vec::with_capacity(BASE_IMAGE_RES * BASE_IMAGE_RES);
    for py in 0..BASE_IMAGE_RES {
        // Nearest-neighbor downsample; flip Y so the base image is
        // north-up like everything else drawn on the minimap.
        let gy = ((BASE_IMAGE_RES - 1 - py) * field_h / BASE_IMAGE_RES).min(field_h - 1);
        for px in 0..BASE_IMAGE_RES {
            let gx = (px * field_w / BASE_IMAGE_RES).min(field_w - 1);
            let is_water = fields.water[gy * field_w + gx] != 0;
            pixels.push(if is_water { water } else { ground });
        }
    }
    let image = egui::ColorImage {
        size: [BASE_IMAGE_RES, BASE_IMAGE_RES],
        source_size: egui::vec2(BASE_IMAGE_RES as f32, BASE_IMAGE_RES as f32),
        pixels,
    };
    let handle = ctx.load_texture("minimap_base", image, egui::TextureOptions::NEAREST);
    cache.base_texture = Some(handle);
    cache.base_key = Some(key);
}

/// Rebuilds the arterial-road world-space polyline cache once per city load
/// (`StaticCityJson.roads` is immutable after that). Guarded by the
/// `static_city` pointer identity via a cheap per-city counter substitute:
/// road count + field dims, good enough to detect "a different city loaded"
/// without threading a dedicated version number through `CurrentCity`.
fn rebuild_roads_cache_system(
    city: Res<CurrentCity>,
    config: Res<MfConfig>,
    mut cache: ResMut<MinimapCache>,
) {
    if !config.minimap_open {
        return;
    }
    let Some(static_city) = city.static_city.as_ref() else {
        cache.roads_built_for = None;
        cache.roads.clear();
        return;
    };
    let fingerprint = static_city.roads.len()
        ^ (static_city.field_w as usize).wrapping_mul(31)
        ^ (static_city.field_h as usize).wrapping_mul(97);
    if cache.roads_built_for == Some(fingerprint) {
        return;
    }
    cache.roads = static_city
        .roads
        .iter()
        .filter(|r| r.cls == "arterial")
        .map(|r| {
            r.points
                .chunks_exact(2)
                .map(|xy| Vec2::new(xy[0] as f32, xy[1] as f32))
                .collect::<Vec<_>>()
        })
        .collect();
    cache.roads_built_for = Some(fingerprint);
}

/// Rebuilds the routes/stations cache whenever `LatestUi` changes (2 Hz
/// network cadence). Cheap: a handful of stations/routes, no allocation
/// beyond the small point vectors themselves.
fn rebuild_transit_cache_system(
    ui_state: Res<LatestUi>,
    config: Res<MfConfig>,
    mut cache: ResMut<MinimapCache>,
) {
    if !config.minimap_open || !ui_state.is_changed() {
        return;
    }
    let Some(state) = ui_state.0.as_ref() else {
        cache.routes.clear();
        cache.stations.clear();
        return;
    };

    cache.stations = state
        .stations
        .iter()
        .map(|s| CachedStation {
            pos: Vec2::new(s.x as f32, s.y as f32),
            mode: s.mode,
        })
        .collect();

    cache.routes = state
        .routes
        .iter()
        .enumerate()
        .map(|(idx, route)| {
            let points = route
                .station_ids
                .iter()
                .filter_map(|id| {
                    state
                        .stations
                        .iter()
                        .find(|s| s.id == *id)
                        .map(|s| Vec2::new(s.x as f32, s.y as f32))
                })
                .collect();
            CachedRoute {
                color: color32_from(palette::vivid_route_color(idx)),
                points,
            }
        })
        .collect();
}

fn minimap_ui_system(
    mut contexts: EguiContexts,
    city: Res<CurrentCity>,
    mut config: ResMut<MfConfig>,
    cache: Res<MinimapCache>,
    mut rigs: Query<&mut CameraRig>,
) -> Result {
    let ctx = contexts.ctx_mut()?;

    let mut open = config.minimap_open;
    let response = egui::Window::new("Minimap")
        .id(egui::Id::new("hud_minimap"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
        .collapsible(true)
        .resizable(false)
        .open(&mut open)
        .default_open(true)
        .show(ctx, |ui| {
            let Some(static_city) = city.static_city.as_ref() else {
                ui.label("No city loaded");
                return;
            };
            let world_half = (static_city.world_size as f32 / 2.0).max(1.0);

            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(MINIMAP_SIZE, MINIMAP_SIZE),
                egui::Sense::click_and_drag(),
            );
            let map_rect = square_map_rect(rect);
            let painter = ui.painter_at(rect);

            if let Some(tex) = &cache.base_texture {
                painter.image(
                    tex.id(),
                    map_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                painter.rect_filled(map_rect, 0.0, color32_from(palette::ground()));
            }

            let road_color = color32_from(palette::road()).gamma_multiply(0.35);
            for road in &cache.roads {
                if road.len() < 2 {
                    continue;
                }
                let pts: Vec<egui::Pos2> = road
                    .iter()
                    .map(|w| world_to_minimap(*w, world_half, map_rect))
                    .collect();
                painter.add(egui::Shape::line(pts, egui::Stroke::new(1.0, road_color)));
            }

            for route in &cache.routes {
                if route.points.len() < 2 {
                    continue;
                }
                let pts: Vec<egui::Pos2> = route
                    .points
                    .iter()
                    .map(|w| world_to_minimap(*w, world_half, map_rect))
                    .collect();
                painter.add(egui::Shape::line(pts, egui::Stroke::new(1.6, route.color)));
            }

            for station in &cache.stations {
                let p = world_to_minimap(station.pos, world_half, map_rect);
                let color = color32_from(palette::mode_accent(station.mode));
                painter.circle_filled(p, 2.2, color);
            }

            if let Ok(rig) = rigs.single() {
                draw_viewport_indicator(&painter, rig, world_half, map_rect);
            }

            // Click or drag pans the main camera toward the clicked ground
            // point. Writes the smoothing GOAL only (never `target`
            // directly), so `camera_smoothing_system` eases in exactly like
            // a WASD pan rather than snapping — see `camera.rs`'s module
            // doc for why that distinction matters.
            if response.clicked() || response.dragged() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let world = minimap_to_world(pos, world_half, map_rect);
                    let clamped = world.clamp(Vec2::splat(-world_half), Vec2::splat(world_half));
                    if let Ok(mut rig) = rigs.single_mut() {
                        rig.target_goal = clamped;
                    }
                }
            }
        });

    if let Some(inner) = response {
        // A drag/click inside the minimap must not also fall through to
        // `camera_input_system`'s world-click handling; the egui window
        // already reports `wants_pointer_input()` while hovered/dragged, so
        // no extra bookkeeping is needed here beyond consuming the event
        // above.
        let _ = inner.response.id;
    }

    if open != config.minimap_open {
        config.set_minimap_open(open);
    }
    Ok(())
}

/// Draws a stylized quad approximating the camera's current ground
/// footprint. This is a cheap visual approximation (scaled/rotated by the
/// rig's yaw and distance), not a physical frustum raycast — accurate
/// enough for "here's roughly where you're looking" without paying for a
/// per-frame projection against the terrain.
fn draw_viewport_indicator(
    painter: &egui::Painter,
    rig: &CameraRig,
    world_half: f32,
    map_rect: egui::Rect,
) {
    // Wider than tall (typical landscape FOV), tightened at steep pitch
    // where the camera sees less ground into the distance.
    let half_w = (rig.distance * 0.5).min(world_half * 1.2);
    let half_h = (rig.distance * 0.32 * (1.4 - rig.pitch.clamp(0.0, 1.3))).max(rig.distance * 0.1);
    let corners_world = [
        Vec2::new(-half_w, -half_h),
        Vec2::new(half_w, -half_h),
        Vec2::new(half_w, half_h),
        Vec2::new(-half_w, half_h),
    ];
    let (sin, cos) = rig.yaw.sin_cos();
    let pts: Vec<egui::Pos2> = corners_world
        .iter()
        .map(|c| {
            let rotated = Vec2::new(c.x * cos - c.y * sin, c.x * sin + c.y * cos);
            world_to_minimap(rig.target + rotated, world_half, map_rect)
        })
        .collect();
    painter.add(egui::Shape::closed_line(
        pts,
        egui::Stroke::new(
            1.5,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200),
        ),
    ));
}

/// The largest centered square that fits inside `rect` — the minimap always
/// draws into this, letterboxing if `rect` itself isn't square (it always
/// is today at `MINIMAP_SIZE`x`MINIMAP_SIZE`, but the mapping stays correct
/// either way, which is what the coordinate-mapping unit tests below rely
/// on).
fn square_map_rect(rect: egui::Rect) -> egui::Rect {
    let size = rect.width().min(rect.height());
    egui::Rect::from_center_size(rect.center(), egui::vec2(size, size))
}

/// World -> minimap point mapping (see module doc for the axis
/// convention). `world_half` is half the square world extent
/// (`world_size / 2`), matching `camera.rs`'s pan-clamp bounds.
fn world_to_minimap(world: Vec2, world_half: f32, map_rect: egui::Rect) -> egui::Pos2 {
    let scale = map_rect.width() / (world_half * 2.0);
    let center = map_rect.center();
    egui::pos2(center.x + world.x * scale, center.y - world.y * scale)
}

/// Inverse of [`world_to_minimap`].
fn minimap_to_world(pos: egui::Pos2, world_half: f32, map_rect: egui::Rect) -> Vec2 {
    let scale = map_rect.width() / (world_half * 2.0);
    let center = map_rect.center();
    Vec2::new((pos.x - center.x) / scale, -(pos.y - center.y) / scale)
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

    fn rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(100.0, 200.0), egui::vec2(220.0, 220.0))
    }

    #[test]
    fn round_trips_arbitrary_points() {
        let map_rect = rect();
        let world_half = 5_000.0;
        for w in [
            Vec2::new(0.0, 0.0),
            Vec2::new(1234.5, -876.0),
            Vec2::new(-4999.0, 4999.0),
            Vec2::new(2500.0, 2500.0),
        ] {
            let p = world_to_minimap(w, world_half, map_rect);
            let back = minimap_to_world(p, world_half, map_rect);
            assert!((back.x - w.x).abs() < 0.01, "x: {back:?} vs {w:?}");
            assert!((back.y - w.y).abs() < 0.01, "y: {back:?} vs {w:?}");
        }
    }

    #[test]
    fn world_origin_maps_to_map_rect_center() {
        let map_rect = rect();
        let p = world_to_minimap(Vec2::ZERO, 5_000.0, map_rect);
        assert!((p.x - map_rect.center().x).abs() < 0.001);
        assert!((p.y - map_rect.center().y).abs() < 0.001);
    }

    #[test]
    fn corners_map_to_map_rect_corners_north_up() {
        let map_rect = rect();
        let world_half = 5_000.0;

        // North-west corner (min x, max y) -> minimap top-left.
        let nw = world_to_minimap(Vec2::new(-world_half, world_half), world_half, map_rect);
        assert!((nw.x - map_rect.left()).abs() < 0.01);
        assert!((nw.y - map_rect.top()).abs() < 0.01);

        // South-east corner (max x, min y) -> minimap bottom-right.
        let se = world_to_minimap(Vec2::new(world_half, -world_half), world_half, map_rect);
        assert!((se.x - map_rect.right()).abs() < 0.01);
        assert!((se.y - map_rect.bottom()).abs() < 0.01);
    }

    #[test]
    fn square_map_rect_letterboxes_a_wide_rect() {
        let wide = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 220.0));
        let squared = square_map_rect(wide);
        assert!((squared.width() - 220.0).abs() < 0.001);
        assert!((squared.height() - 220.0).abs() < 0.001);
        assert!((squared.center().x - wide.center().x).abs() < 0.001);
        assert!((squared.center().y - wide.center().y).abs() < 0.001);
    }

    #[test]
    fn square_map_rect_is_identity_for_a_square_rect() {
        let square = egui::Rect::from_min_size(egui::pos2(5.0, 5.0), egui::vec2(220.0, 220.0));
        let squared = square_map_rect(square);
        assert!((squared.width() - square.width()).abs() < 0.001);
        assert!((squared.min - square.min).length() < 0.001);
    }

    #[test]
    fn aspect_ratio_is_preserved_between_axes() {
        // A non-square map_rect would otherwise stretch world shapes; make
        // sure the scale factor is identical on both axes for a square
        // map_rect (the only shape `square_map_rect` ever hands the
        // world<->minimap mapping).
        let map_rect = rect();
        let world_half = 3_333.0;
        let px = world_to_minimap(Vec2::new(world_half, 0.0), world_half, map_rect);
        let py = world_to_minimap(Vec2::new(0.0, world_half), world_half, map_rect);
        let dx = (px.x - map_rect.center().x).abs();
        let dy = (map_rect.center().y - py.y).abs();
        assert!((dx - dy).abs() < 0.01, "dx={dx} dy={dy}");
    }
}
