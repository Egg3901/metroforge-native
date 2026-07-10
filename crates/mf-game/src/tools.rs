//! INTEGRATION STUB (ship-plan #25): a parallel agent owns the real
//! `tools.rs` - world raycasting/placement, drag-to-draw route building,
//! live cost quoting via `QueryTrackCost`. This file exists only so
//! `build_ui.rs` (owned by this worktree) has the `ActiveTool`/`ToolState`
//! surface it codes against and so `cargo fmt`/`clippy`/`test` pass
//! standing alone in `v02/build-ui`. It is expected to be replaced
//! wholesale by the real implementation at integration, not merged with it
//! - see the mission brief's API contract for the exact shape this mirrors.
//!
//! `route_mode` is read/written by the real (not-yet-merged) `tools.rs`,
//! not by `build_ui.rs` in this worktree - allowed dead here rather than
//! trimmed, since removing it would drift this stub's shape away from the
//! API contract it exists to mirror.
#![allow(dead_code)]

use bevy::prelude::*;
use mf_protocol::TransitMode;

/// Which build tool is currently active. `build_ui.rs`'s toolbar sets this;
/// the real `tools.rs` reads it to decide what a world click does.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ActiveTool {
    #[default]
    None,
    PlaceStation(TransitMode),
    Route,
    Bulldoze,
}

/// Resource: the active tool plus in-progress route-drawing state. Real
/// station-id/cost-quote population happens in the (not-yet-merged) real
/// `tools.rs`; this stub only carries the fields so `build_ui.rs` can read
/// and display them.
#[derive(Resource, Debug, Clone)]
pub struct ToolState {
    pub active: ActiveTool,
    /// Station ids picked so far while drawing a route.
    pub route_draft: Vec<i64>,
    pub route_mode: TransitMode,
    /// Last cost quote returned for the in-progress build/route, if any.
    pub last_cost_quote: Option<f64>,
}

impl Default for ToolState {
    fn default() -> Self {
        ToolState {
            active: ActiveTool::None,
            route_draft: Vec::new(),
            route_mode: TransitMode::Bus,
            last_cost_quote: None,
        }
    }
}

/// Registers only the `ToolState` resource - no click/placement systems
/// live here, those belong to the real `tools.rs`.
pub struct MfToolsStubPlugin;

impl Plugin for MfToolsStubPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ToolState>();
    }
}
