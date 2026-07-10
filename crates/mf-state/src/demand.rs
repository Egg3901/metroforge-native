use bevy_ecs::prelude::*;
use mf_protocol::DemandPayload;

/// The most recent unserved-demand `DemandPayload` pushed by the sidecar:
/// desire-line OD pairs (`DemandLine`s) that are being driven rather than
/// served by transit, plus `max_weight` for normalizing line weight into a
/// 0..1 color ramp. Mirrors [`crate::ui::LatestUi`]'s pattern exactly (a
/// bare `Option` overwritten wholesale on every message, no diffing).
///
/// Cadence, per the sidecar (`metroforge/sidecar/simHost.ts::sendDemand`):
/// NOT a fixed real-time tick. It's resent every time `GameState.flows`
/// changes identity, which happens in lockstep with `sendTraffic` inside
/// `simHost.ts::step()` — itself driven by the core sim's
/// `ASSIGNMENT_INTERVAL_TICKS` (300 simulated ticks, i.e. 1/4 of a
/// `TICKS_PER_DAY` = 1200 game-day) or sooner if `state.demandDirty` was
/// set (a station/track/route edit invalidates the current assignment).
/// So: roughly 4x/simulated-day, but real-time-irregular since it scales
/// with the player's chosen game speed and can also fire early off an edit.
#[derive(Resource, Default)]
pub struct LatestDemand(pub Option<DemandPayload>);
