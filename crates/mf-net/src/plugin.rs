//! `MfNetPlugin` — owns the `SimLink` resource (the in-process sim transport),
//! drains inbound messages into `Events<SimEvent>` each frame, and tracks
//! `SimAlive` from transport liveness.

use std::time::{Duration, Instant};

use bevy_app::{App, Plugin, Update};
use bevy_ecs::prelude::*;
use mf_protocol::{FromSimMsg, ToSim};

use crate::embedded::EmbeddedTransport;
use crate::transport::SimTransport;

/// Client pings at half the transport liveness window so an idle-but-healthy
/// connection (e.g. sitting at `MainMenu` before `init`, where the sim has no
/// game running yet and so sends nothing) stays under the 5 s silence
/// threshold. Without this, an idle menu screen would spuriously look dead.
const PING_INTERVAL: Duration = Duration::from_millis(2500);

/// `mf-protocol` is deliberately Bevy-free, so `FromSimMsg` can't derive
/// `Event` there (and `mf-net` can't `impl Event for FromSimMsg` either —
/// neither type nor trait is local, so that's an orphan-rule violation).
/// This newtype is the one place that bridge happens: it's what actually
/// flows through `Events<T>`. Downstream crates read
/// `EventReader<mf_net::SimEvent>` and match on `.0`.
#[derive(Event, Debug, Clone)]
pub struct SimEvent(pub FromSimMsg);

/// Holds the live transport. Boxed as `dyn SimTransport` so a future
/// in-process/mobile implementation is a drop-in replacement.
#[derive(Resource)]
pub struct SimLink {
    pub transport: Box<dyn SimTransport>,
}

impl SimLink {
    /// Connect the in-process Rust sim ([`EmbeddedTransport`]). There is no
    /// socket to open and no child process, so this never fails.
    pub fn connect_embedded() -> Self {
        SimLink {
            transport: Box::new(EmbeddedTransport::connect()),
        }
    }
}

/// Whether the current `SimLink`'s transport reports the sim as reachable.
/// Updated every frame by `drain_inbound_system`.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub struct SimAlive(pub bool);

/// System ordering label so downstream crates (e.g. `mf-state`) can run
/// their own event-consuming systems strictly after events are pushed for
/// this frame.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetSet {
    Drain,
}

pub struct MfNetPlugin;

impl Plugin for MfNetPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<SimEvent>()
            .init_resource::<SimAlive>()
            .add_systems(
                Update,
                (
                    drain_inbound_system.in_set(NetSet::Drain),
                    ping_system.after(NetSet::Drain),
                ),
            );
    }
}

/// Sends `ToSim::Ping` on a wall-clock cadence. Uses `std::time::Instant`
/// via a system-local rather than Bevy's `Time` resource so `mf-net`
/// doesn't need a `bevy_time` dependency just for this.
fn ping_system(link: Option<Res<SimLink>>, mut last_ping: Local<Option<Instant>>) {
    let Some(link) = link else {
        return;
    };
    let now = Instant::now();
    let due = match *last_ping {
        None => true,
        Some(last) => now.duration_since(last) >= PING_INTERVAL,
    };
    if due {
        let _ = link.transport.send(ToSim::Ping);
        *last_ping = Some(now);
    }
}

fn drain_inbound_system(
    link: Option<Res<SimLink>>,
    mut alive: ResMut<SimAlive>,
    mut writer: EventWriter<SimEvent>,
) {
    let Some(link) = link else {
        alive.0 = false;
        return;
    };
    alive.0 = link.transport.is_alive();
    // Drain everything queued this frame; the wire is far slower than the
    // frame rate (spec §7.3: ~1.8 MB/s at 3000 vehicles) so an unbounded
    // drain never turns into a stall.
    while let Some(msg) = link.transport.try_recv() {
        writer.write(SimEvent(msg));
    }
}
