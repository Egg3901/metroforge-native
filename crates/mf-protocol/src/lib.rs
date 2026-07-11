//! `mf-protocol` — pure (no Bevy) mirror of the MetroForge sidecar wire
//! protocol described in `native-spec.md` §1. Two independent halves:
//!
//! - [`envelope`] / [`types`]: the JSON control channel (text frames).
//! - [`binary`]: the little-endian typed-array hot-path channel (binary
//!   frames: FrameSnapshot, Fields, Traffic, StaticMask, StaticBuildings).
//!
//! [`FromSimMsg`] unifies both into the single event stream `mf-net` forwards
//! into Bevy.

pub mod binary;
pub mod envelope;
pub mod types;

pub use binary::{
    decode_binary, BinaryError, BinaryMsg, BuildingFootprint, Fields, FrameSnapshot, MaskWhich,
    StaticBuildings, StaticMask, Traffic, TrafficHotspot,
};
pub use envelope::{
    ClientHelloPayload, CommandPayload, CommandResultPayload, Envelope, EnvelopeError, FromSimJson,
    InitPayload, LoadSavePayload, QueryTrackCostPayload, ReadyPayload, SavedPayload,
    SetSpeedPayload, ToSim, ToastPayload, TrackCostPayload,
};
pub use types::{
    ActiveEventDto, CityListEntry, CitySize, Command, CommandLogEntry, CommandResult, DayLedger,
    DemandLine, DemandPayload, Difficulty, FailReason, HelloInfo, MapLabel, MapLabelKind,
    ReplayPayload, RoadDto, ScenarioRules, StaticCityJson, ToastTone, TrackGrade, TransitMode,
    UiDistrict, UiRoute, UiState, UiStation, UiTrack, Vec2,
};

use std::sync::Arc;

/// Unified inbound event stream from the sim, merging the JSON control
/// channel and the binary hot-path channel into one type so `mf-net` can
/// funnel everything through a single `Events<FromSimMsg>` in Bevy.
///
/// `Frame` and `Fields` are wrapped in [`Arc`] so `mf-state` can retain
/// "latest" without deep-cloning the decoded arrays on every tick (Frame
/// ~20 Hz) or fields bump (~every 7 sim-days). `EventReader` only yields
/// references, so without Arc the apply path had to `.clone()` the whole
/// payload; Arc clone is a refcount bump.
///
/// `Json` remains the largest stack variant (control-plane payloads); Arc
/// already covers the hot binary paths. Boxing Json would churn the rare
/// control messages for little gain.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum FromSimMsg {
    Json(FromSimJson),
    Frame(Arc<FrameSnapshot>),
    Fields(Arc<Fields>),
    Traffic(Traffic),
    Mask(StaticMask),
    /// msgType=5, sent once. Additive/optional (see `BuildingFootprint`
    /// doc): does NOT bump `PROTOCOL_VERSION`.
    Buildings(StaticBuildings),
}

impl From<FromSimJson> for FromSimMsg {
    fn from(v: FromSimJson) -> Self {
        FromSimMsg::Json(v)
    }
}

impl From<BinaryMsg> for FromSimMsg {
    fn from(v: BinaryMsg) -> Self {
        match v {
            BinaryMsg::Frame(f) => FromSimMsg::Frame(Arc::new(f)),
            BinaryMsg::Fields(f) => FromSimMsg::Fields(Arc::new(f)),
            BinaryMsg::Traffic(t) => FromSimMsg::Traffic(t),
            BinaryMsg::Mask(m) => FromSimMsg::Mask(m),
            BinaryMsg::Buildings(b) => FromSimMsg::Buildings(b),
        }
    }
}

pub const PROTOCOL_VERSION: u32 = 1;
