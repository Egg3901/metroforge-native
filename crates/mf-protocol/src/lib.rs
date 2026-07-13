//! `mf-protocol` — pure (no Bevy) mirror of the MetroForge sidecar wire
//! protocol described in `native-spec.md` §1. Two independent halves:
//!
//! - [`envelope`] / [`types`]: the JSON control channel (text frames).
//! - [`binary`]: the little-endian typed-array hot-path channel (binary
//!   frames: FrameSnapshot, Fields, Traffic, StaticMask, StaticBuildings).
//!
//! [`FromSimMsg`] unifies both into the single event stream `mf-net` forwards
//! into Bevy.
//!
//! Full field tables and binary layouts: `docs/PROTOCOL.md`.

#![warn(missing_docs)]

/// Binary hot-path frame codec (msgType 1–5).
pub mod binary;
/// JSON `{t, seq?, p?}` envelope and `ToSim` / `FromSimJson` enums.
pub mod envelope;
/// Serde mirrors of sidecar JSON DTOs (`Command`, `UiState`, …).
pub mod types;

pub use binary::{
    decode_binary, BinaryError, BinaryMsg, BuildingFootprint, Fields, FrameSnapshot, MaskWhich,
    StaticBuildings, StaticElevation, StaticMask, Traffic, TrafficHotspot,
};
pub use envelope::{
    ClientHelloPayload, CommandPayload, CommandResultPayload, Envelope, EnvelopeError, FromSimJson,
    InitPayload, LoadSavePayload, QueryTrackCostPayload, ReadyPayload, SavedPayload,
    SetSpeedPayload, StrataBandDto, StrataProbePayload, StrataProbeResultPayload, ToSim,
    ToastPayload, TrackCostBreakdown, TrackCostPayload,
};
pub use types::{
    ActiveEventDto, CityListEntry, CityMapPreview, CitySize, Command, CommandLogEntry,
    CommandResult, DayLedger, DemandLine, DemandPayload, Difficulty, FailReason, HelloInfo,
    MapLabel, MapLabelKind, PoiAnchorDto, PoiAnchorKind, ReplayPayload, RoadDto, ScenarioRules,
    Season, StaticCityJson, ToastTone, TrackGrade, TransitMode, UiDepot, UiDistrict,
    UiFleetSummary, UiIncident, UiRoute, UiState, UiStation, UiTrack, Vec2, WeatherEvent,
    WeatherState,
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
    /// JSON control-channel message (`FromSimJson`).
    Json(FromSimJson),
    /// msgType=1 frame snapshot (~20 Hz); held in [`Arc`] for cheap retain.
    Frame(Arc<FrameSnapshot>),
    /// msgType=2 fields grid; held in [`Arc`] for cheap retain.
    Fields(Arc<Fields>),
    /// msgType=3 traffic overlay.
    Traffic(Traffic),
    /// msgType=4 static mask (water/park/building).
    Mask(StaticMask),
    /// msgType=5, sent once. Additive/optional (see `BuildingFootprint`
    /// doc): does NOT bump `PROTOCOL_VERSION`.
    Buildings(StaticBuildings),
    /// msgType=7 static real-elevation heightfield, sent once. Additive/
    /// optional like msgType=5: does NOT bump `PROTOCOL_VERSION`.
    Elevation(Arc<StaticElevation>),
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
            BinaryMsg::Elevation(e) => FromSimMsg::Elevation(Arc::new(e)),
        }
    }
}

/// JSON handshake / sidecar stdout handshake protocol version (currently `1`).
///
/// Bumped only for breaking wire changes; additive optional fields and
/// optional msgType=5 do not bump this. See `docs/PROTOCOL.md` §5.
pub const PROTOCOL_VERSION: u32 = 1;
