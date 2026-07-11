//! JSON control-channel envelope (spec §1.1): every text frame is
//! `{ "t": "<type>", "seq": <u32?>, "p": { ...payload } }`. `seq` only
//! appears on request/response-correlated messages (it carries the
//! `requestId`); `p` is omitted entirely for payloadless messages.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{
    CommandResult, DemandPayload, HelloInfo, ReplayPayload, StaticCityJson, ToastTone, UiState,
};
use crate::Command;
use crate::{CitySize, Difficulty, ScenarioRules, TrackGrade, TransitMode, Vec2};

/// Raw wire envelope, as literally deserialized off a text frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Message type string (`"hello"`, `"command"`, …).
    pub t: String,
    /// Optional request/response correlation id (`requestId`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u32>,
    /// Optional JSON payload object; omitted for payloadless messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p: Option<Value>,
}

/// Errors from parsing an [`Envelope`] into a typed [`FromSimJson`].
#[derive(Debug, thiserror::Error)]
pub enum EnvelopeError {
    /// `t` was not a known sidecar→client message type.
    #[error("unknown message type {0:?}")]
    UnknownType(String),
    /// A message that requires `p` arrived without one.
    #[error("message {0:?} is missing its payload")]
    MissingPayload(&'static str),
    /// `p` failed to deserialize into the expected payload type.
    #[error("failed to decode payload for {0:?}: {1}")]
    BadPayload(&'static str, serde_json::Error),
}

// ---- payload structs with no dedicated home in types.rs -------------------

/// Client → sidecar `hello` payload: advertises the client's protocol version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientHelloPayload {
    /// Client's supported [`crate::PROTOCOL_VERSION`].
    pub client_protocol_version: u32,
}

/// Client → sidecar `init` payload: start a new game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitPayload {
    /// RNG seed for city generation / sim.
    pub seed: u64,
    /// Difficulty preset.
    pub difficulty: Difficulty,
    /// Optional procedural city size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<CitySize>,
    /// Optional OSM / preset city key from `hello.cityList`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_key: Option<String>,
    /// Optional scenario rule overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<ScenarioRules>,
}

/// Client → sidecar `loadSave` payload: resume from a serialized save blob.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadSavePayload {
    /// Opaque save JSON string produced by a prior `saved` message.
    pub json: String,
}

/// Client → sidecar `setSpeed` payload.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SetSpeedPayload {
    /// Simulation speed multiplier (0 = paused).
    pub speed: f64,
}

/// Client → sidecar `command` payload wrapper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandPayload {
    /// The player command to apply.
    pub cmd: Command,
}

/// Client → sidecar `queryTrackCost` payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryTrackCostPayload {
    /// Transit mode for the proposed track.
    pub mode: TransitMode,
    /// Grade (surface / elevated / tunnel).
    pub grade: TrackGrade,
    /// Polyline waypoints in world space.
    pub points: Vec<Vec2>,
}

/// Sidecar → client `ready` payload: static city geometry after init/load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyPayload {
    /// Static city DTO (masks arrive separately as binary frames).
    pub static_city: StaticCityJson,
}

/// Sidecar → client `trackCost` payload.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrackCostPayload {
    /// Estimated construction cost for the queried track.
    pub cost: f64,
}

/// Sidecar → client `saved` payload: serialized game state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedPayload {
    /// Opaque save JSON string for later `loadSave`.
    pub json: String,
}

/// Sidecar → client `toast` payload: transient UI notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToastPayload {
    /// Human-readable toast text.
    pub message: String,
    /// Visual tone (`info` / `warn` / `good`).
    pub tone: ToastTone,
}

/// Sidecar → client `commandResult` payload wrapper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandResultPayload {
    /// Outcome of the correlated `command` request.
    pub result: CommandResult,
}

// ---- Client -> sidecar -----------------------------------------------------

/// Every message the client can send. Mirrors spec §1.1 "Client -> sidecar".
#[derive(Debug, Clone, PartialEq)]
pub enum ToSim {
    /// `t:"hello"` — protocol handshake from the client.
    Hello(ClientHelloPayload),
    /// `t:"init"` — start a new game with the given seed/rules.
    Init(InitPayload),
    /// `t:"loadSave"` — resume from a save blob.
    LoadSave(LoadSavePayload),
    /// `t:"requestSave"` — ask the sidecar to emit a `saved` message.
    RequestSave,
    /// `t:"setSpeed"` — change simulation speed.
    SetSpeed(SetSpeedPayload),
    /// `t:"command"` — apply a player command.
    /// `seq` carries the client-assigned `requestId`.
    Command {
        /// Client-assigned request id echoed in `commandResult.seq`.
        seq: u32,
        /// The command to apply.
        cmd: Command,
    },
    /// `t:"queryTrackCost"` — ask for a construction cost estimate.
    /// `seq` carries the client-assigned `requestId`.
    QueryTrackCost {
        /// Client-assigned request id echoed in `trackCost.seq`.
        seq: u32,
        /// Track geometry and mode/grade for the cost query.
        payload: QueryTrackCostPayload,
    },
    /// `t:"requestReplay"` — ask the sidecar to emit a `replay` message.
    RequestReplay,
    /// `t:"ping"` — keepalive; expects `pong`.
    Ping,
    /// `t:"shutdown"` — ask the sidecar to exit.
    Shutdown,
}

impl ToSim {
    /// Serialize this message into a wire [`Envelope`] ready for a text frame.
    pub fn to_envelope(&self) -> Envelope {
        match self {
            ToSim::Hello(p) => envelope("hello", None, Some(p)),
            ToSim::Init(p) => envelope("init", None, Some(p)),
            ToSim::LoadSave(p) => envelope("loadSave", None, Some(p)),
            ToSim::RequestSave => envelope_no_payload("requestSave", None),
            ToSim::SetSpeed(p) => envelope("setSpeed", None, Some(p)),
            ToSim::Command { seq, cmd } => envelope(
                "command",
                Some(*seq),
                Some(&CommandPayload { cmd: cmd.clone() }),
            ),
            ToSim::QueryTrackCost { seq, payload } => {
                envelope("queryTrackCost", Some(*seq), Some(payload))
            }
            ToSim::RequestReplay => envelope_no_payload("requestReplay", None),
            ToSim::Ping => envelope_no_payload("ping", None),
            ToSim::Shutdown => envelope_no_payload("shutdown", None),
        }
    }
}

// ---- Sidecar -> client ------------------------------------------------------

/// Every JSON (non-binary) message the sidecar can send. Mirrors spec §1.1
/// "Sidecar -> client" minus `fields`/`traffic`/`frame`/the static masks,
/// which are binary (see `crate::binary`).
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum FromSimJson {
    /// `t:"hello"` — sidecar capabilities and city list.
    Hello(HelloInfo),
    /// `t:"ready"` — static city after init/load; binary masks may follow.
    Ready(ReadyPayload),
    /// `t:"demand"` — demand-flow overlay lines.
    Demand(DemandPayload),
    /// The envelope's `p` IS the `UiState` (spec: `ui {...UiState}`).
    Ui(UiState),
    /// `t:"commandResult"` — outcome of a prior `command` (correlated by `seq`).
    CommandResult {
        /// Echo of the client's `requestId`, if present.
        seq: Option<u32>,
        /// Success/failure and optional created entity id.
        result: CommandResult,
    },
    /// `t:"trackCost"` — cost estimate for a prior `queryTrackCost`.
    TrackCost {
        /// Echo of the client's `requestId`, if present.
        seq: Option<u32>,
        /// Estimated construction cost.
        cost: f64,
    },
    /// `t:"saved"` — serialized save blob in response to `requestSave`.
    Saved(SavedPayload),
    /// The envelope's `p` IS the `ReplayPayload` (spec: `replay {...ReplayPayload}`).
    Replay(ReplayPayload),
    /// `t:"toast"` — transient UI notification.
    Toast(ToastPayload),
    /// `t:"pong"` — reply to `ping`.
    Pong,
    /// `t:"bye"` — sidecar is shutting down.
    Bye,
}

impl FromSimJson {
    /// Parse a raw wire [`Envelope`] into a typed sidecar→client message.
    pub fn from_envelope(env: Envelope) -> Result<Self, EnvelopeError> {
        let Envelope { t, seq, p } = env;
        let need = |name: &'static str| p.clone().ok_or(EnvelopeError::MissingPayload(name));
        fn parse<T: serde::de::DeserializeOwned>(
            name: &'static str,
            v: Value,
        ) -> Result<T, EnvelopeError> {
            serde_json::from_value(v).map_err(|e| EnvelopeError::BadPayload(name, e))
        }
        match t.as_str() {
            "hello" => Ok(FromSimJson::Hello(parse("hello", need("hello")?)?)),
            "ready" => Ok(FromSimJson::Ready(parse("ready", need("ready")?)?)),
            "demand" => Ok(FromSimJson::Demand(parse("demand", need("demand")?)?)),
            "ui" => Ok(FromSimJson::Ui(parse("ui", need("ui")?)?)),
            "commandResult" => {
                let payload: CommandResultPayload = parse("commandResult", need("commandResult")?)?;
                Ok(FromSimJson::CommandResult {
                    seq,
                    result: payload.result,
                })
            }
            "trackCost" => {
                let payload: TrackCostPayload = parse("trackCost", need("trackCost")?)?;
                Ok(FromSimJson::TrackCost {
                    seq,
                    cost: payload.cost,
                })
            }
            "saved" => Ok(FromSimJson::Saved(parse("saved", need("saved")?)?)),
            "replay" => Ok(FromSimJson::Replay(parse("replay", need("replay")?)?)),
            "toast" => Ok(FromSimJson::Toast(parse("toast", need("toast")?)?)),
            "pong" => Ok(FromSimJson::Pong),
            "bye" => Ok(FromSimJson::Bye),
            other => Err(EnvelopeError::UnknownType(other.to_string())),
        }
    }
}

fn envelope<T: Serialize>(t: &str, seq: Option<u32>, payload: Option<&T>) -> Envelope {
    Envelope {
        t: t.to_string(),
        seq,
        p: payload.map(|p| serde_json::to_value(p).expect("payload always serializable")),
    }
}

fn envelope_no_payload(t: &str, seq: Option<u32>) -> Envelope {
    Envelope {
        t: t.to_string(),
        seq,
        p: None,
    }
}
