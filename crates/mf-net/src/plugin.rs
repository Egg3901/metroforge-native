//! `MfNetPlugin` — owns the `SimLink` resource (transport + optional sidecar
//! handle), drains inbound messages into `Events<FromSimMsg>` each frame, and
//! tracks `SimAlive`/`NetStatus` for `reconnect.rs` to act on.

use std::time::{Duration, Instant};

use bevy_app::{App, Plugin, Update};
use bevy_ecs::prelude::*;
use mf_protocol::{FromSimMsg, ToSim};

use crate::embedded::EmbeddedTransport;
use crate::reconnect::{reconnect_system, ReconnectState};
use crate::sidecar::SidecarProcess;
use crate::transport::SimTransport;
use crate::ws_transport::WsTransport;

/// Which sim backend `SimLink` boxes behind the `dyn SimTransport` seam.
///
/// Selected by the `MF_SIM` environment variable (`embedded` | `sidecar`).
/// Default is [`SimBackend::Sidecar`] (the shipping Bun sidecar) — the
/// in-process Rust sim ([`SimBackend::Embedded`], `MF_SIM=embedded`) is P4's
/// opt-in path and does not yet own OSM real cities, scenarios, or saves. See
/// `crates/mf-sim/PORT.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimBackend {
    /// The out-of-process Bun sidecar over a WebSocket ([`WsTransport`]).
    Sidecar,
    /// The in-process native Rust sim ([`EmbeddedTransport`]).
    Embedded,
}

impl SimBackend {
    /// Resolve the backend from `MF_SIM` (case-insensitive). Anything other than
    /// `embedded` — including unset — is [`SimBackend::Sidecar`] (safe default).
    pub fn from_env() -> Self {
        match std::env::var("MF_SIM").ok().as_deref().map(str::trim) {
            Some(v) if v.eq_ignore_ascii_case("embedded") => SimBackend::Embedded,
            _ => SimBackend::Sidecar,
        }
    }
}

/// Client pings at half the websocket liveness window so an idle-but-healthy
/// connection (e.g. sitting at `MainMenu` before `init`, where the sidecar
/// has no game running yet and so sends nothing) stays under the 5 s silence
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

/// Holds the live transport (and, on desktop, the child sidecar process it
/// owns). Boxed as `dyn SimTransport` so a future in-process/mobile
/// implementation is a drop-in replacement.
#[derive(Resource)]
pub struct SimLink {
    pub transport: Box<dyn SimTransport>,
    /// `None` once the sidecar was launched externally (e.g. a future
    /// in-process engine) or via a pre-existing `$MF_SIDECAR_PATH` process
    /// this crate doesn't own the lifecycle of.
    pub sidecar: Option<SidecarProcess>,
}

impl SimLink {
    /// Convenience used by Boot: spawn the sidecar (per the lookup order in
    /// `sidecar.rs`) and connect a `WsTransport` to it.
    pub fn spawn_and_connect(headless_speed: Option<f64>) -> anyhow::Result<Self> {
        let sidecar = SidecarProcess::spawn(headless_speed)?;
        let transport = WsTransport::connect(&sidecar.ws_url())?;
        Ok(SimLink {
            transport: Box::new(transport),
            sidecar: Some(sidecar),
        })
    }

    /// Connect the in-process Rust sim ([`EmbeddedTransport`]). No child
    /// process, so `sidecar` is `None` and there is nothing for `reconnect.rs`
    /// to respawn (an in-process sim cannot die independently). Infallible.
    pub fn connect_embedded() -> Self {
        SimLink {
            transport: Box::new(EmbeddedTransport::connect()),
            sidecar: None,
        }
    }

    /// Boot entry point that honors the `MF_SIM` flag: [`SimBackend::Embedded`]
    /// connects the in-process Rust sim; [`SimBackend::Sidecar`] (default)
    /// spawns and connects the Bun sidecar.
    pub fn connect_for_backend(
        backend: SimBackend,
        headless_speed: Option<f64>,
    ) -> anyhow::Result<Self> {
        match backend {
            SimBackend::Embedded => Ok(Self::connect_embedded()),
            SimBackend::Sidecar => Self::spawn_and_connect(headless_speed),
        }
    }

    /// Test/harness helper: force-kill the owned sidecar process (if any)
    /// without dropping the transport first. Used by `MF_TEST_KILL_SIDECAR`.
    pub fn kill_sidecar_for_test(&mut self) {
        if let Some(sidecar) = self.sidecar.as_mut() {
            sidecar.kill_now();
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
            .init_resource::<ReconnectState>()
            .add_systems(
                Update,
                (
                    drain_inbound_system.in_set(NetSet::Drain),
                    ping_system.after(NetSet::Drain),
                    reconnect_system.after(NetSet::Drain),
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
