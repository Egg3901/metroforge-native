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
    pub t: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p: Option<Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum EnvelopeError {
    #[error("unknown message type {0:?}")]
    UnknownType(String),
    #[error("message {0:?} is missing its payload")]
    MissingPayload(&'static str),
    #[error("failed to decode payload for {0:?}: {1}")]
    BadPayload(&'static str, serde_json::Error),
}

// ---- payload structs with no dedicated home in types.rs -------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientHelloPayload {
    pub client_protocol_version: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitPayload {
    pub seed: u64,
    pub difficulty: Difficulty,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<CitySize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<ScenarioRules>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadSavePayload {
    pub json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SetSpeedPayload {
    pub speed: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandPayload {
    pub cmd: Command,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryTrackCostPayload {
    pub mode: TransitMode,
    pub grade: TrackGrade,
    pub points: Vec<Vec2>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyPayload {
    pub static_city: StaticCityJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrackCostPayload {
    pub cost: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedPayload {
    pub json: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToastPayload {
    pub message: String,
    pub tone: ToastTone,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandResultPayload {
    pub result: CommandResult,
}

// ---- Client -> sidecar -----------------------------------------------------

/// Every message the client can send. Mirrors spec §1.1 "Client -> sidecar".
#[derive(Debug, Clone, PartialEq)]
pub enum ToSim {
    Hello(ClientHelloPayload),
    Init(InitPayload),
    LoadSave(LoadSavePayload),
    RequestSave,
    SetSpeed(SetSpeedPayload),
    /// `seq` carries the client-assigned `requestId`.
    Command {
        seq: u32,
        cmd: Command,
    },
    /// `seq` carries the client-assigned `requestId`.
    QueryTrackCost {
        seq: u32,
        payload: QueryTrackCostPayload,
    },
    RequestReplay,
    Ping,
    Shutdown,
}

impl ToSim {
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
    Hello(HelloInfo),
    Ready(ReadyPayload),
    Demand(DemandPayload),
    /// The envelope's `p` IS the `UiState` (spec: `ui {...UiState}`).
    Ui(UiState),
    CommandResult {
        seq: Option<u32>,
        result: CommandResult,
    },
    TrackCost {
        seq: Option<u32>,
        cost: f64,
    },
    Saved(SavedPayload),
    /// The envelope's `p` IS the `ReplayPayload` (spec: `replay {...ReplayPayload}`).
    Replay(ReplayPayload),
    Toast(ToastPayload),
    Pong,
    Bye,
}

impl FromSimJson {
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
