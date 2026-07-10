use bevy_ecs::prelude::*;
use mf_protocol::Fields;

/// The most recent `Fields` binary frame (msgType=2: terrain/population/
/// jobs/landValue/water/parks), sent at init and every 7 sim-days. Array
/// lengths reuse `CurrentCity.static_city.{field_w,field_h}` — this resource
/// carries no grid dims of its own, mirroring the wire payload.
///
/// `mf-render`'s `terrain.rs` rebuilds ground geometry when
/// `LatestFields.0.as_ref().map(|f| f.version)` changes.
#[derive(Resource, Default)]
pub struct LatestFields(pub Option<Fields>);
