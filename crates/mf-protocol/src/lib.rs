//! `mf-protocol` — pure (no Bevy) mirror of the MetroForge sidecar wire
//! protocol described in `native-spec.md` §1. Two independent halves:
//!
//! - [`envelope`] / [`types`]: the JSON control channel (text frames).
//! - [`binary`]: the little-endian typed-array hot-path channel (binary
//!   frames: FrameSnapshot, Fields, Traffic, StaticMask).
//!
//! [`FromSimMsg`] unifies both into the single event stream `mf-net` forwards
//! into Bevy.

pub mod binary;
pub mod envelope;
pub mod types;

pub use binary::{
    decode_binary, BinaryError, BinaryMsg, Fields, FrameSnapshot, MaskWhich, StaticMask, Traffic,
    TrafficHotspot,
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
    UiRoute, UiState, UiStation, UiTrack, Vec2,
};

/// Unified inbound event stream from the sim, merging the JSON control
/// channel and the binary hot-path channel into one type so `mf-net` can
/// funnel everything through a single `Events<FromSimMsg>` in Bevy.
#[derive(Debug, Clone, PartialEq)]
pub enum FromSimMsg {
    Json(FromSimJson),
    Frame(FrameSnapshot),
    Fields(Fields),
    Traffic(Traffic),
    Mask(StaticMask),
}

impl From<FromSimJson> for FromSimMsg {
    fn from(v: FromSimJson) -> Self {
        FromSimMsg::Json(v)
    }
}

impl From<BinaryMsg> for FromSimMsg {
    fn from(v: BinaryMsg) -> Self {
        match v {
            BinaryMsg::Frame(f) => FromSimMsg::Frame(f),
            BinaryMsg::Fields(f) => FromSimMsg::Fields(f),
            BinaryMsg::Traffic(t) => FromSimMsg::Traffic(t),
            BinaryMsg::Mask(m) => FromSimMsg::Mask(m),
        }
    }
}

pub const PROTOCOL_VERSION: u32 = 1;
