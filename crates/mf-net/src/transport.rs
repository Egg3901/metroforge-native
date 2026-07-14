//! The `SimTransport` trait (spec §3.2): the one seam that knows the sim is
//! reachable over some channel. The cutover path uses an in-process worker
//! thread (`EmbeddedTransport`) and channels into ECS; a Bevy system drains
//! `try_recv` each frame.
//!
//! The same trait can later be satisfied by an in-process JS engine or a
//! native Rust sim on mobile (iOS forbids subprocesses) with zero call-site
//! changes — NOTHING outside `mf-net` may know the sim is a separate
//! process.

use std::time::Duration;

use mf_protocol::{FromSimMsg, ToSim};

pub trait SimTransport: Send + Sync {
    /// Non-blocking enqueue of an outbound message.
    fn send(&self, msg: ToSim) -> anyhow::Result<()>;
    /// Non-blocking drain of one inbound message, if any is queued.
    fn try_recv(&self) -> Option<FromSimMsg>;
    /// Whether the transport believes the sim is currently reachable
    /// (received *something* within the liveness window).
    fn is_alive(&self) -> bool;
    /// How long since the last inbound frame. `None` if nothing has been
    /// received yet (caller should treat connect-time as the baseline).
    fn silence_duration(&self) -> Duration;
}
