use bevy_ecs::prelude::*;
use mf_protocol::UiState;

/// The most recent `UiState` pushed by the sidecar at 2 Hz: budget,
/// approval, stations/tracks/routes, active events, etc. `mf-game`'s
/// `hud.rs` reads this directly to draw the egui HUD; `mf-render`'s
/// `transit.rs` reads `stations`/`tracks`/`routes` to rebuild the network
/// visualization when their identities/counts change.
#[derive(Resource, Default)]
pub struct LatestUi(pub Option<UiState>);
