//! Promotional screenshot harness. Inert unless `MF_PROMO_DIR` is set;
//! pairs with `MF_AUTOSTART` + `MF_VERIFY_NETWORK` (the network-demo build
//! from `verify.rs` supplies the colorful routes every hero shot needs) and
//! ideally `MF_QUALITY=high MF_RESOLUTION=1920x1200`.
//!
//! Unlike `verify.rs` (regression eyes, fixed small frames), this walks a
//! deliberate shot list: day skyline, street canyon, the bundled-network
//! money shot, demand arcs, the reveal dissolve, night glow, and the subway
//! graph. Runs AFTER the network demo settles; both harnesses share the
//! screenshot mechanics but not stages, so neither can regress the other.

use bevy::gizmos::config::{DefaultGizmoConfigGroup, GizmoConfigStore};
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use mf_net::SimLink;
use mf_protocol::{SetSpeedPayload, ToSim};
use mf_render::BuildingsDenseCenter;
use mf_state::{LatestUi, OverlayMode, OverlayState, RevealState, SubwayView};

use crate::camera::CameraRig;
use crate::state::AppState;

const TICKS_PER_DAY: u64 = 1200;
/// Generous settle so shadows/statics/eased camera are all at rest on a
/// software rasterizer before each capture.
const SETTLE: u64 = 26;

pub struct MfPromoPlugin;

impl Plugin for MfPromoPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var_os("MF_PROMO_DIR").is_none() {
            return;
        }
        app.init_resource::<PromoState>()
            .add_systems(Update, promo_system.run_if(in_state(AppState::InGame)));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Stage {
    /// Wait for the network demo (verify.rs) to finish building and the
    /// clock to sit in flattering late-morning light.
    #[default]
    WaitDay,
    Skyline,
    Canyon,
    Network,
    Arcs,
    SubwayDay,
    Reveal,
    /// Run the clock forward to evening, then freeze.
    WaitNight,
    NightGlow,
    SubwayGraph,
    Done,
}

#[derive(Resource, Default)]
struct PromoState {
    frame: u64,
    stage: Stage,
    stage_start: u64,
}

fn hour_of(ui: &LatestUi) -> f64 {
    ui.0.as_ref()
        .map(|s| (s.tick % TICKS_PER_DAY) as f64 / TICKS_PER_DAY as f64 * 24.0)
        .unwrap_or(0.0)
}

/// The network demo (5 routes) exists once the sim's route list says so.
fn network_ready(ui: &LatestUi) -> bool {
    ui.0.as_ref().map(|s| s.routes.len() >= 4).unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
fn promo_system(
    mut state: ResMut<PromoState>,
    mut commands: Commands,
    mut rigs: Query<&mut CameraRig>,
    mut subway: ResMut<SubwayView>,
    mut overlay: ResMut<OverlayState>,
    mut reveal: ResMut<RevealState>,
    mut exit: EventWriter<AppExit>,
    link: Option<Res<SimLink>>,
    ui: Res<LatestUi>,
    dense: Res<BuildingsDenseCenter>,
    mut gizmo_config: ResMut<GizmoConfigStore>,
) {
    let Some(dir) = std::env::var_os("MF_PROMO_DIR").map(|s| s.to_string_lossy().into_owned())
    else {
        return;
    };
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
    let frame_rig = |rigs: &mut Query<&mut CameraRig>, target: Vec2, d: f32, p: f32, y: f32| {
        if let Ok(mut rig) = rigs.single_mut() {
            rig.target = target;
            rig.target_goal = target;
            rig.distance = d;
            rig.distance_goal = d;
            rig.pitch = p;
            rig.pitch_goal = p;
            rig.yaw = y;
            rig.yaw_goal = y;
        }
    };

    match state.stage {
        Stage::WaitDay => {
            let h = hour_of(&ui);
            let ready = network_ready(&ui) && (10.5..13.5).contains(&h) && c != Vec2::ZERO;
            // The network demo leaves speed 0; run the clock ourselves while
            // waiting for the light.
            if elapsed == 1 {
                set_speed(&link, 120.0);
            }
            if ready || elapsed > 2400 {
                set_speed(&link, 0.0);
                advance = Some(Stage::Skyline);
            }
        }
        Stage::Skyline => {
            if elapsed == 1 {
                frame_rig(&mut rigs, c, 2100.0, 0.50, 5.9);
            }
            if elapsed == SETTLE {
                shoot(&mut commands, format!("{dir}/promo_skyline.png"));
                advance = Some(Stage::Canyon);
            }
        }
        Stage::Canyon => {
            if elapsed == 1 {
                frame_rig(&mut rigs, c + Vec2::new(220.0, 140.0), 300.0, 0.24, 2.1);
            }
            if elapsed == SETTLE {
                shoot(&mut commands, format!("{dir}/promo_canyon.png"));
                advance = Some(Stage::Network);
            }
        }
        Stage::Network => {
            if elapsed == 1 {
                frame_rig(&mut rigs, c, 640.0, 0.78, 0.9);
            }
            if elapsed == SETTLE {
                shoot(&mut commands, format!("{dir}/promo_network.png"));
                advance = Some(Stage::Arcs);
            }
        }
        Stage::Arcs => {
            if elapsed == 1 {
                overlay.mode = OverlayMode::Demand;
                // 2px hairline gizmos vanish at promo altitude; fatten for
                // this shot only and restore after.
                gizmo_config
                    .config_mut::<DefaultGizmoConfigGroup>()
                    .0
                    .line
                    .width = 6.0;
                frame_rig(&mut rigs, c, 2300.0, 0.82, 0.4);
            }
            if elapsed == SETTLE {
                shoot(&mut commands, format!("{dir}/promo_demand_arcs.png"));
                overlay.mode = OverlayMode::Off;
                gizmo_config
                    .config_mut::<DefaultGizmoConfigGroup>()
                    .0
                    .line
                    .width = 2.0;
                advance = Some(Stage::SubwayDay);
            }
        }
        Stage::SubwayDay => {
            if elapsed == 1 {
                subway.active = true;
                frame_rig(&mut rigs, c, 2200.0, 1.05, 0.15);
            }
            if elapsed == SETTLE + 14 {
                shoot(&mut commands, format!("{dir}/promo_subway_day.png"));
                subway.active = false;
                advance = Some(Stage::Reveal);
            }
        }
        Stage::Reveal => {
            if elapsed == 1 {
                frame_rig(&mut rigs, c, 750.0, 0.62, 3.6);
            }
            // Drive the dissolve directly: promo runs headless, no cursor.
            reveal.center = c;
            reveal.inner = 130.0;
            reveal.outer = 320.0;
            reveal.strength = 1.0;
            if elapsed == SETTLE {
                shoot(&mut commands, format!("{dir}/promo_reveal.png"));
                reveal.strength = 0.0;
                advance = Some(Stage::WaitNight);
            }
        }
        Stage::WaitNight => {
            if elapsed == 1 {
                set_speed(&link, 120.0);
            }
            let h = hour_of(&ui);
            if (20.2..22.5).contains(&h) || elapsed > 2400 {
                set_speed(&link, 0.0);
                advance = Some(Stage::NightGlow);
            }
        }
        Stage::NightGlow => {
            if elapsed == 1 {
                frame_rig(&mut rigs, c, 900.0, 0.66, 5.2);
            }
            if elapsed == SETTLE {
                shoot(&mut commands, format!("{dir}/promo_night_glow.png"));
                advance = Some(Stage::SubwayGraph);
            }
        }
        Stage::SubwayGraph => {
            if elapsed == 1 {
                subway.active = true;
                frame_rig(&mut rigs, c, 2200.0, 1.05, 0.15);
            }
            if elapsed == SETTLE + 14 {
                shoot(&mut commands, format!("{dir}/promo_subway_graph.png"));
                advance = Some(Stage::Done);
            }
        }
        Stage::Done => {
            if elapsed == 30 {
                exit.write(AppExit::Success);
            }
        }
    }

    if let Some(next) = advance {
        state.stage = next;
        state.stage_start = state.frame;
    }
}

fn shoot(commands: &mut Commands, path: String) {
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path));
}
