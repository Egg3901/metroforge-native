//! Pointer/keyboard -> `ToSim` commands (spec §3.4 `input.rs`). v1 scope is
//! deliberately thin (spec §5 IN-list stops at "camera -> speeds -> vehicles
//! animate -> ... -> subway view -> quality switcher -> budget HUD"; the
//! build panel is a stretch goal) — today this module owns: Tab toggles
//! subway view, Esc toggles the pause overlay.

use bevy::prelude::*;
use mf_net::SimLink;
use mf_state::{LatestUi, SubwayView};

use crate::state::{toggle_pause, PauseState};

pub struct MfInputPlugin;

impl Plugin for MfInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (subway_toggle_system, pause_toggle_system)
                .run_if(in_state(crate::state::AppState::InGame)),
        );
    }
}

fn subway_toggle_system(keys: Res<ButtonInput<KeyCode>>, mut subway: ResMut<SubwayView>) {
    if keys.just_pressed(KeyCode::Tab) {
        subway.toggle();
    }
}

/// Esc freezes/resumes the sim clock (`state::toggle_pause`); `hud.rs`'s
/// pause overlay is the visual half of this, driven off the same
/// `PauseState` resource so key and button agree.
fn pause_toggle_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut pause: ResMut<PauseState>,
    ui_state: Res<LatestUi>,
    link: Option<Res<SimLink>>,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    toggle_pause(&mut pause, &ui_state, link.as_deref());
}
