use std::sync::Arc;

use bevy_ecs::prelude::*;
use mf_protocol::FrameSnapshot;

/// The most recent `FrameSnapshot` binary frame (msgType=1: tick + vehicles
/// stride-6 + agents stride-3 + color table), sent every 50 ms simulation
/// step. `mf-render`'s `vehicles.rs`/`agents.rs` read this every render
/// frame to update transform pools; nothing here is retained across frames
/// beyond "latest" — there is no interpolation buffer in v1.
///
/// Held behind [`Arc`] so applying a new frame from `Events<SimEvent>` is a
/// refcount bump rather than a deep clone of the vehicle/agent arrays
/// (~20 Hz hot path).
#[derive(Resource, Default)]
pub struct LatestFrame(pub Option<Arc<FrameSnapshot>>);
