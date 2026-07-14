//! The `BuildDepot` command logic (v0.9 System A / A4). Ports the `buildDepot`
//! case body from `sim/src/core/commands.ts`.
//!
//! One placeable depot per mode. Its presence enables maintenance windows for
//! that mode (see `ops_daily_close`). Delivered as a standalone fn; the frozen
//! `apply_command` dispatch is wired by the coordinator.

use crate::commands::CommandResult;
use crate::constants::modes;
use crate::fields::is_water_at;
use crate::geometry::Vec2;
use crate::ops::tunables::ops_tunables;
use crate::types::{Depot, GameState, TransitMode};

/// Build a maintenance depot for `mode` at `pos`. Validates: not on water, one
/// depot per mode, and sufficient cash. On success mints the depot entity,
/// charges the build cost, and returns the new id. Mirrors the `buildDepot`
/// command in commands.ts.
pub fn build_depot(state: &mut GameState, mode: TransitMode, pos: Vec2) -> CommandResult {
    if is_water_at(&state.fields, pos) {
        return CommandResult {
            ok: false,
            error: Some("Cannot build a depot on water".to_string()),
            created_id: None,
        };
    }
    let depots = state.depots.get_or_insert_with(Vec::new);
    if depots.iter().any(|d| d.mode == mode) {
        return CommandResult {
            ok: false,
            error: Some(format!("A {} depot already exists", modes(mode).label)),
            created_id: None,
        };
    }
    let cost = ops_tunables(state.difficulty).depot_build_cost;
    if state.budget.cash < cost {
        return CommandResult {
            ok: false,
            error: Some("Insufficient funds for depot".to_string()),
            created_id: None,
        };
    }
    let id = state.next_id;
    state.next_id += 1;
    state.depots.get_or_insert_with(Vec::new).push(Depot {
        id,
        mode,
        pos,
        build_tick: state.tick,
    });
    state.budget.cash -= cost;
    CommandResult {
        ok: true,
        error: None,
        created_id: Some(id),
    }
}
