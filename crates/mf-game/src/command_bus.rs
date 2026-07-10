//! INTEGRATION STUB (ship-plan #25): a parallel agent owns the real
//! `command_bus.rs` - seq bookkeeping tied to a proper undo stack, replay
//! log, etc. This file exists only so `build_ui.rs` (owned by this
//! worktree) has the `CommandBus`/`CommandFeedback`/`CmdMeta` surface it
//! codes against, and so `cargo fmt`/`clippy`/`test` pass standing alone on
//! `v02/build-ui`. It is expected to be replaced wholesale by the real
//! implementation at integration, not merged with it.
//!
//! What *is* real here: `submit` actually sends `ToSim::Command` over the
//! live `SimLink`, and [`command_bus_feedback_system`] actually correlates
//! `commandResult`s back to the `CmdMeta` recorded at submit time and
//! republishes them as [`CommandFeedback`] - that plumbing is simple enough
//! to write once rather than mock, and it means `build_ui.rs`'s feedback
//! listener has something genuine to exercise even before the real
//! command_bus lands. Undo is the one piece left as a bare stub (`can_undo`
//! /`undo_last`): real undo semantics (what "invert the last command" means
//! per command kind) is squarely the parallel agent's design, not
//! something to guess at here.
//!
//! `CmdMeta`'s non-`EditRoute` variants and `CommandFeedback::seq`/
//! `created_id` aren't constructed/read by `build_ui.rs` in this worktree
//! (it only ever submits `EditRoute`-tagged commands - world-click-driven
//! `BuildStation`/`BuildTrack`/`CreateRoute`/`Demolish`/`Undo`/`Query`
//! belong to the real `tools.rs`) - allowed dead here rather than trimmed,
//! since removing them would drift this stub away from the API contract
//! it exists to mirror.
#![allow(dead_code)]

use std::collections::HashMap;

use bevy::prelude::*;
use mf_net::{SimEvent, SimLink};
use mf_protocol::{Command, FromSimJson, FromSimMsg, ToSim, TransitMode, Vec2};

/// What a submitted command was *for*, carried alongside the wire `seq` so
/// a `CommandFeedback` listener can react without re-deriving intent from
/// the `Command` payload itself. Fields mirror the real `command_bus.rs`'s
/// `CmdMeta` exactly (not just the mission brief's abbreviated `{..}`) so a
/// `matches!` pattern written against this stub (see `build_ui.rs`'s
/// `CmdMeta::CreateRoute { .. }` feedback check) doesn't need editing once
/// the real implementation replaces this file.
#[derive(Debug, Clone, PartialEq)]
pub enum CmdMeta {
    BuildStation {
        mode: TransitMode,
        pos: Vec2,
    },
    BuildTrack {
        from: i64,
        to: i64,
    },
    CreateRoute {
        mode: TransitMode,
        station_ids: Vec<i64>,
    },
    EditRoute {
        route_id: i64,
    },
    Demolish,
    Undo,
    Query,
}

/// Fired once the sidecar's `commandResult` for a given `seq` arrives.
#[derive(Event, Debug, Clone)]
pub struct CommandFeedback {
    pub seq: u32,
    pub ok: bool,
    pub error: Option<String>,
    pub created_id: Option<i64>,
    pub meta: CmdMeta,
}

/// Assigns wire `seq`s, remembers which [`CmdMeta`] each is for until its
/// `commandResult` comes back, and (stub) tracks whether *anything* has
/// round-tripped successfully so the toolbar's Undo button isn't
/// permanently disabled during standalone development in this worktree.
#[derive(Resource, Default)]
pub struct CommandBus {
    next_seq: u32,
    pending: HashMap<u32, CmdMeta>,
    any_ok_command: bool,
}

impl CommandBus {
    /// Assigns the next `seq`, records `meta` for correlation, and sends
    /// `cmd` over `link`. Returns the assigned `seq` (callers generally
    /// don't need it - `CommandFeedback` carries it back - but it's useful
    /// for tests/logging).
    pub fn submit(&mut self, link: &SimLink, cmd: Command, meta: CmdMeta) -> u32 {
        self.next_seq += 1;
        let seq = self.next_seq;
        self.pending.insert(seq, meta);
        let _ = link.transport.send(ToSim::Command { seq, cmd });
        seq
    }

    pub fn can_undo(&self) -> bool {
        self.any_ok_command
    }

    /// Stub: does not actually invert anything yet (see module docs). Real
    /// undo semantics ship with the real `command_bus.rs`.
    pub fn undo_last(&mut self, _link: &SimLink) -> bool {
        false
    }
}

/// Drains `commandResult`s off the wire, matches each back to the
/// [`CmdMeta`] [`CommandBus::submit`] recorded, and republishes as
/// [`CommandFeedback`] for `build_ui.rs` (and, later, the real
/// command_bus's own consumers) to act on.
fn command_bus_feedback_system(
    mut sim_events: EventReader<SimEvent>,
    mut bus: ResMut<CommandBus>,
    mut feedback: EventWriter<CommandFeedback>,
) {
    for SimEvent(msg) in sim_events.read() {
        if let FromSimMsg::Json(FromSimJson::CommandResult { seq, result }) = msg {
            let Some(seq) = seq else { continue };
            let Some(meta) = bus.pending.remove(seq) else {
                continue;
            };
            if result.ok {
                bus.any_ok_command = true;
            }
            feedback.write(CommandFeedback {
                seq: *seq,
                ok: result.ok,
                error: result.error.clone(),
                created_id: result.created_id,
                meta,
            });
        }
    }
}

pub struct MfCommandBusStubPlugin;

impl Plugin for MfCommandBusStubPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CommandBus>()
            .add_event::<CommandFeedback>()
            .add_systems(Update, command_bus_feedback_system);
    }
}
