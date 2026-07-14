//! `mf-net` — sim transport and lifecycle glue (spec §3.2). The shipped backend
//! is the in-process Rust sim (`EmbeddedTransport`) drained into
//! `Events<FromSimMsg>` each frame by [`plugin::MfNetPlugin`].
//!
//! [`transport::SimTransport`] is the one seam that boxes the sim behind a
//! trait object — a future in-process (e.g. mobile) engine implements the same
//! trait with zero call-site changes elsewhere.

pub mod cities;
pub mod embedded;
pub mod host;
pub mod plugin;
pub mod transport;

pub use embedded::EmbeddedTransport;
pub use plugin::{MfNetPlugin, NetSet, SimAlive, SimEvent, SimLink};
pub use transport::SimTransport;
