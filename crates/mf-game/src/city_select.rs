//! City-select screen (menu flow from PR #47): a grid of city cards with
//! miniature maps, real hello/catalog stats, Continue slots for existing
//! saves, accent hover, and arrow-key navigation.
//!
//! Lives in its own file so the title/settings/load screens in `hud.rs`
//! stay readable; `hud::main_menu_hud_system` still owns the
//! `MenuScreen::CitySelect` dispatch.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use mf_protocol::{CityListEntry, CityMapPreview, Difficulty};
use mf_render::palette;

use crate::audio::{PlaySfx, Sfx};
use crate::campaign::{self, CampaignProgress};
use crate::city_catalog::{self, CityCatalogEntry, PREVIEW_RES};
use crate::design_system as ds;
use crate::hud::{
    capitalize, draw_lock, draw_logo, draw_star, field_label, format_cash, format_playtime,
    hover_tick, thin_separator, ToastLog,
};
use crate::map_paint::{
    arterial_polylines_from_f32, arterial_polylines_from_points, bake_city_preview_image,
    WorldPolyline,
};
use crate::saves::{self, PlaytimeTracker, SaveManager, SaveMeta, SaveSlot, SlotEntry};
use crate::state::{AppState, MenuScreen, PendingInit, SimHello};

const GRID_COLS: usize = 2;
const CARD_W: f32 = 280.0;
const CARD_H: f32 = 196.0;
const CARD_GAP: f32 = ds::SPACE_SM;
const MAP_SIDE: f32 = 88.0;

/// Focusable cell in the city-select grid (Continue card and/or New Game card).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusTarget {
    /// Continue the newest save for this city key.
    Continue(&'static str),
    /// Select / start a fresh run on this city key.
    City(&'static str),
}

/// Per-city miniature map texture cache (keyed by city preset key).
#[derive(Default)]
pub struct PreviewCache {
    textures: HashMap<String, egui::TextureHandle>,
}

/// Locals owned by the city-select screen (bundled so `main_menu_hud_system`
/// stays under Bevy's 16-param `System` limit).
#[derive(Default)]
pub struct CitySelectLocals {
    pub slots_cache: Option<Vec<SlotEntry>>,
    pub preview_cache: PreviewCache,
    pub focus_idx: usize,
}

/// City-select screen system — called from `hud::main_menu_hud_system`.
#[allow(clippy::too_many_arguments)]
pub fn city_select_screen_ui(
    mut contexts: EguiContexts,
    hello: Res<SimHello>,
    progress: Res<CampaignProgress>,
    mut pending: ResMut<PendingInit>,
    mut save_manager: ResMut<SaveManager>,
    mut toasts: ResMut<ToastLog>,
    state: Res<State<AppState>>,
    mut next_state: ResMut<NextState<AppState>>,
    mut screen: ResMut<MenuScreen>,
    mut sfx: EventWriter<PlaySfx>,
    mut playtime: ResMut<PlaytimeTracker>,
    mut hovered: Local<Option<egui::Id>>,
    mut locals: Local<CitySelectLocals>,
) -> Result {
    if state.is_changed() || locals.slots_cache.is_none() {
        locals.slots_cache = Some(saves::list());
        locals.preview_cache.textures.clear();
    }
    // Clone so later mutable borrows of `locals` (preview cache / focus) don't
    // fight the slot-list borrow used to build Continue cards.
    let slots = locals.slots_cache.clone().expect("populated just above");
    let newest_by_city = saves::newest_saves_by_city(&slots);

    let ctx = contexts.ctx_mut()?;
    let fade = ctx.animate_value_with_time(egui::Id::new("city_select_fade"), 1.0, 0.2);

    let cities_wire = hello
        .0
        .as_ref()
        .map(|h| h.city_list.as_slice())
        .unwrap_or(&[]);

    // Build the focusable grid order: cities with a Continue card first
    // (Continue cell, then New Game cell), then the rest as New Game only.
    let focus_order = build_focus_order(&newest_by_city);
    if locals.focus_idx >= focus_order.len() {
        locals.focus_idx = 0;
    }

    let mut go_back = false;
    let mut start_pressed = false;
    let mut load_slot: Option<SaveSlot> = None;
    let mut activate_focus = false;

    // Keyboard: arrows move focus across the 2-col grid; Enter activates.
    let (cols, _) = grid_dims(focus_order.len());
    ctx.input(|input| {
        if input.key_pressed(egui::Key::ArrowRight) {
            locals.focus_idx = (locals.focus_idx + 1).min(focus_order.len().saturating_sub(1));
        } else if input.key_pressed(egui::Key::ArrowLeft) {
            locals.focus_idx = locals.focus_idx.saturating_sub(1);
        } else if input.key_pressed(egui::Key::ArrowDown) {
            locals.focus_idx = (locals.focus_idx + cols).min(focus_order.len().saturating_sub(1));
        } else if input.key_pressed(egui::Key::ArrowUp) {
            locals.focus_idx = locals.focus_idx.saturating_sub(cols);
        } else if input.key_pressed(egui::Key::Enter) {
            activate_focus = true;
        } else if input.key_pressed(egui::Key::Escape) {
            go_back = true;
        }
    });

    if activate_focus {
        if let Some(target) = focus_order.get(locals.focus_idx).copied() {
            match target {
                FocusTarget::Continue(key) => {
                    if let Some(entry) = newest_by_city.get(key) {
                        load_slot = Some(entry.slot);
                    }
                }
                FocusTarget::City(key) => {
                    if progress.city_unlocked(key) {
                        pending.preset_key = key.to_string();
                        start_pressed = true;
                    }
                }
            }
        }
    }

    egui::TopBottomPanel::top("city_select_top")
        .frame(
            egui::Frame::default()
                .fill(ds::panel_bg())
                .inner_margin(egui::Margin::symmetric(14, 10)),
        )
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            ui.horizontal(|ui| {
                let back = ui.add(egui::Button::new(
                    egui::RichText::new(crate::strings::current().back_arrow).size(ds::TEXT_SM),
                ));
                hover_tick(&back, &mut hovered, &mut sfx);
                if back.clicked() {
                    go_back = true;
                }
                ui.add_space(ds::SPACE_SM);
                draw_logo(ui, 28.0);
                ui.add_space(ds::SPACE_XS);
                ui.label(
                    egui::RichText::new(crate::strings::current().brand)
                        .size(ds::TEXT_SM)
                        .strong()
                        .color(ds::text()),
                );
            });
        });

    let selected_label = cities_wire
        .iter()
        .find(|c| c.key == pending.preset_key)
        .map(|c| c.label.clone())
        .unwrap_or_else(|| capitalize(&pending.preset_key));
    let start_caption =
        crate::strings::current().start_city(&selected_label, pending.difficulty.label());

    egui::TopBottomPanel::bottom("city_select_bottom")
        .frame(
            egui::Frame::default()
                .fill(ds::panel_bg())
                .inner_margin(egui::Margin::symmetric(14, 12)),
        )
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            ui.vertical_centered(|ui| {
                let start = ui.add_sized(
                    [280.0, 44.0],
                    egui::Button::new(
                        egui::RichText::new(start_caption)
                            .color(egui::Color32::WHITE)
                            .size(ds::TEXT_MD)
                            .strong(),
                    )
                    .fill(ds::accent())
                    .corner_radius(ds::CORNER_RADIUS),
                );
                hover_tick(&start, &mut hovered, &mut sfx);
                if start.clicked() {
                    start_pressed = true;
                }
                ui.add_space(ds::SPACE_XXS + 2.0);
                ui.label(
                    egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .size(ds::TEXT_XS)
                        .color(ds::muted()),
                );
            });
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(ds::menu_wash()))
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ds::SPACE_MD);
                        ui.scope(|ui| {
                            let content_w = CARD_W * 2.0 + CARD_GAP;
                            ui.set_width(content_w);
                            ui.vertical_centered(|ui| {
                                let total_stars: u32 = campaign::CITY_ORDER
                                    .iter()
                                    .map(|&key| progress.stars(key) as u32)
                                    .sum();

                                field_label(ui, crate::strings::current().city);
                                ui.add_space(ds::SPACE_XXS);
                                ui.label(ds::label_muted(
                                    crate::strings::current().city_select_hint,
                                ));
                                ui.add_space(ds::SPACE_SM);

                                // Ensure preview textures exist for every city we show.
                                for &key in campaign::CITY_ORDER.iter() {
                                    ensure_preview(
                                        ui.ctx(),
                                        &mut locals.preview_cache,
                                        key,
                                        cities_wire.iter().find(|c| c.key == key),
                                    );
                                }

                                let mut cell_i = 0usize;
                                egui::Grid::new("city_grid")
                                    .num_columns(GRID_COLS)
                                    .spacing(egui::vec2(CARD_GAP, CARD_GAP))
                                    .show(ui, |ui| {
                                        // Pass 1: Continue cards first (cities with saves).
                                        for &key in campaign::CITY_ORDER.iter() {
                                            let Some(entry) = newest_by_city.get(key) else {
                                                continue;
                                            };
                                            let meta = entry.meta.as_ref().expect("occupied");
                                            let focused = focus_order.get(cell_i)
                                                == Some(&FocusTarget::Continue(key));
                                            let wire = cities_wire.iter().find(|c| c.key == key);
                                            let label = wire
                                                .map(|c| c.label.clone())
                                                .unwrap_or_else(|| capitalize(key));
                                            let tex = locals.preview_cache.textures.get(key);
                                            let clicked = continue_city_card(
                                                ui,
                                                egui::vec2(CARD_W, CARD_H),
                                                &label,
                                                meta,
                                                entry.slot,
                                                tex,
                                                focused,
                                                &mut hovered,
                                                &mut sfx,
                                            );
                                            if clicked {
                                                locals.focus_idx = cell_i;
                                                load_slot = Some(entry.slot);
                                            }
                                            cell_i += 1;
                                            if cell_i.is_multiple_of(GRID_COLS) {
                                                ui.end_row();
                                            }
                                        }

                                        // Pass 2: New Game cards for every city.
                                        for (i, &key) in campaign::CITY_ORDER.iter().enumerate() {
                                            let wire = cities_wire.iter().find(|c| c.key == key);
                                            let catalog = city_catalog::lookup(key);
                                            let label = wire
                                                .map(|c| c.label.clone())
                                                .unwrap_or_else(|| capitalize(key));
                                            let stats = resolve_stats(wire, catalog);
                                            let flavor = catalog.map(|c| c.flavor).unwrap_or("");
                                            let stars = progress.stars(key);
                                            let unlocked = progress.city_unlocked(key);
                                            let selected = pending.preset_key == key;
                                            let stars_needed =
                                                (2 * i as u32).saturating_sub(total_stars);
                                            let focused = focus_order.get(cell_i)
                                                == Some(&FocusTarget::City(key));
                                            let tex = locals.preview_cache.textures.get(key);
                                            let (clicked, double_clicked) = city_card(
                                                ui,
                                                egui::vec2(CARD_W, CARD_H),
                                                &label,
                                                &stats,
                                                flavor,
                                                stars,
                                                unlocked,
                                                selected || focused,
                                                stars_needed,
                                                tex,
                                                &mut hovered,
                                                &mut sfx,
                                            );
                                            if clicked {
                                                locals.focus_idx = cell_i;
                                                pending.preset_key = key.to_string();
                                                sfx.write(PlaySfx(Sfx::Confirm));
                                            }
                                            if double_clicked {
                                                pending.preset_key = key.to_string();
                                                start_pressed = true;
                                            }
                                            cell_i += 1;
                                            if cell_i.is_multiple_of(GRID_COLS) {
                                                ui.end_row();
                                            }
                                        }
                                        if !cell_i.is_multiple_of(GRID_COLS) {
                                            ui.end_row();
                                        }
                                    });

                                ui.add_space(ds::SPACE_MD);
                                thin_separator(ui);
                                ui.add_space(ds::SPACE_XS);
                                field_label(ui, crate::strings::current().difficulty);
                                egui::ComboBox::from_id_salt("difficulty_picker")
                                    .selected_text(pending.difficulty.label())
                                    .width(300.0)
                                    .show_ui(ui, |ui| {
                                        for d in
                                            [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard]
                                        {
                                            ui.selectable_value(
                                                &mut pending.difficulty,
                                                d,
                                                d.label(),
                                            );
                                        }
                                    });
                                ui.add_space(ds::SPACE_MD);
                            });
                        });
                    });
                });
        });

    if let Some(slot) = load_slot {
        if save_manager.load(slot, &mut toasts, &mut sfx).is_some() {
            next_state.set(AppState::Loading);
        }
    }
    if start_pressed {
        sfx.write(PlaySfx(Sfx::Confirm));
        saves::reset_playtime(&mut playtime);
        next_state.set(AppState::Loading);
    }
    if go_back {
        sfx.write(PlaySfx(Sfx::Cancel));
        *screen = MenuScreen::Title;
    }
    Ok(())
}

struct CityStats {
    country: String,
    population: Option<f64>,
    building_count: Option<u32>,
    size_km: Option<f64>,
}

fn resolve_stats(wire: Option<&CityListEntry>, catalog: Option<&CityCatalogEntry>) -> CityStats {
    let country = wire
        .and_then(|c| c.country.clone())
        .or_else(|| catalog.map(|c| c.country.to_string()))
        .unwrap_or_else(|| "USA".to_string());
    let population = wire
        .and_then(|c| c.population)
        .or_else(|| catalog.and_then(|c| c.population));
    let building_count = wire
        .and_then(|c| c.building_count)
        .or_else(|| catalog.and_then(|c| c.building_count));
    let size_km = wire
        .and_then(|c| c.size_km)
        .or_else(|| catalog.map(|c| c.size_km));
    CityStats {
        country,
        population,
        building_count,
        size_km,
    }
}

fn build_focus_order(newest_by_city: &HashMap<String, &SlotEntry>) -> Vec<FocusTarget> {
    let mut order = Vec::with_capacity(campaign::CITY_ORDER.len() * 2);
    for &key in campaign::CITY_ORDER.iter() {
        if newest_by_city.contains_key(key) {
            order.push(FocusTarget::Continue(key));
        }
    }
    for &key in campaign::CITY_ORDER.iter() {
        order.push(FocusTarget::City(key));
    }
    order
}

fn grid_dims(n: usize) -> (usize, usize) {
    let cols = GRID_COLS;
    let rows = n.div_ceil(cols);
    (cols, rows)
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

fn ensure_preview(
    ctx: &egui::Context,
    cache: &mut PreviewCache,
    key: &str,
    wire: Option<&CityListEntry>,
) {
    if cache.textures.contains_key(key) {
        return;
    }
    let ground = color32_from(palette::ground());
    let water_c = color32_from(palette::water());
    let road_c = color32_from(palette::road()).gamma_multiply(0.45);

    let image = if let Some(preview) = wire.and_then(|c| c.map_preview.as_ref()) {
        bake_from_hello_preview(preview, ground, water_c, road_c)
    } else if let Some(cat) = city_catalog::lookup(key) {
        bake_from_catalog(cat, ground, water_c, road_c)
    } else {
        bake_city_preview_image(
            &[],
            0,
            0,
            &[],
            12_000.0,
            PREVIEW_RES,
            ground,
            water_c,
            road_c,
        )
    };
    let handle = ctx.load_texture(
        format!("city_select_preview_{key}"),
        image,
        egui::TextureOptions::NEAREST,
    );
    cache.textures.insert(key.to_string(), handle);
}

fn bake_from_hello_preview(
    preview: &CityMapPreview,
    ground: egui::Color32,
    water_c: egui::Color32,
    road_c: egui::Color32,
) -> egui::ColorImage {
    let res = preview.res as usize;
    let arterials: Vec<WorldPolyline> = arterial_polylines_from_points(
        preview
            .arterials
            .iter()
            .map(|pts| ("arterial", pts.as_slice())),
    );
    bake_city_preview_image(
        &preview.water,
        res,
        res,
        &arterials,
        preview.world_size as f32,
        PREVIEW_RES,
        ground,
        water_c,
        road_c,
    )
}

fn bake_from_catalog(
    cat: &CityCatalogEntry,
    ground: egui::Color32,
    water_c: egui::Color32,
    road_c: egui::Color32,
) -> egui::ColorImage {
    let mut water = vec![0u8; PREVIEW_RES * PREVIEW_RES];
    city_catalog::unpack_water(cat.water_bits, &mut water);
    let arterials = arterial_polylines_from_f32(cat.arterials);
    bake_city_preview_image(
        &water,
        PREVIEW_RES,
        PREVIEW_RES,
        &arterials,
        cat.world_size as f32,
        PREVIEW_RES,
        ground,
        water_c,
        road_c,
    )
}

fn format_pop(n: f64) -> String {
    if n >= 1_000_000.0 {
        format!("{:.1}M", n / 1_000_000.0)
    } else if n >= 1_000.0 {
        format!("{:.0}k", n / 1_000.0)
    } else {
        format!("{:.0}", n)
    }
}

fn format_buildings(n: u32) -> String {
    if n >= 1_000 {
        format!("{:.0}k bldgs", n as f64 / 1_000.0)
    } else {
        format!("{n} bldgs")
    }
}

#[allow(clippy::too_many_arguments)]
fn city_card(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    label: &str,
    stats: &CityStats,
    flavor: &str,
    stars: u8,
    unlocked: bool,
    selected: bool,
    stars_needed: u32,
    preview: Option<&egui::TextureHandle>,
    hovered: &mut Option<egui::Id>,
    sfx: &mut EventWriter<PlaySfx>,
) -> (bool, bool) {
    let sense = if unlocked {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    if unlocked {
        hover_tick(&response, hovered, sfx);
    }

    let painter = ui.painter_at(rect);
    let hovering = unlocked && (response.hovered() || selected);
    let bg = if hovering {
        ds::hover_bg()
    } else {
        ds::inactive_bg()
    };
    painter.rect_filled(rect, ds::CORNER_RADIUS, bg);
    let border = if selected || response.hovered() && unlocked {
        egui::Stroke::new(2.0, ds::accent())
    } else {
        egui::Stroke::new(1.0, ds::border())
    };
    painter.rect_stroke(rect, ds::CORNER_RADIUS, border, egui::StrokeKind::Inside);

    let pad = ds::SPACE_XS;
    let content = rect.shrink(pad);
    let map_rect = egui::Rect::from_min_size(content.min, egui::vec2(MAP_SIDE, MAP_SIDE));
    if let Some(tex) = preview {
        painter.image(
            tex.id(),
            map_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            if unlocked {
                egui::Color32::WHITE
            } else {
                egui::Color32::from_white_alpha(140)
            },
        );
    } else {
        painter.rect_filled(map_rect, 0.0, color32_from(palette::ground()));
    }
    painter.rect_stroke(
        map_rect,
        0.0,
        egui::Stroke::new(1.0, ds::border()),
        egui::StrokeKind::Inside,
    );

    let text_left = map_rect.right() + ds::SPACE_XS;
    let text_rect = egui::Rect::from_min_max(
        egui::pos2(text_left, content.top()),
        egui::pos2(content.right(), content.bottom()),
    );
    let text_color = if unlocked { ds::text() } else { ds::muted() };
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.label(
        egui::RichText::new(label)
            .size(ds::TEXT_MD)
            .strong()
            .color(text_color),
    );
    child.label(ds::label_small(stats.country.clone()).color(ds::muted()));
    child.add_space(ds::SPACE_XXS);

    let mut stat_bits = Vec::new();
    if let Some(pop) = stats.population {
        stat_bits.push(format_pop(pop));
    }
    if let Some(bc) = stats.building_count {
        stat_bits.push(format_buildings(bc));
    }
    if let Some(km) = stats.size_km {
        stat_bits.push(format!("{km:.0} km"));
    }
    if !stat_bits.is_empty() {
        child.label(
            egui::RichText::new(stat_bits.join(" · "))
                .size(ds::TEXT_XS)
                .color(ds::muted()),
        );
    }

    if !flavor.is_empty() {
        child.add_space(ds::SPACE_XXS);
        child.label(
            egui::RichText::new(flavor)
                .size(ds::TEXT_XS)
                .italics()
                .color(if unlocked {
                    ds::text().gamma_multiply(0.85)
                } else {
                    ds::muted()
                }),
        );
    }

    child.add_space(ds::SPACE_XXS);
    let star_size = egui::vec2(text_rect.width(), 14.0);
    let (star_rect, _) = child.allocate_exact_size(star_size, egui::Sense::hover());
    let star_painter = child.painter_at(star_rect);
    let star_r = 6.5;
    for i in 0..3u8 {
        let cx = star_rect.left() + star_r + i as f32 * (star_r * 2.4);
        let center = egui::pos2(cx, star_rect.center().y);
        let filled = i < stars.min(3);
        let color = if !unlocked {
            ds::muted()
        } else if filled {
            ds::accent()
        } else {
            ds::border()
        };
        draw_star(&star_painter, center, star_r, filled, color);
    }

    if !unlocked {
        child.add_space(ds::SPACE_XXS);
        child.horizontal(|ui| {
            let (lock_rect, _) =
                ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
            draw_lock(&ui.painter_at(lock_rect), lock_rect, ds::muted());
            ui.label(
                egui::RichText::new(crate::strings::current().earn_more_stars(stars_needed))
                    .size(ds::TEXT_XS)
                    .color(ds::muted()),
            );
        });
    }

    (
        unlocked && response.clicked(),
        unlocked && response.double_clicked(),
    )
}

#[allow(clippy::too_many_arguments)]
fn continue_city_card(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    label: &str,
    meta: &SaveMeta,
    _slot: SaveSlot,
    preview: Option<&egui::TextureHandle>,
    focused: bool,
    hovered: &mut Option<egui::Id>,
    sfx: &mut EventWriter<PlaySfx>,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    hover_tick(&response, hovered, sfx);

    let painter = ui.painter_at(rect);
    let hovering = response.hovered() || focused;
    let bg = if hovering {
        ds::hover_bg()
    } else {
        ds::inactive_bg()
    };
    painter.rect_filled(rect, ds::CORNER_RADIUS, bg);
    let border = if hovering {
        egui::Stroke::new(2.0, ds::accent())
    } else {
        egui::Stroke::new(1.5, ds::accent().gamma_multiply(0.7))
    };
    painter.rect_stroke(rect, ds::CORNER_RADIUS, border, egui::StrokeKind::Inside);

    // Accent bar on the left — Continue treatment.
    let bar = egui::Rect::from_min_max(rect.min, egui::pos2(rect.min.x + 3.0, rect.max.y));
    painter.rect_filled(bar, 0.0, ds::accent());

    let pad = ds::SPACE_XS;
    let content = rect.shrink(pad);
    let map_rect = egui::Rect::from_min_size(
        egui::pos2(content.min.x + 4.0, content.min.y),
        egui::vec2(MAP_SIDE, MAP_SIDE),
    );
    if let Some(tex) = preview {
        painter.image(
            tex.id(),
            map_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
    painter.rect_stroke(
        map_rect,
        0.0,
        egui::Stroke::new(1.0, ds::border()),
        egui::StrokeKind::Inside,
    );

    let text_left = map_rect.right() + ds::SPACE_XS;
    let text_rect = egui::Rect::from_min_max(
        egui::pos2(text_left, content.top()),
        egui::pos2(content.right(), content.bottom()),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.label(
        egui::RichText::new(crate::strings::current().continue_label)
            .size(ds::TEXT_XS)
            .strong()
            .color(ds::accent()),
    );
    child.label(
        egui::RichText::new(label)
            .size(ds::TEXT_MD)
            .strong()
            .color(ds::text()),
    );
    child.add_space(ds::SPACE_XXS);
    child.label(
        egui::RichText::new(format!(
            "{}{} · {}",
            crate::strings::current().day_prefix,
            meta.day,
            format_playtime(meta.playtime_secs)
        ))
        .size(ds::TEXT_SM)
        .color(ds::muted()),
    );
    child.label(
        egui::RichText::new(format_cash(meta.cash))
            .size(ds::TEXT_XS)
            .color(ds::muted()),
    );

    response.clicked()
}
