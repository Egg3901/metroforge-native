//! `mf-net` — sim transport and lifecycle glue (spec §3.2). The shipped backend
//! is the in-process Rust sim (`EmbeddedTransport`) drained into
//! `Events<FromSimMsg>` each frame by [`plugin::MfNetPlugin`].
//!
//! [`transport::SimTransport`] is the one seam that knows the sim is a
//! separate process — a future in-process (e.g. mobile) engine implements
//! the same trait with zero call-site changes elsewhere.

pub mod cities;
pub mod embedded;
pub mod host;
pub mod plugin;
pub mod reconnect;
pub mod sidecar;
pub mod transport;
pub mod ws_transport;

pub use embedded::EmbeddedTransport;
pub use plugin::{MfNetPlugin, NetSet, SimAlive, SimBackend, SimEvent, SimLink};
pub use reconnect::{
    FatalDiagnostics, NetStatus, ReconnectPhase, ReconnectState, ResumePolicy, MAX_ATTEMPTS,
};
pub use sidecar::{SidecarDeathReason, SidecarProcess};
pub use transport::SimTransport;
pub use ws_transport::LIVENESS_WINDOW;
