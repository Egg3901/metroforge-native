//! Deterministic lighting-regression capture harness (issues #40, #141).
//! Inert unless `MF_SEAM_DIR` is set. Frames the exact promo Skyline rig,
//! pins the sun via the photo-mode hour override, and screenshots one frame
//! per pinned hour (`seam_0800.png`, `seam_1200.png`, `seam_1330.png`,
//! `seam_1830.png`) — identical camera and sun on every run and every
//! branch, so before/after frames are pixel-comparable.
//!
//! Born as a throwaway patch during the #40 cascade-seam investigation,
//! landed permanently with the #141 black-tower fix (both issues needed
//! exactly this: same camera, same sun, different build). Pair with
//! `MF_AUTOSTART=<city> MF_QUALITY=high MF_RESOLUTION=1920x1200`; add
//! `MF_FORCE_REVEAL=1` to also exercise the reveal dissolve in the same
//! frames.

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use mf_render::{BuildingsDenseCenter, PhotoModeRender};

use crate::camera::CameraRig;
use crate::state::AppState;

/// Frames to wait after pinning hour + camera before capturing: generous so
/// shadows/statics/eased camera are all at rest even on a software
/// rasterizer.
const SETTLE: u64 = 40;

/// Pinned capture hours: morning / noon / early afternoon / dusk. Noon and
/// 13:30 are the pair the #40 seam analysis used; 08:00 and 18:30 bracket
/// the day so a lighting regression that only shows at low sun still gets
/// caught.
const HOURS: [(f32, &str); 4] = [
    (8.0, "seam_0800.png"),
    (12.0, "seam_1200.png"),
    (13.5, "seam_1330.png"),
    (18.5, "seam_1830.png"),
];

pub struct MfSeamShotsPlugin;

impl Plugin for MfSeamShotsPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var_os("MF_SEAM_DIR").is_none() {
            return;
        }
        app.init_resource::<SeamState>()
            .add_systems(Update, seam_system.run_if(in_state(AppState::InGame)));
    }
}

#[derive(Resource, Default)]
struct SeamState {
    frame: u64,
    stage: usize,
    stage_start: u64,
}

fn seam_system(
    mut state: ResMut<SeamState>,
    mut commands: Commands,
    mut rigs: Query<&mut CameraRig>,
    mut photo: ResMut<PhotoModeRender>,
    dense: Res<BuildingsDenseCenter>,
    mut exit: EventWriter<AppExit>,
) {
    let Some(dir) = std::env::var_os("MF_SEAM_DIR").map(|s| s.to_string_lossy().into_owned())
    else {
        return;
    };
    let c = dense.0;
    if c == Vec2::ZERO {
        return;
    }
    state.frame += 1;
    let elapsed = state.frame - state.stage_start;
    let mut advance = false;

    // The exact promo Skyline framing (see promo.rs `Stage::Skyline`), so
    // captures line up with existing promo/issue reference shots.
    let frame_rig = |rigs: &mut Query<&mut CameraRig>| {
        if let Ok(mut rig) = rigs.single_mut() {
            rig.target = c;
            rig.target_goal = c;
            rig.distance = 2100.0;
            rig.distance_goal = 2100.0;
            rig.pitch = 0.50;
            rig.pitch_goal = 0.50;
            rig.yaw = 5.9;
            rig.yaw_goal = 5.9;
        }
    };

    if let Some(&(hour, name)) = HOURS.get(state.stage) {
        photo.override_hour = Some(hour);
        frame_rig(&mut rigs);
        if elapsed >= SETTLE {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(format!("{dir}/{name}")));
            advance = true;
        }
    } else if elapsed >= 30 {
        exit.write(AppExit::Success);
    }
    if advance {
        state.stage += 1;
        state.stage_start = state.frame;
    }
}
