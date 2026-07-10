//! Dev/CI end-to-end verification harness — **not** part of the spec's v1
//! feature list; added while implementing/verifying `mf-render` so this box
//! (xvfb + lavapipe software Vulkan, no way to click through an egui
//! `MainMenu` headlessly) can drive the game all the way to `InGame` and
//! capture screenshots of what `mf-render` actually draws, without a human
//! at a display.
//!
//! Entirely inert unless `MF_VERIFY_DIR` is set (paired with
//! `MF_AUTOSTART=<presetKey>` in `state.rs` to skip the menu). When set, it
//! drives a fixed sequence once `InGame` is reached:
//!
//! 1. Run the sim at 120x so the day/night cycle reaches daylight quickly,
//!    and wait for both a daytime hour (so screenshots show the Mirror's
//!    Edge white-city look, not a night reading) and `mf_render`'s
//!    `BuildingsDenseCenter` (the city's densest built-up chunk, e.g.
//!    Manhattan for NYC — the origin alone is frequently open water).
//! 2. Frame an elevated 3/4 view over that dense area -> `default.png`.
//! 3. Dolly down low over the same area (street level, buildings on both
//!    sides) -> `street.png`.
//! 4. Restore the elevated framing, toggle subway view -> `subway.png`
//!    (subway view is about the *world* changing, not the camera, so it
//!    reuses the `default` framing rather than the street one).
//! 5. Drop to potato quality, same elevated framing -> `potato.png` -> quit.
//!
//! Frame counts are generous since software rasterization on this box is
//! slow (seconds per frame is fine).

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use mf_net::SimLink;
use mf_protocol::{SetSpeedPayload, ToSim};
use mf_render::BuildingsDenseCenter;
use mf_state::{LatestUi, QualityTier, SubwayView};

use crate::camera::CameraRig;
use crate::state::AppState;

/// Minimum frames to hold a given camera/world configuration before
/// screenshotting, so static layers (roads/buildings/transit rebuilds,
/// subway ease transition) have settled. Deliberately small: software
/// rendering is slow in wall-clock terms (each frame can be 100-300ms), and
/// at the 120x sim speed used to reach daylight quickly, even this many
/// frames' worth of real time is enough to cycle through several sim
/// hours — see the speed=0 freeze below, which is what actually keeps the
/// later screenshots' lighting consistent with the moment daylight was
/// detected.
const SETTLE_FRAMES: u64 = 20;
/// Hard cap on how long we'll wait for the "daytime + dense-center known"
/// gate before proceeding anyway, so a pathological sim state can't hang
/// this indefinitely in CI.
const MAX_WAIT_FRAMES: u64 = 900;

const TICKS_PER_DAY: u64 = 1200;
/// Daytime window (hours) we're willing to screenshot in — wide enough to
/// hit reliably within a couple of seconds at 120x, centered on noon.
const DAY_HOUR_MIN: f64 = 9.0;
const DAY_HOUR_MAX: f64 = 15.0;

pub struct MfVerifyPlugin;

impl Plugin for MfVerifyPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var_os("MF_VERIFY_DIR").is_none() {
            return; // inert in every normal build/run
        }
        app.init_resource::<VerifyState>()
            .add_systems(
                Update,
                verify_sequence_system.run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                menu_screenshot_system.run_if(in_state(AppState::MainMenu)),
            );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Stage {
    #[default]
    WaitForDayAndCity,
    Elevated,
    Street,
    Subway,
    Potato,
    Pause,
    Done,
}

#[derive(Resource, Default)]
struct VerifyState {
    frame: u64,
    stage: Stage,
    /// Frame count at the start of the current stage — every stage's
    /// "have we settled" check is relative to this, not the global frame
    /// counter.
    stage_start: u64,
    speed_sent: bool,
}

fn is_daytime(ui: &LatestUi) -> bool {
    let Some(state) = &ui.0 else { return false };
    let hour = (state.tick % TICKS_PER_DAY) as f64 / TICKS_PER_DAY as f64 * 24.0;
    (DAY_HOUR_MIN..DAY_HOUR_MAX).contains(&hour)
}

fn frame_elevated(rig: &mut CameraRig, center: Vec2) {
    rig.target = center;
    rig.distance = 1400.0;
    rig.pitch = 0.62; // a clear 3/4 angle over the skyline
    rig.yaw = 0.5;
}

fn frame_street(rig: &mut CameraRig, center: Vec2) {
    rig.target = center;
    rig.distance = 220.0;
    rig.pitch = 0.28; // low, looking mostly along the ground
    rig.yaw = 0.5;
}

#[allow(clippy::too_many_arguments)]
fn verify_sequence_system(
    mut state: ResMut<VerifyState>,
    mut commands: Commands,
    mut rigs: Query<&mut CameraRig>,
    mut subway: ResMut<SubwayView>,
    mut quality: ResMut<QualityTier>,
    mut exit: EventWriter<AppExit>,
    link: Option<Res<SimLink>>,
    ui: Res<LatestUi>,
    dense_center: Res<BuildingsDenseCenter>,
    mut pause: ResMut<crate::state::PauseState>,
) {
    let Some(dir) = std::env::var_os("MF_VERIFY_DIR").map(|s| s.to_string_lossy().into_owned())
    else {
        return;
    };
    state.frame += 1;

    if !state.speed_sent {
        state.speed_sent = true;
        if let Some(link) = &link {
            let _ = link
                .transport
                .send(ToSim::SetSpeed(SetSpeedPayload { speed: 120.0 }));
        }
    }

    let elapsed_in_stage = state.frame - state.stage_start;
    let mut advance_to = None;

    match state.stage {
        Stage::WaitForDayAndCity => {
            let ready = (is_daytime(&ui) && dense_center.0 != Vec2::ZERO)
                || elapsed_in_stage > MAX_WAIT_FRAMES;
            if ready {
                if let Ok(mut rig) = rigs.single_mut() {
                    frame_elevated(&mut rig, dense_center.0);
                }
                // Freeze the clock right here: at 120x, even the handful of
                // real seconds the remaining stages take would otherwise
                // cycle through several more sim hours, so every later
                // screenshot would land at an arbitrary (possibly nighttime)
                // moment again. Speed 0 holds `hour` steady for the rest of
                // the sequence.
                if let Some(link) = &link {
                    let _ = link
                        .transport
                        .send(ToSim::SetSpeed(SetSpeedPayload { speed: 0.0 }));
                }
                advance_to = Some(Stage::Elevated);
            }
        }
        Stage::Elevated => {
            if elapsed_in_stage == SETTLE_FRAMES {
                take_screenshot(&mut commands, format!("{dir}/default.png"));
                advance_to = Some(Stage::Street);
            }
        }
        Stage::Street => {
            if elapsed_in_stage == 5 {
                if let Ok(mut rig) = rigs.single_mut() {
                    frame_street(&mut rig, dense_center.0);
                }
            }
            if elapsed_in_stage == SETTLE_FRAMES {
                take_screenshot(&mut commands, format!("{dir}/street.png"));
                advance_to = Some(Stage::Subway);
            }
        }
        Stage::Subway => {
            if elapsed_in_stage == 5 {
                if let Ok(mut rig) = rigs.single_mut() {
                    frame_elevated(&mut rig, dense_center.0);
                }
                subway.toggle();
            }
            if elapsed_in_stage == SETTLE_FRAMES {
                take_screenshot(&mut commands, format!("{dir}/subway.png"));
                advance_to = Some(Stage::Potato);
            }
        }
        Stage::Potato => {
            if elapsed_in_stage == 5 {
                *quality = QualityTier::Potato;
            }
            if elapsed_in_stage == SETTLE_FRAMES {
                take_screenshot(&mut commands, format!("{dir}/potato.png"));
                advance_to = Some(Stage::Pause);
            }
        }
        Stage::Pause => {
            // Direct flag write (not `toggle_pause`): the overlay render path
            // is what's under test, and the sim was already frozen at speed 0
            // back in WaitForDayAndCity, so no SetSpeed round-trip is needed.
            if elapsed_in_stage == 5 {
                pause.active = true;
            }
            if elapsed_in_stage == SETTLE_FRAMES {
                take_screenshot(&mut commands, format!("{dir}/pause.png"));
                advance_to = Some(Stage::Done);
            }
        }
        Stage::Done => {
            // A little extra headroom so the last screenshot's async
            // GPU->CPU readback finishes before the process exits.
            if elapsed_in_stage == 30 {
                exit.write(AppExit::Success);
            }
        }
    }

    if let Some(next) = advance_to {
        state.stage = next;
        state.stage_start = state.frame;
    }
}

/// Screenshot the main menu itself (`menu.png`). `state.rs`'s autostart holds
/// at MainMenu for ~30 frames when `MF_VERIFY_DIR` is set precisely so this
/// can run — the menu being invisible (no camera = no egui context) shipped
/// in v0.1.0-alpha because no verify path ever rendered it.
fn menu_screenshot_system(mut state: Local<u64>, mut commands: Commands) {
    let Some(dir) = std::env::var_os("MF_VERIFY_DIR").map(|s| s.to_string_lossy().into_owned())
    else {
        return;
    };
    *state += 1;
    if *state == 15 {
        take_screenshot(&mut commands, format!("{dir}/menu.png"));
    }
}

fn take_screenshot(commands: &mut Commands, path: String) {
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path));
}
