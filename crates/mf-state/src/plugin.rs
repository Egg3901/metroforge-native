//! `MfStatePlugin` registers every shared resource and the single system
//! that fills them from `mf-net`'s `Events<SimEvent>` stream.

use bevy_app::{App, Plugin, Update};
use bevy_ecs::prelude::*;
use mf_net::{NetSet, SimEvent};
use mf_protocol::{FromSimJson, FromSimMsg};

use crate::attract::AttractLighting;
use crate::city::CurrentCity;
use crate::colorblind::ColorblindMode;
use crate::day_night::DayNightEnabled;
use crate::demand::LatestDemand;
use crate::fields::LatestFields;
use crate::frame::LatestFrame;
use crate::height::HeightAt;
use crate::overlay::OverlayState;
use crate::quality::{
    sync_effective_knobs_system, DetectedQuality, EffectiveKnobs, QualityOverrides, QualityTier,
};
use crate::reveal::RevealState;
use crate::route_focus::RouteFocus;
use crate::subway::SubwayView;
use crate::theme::Theme;
use crate::traffic::LatestTraffic;
use crate::ui::LatestUi;
use crate::weather::{WeatherEffects, WeatherRender};

/// Registers shared sim-mirror resources and applies inbound `SimEvent`s.
pub struct MfStatePlugin;

impl Plugin for MfStatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CurrentCity>()
            .init_resource::<LatestFields>()
            .init_resource::<LatestUi>()
            .init_resource::<LatestFrame>()
            .init_resource::<QualityTier>()
            .init_resource::<QualityOverrides>()
            .init_resource::<EffectiveKnobs>()
            .init_resource::<DetectedQuality>()
            .init_resource::<Theme>()
            .init_resource::<ColorblindMode>()
            .init_resource::<SubwayView>()
            .init_resource::<HeightAt>()
            .init_resource::<RevealState>()
            .init_resource::<LatestDemand>()
            .init_resource::<LatestTraffic>()
            .init_resource::<OverlayState>()
            .init_resource::<RouteFocus>()
            .init_resource::<WeatherEffects>()
            .init_resource::<WeatherRender>()
            .init_resource::<DayNightEnabled>()
            .init_resource::<AttractLighting>()
            // `add_event` is idempotent (it's an `init_resource` under the
            // hood), so it's safe whether or not `MfNetPlugin` was added
            // first; declared explicitly here since `mf-state` reads it.
            .add_event::<SimEvent>()
            .add_systems(
                Update,
                (
                    apply_sim_events_system.after(NetSet::Drain),
                    // Before any render consumer reads knobs this frame.
                    // Registered exactly once, inside `KnobSyncSet` — other
                    // crates order against the SET, never the bare system, so
                    // `sync_effective_knobs_system` is a single schedule
                    // instance and stays usable as an ordering target.
                    sync_effective_knobs_system.in_set(KnobSyncSet),
                ),
            );
    }
}

/// Ordering handle for [`sync_effective_knobs_system`]. The system merges the
/// active quality preset with the player's Advanced overrides into
/// [`EffectiveKnobs`]; anything that reads knobs this frame must run after it.
///
/// It exists because the system was previously `add_systems`'d in BOTH
/// `MfStatePlugin` and `MfRenderPlugin` to hang a `.before(Terrain)` edge off
/// it, which made it a duplicated `SystemTypeSet`. `MfQualityBootPlugin` then
/// ordered `.before`/`.after` the bare system, and Bevy panics at schedule
/// build when you order against an ambiguous duplicate — so the release
/// binary crashed on boot. Registering the system once and ordering everyone
/// against this set removes the ambiguity.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct KnobSyncSet;

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
    mut traffic: ResMut<LatestTraffic>,
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
            FromSimMsg::Elevation(e) => {
                city.apply_elevation((**e).clone());
            }
            FromSimMsg::Fields(f) => {
                // Arc clone — Fields arrays are large; avoid deep-copying
                // every 7 sim-days on NYC-scale grids.
                fields.0 = Some(std::sync::Arc::clone(f));
            }
            FromSimMsg::Frame(f) => {
                // Arc clone — Frame arrives ~20 Hz; deep-cloning vehicle/
                // agent Vecs here was pure allocator churn.
                frame.0 = Some(std::sync::Arc::clone(f));
            }
            FromSimMsg::Json(FromSimJson::Ui(u)) => {
                ui.0 = Some(u.clone());
            }
            FromSimMsg::Json(FromSimJson::Demand(d)) => {
                demand.0 = Some(d.clone());
            }
            FromSimMsg::Traffic(t) => {
                traffic.0 = Some(t.clone());
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
