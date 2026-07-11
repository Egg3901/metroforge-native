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
    /// Bus mode.
    Bus,
    /// Tram / light-rail mode.
    Tram,
    /// Metro / subway mode.
    Metro,
    /// Heavy rail mode.
    Rail,
}

/// `TrackGrade` — metroforge/src/core/types.ts:52
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackGrade {
    /// At-grade / surface track.
    Surface,
    /// Elevated track.
    Elevated,
    /// Underground tunnel.
    Tunnel,
}

/// `Difficulty` — metroforge/src/core/types.ts:164
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Difficulty {
    /// Easy difficulty.
    Easy,
    /// Normal difficulty.
    Normal,
    /// Hard difficulty.
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
    /// Small procedural city.
    Small,
    /// Medium procedural city.
    Medium,
    /// Large procedural city.
    Large,
}

/// `Vec2` — metroforge/src/core/geometry.ts:7-10
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    /// World X coordinate.
    pub x: f64,
    /// World Y coordinate.
    pub y: f64,
}

/// `ScenarioRules` — metroforge/src/core/scenarioRules.ts:7-24
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioRules {
    /// Optional scenario identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario_id: Option<String>,
    /// Transit modes available at game start.
    pub starting_modes: Vec<TransitMode>,
    /// When true, modes beyond `starting_modes` stay locked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_modes: Option<bool>,
    /// Optional day limit for timed scenarios.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_day: Option<u32>,
    /// Minimum approval before failure (0..1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_floor: Option<f64>,
    /// Starting cash override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starting_cash: Option<f64>,
    /// Daily subsidy override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_subsidy: Option<f64>,
    /// Optional era label shown in the HUD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub era_label: Option<String>,
}

/// `MapLabel.kind` — metroforge/src/core/city/osmCity.ts:26-35
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MapLabelKind {
    /// Road name label.
    Road,
    /// Water body name label.
    Water,
    /// Park name label.
    Park,
}

/// `MapLabel` — metroforge/src/core/city/osmCity.ts:26-35
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapLabel {
    /// Label category (road / water / park).
    pub kind: MapLabelKind,
    /// Display name.
    pub name: String,
    /// World X of the label anchor.
    pub x: f64,
    /// World Y of the label anchor.
    pub y: f64,
    /// Optional rotation in radians.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<f64>,
    /// Importance / priority for LOD culling (higher = more important).
    pub imp: f64,
}

/// One entry of `StaticCity.roads` — `{ cls: string; points: number[] }`.
/// `cls` is semantically a `RoadClass` (`arterial`|`collector`|`local`) but is
/// widened to `string` in the wire DTO (metroforge/src/host/protocol.ts:97),
/// so we mirror that widening rather than the stricter core type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoadDto {
    /// Road class string (`arterial` / `collector` / `local`, …).
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
    /// Fields grid width in cells.
    pub field_w: u32,
    /// Fields grid height in cells.
    pub field_h: u32,
    /// World meters per fields cell.
    pub cell_size: f64,
    /// World X of the fields/city origin.
    pub origin_x: f64,
    /// World Y of the fields/city origin.
    pub origin_y: f64,
    /// World extent (square side length in meters).
    pub world_size: f64,
    /// Road geometry scale factor.
    pub road_scale: f64,
    /// Side length of static masks when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask_res: Option<u32>,
    /// Whether a water `StaticMask` frame will follow `ready`.
    #[serde(default)]
    pub has_water_mask: bool,
    /// Whether a park `StaticMask` frame will follow `ready`.
    #[serde(default)]
    pub has_park_mask: bool,
    /// Whether a building `StaticMask` frame will follow `ready`.
    #[serde(default)]
    pub has_building_mask: bool,
    /// Optional map labels (roads, water, parks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<MapLabel>>,
    /// Road polylines for the static map layer.
    pub roads: Vec<RoadDto>,
}

/// `UiStation` — metroforge/src/host/protocol.ts:8-17
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiStation {
    /// Station entity id.
    pub id: i64,
    /// Display name.
    pub name: String,
    /// World X.
    pub x: f64,
    /// World Y.
    pub y: f64,
    /// Transit mode of this station.
    pub mode: TransitMode,
    /// Station upgrade level.
    pub level: u32,
    /// Boardings / ridership metric for the current period.
    pub ridership: f64,
    /// Alightings metric for the current period.
    pub alightings: f64,
}

/// `UiTrack` — metroforge/src/host/protocol.ts:19-26. Note `grade` is typed
/// as plain `string` in the TS DTO (not the `TrackGrade` union), mirrored
/// here as `String` on purpose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiTrack {
    /// Track entity id.
    pub id: i64,
    /// Transit mode of this track.
    pub mode: TransitMode,
    /// Grade string (`surface` / `elevated` / `tunnel`, …).
    pub grade: String,
    /// flat x,y pairs
    pub points: Vec<f64>,
    /// Endpoint station id (from).
    pub from_station_id: i64,
    /// Endpoint station id (to).
    pub to_station_id: i64,
}

/// `UiRoute` — metroforge/src/host/protocol.ts:28-44
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiRoute {
    /// Route entity id.
    pub id: i64,
    /// Display name.
    pub name: String,
    /// Route color (CSS / hex string).
    pub color: String,
    /// Transit mode of this route.
    pub mode: TransitMode,
    /// Ordered station ids on the route.
    pub station_ids: Vec<i64>,
    /// Scheduled headway in seconds.
    pub headway_seconds: f64,
    /// Fare charged per trip.
    pub fare: f64,
    /// Number of vehicles assigned.
    pub vehicle_count: u32,
    /// Daily ridership total.
    pub daily_ridership: f64,
    /// Daily revenue total.
    pub daily_revenue: f64,
    /// Route length in meters.
    pub length_meters: f64,
    /// Vehicle capacity (passengers).
    pub capacity: f64,
    /// Current load (passengers).
    pub load: f64,
    /// Crowding ratio (load / capacity).
    pub crowding: f64,
    /// Per-segment load values along the route.
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
    /// Fare revenue for the day.
    pub fares: f64,
    /// Subsidy income for the day.
    pub subsidy: f64,
    /// Operating costs for the day.
    pub operations: f64,
    /// Maintenance costs for the day.
    pub maintenance: f64,
    /// Loan interest for the day.
    pub interest: f64,
}

/// One entry of `UiState.activeEvents`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveEventDto {
    /// Event id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Simulated days remaining until the event ends.
    pub days_left: u32,
}

/// `UiState.failed` — the literal union `'bankrupt'|'approval'|'time'|null`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailReason {
    /// Player went bankrupt.
    Bankrupt,
    /// Approval fell below the floor.
    Approval,
    /// Timed scenario ran out of days.
    Time,
}

/// `UiDistrict` — sim-depth (PR #31). A catchment district with a centroid
/// and population/jobs, used to label a station's catchment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiDistrict {
    /// District id.
    pub id: i64,
    /// Display name.
    pub name: String,
    /// Centroid world X.
    pub x: f64,
    /// Centroid world Y.
    pub y: f64,
    /// Population in the district.
    pub population: f64,
    /// Jobs in the district.
    pub jobs: f64,
}

/// Mirrors the sidecar's `LifetimeLedger` (`core/types.ts`): cumulative
/// money flows since game start, accumulated as each sim-day closes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiLifetimeLedger {
    /// Cumulative fare revenue collected.
    pub fares: f64,
    /// Cumulative subsidy income received.
    pub subsidy: f64,
    /// Cumulative operating costs paid.
    pub operations: f64,
    /// Cumulative maintenance costs paid.
    pub maintenance: f64,
    /// Cumulative interest paid on debt.
    pub interest: f64,
    /// Number of days accumulated into the totals above.
    pub days: f64,
}

/// `UiState` — metroforge/src/host/protocol.ts:46-78. Sent at 2 Hz; the
/// native wire envelope for `t:"ui"` carries this struct directly as `p`
/// (spec §1.1: `ui {...UiState}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    /// Simulation tick counter.
    pub tick: u64,
    /// Player-facing insight strings for the HUD.
    pub insights: Vec<String>,
    /// Current simulated day number.
    pub day: u32,
    /// Current simulation speed multiplier.
    pub speed: f64,
    /// Player cash balance.
    pub cash: f64,
    /// Outstanding loan principal.
    pub loan_balance: f64,
    /// Previous day's ledger breakdown.
    pub last_day: DayLedger,
    /// rolling net/day (oldest -> newest), up to 7 entries
    pub net_history: Vec<f64>,
    /// City population.
    pub population: f64,
    /// Player approval (0..1).
    pub approval: f64,
    /// Transit mode share of trips (0..1).
    pub transit_share: f64,
    /// Network coverage fraction (0..1).
    pub coverage: f64,
    /// Daily transit trip count.
    pub daily_transit_trips: f64,
    /// Modes currently unlocked for construction.
    pub unlocked_modes: Vec<TransitMode>,
    /// All stations for the UI map/list.
    pub stations: Vec<UiStation>,
    /// All tracks for the UI map.
    pub tracks: Vec<UiTrack>,
    /// All routes for the UI map/list.
    pub routes: Vec<UiRoute>,
    /// Currently active random/scenario events.
    pub active_events: Vec<ActiveEventDto>,
    /// Fields bump counter (matches binary `Fields.version`).
    pub fields_version: u32,
    /// True when cash has gone negative / bankrupt flag is set.
    pub bankrupt: bool,
    /// Failure reason when the run has ended; `None` while playing.
    #[serde(default)]
    pub failed: Option<FailReason>,
    /// Scenario day limit, if any.
    #[serde(default)]
    pub max_day: Option<u32>,
    /// Optional era label for the HUD.
    #[serde(default)]
    pub era_label: Option<String>,
    /// Number of commands applied so far (for replay / sync).
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
    /// Sim-depth (PR #31): cumulative lifetime ledger across the game.
    /// The sidecar emits an object (`uiExtras.ts` / `LifetimeLedger` in
    /// `core/types.ts`), NOT a scalar - mistyping this as `f64` made every
    /// `ui` envelope fail to decode once the first sim-day closed (the
    /// v0.5.1 release-gate blocker).
    #[serde(default)]
    pub lifetime: Option<UiLifetimeLedger>,
    /// Sim-depth (PR #31): catchment districts.
    #[serde(default)]
    pub districts: Vec<UiDistrict>,
    /// Sim-depth (PR #31): COUNT of routes currently over capacity. The
    /// sidecar emits a scalar (`routes.filter(crowding > 1).length`), not a
    /// list of ids - mistyping this as `Vec` broke `ui` decoding from tick
    /// 0 (the v0.5.1 release-gate blocker). Which routes are crowded comes
    /// from per-route `live_crowding` instead.
    #[serde(default)]
    pub overcrowded_routes: Option<u32>,
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
    /// Line start X.
    pub x1: f64,
    /// Line start Y.
    pub y1: f64,
    /// Line end X.
    pub x2: f64,
    /// Line end Y.
    pub y2: f64,
    /// Absolute demand weight along this OD pair.
    pub weight: f64,
    /// Share of total demand (0..1).
    pub share: f64,
}

/// `DemandPayload` — metroforge/src/host/protocol.ts:140-145
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemandPayload {
    /// Demand-flow lines for the overlay.
    pub lines: Vec<DemandLine>,
    /// Maximum `weight` among `lines` (for normalization).
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
    /// Place a new station at `pos`.
    BuildStation {
        /// Transit mode of the new station.
        mode: TransitMode,
        /// World position.
        pos: Vec2,
    },
    /// Build a track between two stations (optional waypoints).
    BuildTrack {
        /// Transit mode of the track.
        mode: TransitMode,
        /// Grade (surface / elevated / tunnel).
        grade: TrackGrade,
        /// From-station entity id.
        from_station_id: i64,
        /// To-station entity id.
        to_station_id: i64,
        /// Intermediate waypoints in world space.
        waypoints: Vec<Vec2>,
    },
    /// Create a new route through the given stations.
    CreateRoute {
        /// Transit mode of the route.
        mode: TransitMode,
        /// Ordered station ids.
        station_ids: Vec<i64>,
    },
    /// Edit mutable properties of an existing route (unset fields unchanged).
    EditRoute {
        /// Route entity id.
        route_id: i64,
        /// New headway in seconds, if changing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        headway_seconds: Option<f64>,
        /// New fare, if changing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fare: Option<f64>,
        /// New vehicle count, if changing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vehicle_count: Option<u32>,
        /// New display name, if changing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// New color string, if changing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
    },
    /// Delete a route by id.
    DeleteRoute {
        /// Route entity id.
        route_id: i64,
    },
    /// Demolish a station by id.
    DemolishStation {
        /// Station entity id.
        station_id: i64,
    },
    /// Demolish a track by id.
    DemolishTrack {
        /// Track entity id.
        track_id: i64,
    },
    /// Upgrade a station's level.
    UpgradeStation {
        /// Station entity id.
        station_id: i64,
    },
    /// Take out a loan for `amount`.
    TakeLoan {
        /// Loan principal to borrow.
        amount: f64,
    },
    /// Repay loan principal by `amount`.
    RepayLoan {
        /// Amount of principal to repay.
        amount: f64,
    },
    /// Rename a station.
    RenameStation {
        /// Station entity id.
        station_id: i64,
        /// New display name.
        name: String,
    },
}

/// `CommandResult` — metroforge/src/core/types.ts:225-230
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    /// Whether the command succeeded.
    pub ok: bool,
    /// Error message when `ok` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Id of a newly created entity when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_id: Option<i64>,
}

/// One `ReplayPayload.commandLog[]` entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandLogEntry {
    /// Tick at which the command was applied.
    pub tick: u64,
    /// The command that was applied.
    pub cmd: Command,
}

/// `ReplayPayload` — metroforge/src/host/protocol.ts:160-170
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayPayload {
    /// RNG seed used for the run.
    pub seed: u64,
    /// Difficulty of the run.
    pub difficulty: Difficulty,
    /// Optional preset city key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_key: Option<String>,
    /// Optional procedural city size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<CitySize>,
    /// Optional scenario rules used for the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<ScenarioRules>,
    /// Ordered command log for deterministic replay.
    pub command_log: Vec<CommandLogEntry>,
    /// Tick at end of the recorded run.
    pub final_tick: u64,
    /// Hash of final sim state for verification.
    pub state_hash: i64,
    /// Soft score hint for leaderboards / UI.
    pub score_hint: f64,
}

/// Compact north-up map preview carried on an enriched `hello.cityList`
/// entry so the city-select screen can paint a miniature without loading
/// the city. Older sidecars omit this entirely (`None`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CityMapPreview {
    /// World edge length in meters (same units as [`StaticCityJson::world_size`]).
    pub world_size: f64,
    /// Side length of the square [`water`] grid.
    pub res: u32,
    /// Row-major water flags (`0` land, nonzero water), length `res * res`.
    /// Sidecars may also send bit-packed previews via a parallel channel;
    /// the JSON form stays byte-per-cell for simplicity.
    pub water: Vec<u8>,
    /// Arterial polylines as flat x,y world-meter pairs (same convention as
    /// [`RoadDto::points`]).
    #[serde(default)]
    pub arterials: Vec<Vec<f64>>,
}

/// One entry of the sidecar `hello.cityList`.
///
/// Additive fields (`country`, `population`, `building_count`, `size_km`,
/// `map_preview`) are optional so older sidecars that only send `{key,label}`
/// keep deserializing; the city-select UI fills gaps from its local catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CityListEntry {
    /// Preset key sent in `init.presetKey`.
    pub key: String,
    /// Player-facing city name.
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// City-proper population when the sidecar knows one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub population: Option<f64>,
    /// Count of vector building footprints in the city bundle, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub building_count: Option<u32>,
    /// World edge length in kilometers (`worldSize / 1000`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_km: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_preview: Option<CityMapPreview>,
}

/// Sidecar -> client `hello` payload — spec §1.1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloInfo {
    /// Sidecar protocol version (must match [`crate::PROTOCOL_VERSION`]).
    pub protocol_version: u32,
    /// Sidecar / game version string.
    pub game_version: String,
    /// Available preset cities.
    pub city_list: Vec<CityListEntry>,
    /// Default world size in meters for procedural cities.
    pub default_world_size: f64,
}

/// `tone` on the sidecar `toast` message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToastTone {
    /// Informational toast.
    Info,
    /// Warning toast.
    Warn,
    /// Positive / success toast.
    Good,
}
