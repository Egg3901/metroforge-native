//! Player-facing command dispatcher for build tools (ship-plan issue #25,
//! v0.2). Generalizes `verify.rs`'s `NetworkBuildState` seq-correlation
//! pattern (one `ToSim::Command` matched back to its `commandResult` by
//! `seq`) to support MULTIPLE commands in flight at once, since the route
//! tool fires a burst of `BuildTrack` commands back to back rather than one
//! at a time.
//!
//! Deliberately decoupled from UI: this module only emits [`CommandFeedback`]
//! events. Toast/SFX wiring (`hud.rs`'s `ToastLog`, `audio`'s `PlaySfx`)
//! is the UI layer's job, subscribing to those events. Keeping the bus
//! itself silent/pure means it can be unit tested without a Bevy `App`.

use std::collections::HashMap;

use bevy::prelude::*;
use mf_net::{NetSet, SimEvent, SimLink};
use mf_protocol::{Command, CommandResult, FromSimJson, FromSimMsg, ToSim, TransitMode, Vec2};

/// First seq the bus hands out. `verify.rs`'s `NetworkBuildState` harness
/// starts its own counter at 1 (see that module), and only ever runs
/// standalone in a verify build alongside this bus, never sharing a wire
/// connection with it today. Starting well above any range that harness
/// could plausibly reach removes any need to ever prove the two can't
/// collide, at zero cost.
const SEQ_START: u32 = 1000;

/// How many pump-system frames an in-flight command can go unanswered
/// before the bus gives up on it. Mirrors the budget `verify.rs` uses for
/// its own analogous per-command timeout (`NETWORK_COMMAND_TIMEOUT_FRAMES`),
/// generous because a slow host, not the wire, is the likely source of any
/// real delay.
const TIMEOUT_FRAMES: u64 = 600;

/// Cap on the undo stack so a very long build session can't grow it
/// without bound. Each entry is a small `Command` value, so the memory
/// cost is negligible either way; the cap exists to keep undo depth a
/// deliberate, documented number rather than "however long the session
/// has been running."
const UNDO_STACK_CAP: usize = 32;

/// What a `submit`ted command was *for*, from the player's perspective.
/// Round-trips through [`CommandFeedback`] so a UI layer can react to the
/// specific tool that fired it, and tells the pump system which inverse
/// command (if any) to push onto the undo stack on success.
///
/// `#[allow(dead_code)]`: this crate is a binary, so rustc's dead-code pass
/// only counts code reachable from `main`. Nothing in `mf-game` constructs
/// these variants yet; the v0.2 build-tool agents landing in parallel
/// (ship-plan issue #25) are what call `CommandBus::submit` with them. Kept
/// as a real (if temporarily unreachable) public API rather than papered
/// over with a synthetic caller, which would just be dead code with extra
/// steps.
#[allow(dead_code)]
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
    /// Operations (v0.9 A1): set one service period's target headway.
    SetRouteFrequency {
        route_id: i64,
    },
    /// Operations (v0.9 A4): place a maintenance depot for a mode.
    BuildDepot {
        mode: TransitMode,
    },
    Demolish,
    Undo,
    Query,
}

/// Fired once per resolved `commandResult` that matches a still-pending
/// entry (an unknown or already-timed-out seq resolves to nothing, see
/// [`CommandBus::resolve`]). A UI layer subscribes to this to play a
/// confirm/error sound, toast an error, or hand a freshly created id to
/// whatever tool asked for it. The bus itself never touches audio or the
/// HUD's `ToastLog`; that coupling belongs to the UI systems that read
/// this event.
///
/// `#[allow(dead_code)]`: constructed by [`CommandBus::resolve`] (live, via
/// the pump system) but not yet read by anything in `mf-game` itself, only
/// by this module's tests, since no UI subscriber has landed here yet. Same
/// "real API, temporarily unreachable from `main`" situation as [`CmdMeta`].
#[allow(dead_code)]
#[derive(Event, Debug, Clone)]
pub struct CommandFeedback {
    pub seq: u32,
    pub ok: bool,
    pub error: Option<String>,
    pub created_id: Option<i64>,
    pub meta: CmdMeta,
}

/// One command the bus is waiting on a reply for.
struct InFlightEntry {
    meta: CmdMeta,
    /// The bus's own frame counter (see [`CommandBus::frame`]) at the
    /// moment this was submitted, so the pump system can time it out.
    since_frame: u64,
}

/// Player command dispatcher. Owns seq allocation, the in-flight
/// seq -> [`CmdMeta`] correlation map (plural entries, unlike `verify.rs`'s
/// single-pending harness), and the undo stack.
#[derive(Resource, Default)]
pub struct CommandBus {
    /// `None` until the first command is ever submitted, at which point it
    /// is seeded to [`SEQ_START`]. Kept as `Option` (rather than a plain
    /// `u32` pre-initialized to `SEQ_START`) so this struct can still
    /// `#[derive(Default)]` per the v0.2 API contract, instead of a
    /// hand-rolled `Default` impl that exists solely to special-case one
    /// field.
    ///
    /// `#[allow(dead_code)]`: only read from `alloc_seq`, which is itself
    /// only reachable via `submit`/`undo_last`; see those methods' own
    /// `#[allow(dead_code)]` notes below for why they're unreachable from
    /// `main` today.
    #[allow(dead_code)]
    next_seq: Option<u32>,
    in_flight: HashMap<u32, InFlightEntry>,
    undo_stack: Vec<Command>,
    /// Frames since the bus was created, advanced once per pump-system
    /// tick. Deliberately a frame count rather than wall-clock time: it is
    /// the same budget unit `verify.rs` already uses for its own timeout,
    /// and it means tests never depend on real elapsed time.
    frame: u64,
}

impl CommandBus {
    /// Sends `cmd` immediately over `link`, remembers `meta` against the
    /// seq it was assigned, and returns that seq. Most callers can ignore
    /// the return value and just listen for `CommandFeedback` broadly; it
    /// exists for a tool that wants to correlate its own local state to one
    /// specific reply.
    ///
    /// `#[allow(dead_code)]`: this is the v0.2 build tools' entry point
    /// into the bus (ship-plan issue #25); those tools land in parallel
    /// branches and aren't wired up on this one, so nothing in `mf-game`'s
    /// own `main`-reachable graph calls this yet, only this module's tests.
    #[allow(dead_code)]
    pub fn submit(&mut self, link: &SimLink, cmd: Command, meta: CmdMeta) -> u32 {
        let seq = self.alloc_seq(meta);
        // Fire-and-forget: a send failure only happens if the in-process sim
        // worker has gone away, and there is nothing useful for this bus to do
        // with the error beyond not panicking.
        let _ = link.transport.send(ToSim::Command { seq, cmd });
        seq
    }

    /// True if there is at least one successful, undoable action recorded.
    ///
    /// `#[allow(dead_code)]`: an undo-button UI reads this to decide
    /// whether to enable itself; that UI hasn't landed on this branch yet.
    /// See `submit`'s note above for the general "unreachable from `main`
    /// until v0.2 wiring lands" situation.
    #[allow(dead_code)]
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Pops the most recently pushed inverse command and submits it,
    /// tagged `CmdMeta::Undo`. `CmdMeta::Undo` is not itself undoable (see
    /// [`inverse_for`]), so this can never grow the stack back, i.e. no
    /// redo in v0.2. Returns `false` (and sends nothing) if the stack is
    /// empty.
    ///
    /// `#[allow(dead_code)]`: same situation as `submit`/`can_undo` above.
    #[allow(dead_code)]
    pub fn undo_last(&mut self, link: &SimLink) -> bool {
        let Some(inverse) = self.undo_stack.pop() else {
            return false;
        };
        self.submit(link, inverse, CmdMeta::Undo);
        true
    }

    /// Pure seq allocation plus in-flight bookkeeping, split out from
    /// `submit` so tests can exercise it without a live `SimLink`.
    ///
    /// `#[allow(dead_code)]`: only called from `submit`, which is itself
    /// unreachable from `main` today; see its note above.
    #[allow(dead_code)]
    fn alloc_seq(&mut self, meta: CmdMeta) -> u32 {
        let seq = self.next_seq.unwrap_or(SEQ_START);
        self.next_seq = Some(seq + 1);
        self.in_flight.insert(
            seq,
            InFlightEntry {
                meta,
                since_frame: self.frame,
            },
        );
        seq
    }

    /// Pushes `cmd` as the next undo step. Once at [`UNDO_STACK_CAP`], the
    /// OLDEST entry is dropped to make room, so the stack always reflects
    /// the most recent successful actions rather than refusing new ones
    /// once full.
    fn push_undo(&mut self, cmd: Command) {
        if self.undo_stack.len() >= UNDO_STACK_CAP {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(cmd);
    }

    /// Resolves an inbound `commandResult` against the in-flight entry
    /// with a matching seq, if any is still pending. A seq with no
    /// matching entry (unknown to begin with, or already dropped by
    /// [`CommandBus::drop_timed_out`]) is silently ignored: there is
    /// nothing meaningful to report for it. On success, pushes the
    /// action's inverse (if it has one) onto the undo stack. Pure/no I/O,
    /// so it is directly unit-testable; the pump system is a thin wrapper
    /// that also writes the resulting event.
    fn resolve(&mut self, seq: u32, result: &CommandResult) -> Option<CommandFeedback> {
        let entry = self.in_flight.remove(&seq)?;
        if result.ok {
            if let Some(inverse) = inverse_for(&entry.meta, result.created_id) {
                self.push_undo(inverse);
            }
        }
        Some(CommandFeedback {
            seq,
            ok: result.ok,
            error: result.error.clone(),
            created_id: result.created_id,
            meta: entry.meta,
        })
    }

    /// Drops any in-flight entry older than [`TIMEOUT_FRAMES`], warning
    /// once per dropped entry. No `CommandFeedback` fires for these: a
    /// timeout means "no answer," not a definite ok/fail the UI should
    /// render as either, and a late reply arriving afterward is simply an
    /// unknown seq to [`CommandBus::resolve`] from then on.
    fn drop_timed_out(&mut self) {
        let frame = self.frame;
        let stale: Vec<u32> = self
            .in_flight
            .iter()
            .filter(|(_, entry)| frame.saturating_sub(entry.since_frame) > TIMEOUT_FRAMES)
            .map(|(seq, _)| *seq)
            .collect();
        for seq in stale {
            self.in_flight.remove(&seq);
            tracing::warn!(
                "command_bus: seq={seq} timed out waiting for a commandResult ({TIMEOUT_FRAMES} frames)"
            );
        }
    }
}

/// Which command undoes a given completed action, if any. Only the three
/// player-creation commands are undoable in v0.2 (`EditRoute`/`Demolish`/
/// `Undo`/`Query` all return `None` and are simply never pushed onto the
/// stack). A missing `created_id` on an otherwise-successful reply also
/// yields `None`: there is no id to reverse the action against, even
/// though the sim's own contract shouldn't produce that combination.
fn inverse_for(meta: &CmdMeta, created_id: Option<i64>) -> Option<Command> {
    match meta {
        CmdMeta::BuildStation { .. } => {
            created_id.map(|id| Command::DemolishStation { station_id: id })
        }
        CmdMeta::BuildTrack { .. } => created_id.map(|id| Command::DemolishTrack { track_id: id }),
        CmdMeta::CreateRoute { .. } => created_id.map(|id| Command::DeleteRoute { route_id: id }),
        // Depot placement has no demolish command in the sim contract, and a
        // frequency change is a value edit, not an entity creation. Neither is
        // undoable, so both are simply never pushed onto the stack.
        CmdMeta::EditRoute { .. }
        | CmdMeta::SetRouteFrequency { .. }
        | CmdMeta::BuildDepot { .. }
        | CmdMeta::Demolish
        | CmdMeta::Undo
        | CmdMeta::Query => None,
    }
}

/// Advances the bus's frame counter, drops anything that has timed out,
/// then reads every `commandResult` off the wire this frame and turns each
/// one that matches an in-flight entry into a `CommandFeedback` event.
fn command_bus_pump_system(
    mut bus: ResMut<CommandBus>,
    mut sim_events: EventReader<SimEvent>,
    mut feedback: EventWriter<CommandFeedback>,
) {
    bus.frame += 1;
    bus.drop_timed_out();
    for SimEvent(msg) in sim_events.read() {
        if let FromSimMsg::Json(FromSimJson::CommandResult {
            seq: Some(seq),
            result,
        }) = msg
        {
            if let Some(fb) = bus.resolve(*seq, result) {
                feedback.write(fb);
            }
        }
        // `seq: None` commandResults are unaddressed replies (shouldn't
        // happen for anything this bus itself sent, since every submit
        // assigns a seq) and are simply not this bus's concern.
    }
}

pub struct MfCommandBusPlugin;

impl Plugin for MfCommandBusPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CommandBus>()
            .add_event::<CommandFeedback>()
            .add_systems(Update, command_bus_pump_system.after(NetSet::Drain));
    }
}

// ---------------------------------------------------------------------
// Tests: pure bus logic, no Bevy App needed. A tiny in-memory
// `SimTransport` double lets `submit`/`undo_last` be exercised through
// their real public API (recording what was actually sent) rather than
// only through their private helpers.
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mf_net::SimTransport;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct TestTransport {
        sent: Arc<Mutex<Vec<ToSim>>>,
    }

    impl TestTransport {
        fn sent(&self) -> Vec<ToSim> {
            self.sent.lock().unwrap().clone()
        }
    }

    impl SimTransport for TestTransport {
        fn send(&self, msg: ToSim) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push(msg);
            Ok(())
        }
        fn try_recv(&self) -> Option<FromSimMsg> {
            None
        }
        fn is_alive(&self) -> bool {
            true
        }
        fn silence_duration(&self) -> std::time::Duration {
            std::time::Duration::ZERO
        }
    }

    fn test_link() -> (SimLink, TestTransport) {
        let transport = TestTransport::default();
        let link = SimLink {
            transport: Box::new(transport.clone()),
        };
        (link, transport)
    }

    fn ok(created_id: Option<i64>) -> CommandResult {
        CommandResult {
            ok: true,
            error: None,
            created_id,
        }
    }

    fn err(msg: &str) -> CommandResult {
        CommandResult {
            ok: false,
            error: Some(msg.to_string()),
            created_id: None,
        }
    }

    // ---- seq increment ----

    #[test]
    fn seq_increments_from_seq_start() {
        let mut bus = CommandBus::default();
        let a = bus.alloc_seq(CmdMeta::Query);
        let b = bus.alloc_seq(CmdMeta::Query);
        let c = bus.alloc_seq(CmdMeta::Query);
        assert_eq!(a, SEQ_START);
        assert_eq!(b, SEQ_START + 1);
        assert_eq!(c, SEQ_START + 2);
    }

    #[test]
    fn submit_sends_over_the_link_with_the_allocated_seq() {
        let (link, transport) = test_link();
        let mut bus = CommandBus::default();
        let pos = Vec2 { x: 5.0, y: 6.0 };
        let seq = bus.submit(
            &link,
            Command::BuildStation {
                mode: TransitMode::Bus,
                pos,
            },
            CmdMeta::BuildStation {
                mode: TransitMode::Bus,
                pos,
            },
        );
        assert_eq!(seq, SEQ_START);
        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        match &sent[0] {
            ToSim::Command { seq: sent_seq, cmd } => {
                assert_eq!(*sent_seq, SEQ_START);
                assert_eq!(
                    *cmd,
                    Command::BuildStation {
                        mode: TransitMode::Bus,
                        pos
                    }
                );
            }
            other => panic!("expected ToSim::Command, got {other:?}"),
        }
    }

    // ---- inverse mapping per meta type ----

    #[test]
    fn inverse_mapping_per_meta_type() {
        assert_eq!(
            inverse_for(
                &CmdMeta::BuildStation {
                    mode: TransitMode::Bus,
                    pos: Vec2 { x: 1.0, y: 2.0 }
                },
                Some(10)
            ),
            Some(Command::DemolishStation { station_id: 10 })
        );
        assert_eq!(
            inverse_for(&CmdMeta::BuildTrack { from: 1, to: 2 }, Some(20)),
            Some(Command::DemolishTrack { track_id: 20 })
        );
        assert_eq!(
            inverse_for(
                &CmdMeta::CreateRoute {
                    mode: TransitMode::Bus,
                    station_ids: vec![1, 2, 3]
                },
                Some(30)
            ),
            Some(Command::DeleteRoute { route_id: 30 })
        );
        for meta in [
            CmdMeta::EditRoute { route_id: 1 },
            CmdMeta::Demolish,
            CmdMeta::Undo,
            CmdMeta::Query,
        ] {
            assert_eq!(
                inverse_for(&meta, Some(99)),
                None,
                "{meta:?} must not be undoable"
            );
        }
        // No created_id means no inverse even for an otherwise-undoable meta.
        assert_eq!(
            inverse_for(
                &CmdMeta::BuildStation {
                    mode: TransitMode::Bus,
                    pos: Vec2 { x: 0.0, y: 0.0 }
                },
                None
            ),
            None
        );
    }

    // ---- undo stack cap + ordering ----

    #[test]
    fn undo_stack_caps_and_drops_the_oldest_entry() {
        let mut bus = CommandBus::default();
        for i in 0..40i64 {
            bus.push_undo(Command::DemolishStation { station_id: i });
        }
        assert_eq!(bus.undo_stack.len(), UNDO_STACK_CAP);
        // The oldest 8 (ids 0..8) were dropped to stay at the 32 cap; the
        // remaining run is 8..40, still in push order.
        assert_eq!(
            bus.undo_stack.first(),
            Some(&Command::DemolishStation { station_id: 8 })
        );
        assert_eq!(
            bus.undo_stack.last(),
            Some(&Command::DemolishStation { station_id: 39 })
        );
    }

    #[test]
    fn undo_pops_most_recently_pushed_first() {
        let mut bus = CommandBus::default();
        bus.push_undo(Command::DemolishStation { station_id: 1 });
        bus.push_undo(Command::DemolishStation { station_id: 2 });
        bus.push_undo(Command::DemolishStation { station_id: 3 });
        assert_eq!(
            bus.undo_stack.pop(),
            Some(Command::DemolishStation { station_id: 3 })
        );
        assert_eq!(
            bus.undo_stack.pop(),
            Some(Command::DemolishStation { station_id: 2 })
        );
        assert_eq!(
            bus.undo_stack.pop(),
            Some(Command::DemolishStation { station_id: 1 })
        );
    }

    #[test]
    fn undo_last_submits_the_inverse_and_is_not_itself_undoable() {
        let (link, transport) = test_link();
        let mut bus = CommandBus::default();

        let seq = bus.submit(
            &link,
            Command::BuildStation {
                mode: TransitMode::Bus,
                pos: Vec2 { x: 0.0, y: 0.0 },
            },
            CmdMeta::BuildStation {
                mode: TransitMode::Bus,
                pos: Vec2 { x: 0.0, y: 0.0 },
            },
        );
        let fb = bus.resolve(seq, &ok(Some(77))).expect("known seq resolves");
        assert!(fb.ok);
        assert!(bus.can_undo());

        assert!(bus.undo_last(&link));
        assert!(
            !bus.can_undo(),
            "undo_last must not push its own inverse back onto the stack"
        );

        let sent = transport.sent();
        assert_eq!(sent.len(), 2, "the original build plus the undo");
        match &sent[1] {
            ToSim::Command { cmd, .. } => {
                assert_eq!(*cmd, Command::DemolishStation { station_id: 77 });
            }
            other => panic!("expected ToSim::Command, got {other:?}"),
        }
    }

    #[test]
    fn undo_last_on_an_empty_stack_sends_nothing() {
        let (link, transport) = test_link();
        let mut bus = CommandBus::default();
        assert!(!bus.undo_last(&link));
        assert!(transport.sent().is_empty());
    }

    // ---- non-undoable metas do not stack ----

    #[test]
    fn non_undoable_metas_do_not_stack_even_on_success() {
        let mut bus = CommandBus::default();
        for meta in [
            CmdMeta::EditRoute { route_id: 1 },
            CmdMeta::Demolish,
            CmdMeta::Undo,
            CmdMeta::Query,
        ] {
            let seq = bus.alloc_seq(meta);
            let fb = bus.resolve(seq, &ok(Some(42))).expect("known seq resolves");
            assert!(fb.ok);
        }
        assert!(!bus.can_undo());
        assert!(bus.undo_stack.is_empty());
    }

    #[test]
    fn a_failed_undoable_command_does_not_stack_either() {
        let mut bus = CommandBus::default();
        let seq = bus.alloc_seq(CmdMeta::BuildStation {
            mode: TransitMode::Bus,
            pos: Vec2 { x: 0.0, y: 0.0 },
        });
        let fb = bus
            .resolve(seq, &err("no room"))
            .expect("known seq resolves");
        assert!(!fb.ok);
        assert!(!bus.can_undo());
    }

    // ---- feedback for unknown seq is ignored ----

    #[test]
    fn resolve_ignores_an_unknown_seq() {
        let mut bus = CommandBus::default();
        assert!(bus.resolve(999_999, &ok(Some(1))).is_none());
        assert!(bus.undo_stack.is_empty());
    }

    // ---- multiple in-flight commands (route-tool bursts) ----

    #[test]
    fn multiple_in_flight_commands_resolve_independently() {
        let mut bus = CommandBus::default();
        let seq_a = bus.alloc_seq(CmdMeta::BuildTrack { from: 1, to: 2 });
        let seq_b = bus.alloc_seq(CmdMeta::BuildTrack { from: 2, to: 3 });
        assert_ne!(seq_a, seq_b);
        assert_eq!(bus.in_flight.len(), 2);

        let fb_b = bus.resolve(seq_b, &ok(Some(200))).unwrap();
        assert_eq!(fb_b.seq, seq_b);
        assert_eq!(bus.in_flight.len(), 1, "seq_a must still be pending");

        let fb_a = bus.resolve(seq_a, &ok(Some(100))).unwrap();
        assert_eq!(fb_a.seq, seq_a);
        assert!(bus.in_flight.is_empty());
    }

    // ---- timeouts ----

    #[test]
    fn drop_timed_out_removes_stale_entries_and_a_late_reply_is_then_ignored() {
        let mut bus = CommandBus::default();
        let seq = bus.alloc_seq(CmdMeta::Query);
        bus.frame = TIMEOUT_FRAMES + 1;
        bus.drop_timed_out();
        assert!(bus.in_flight.is_empty());
        assert!(bus.resolve(seq, &ok(None)).is_none());
    }

    #[test]
    fn drop_timed_out_leaves_fresh_entries_alone() {
        let mut bus = CommandBus::default();
        let seq = bus.alloc_seq(CmdMeta::Query);
        bus.frame = TIMEOUT_FRAMES; // exactly at, not over, the budget
        bus.drop_timed_out();
        assert_eq!(bus.in_flight.len(), 1);
        assert!(bus.resolve(seq, &ok(None)).is_some());
    }
}
