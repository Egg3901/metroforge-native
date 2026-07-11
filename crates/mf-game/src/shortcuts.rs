//! Keyboard-shortcuts help overlay (web-parity gap #45): `?` / `/` toggles a
//! reference card of every in-game keybind so they are discoverable. Purely
//! client-side; all copy routes through the `strings` table.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::design_system as ds;
use crate::state::AppState;

/// Whether the shortcuts card is showing.
#[derive(Resource, Default)]
pub struct ShortcutsOverlayOpen(pub bool);

pub struct MfShortcutsPlugin;

impl Plugin for MfShortcutsPlugin {
    fn build(&self, app: &mut App) {
        // MF_SHOW_SHORTCUTS opens the card at boot (verify/screenshot harness).
        app.insert_resource(ShortcutsOverlayOpen(
            std::env::var_os("MF_SHOW_SHORTCUTS").is_some(),
        ))
        .add_systems(
            Update,
            toggle_shortcuts_system.run_if(in_state(AppState::InGame)),
        )
        .add_systems(
            EguiPrimaryContextPass,
            shortcuts_overlay_system
                .run_if(in_state(AppState::InGame))
                .run_if(|| !ds::hud_hidden()),
        );
    }
}

/// `/` (and thus `?` = Shift+`/`) toggles the card. Ignored while a text field
/// wants keyboard input so it never eats a keystroke mid-typing.
fn toggle_shortcuts_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut open: ResMut<ShortcutsOverlayOpen>,
    mut contexts: EguiContexts,
) {
    let typing = contexts
        .ctx_mut()
        .map(|c| c.wants_keyboard_input())
        .unwrap_or(false);
    if !typing && keys.just_pressed(KeyCode::Slash) {
        open.0 = !open.0;
    }
}

fn shortcuts_overlay_system(
    mut contexts: EguiContexts,
    mut open: ResMut<ShortcutsOverlayOpen>,
    keys: Res<ButtonInput<KeyCode>>,
) -> Result {
    if !open.0 {
        return Ok(());
    }
    // Esc closes the card first (before the tool/pause Esc-chain feels natural).
    if keys.just_pressed(KeyCode::Escape) {
        open.0 = false;
        return Ok(());
    }

    let ctx = contexts.ctx_mut()?;
    let s = crate::strings::current();
    let fade = ds::animate(ctx, egui::Id::new("shortcuts_fade"), 1.0);
    let mut close = false;

    ds::modal(ctx, egui::Id::new("shortcuts_modal"), fade, |ui| {
        ui.set_max_width(430.0);
        ui.label(ds::heading(s.shortcuts_title));
        ui.add_space(8.0);

        let row = |ui: &mut egui::Ui, key: &str, action: &str| {
            ui.horizontal(|ui| {
                ui.add_sized(
                    [110.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new(key)
                            .monospace()
                            .strong()
                            .color(ds::accent()),
                    ),
                );
                ui.label(ds::label_body(action));
            });
        };
        let section = |ui: &mut egui::Ui, label: &str| {
            ui.add_space(6.0);
            ui.label(ds::label_muted(label));
        };

        section(ui, s.sc_section_camera);
        row(ui, "WASD / Arrows", s.sc_pan);
        row(ui, "Right drag", s.sc_orbit);
        row(ui, "Wheel", s.sc_zoom);

        section(ui, s.sc_section_build);
        row(ui, "1", s.sc_bus_tool);
        row(ui, "2", s.sc_route_tool);
        row(ui, "3", s.sc_bulldoze_tool);
        row(ui, "R", s.sc_rotate);
        row(ui, "Shift + click", s.sc_multiselect);
        row(ui, "Enter", s.sc_confirm);

        section(ui, s.sc_section_view);
        row(ui, "Tab", s.sc_subway);
        row(ui, "G", s.sc_demand);
        row(ui, "F", s.sc_finance);
        row(ui, "M", s.sc_map);
        row(ui, "N", s.sc_minimap);
        row(ui, "P", s.sc_photo);
        row(ui, "F11", s.sc_fullscreen);
        row(ui, "Esc", s.sc_pause);
        row(ui, "?", s.sc_help);

        ui.add_space(10.0);
        ui.label(ds::label_muted(s.shortcuts_hint));
        ui.add_space(6.0);
        if ds::button(ui, s.close, ds::ButtonKind::Primary).clicked() {
            close = true;
        }
    });

    if close {
        open.0 = false;
    }
    Ok(())
}
