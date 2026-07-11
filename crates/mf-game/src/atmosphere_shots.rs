//! Focused atmosphere screenshot harness. Inert unless `MF_ATMOSPHERE_DIR`
//! is set (pair with `MF_AUTOSTART` + `MF_QUALITY=medium|high`). Captures:
//! - `day_sparse.png` — elevated day with sparse volumes + ground shadows
//! - `golden_hour.png` — same framing near dusk (warm tint from sun elevation)
//!
//! Exists so the atmosphere PR can justify keeping Bevy VolumetricFog with
//! real frames, without waiting out the full promo/network demo.

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use mf_net::SimLink;
use mf_protocol::{SetSpeedPayload, ToSim};
use mf_render::BuildingsDenseCenter;
use mf_state::LatestUi;

use crate::camera::CameraRig;
use crate::state::AppState;

const TICKS_PER_DAY: u64 = 1200;
const SETTLE: u64 = 32;

pub struct MfAtmosphereShotsPlugin;

impl Plugin for MfAtmosphereShotsPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var_os("MF_ATMOSPHERE_DIR").is_none() {
            return;
        }
        app.init_resource::<AtmosphereShotState>().add_systems(
            Update,
            atmosphere_shots_system.run_if(in_state(AppState::InGame)),
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Stage {
    #[default]
    WaitDay,
    DaySparse,
    WaitGolden,
    GoldenHour,
    Done,
}

#[derive(Resource, Default)]
struct AtmosphereShotState {
    frame: u64,
    stage: Stage,
    stage_start: u64,
}

fn hour_of(ui: &LatestUi) -> f64 {
    ui.0.as_ref()
        .map(|s| (s.tick % TICKS_PER_DAY) as f64 / TICKS_PER_DAY as f64 * 24.0)
        .unwrap_or(0.0)
}

fn shoot(commands: &mut Commands, path: String) {
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path));
}

fn frame_elevated(rig: &mut CameraRig, target: Vec2) {
    rig.target = target;
    rig.target_goal = target;
    rig.distance = 2_200.0;
    rig.distance_goal = 2_200.0;
    rig.pitch = 0.52;
    rig.pitch_goal = 0.52;
    rig.yaw = 5.9;
    rig.yaw_goal = 5.9;
}

#[allow(clippy::too_many_arguments)]
fn atmosphere_shots_system(
    mut state: ResMut<AtmosphereShotState>,
    mut commands: Commands,
    mut rigs: Query<&mut CameraRig>,
    mut exit: EventWriter<AppExit>,
    link: Option<Res<SimLink>>,
    ui: Res<LatestUi>,
    dense: Res<BuildingsDenseCenter>,
    mut reveal: ResMut<mf_state::RevealState>,
) {
    let Some(dir) = std::env::var_os("MF_ATMOSPHERE_DIR").map(|s| s.to_string_lossy().into_owned())
    else {
        return;
    };
    // Keep the dissolve hole off so shots show weather, not reveal dither.
    reveal.strength = 0.0;
    state.frame += 1;
    let elapsed = state.frame - state.stage_start;
    let c = dense.0;
    let mut advance = None;

    let set_speed = |link: &Option<Res<SimLink>>, speed: f64| {
        if let Some(link) = link {
            let _ = link
                .transport
                .send(ToSim::SetSpeed(SetSpeedPayload { speed }));
        }
    };

    match state.stage {
        Stage::WaitDay => {
            let h = hour_of(&ui);
            if elapsed == 1 {
                set_speed(&link, 120.0);
            }
            let ready = (10.5..13.5).contains(&h) && c != Vec2::ZERO;
            if ready || elapsed > 2_400 {
                set_speed(&link, 0.0);
                if let Ok(mut rig) = rigs.single_mut() {
                    frame_elevated(&mut rig, c);
                }
                advance = Some(Stage::DaySparse);
            }
        }
        Stage::DaySparse => {
            if elapsed == 1 {
                if let Ok(mut rig) = rigs.single_mut() {
                    frame_elevated(&mut rig, c);
                }
            }
            if elapsed == SETTLE {
                shoot(&mut commands, format!("{dir}/day_sparse.png"));
                set_speed(&link, 180.0);
                advance = Some(Stage::WaitGolden);
            }
        }
        Stage::WaitGolden => {
            // ~17.6–18.5h: low sun, night_factor still near 0 → peak golden tint.
            // Later (19h+) night_factor rises and mixes the warmth toward navy gray.
            let h = hour_of(&ui);
            if (17.6..18.5).contains(&h) || elapsed > 2_000 {
                set_speed(&link, 0.0);
                if let Ok(mut rig) = rigs.single_mut() {
                    frame_elevated(&mut rig, c);
                }
                advance = Some(Stage::GoldenHour);
            }
        }
        Stage::GoldenHour => {
            if elapsed == SETTLE {
                shoot(&mut commands, format!("{dir}/golden_hour.png"));
                advance = Some(Stage::Done);
            }
        }
        Stage::Done => {
            if elapsed == 12 {
                exit.write(AppExit::Success);
            }
        }
    }

    if let Some(next) = advance {
        state.stage = next;
        state.stage_start = state.frame;
    }
}
