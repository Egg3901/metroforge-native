//! `MfStatePlugin` registers every shared resource and the single system
//! that fills them from `mf-net`'s `Events<SimEvent>` stream.

use bevy_app::{App, Plugin, Update};
use bevy_ecs::prelude::*;
use mf_net::{NetSet, SimEvent};
use mf_protocol::{FromSimJson, FromSimMsg};

use crate::city::CurrentCity;
use crate::demand::LatestDemand;
use crate::fields::LatestFields;
use crate::frame::LatestFrame;
use crate::height::HeightAt;
use crate::overlay::OverlayState;
use crate::quality::QualityTier;
use crate::reveal::RevealState;
use crate::subway::SubwayView;
use crate::theme::Theme;
use crate::ui::LatestUi;

pub struct MfStatePlugin;

impl Plugin for MfStatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CurrentCity>()
            .init_resource::<LatestFields>()
            .init_resource::<LatestUi>()
            .init_resource::<LatestFrame>()
            .init_resource::<QualityTier>()
            .init_resource::<Theme>()
            .init_resource::<SubwayView>()
            .init_resource::<HeightAt>()
            .init_resource::<RevealState>()
            .init_resource::<LatestDemand>()
            .init_resource::<OverlayState>()
            // `add_event` is idempotent (it's an `init_resource` under the
            // hood), so it's safe whether or not `MfNetPlugin` was added
            // first; declared explicitly here since `mf-state` reads it.
            .add_event::<SimEvent>()
            .add_systems(Update, apply_sim_events_system.after(NetSet::Drain));
    }
}

/// Drains this frame's `SimEvent`s into the shared resources. Runs after
/// `mf-net`'s drain system (`NetSet::Drain`) so events pushed this frame are
/// visible to it.
fn apply_sim_events_system(
    mut events: EventReader<SimEvent>,
    mut city: ResMut<CurrentCity>,
    mut fields: ResMut<LatestFields>,
    mut ui: ResMut<LatestUi>,
    mut frame: ResMut<LatestFrame>,
    mut demand: ResMut<LatestDemand>,
) {
    for SimEvent(msg) in events.read() {
        match msg {
            FromSimMsg::Json(FromSimJson::Ready(ready)) => {
                city.set_static_city(ready.static_city.clone());
            }
            FromSimMsg::Mask(mask) => {
                city.apply_mask(mask.clone());
            }
            FromSimMsg::Buildings(buildings) => {
                city.apply_buildings(buildings.clone());
            }
            FromSimMsg::Fields(f) => {
                fields.0 = Some(f.clone());
            }
            FromSimMsg::Frame(f) => {
                frame.0 = Some(f.clone());
            }
            FromSimMsg::Json(FromSimJson::Ui(u)) => {
                ui.0 = Some(u.clone());
            }
            FromSimMsg::Json(FromSimJson::Demand(d)) => {
                demand.0 = Some(d.clone());
            }
            // Other JSON messages (hello/commandResult/trackCost/saved/
            // replay/toast/pong/bye) and the Traffic binary frame are
            // consumed directly by `mf-game`/`mf-render` systems that need
            // them (HUD toasts, command result correlation, traffic overlay
            // — out of v1 scope per spec §5) rather than mirrored here.
            _ => {}
        }
    }
}
