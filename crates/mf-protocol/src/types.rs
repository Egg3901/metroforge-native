//! Serde mirrors of `metroforge/src/core/types.ts` and
//! `metroforge/src/host/protocol.ts`. Field names use `camelCase` renames so
//! these types serialize/deserialize byte-for-byte compatible with the
//! sidecar's JSON, which is produced by `JSON.stringify` over the plain TS
//! objects.
//!
//! Numeric widths were chosen for plausibility (ids/ticks as unsigned
//! integers, money/ratios as `f64`) since JS `number` carries no width of its
//! own; JSON round-trips are unaffected by the Rust-side type as long as the
//! value fits.

use serde::{Deserialize, Serialize};

/// `TransitMode` — metroforge/src/core/types.ts:51
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransitMode {
    Bus,
    Tram,
    Metro,
    Rail,
}

/// `TrackGrade` — metroforge/src/core/types.ts:52
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackGrade {
    Surface,
    Elevated,
    Tunnel,
}

/// `Difficulty` — metroforge/src/core/types.ts:164
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Difficulty {
    Easy,
    Normal,
    Hard,
}

impl Difficulty {
    /// Player-facing label for menu combo boxes (avoids `Debug` formatting).
    pub fn label(self) -> &'static str {
        match self {
            Difficulty::Easy => "Easy",
            Difficulty::Normal => "Normal",
            Difficulty::Hard => "Hard",
        }
    }
}

/// The `size?` field on `init`/`ReplayPayload` — metroforge/src/host/protocol.ts:122.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CitySize {
    Small,
    Medium,
    Large,
}

/// `Vec2` — metroforge/src/core/geometry.ts:7-10
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

/// `ScenarioRules` — metroforge/src/core/scenarioRules.ts:7-24
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioRules {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario_id: Option<String>,
    pub starting_modes: Vec<TransitMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_modes: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_day: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_floor: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starting_cash: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_subsidy: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub era_label: Option<String>,
}

/// `MapLabel.kind` — metroforge/src/core/city/osmCity.ts:26-35
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MapLabelKind {
    Road,
    Water,
    Park,
}

/// `MapLabel` — metroforge/src/core/city/osmCity.ts:26-35
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapLabel {
    pub kind: MapLabelKind,
    pub name: String,
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<f64>,
    pub imp: f64,
}

/// One entry of `StaticCity.roads` — `{ cls: string; points: number[] }`.
/// `cls` is semantically a `RoadClass` (`arterial`|`collector`|`local`) but is
/// widened to `string` in the wire DTO (metroforge/src/host/protocol.ts:97),
/// so we mirror that widening rather than the stricter core type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoadDto {
    pub cls: String,
    /// flat x,y pairs
    pub points: Vec<f64>,
}

/// Native-wire `StaticCity`: identical to the TS `StaticCity`
/// (metroforge/src/host/protocol.ts:80-98) MINUS the three raw mask byte
/// arrays, which arrive separately as binary `StaticMask` frames (msgType=4)
/// per spec §1.2. `maskRes` + `has*Mask` flags tell the client which masks to
/// expect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticCityJson {
    pub field_w: u32,
    pub field_h: u32,
    pub cell_size: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    pub world_size: f64,
    pub road_scale: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask_res: Option<u32>,
    #[serde(default)]
    pub has_water_mask: bool,
    #[serde(default)]
    pub has_park_mask: bool,
    #[serde(default)]
    pub has_building_mask: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<MapLabel>>,
    pub roads: Vec<RoadDto>,
}

/// `UiStation` — metroforge/src/host/protocol.ts:8-17
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiStation {
    pub id: i64,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub mode: TransitMode,
    pub level: u32,
    pub ridership: f64,
    pub alightings: f64,
}

/// `UiTrack` — metroforge/src/host/protocol.ts:19-26. Note `grade` is typed
/// as plain `string` in the TS DTO (not the `TrackGrade` union), mirrored
/// here as `String` on purpose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiTrack {
    pub id: i64,
    pub mode: TransitMode,
    pub grade: String,
    /// flat x,y pairs
    pub points: Vec<f64>,
    pub from_station_id: i64,
    pub to_station_id: i64,
}

/// `UiRoute` — metroforge/src/host/protocol.ts:28-44
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiRoute {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub mode: TransitMode,
    pub station_ids: Vec<i64>,
    pub headway_seconds: f64,
    pub fare: f64,
    pub vehicle_count: u32,
    pub daily_ridership: f64,
    pub daily_revenue: f64,
    pub length_meters: f64,
    pub capacity: f64,
    pub load: f64,
    pub crowding: f64,
    pub segment_loads: Vec<f64>,
    /// Sim-depth (PR #31): live 0..1 crowding for this route on the current
    /// tick. `default` keeps old sidecars (which omit it) parseable.
    #[serde(default)]
    pub live_crowding: Option<f64>,
    /// Sim-depth (PR #31): this route's share of daily operating cost.
    #[serde(default)]
    pub operating_cost: Option<f64>,
    /// Sim-depth (PR #31): this route's daily farebox revenue.
    #[serde(default)]
    pub farebox: Option<f64>,
}

/// `DayLedger` — metroforge/src/core/types.ts:134-140
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayLedger {
    pub fares: f64,
    pub subsidy: f64,
    pub operations: f64,
    pub maintenance: f64,
    pub interest: f64,
}

/// One entry of `UiState.activeEvents`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveEventDto {
    pub id: String,
    pub name: String,
    pub days_left: u32,
}

/// `UiState.failed` — the literal union `'bankrupt'|'approval'|'time'|null`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailReason {
    Bankrupt,
    Approval,
    Time,
}

/// `UiDistrict` — sim-depth (PR #31). A catchment district with a centroid
/// and population/jobs, used to label a station's catchment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiDistrict {
    pub id: i64,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub population: f64,
    pub jobs: f64,
}

/// `UiState` — metroforge/src/host/protocol.ts:46-78. Sent at 2 Hz; the
/// native wire envelope for `t:"ui"` carries this struct directly as `p`
/// (spec §1.1: `ui {...UiState}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    pub tick: u64,
    pub insights: Vec<String>,
    pub day: u32,
    pub speed: f64,
    pub cash: f64,
    pub loan_balance: f64,
    pub last_day: DayLedger,
    /// rolling net/day (oldest -> newest), up to 7 entries
    pub net_history: Vec<f64>,
    pub population: f64,
    pub approval: f64,
    pub transit_share: f64,
    pub coverage: f64,
    pub daily_transit_trips: f64,
    pub unlocked_modes: Vec<TransitMode>,
    pub stations: Vec<UiStation>,
    pub tracks: Vec<UiTrack>,
    pub routes: Vec<UiRoute>,
    pub active_events: Vec<ActiveEventDto>,
    pub fields_version: u32,
    pub bankrupt: bool,
    #[serde(default)]
    pub failed: Option<FailReason>,
    #[serde(default)]
    pub max_day: Option<u32>,
    #[serde(default)]
    pub era_label: Option<String>,
    pub command_count: u32,
    /// Sim-depth (PR #31): hour of the simulated day in `0.0..24.0`. When
    /// present, the client prefers it over its tick-driven day/night clock.
    #[serde(default)]
    pub hour_of_day: Option<f64>,
    /// Sim-depth (PR #31): current demand multiplier (peak/off-peak).
    #[serde(default)]
    pub demand_factor: Option<f64>,
    /// Sim-depth (PR #31): farebox recovery ratio (fares / operating cost).
    #[serde(default)]
    pub farebox_recovery: Option<f64>,
    /// Sim-depth (PR #31): cumulative lifetime earnings across the game.
    #[serde(default)]
    pub lifetime: Option<f64>,
    /// Sim-depth (PR #31): catchment districts.
    #[serde(default)]
    pub districts: Vec<UiDistrict>,
    /// Sim-depth (PR #31): ids of routes flagged as overcrowded.
    #[serde(default)]
    pub overcrowded_routes: Vec<i64>,
}

impl UiState {
    /// Hour of the simulated day in `0.0..24.0`. Prefers the sidecar's
    /// sim-depth `hour_of_day` (PR #31) when present and finite; otherwise
    /// falls back to the tick-derived clock (`TICKS_PER_DAY = 1200`, mirroring
    /// `metroforge/src/core/constants.ts`), so old sidecars keep a sensible
    /// clock and the day/night rig stays consistent with the HUD.
    pub fn display_hour(&self) -> f64 {
        const TICKS_PER_DAY: u64 = 1200;
        match self.hour_of_day {
            Some(h) if h.is_finite() => h.rem_euclid(24.0),
            _ => (self.tick % TICKS_PER_DAY) as f64 / TICKS_PER_DAY as f64 * 24.0,
        }
    }
}

/// `DemandPayload.lines[]` — metroforge/src/host/protocol.ts:142-145
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DemandLine {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub weight: f64,
    pub share: f64,
}

/// `DemandPayload` — metroforge/src/host/protocol.ts:140-145
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemandPayload {
    pub lines: Vec<DemandLine>,
    pub max_weight: f64,
}

/// `Command` — metroforge/src/core/types.ts:212-223. Internally tagged on
/// `"kind"` exactly like the TS discriminated union; field names inside each
/// variant are also camelCased via `rename_all_fields`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Command {
    BuildStation {
        mode: TransitMode,
        pos: Vec2,
    },
    BuildTrack {
        mode: TransitMode,
        grade: TrackGrade,
        from_station_id: i64,
        to_station_id: i64,
        waypoints: Vec<Vec2>,
    },
    CreateRoute {
        mode: TransitMode,
        station_ids: Vec<i64>,
    },
    EditRoute {
        route_id: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        headway_seconds: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fare: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vehicle_count: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
    },
    DeleteRoute {
        route_id: i64,
    },
    DemolishStation {
        station_id: i64,
    },
    DemolishTrack {
        track_id: i64,
    },
    UpgradeStation {
        station_id: i64,
    },
    TakeLoan {
        amount: f64,
    },
    RepayLoan {
        amount: f64,
    },
    RenameStation {
        station_id: i64,
        name: String,
    },
}

/// `CommandResult` — metroforge/src/core/types.ts:225-230
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_id: Option<i64>,
}

/// One `ReplayPayload.commandLog[]` entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandLogEntry {
    pub tick: u64,
    pub cmd: Command,
}

/// `ReplayPayload` — metroforge/src/host/protocol.ts:160-170
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayPayload {
    pub seed: u64,
    pub difficulty: Difficulty,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<CitySize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<ScenarioRules>,
    pub command_log: Vec<CommandLogEntry>,
    pub final_tick: u64,
    pub state_hash: i64,
    pub score_hint: f64,
}

/// One entry of the sidecar `hello.cityList`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CityListEntry {
    pub key: String,
    pub label: String,
}

/// Sidecar -> client `hello` payload — spec §1.1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloInfo {
    pub protocol_version: u32,
    pub game_version: String,
    pub city_list: Vec<CityListEntry>,
    pub default_world_size: f64,
}

/// `tone` on the sidecar `toast` message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToastTone {
    Info,
    Warn,
    Good,
}
