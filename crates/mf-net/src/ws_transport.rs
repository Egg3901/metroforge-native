//! Legacy shim after embedded-sim cutover.
//!
//! `WsTransport` has been removed; only the shared liveness window constant
//! remains so embedded/reconnect code keeps one timeout source.

use std::time::Duration;

/// Silence threshold used by transport liveness checks.
pub const LIVENESS_WINDOW: Duration = Duration::from_millis(5000);
