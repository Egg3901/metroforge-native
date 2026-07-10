//! INTEGRATION STUB - replaced by v02/command-bus.
//!
//! This worktree owns `tools.rs` only; the real `CommandBus` (seq
//! allocation shared across multiple in-flight commands, undo stack,
//! `CommandFeedback` resolution off `commandResult` replies) is being built
//! in parallel on the `v02/command-bus` branch against the exact same
//! public API declared here. This file exists purely so `tools.rs` compiles
//! and its own logic can be reviewed/tested in isolation; integration
//! deletes this file and drops the real module in unmodified.
//!
//! Deliberately minimal: no undo, no timeout handling, no `CommandFeedback`
//! ever actually fires (there is no pump system reading `commandResult`
//! replies into it) since nothing in THIS worktree depends on that firing
//! to compile or to unit-test `tools.rs`'s own pure logic. Do not extend
//! this file: extend the real one on `v02/command-bus` instead.
//!
//! Nothing in this stub is called outside of `tools.rs`, which is itself
//! not yet wired into `main.rs` (see that module's doc comment), so this
//! whole file is unreachable dead code for now; allowed here rather than
//! per-item since the entire file is temporary scaffolding.
#![allow(dead_code)]

use bevy::prelude::*;
use mf_net::SimLink;
use mf_protocol::{Command, ToSim, TransitMode, Vec2};

/// What a submitted command was for, from the player's perspective. Matches
/// the real `command_bus::CmdMeta`'s variants exactly (see the ship-plan
/// #25 API contract); round-trips through `CommandFeedback` in the real
/// module so a UI layer can react to the specific tool that fired it.
#[derive(Debug, Clone)]
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

/// Fired once per resolved `commandResult` in the real module. Declared
/// here (unused by anything in this stub) purely so `tools.rs` can
/// `EventReader<CommandFeedback>` and compile.
#[derive(Event, Debug, Clone)]
pub struct CommandFeedback {
    pub seq: u32,
    pub ok: bool,
    pub error: Option<String>,
    pub created_id: Option<i64>,
    pub meta: CmdMeta,
}

/// Minimal stand-in: allocates an incrementing seq and fires the command
/// over the wire immediately, same as the real bus's `submit`. Has no
/// in-flight bookkeeping, so it never actually resolves a `CommandFeedback`
/// (that's fine for this worktree, since nothing here depends on that
/// happening at runtime, only on the type existing for `tools.rs` to name).
#[derive(Resource, Default)]
pub struct CommandBus {
    next_seq: u32,
}

impl CommandBus {
    pub fn submit(&mut self, link: &SimLink, cmd: Command, _meta: CmdMeta) -> u32 {
        self.next_seq += 1;
        let seq = self.next_seq;
        let _ = link.transport.send(ToSim::Command { seq, cmd });
        seq
    }

    pub fn can_undo(&self) -> bool {
        false
    }

    pub fn undo_last(&mut self, _link: &SimLink) -> bool {
        false
    }
}
