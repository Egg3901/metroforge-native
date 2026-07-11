//! Pointer/keyboard -> `ToSim` commands (spec §3.4 `input.rs`). Tab toggles
//! subway view; Esc is a three-way priority chain shared with the v0.2
//! build tools (ship-plan #25, `tools.rs`): cancel an in-progress route
//! draft, else deactivate the current build tool, else toggle the pause
//! overlay (the only behavior this module had before v0.2).

use bevy::prelude::*;
use mf_net::SimLink;
use mf_state::{LatestUi, SubwayView};

use crate::audio::{PlaySfx, Sfx};
use crate::state::{toggle_pause, PauseState};
use crate::tools::{ActiveTool, ToolState};

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

fn subway_toggle_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut subway: ResMut<SubwayView>,
    mut sfx: EventWriter<PlaySfx>,
) {
    if keys.just_pressed(KeyCode::Tab) {
        subway.toggle();
        sfx.write(PlaySfx(if subway.active {
            Sfx::Confirm
        } else {
            Sfx::Cancel
        }));
    }
}

/// Esc priority chain (v0.2, ship-plan #25): a non-empty route draft is the
/// most "undo-able" state on screen, so Esc cancels IT first rather than
/// yanking the player out of the tool entirely; only an already-empty
/// draft falls through to deactivating the tool; only no-tool-active falls
/// through further to the original behavior, `state::toggle_pause`
/// freezing/resuming the sim clock (`hud.rs`'s pause overlay is the visual
/// half of that, driven off the same `PauseState` resource so key and
/// button agree). Each rung returns immediately so at most one of the three
/// things happens per Esc press.
fn pause_toggle_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut pause: ResMut<PauseState>,
    mut tool: ResMut<ToolState>,
    ui_state: Res<LatestUi>,
    link: Option<Res<SimLink>>,
    mut sfx: EventWriter<PlaySfx>,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    if !tool.route_draft.is_empty() {
        tool.route_draft.clear();
        tool.last_cost_quote = None;
        tool.editing_route_id = None;
        sfx.write(PlaySfx(Sfx::Cancel));
        return;
    }
    if tool.active != ActiveTool::None {
        tool.active = ActiveTool::None;
        tool.editing_route_id = None;
        return;
    }
    if toggle_pause(&mut pause, &ui_state, link.as_deref()) {
        sfx.write(PlaySfx(if pause.active {
            Sfx::Pause
        } else {
            Sfx::Unpause
        }));
    }
}
