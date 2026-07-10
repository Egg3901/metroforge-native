//! Pointer/keyboard -> `ToSim` commands (spec §3.4 `input.rs`). v1 scope is
//! deliberately thin (spec §5 IN-list stops at "camera -> speeds -> vehicles
//! animate -> ... -> subway view -> quality switcher -> budget HUD"; the
//! build panel is a stretch goal) — today this module owns exactly the one
//! required binding: Tab toggles subway view.

use bevy::prelude::*;
use mf_state::SubwayView;

pub struct MfInputPlugin;

impl Plugin for MfInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            subway_toggle_system.run_if(in_state(crate::state::AppState::InGame)),
        );
    }
}

fn subway_toggle_system(keys: Res<ButtonInput<KeyCode>>, mut subway: ResMut<SubwayView>) {
    if keys.just_pressed(KeyCode::Tab) {
        subway.toggle();
    }
}
