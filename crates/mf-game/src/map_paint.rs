//! Shared water/land + arterial painting for the HUD minimap and the
//! city-select card previews. Both surfaces need the same north-up mapping
//! and the same downsample of a water grid into an egui texture; keeping
//! that logic here stops `minimap.rs` and `city_select.rs` from drifting.

use bevy::prelude::Vec2;
use bevy_egui::egui;

/// Default texel side length for a cached water/land base image. City shape
/// reads fine at this resolution and the upload stays tiny.
pub const BASE_IMAGE_RES: usize = 96;

/// One arterial polyline in world meters (x,y pairs already decoded).
pub type WorldPolyline = Vec<Vec2>;

/// Nearest-neighbor downsample of a row-major water grid into a square
/// north-up [`egui::ColorImage`]. World +Y (north) maps to image top, matching
/// [`world_to_map`] and the in-game minimap.
pub fn build_water_land_image(
    water: &[u8],
    field_w: usize,
    field_h: usize,
    out_res: usize,
    ground: egui::Color32,
    water_color: egui::Color32,
) -> egui::ColorImage {
    let mut pixels = Vec::with_capacity(out_res * out_res);
    if field_w == 0 || field_h == 0 || out_res == 0 || water.len() < field_w * field_h {
        pixels.resize(out_res * out_res, ground);
        return egui::ColorImage {
            size: [out_res, out_res],
            source_size: egui::vec2(out_res as f32, out_res as f32),
            pixels,
        };
    }
    for py in 0..out_res {
        let gy = ((out_res - 1 - py) * field_h / out_res).min(field_h - 1);
        for px in 0..out_res {
            let gx = (px * field_w / out_res).min(field_w - 1);
            let is_water = water[gy * field_w + gx] != 0;
            pixels.push(if is_water { water_color } else { ground });
        }
    }
    egui::ColorImage {
        size: [out_res, out_res],
        source_size: egui::vec2(out_res as f32, out_res as f32),
        pixels,
    }
}

/// Extract arterial road polylines from a `StaticCityJson`-style road list
/// (`cls == "arterial"`, flat x,y point arrays).
pub fn arterial_polylines_from_points<'a, I>(roads: I) -> Vec<WorldPolyline>
where
    I: IntoIterator<Item = (&'a str, &'a [f64])>,
{
    roads
        .into_iter()
        .filter(|(cls, _)| *cls == "arterial")
        .map(|(_, points)| {
            points
                .chunks_exact(2)
                .map(|xy| Vec2::new(xy[0] as f32, xy[1] as f32))
                .collect::<Vec<_>>()
        })
        .filter(|pts| pts.len() >= 2)
        .collect()
}

/// Same as [`arterial_polylines_from_points`] but for flat `f32` slices
/// (city-select catalog / hello preview arterials).
pub fn arterial_polylines_from_f32(roads: &[&[f32]]) -> Vec<WorldPolyline> {
    roads
        .iter()
        .map(|points| {
            points
                .chunks_exact(2)
                .map(|xy| Vec2::new(xy[0], xy[1]))
                .collect::<Vec<_>>()
        })
        .filter(|pts| pts.len() >= 2)
        .collect()
}

/// Burn arterial polylines into an existing square north-up pixel buffer
/// (in-place). Used by city-select to cache water+roads as one texture.
pub fn rasterize_arterials(
    pixels: &mut [egui::Color32],
    out_res: usize,
    arterials: &[WorldPolyline],
    world_half: f32,
    color: egui::Color32,
) {
    if out_res == 0 || pixels.len() < out_res * out_res || world_half <= 0.0 {
        return;
    }
    let map_rect = egui::Rect::from_min_size(
        egui::pos2(0.0, 0.0),
        egui::vec2(out_res as f32, out_res as f32),
    );
    for road in arterials {
        if road.len() < 2 {
            continue;
        }
        for pair in road.windows(2) {
            let a = world_to_map(pair[0], world_half, map_rect);
            let b = world_to_map(pair[1], world_half, map_rect);
            draw_line_px(pixels, out_res, a, b, color);
        }
    }
}

/// Build a single cached preview image: water/land base + arterials burned in.
#[allow(clippy::too_many_arguments)]
pub fn bake_city_preview_image(
    water: &[u8],
    field_w: usize,
    field_h: usize,
    arterials: &[WorldPolyline],
    world_size: f32,
    out_res: usize,
    ground: egui::Color32,
    water_color: egui::Color32,
    road_color: egui::Color32,
) -> egui::ColorImage {
    let mut image = build_water_land_image(water, field_w, field_h, out_res, ground, water_color);
    let world_half = (world_size / 2.0).max(1.0);
    rasterize_arterials(
        &mut image.pixels,
        out_res,
        arterials,
        world_half,
        road_color,
    );
    image
}

/// Paint a cached water/land texture (if any) plus live arterial hairlines
/// into `map_rect` — the in-game minimap path.
pub fn paint_water_land_and_arterials(
    painter: &egui::Painter,
    map_rect: egui::Rect,
    base_texture: Option<&egui::TextureHandle>,
    fallback_ground: egui::Color32,
    arterials: &[WorldPolyline],
    world_half: f32,
    road_color: egui::Color32,
) {
    if let Some(tex) = base_texture {
        painter.image(
            tex.id(),
            map_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        painter.rect_filled(map_rect, 0.0, fallback_ground);
    }

    for road in arterials {
        if road.len() < 2 {
            continue;
        }
        let pts: Vec<egui::Pos2> = road
            .iter()
            .map(|w| world_to_map(*w, world_half, map_rect))
            .collect();
        painter.add(egui::Shape::line(pts, egui::Stroke::new(1.0, road_color)));
    }
}

/// Largest centered square inside `rect` (letterbox when the rect isn't square).
pub fn square_map_rect(rect: egui::Rect) -> egui::Rect {
    let size = rect.width().min(rect.height());
    egui::Rect::from_center_size(rect.center(), egui::vec2(size, size))
}

/// World -> map point. World +X → right; world +Y (north) → up (map -Y).
pub fn world_to_map(world: Vec2, world_half: f32, map_rect: egui::Rect) -> egui::Pos2 {
    let scale = map_rect.width() / (world_half * 2.0);
    let center = map_rect.center();
    egui::pos2(center.x + world.x * scale, center.y - world.y * scale)
}

/// Inverse of [`world_to_map`].
pub fn map_to_world(pos: egui::Pos2, world_half: f32, map_rect: egui::Rect) -> Vec2 {
    let scale = map_rect.width() / (world_half * 2.0);
    let center = map_rect.center();
    Vec2::new((pos.x - center.x) / scale, -(pos.y - center.y) / scale)
}

fn draw_line_px(
    pixels: &mut [egui::Color32],
    res: usize,
    a: egui::Pos2,
    b: egui::Pos2,
    color: egui::Color32,
) {
    let x0 = a.x.round() as i32;
    let y0 = a.y.round() as i32;
    let x1 = b.x.round() as i32;
    let y1 = b.y.round() as i32;
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;
    let max_steps = (dx + dy.abs() + 2) as usize;
    for _ in 0..max_steps {
        if x >= 0 && y >= 0 && (x as usize) < res && (y as usize) < res {
            pixels[y as usize * res + x as usize] = color;
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
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
            let p = world_to_map(w, world_half, map_rect);
            let back = map_to_world(p, world_half, map_rect);
            assert!((back.x - w.x).abs() < 0.01, "x: {back:?} vs {w:?}");
            assert!((back.y - w.y).abs() < 0.01, "y: {back:?} vs {w:?}");
        }
    }

    #[test]
    fn world_origin_maps_to_map_rect_center() {
        let map_rect = rect();
        let p = world_to_map(Vec2::ZERO, 5_000.0, map_rect);
        assert!((p.x - map_rect.center().x).abs() < 0.001);
        assert!((p.y - map_rect.center().y).abs() < 0.001);
    }

    #[test]
    fn corners_map_to_map_rect_corners_north_up() {
        let map_rect = rect();
        let world_half = 5_000.0;
        let nw = world_to_map(Vec2::new(-world_half, world_half), world_half, map_rect);
        assert!((nw.x - map_rect.left()).abs() < 0.01);
        assert!((nw.y - map_rect.top()).abs() < 0.01);
        let se = world_to_map(Vec2::new(world_half, -world_half), world_half, map_rect);
        assert!((se.x - map_rect.right()).abs() < 0.01);
        assert!((se.y - map_rect.bottom()).abs() < 0.01);
    }

    #[test]
    fn square_map_rect_letterboxes_a_wide_rect() {
        let wide = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 220.0));
        let squared = square_map_rect(wide);
        assert!((squared.width() - 220.0).abs() < 0.001);
        assert!((squared.height() - 220.0).abs() < 0.001);
    }

    #[test]
    fn water_land_image_marks_water_cells() {
        // 2x2: top-left water (after north-up flip → bottom-left in image).
        let water = [1u8, 0, 0, 0];
        let img =
            build_water_land_image(&water, 2, 2, 2, egui::Color32::BLACK, egui::Color32::BLUE);
        assert_eq!(img.pixels.len(), 4);
        // py=0 samples gy=1 (north-up flip of top row) → water[2]=0 land
        // py=1 samples gy=0 → water[0]=1 water on left
        assert_eq!(img.pixels[2], egui::Color32::BLUE);
        assert_eq!(img.pixels[0], egui::Color32::BLACK);
    }
}
