//! Core simulation types: the full `GameState` and its constituents. Port of
//! `sim/src/core/types.ts` (~459 lines).
//!
//! # Hashed vs transient
//!
//! `GameState` carries two classes of field:
//!
//! * **Persisted** fields ride in the save file (serde) AND, for a small
//!   subset, in the determinism hash.
//! * **Transient** fields are recomputed on load and are never serialized and
//!   never hashed (weather, traffic, analytics, the OSM render masks, POI
//!   anchors, `instance_id`). They live in the clearly marked
//!   `// ==== TRANSIENT ====` region below and carry
//!   `#[serde(skip)]`, mirroring `save.ts::serialize`'s destructured exclusion
//!   list exactly.
//!
//! The determinism **hash** is even narrower than "persisted": it is defined by
//! `sim/src/core/save.ts::stateHash` (line 186) and implemented in
//! [`crate::save::state_hash`]. That is the single source of truth for which
//! fields hash and in what order; see that function for the audited field set.
//!
//! Enums here mirror the wire enums in `mf-protocol` variant-for-variant (same
//! names, same order) so a P4 bridge is a trivial `match`. They are duplicated
//! rather than re-exported to keep `mf-sim` free of the mandatory `serde` /
//! `serde_json` / `thiserror` dependency graph `mf-protocol` pulls in; the sim
//! core stays std-only plus an OPTIONAL `serde` feature.

use crate::geometry::{Polyline, Vec2};
use crate::rng::RngState;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Convenience macro: derive serde only when the `serde` feature is on.
macro_rules! sim_type {
    ($item:item) => {
        #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
        $item
    };
}

// ── Enums (aligned with mf-protocol) ─────────────────────────────────────────

sim_type! {
    /// Transit mode. Mirrors `TransitMode` (types.ts:51) and
    /// `mf_protocol::TransitMode`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
    pub enum TransitMode {
        /// Bus.
        Bus,
        /// Tram / light-rail.
        Tram,
        /// Metro / subway.
        Metro,
        /// Heavy / commuter rail.
        Rail,
    }
}

sim_type! {
    /// Track grade. Mirrors `TrackGrade` (types.ts:52) and
    /// `mf_protocol::TrackGrade`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
    pub enum TrackGrade {
        /// At-grade / surface.
        Surface,
        /// Elevated.
        Elevated,
        /// Underground tunnel.
        Tunnel,
    }
}

sim_type! {
    /// Road classification. Mirrors `RoadClass` (types.ts).
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
    pub enum RoadClass {
        /// Arterial road.
        Arterial,
        /// Collector road.
        Collector,
        /// Local street.
        Local,
    }
}

sim_type! {
    /// Difficulty preset. Mirrors `Difficulty` (types.ts:164) and
    /// `mf_protocol::Difficulty`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
    pub enum Difficulty {
        /// Easy.
        Easy,
        /// Normal.
        Normal,
        /// Hard.
        Hard,
    }
}

sim_type! {
    /// POI anchor kind. Mirrors `PoiAnchor.kind` and `mf_protocol::PoiAnchorKind`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
    pub enum PoiKind {
        /// Stadium / arena.
        Stadium,
        /// Airport.
        Airport,
        /// University.
        University,
        /// Hospital.
        Hospital,
        /// Museum.
        Museum,
    }
}

sim_type! {
    /// Service period for per-period frequency (v0.9 System A). Mirrors
    /// `Period` in `sim/src/core/ops/periods.ts`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
    #[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
    pub enum Period {
        /// AM peak.
        AmPeak,
        /// Midday.
        Midday,
        /// PM peak.
        PmPeak,
        /// Evening.
        Evening,
        /// Night.
        Night,
    }
}

sim_type! {
    /// Fleet-unit operational status. Mirrors `FleetUnit.status` (types.ts).
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
    pub enum FleetStatus {
        /// In service.
        Active,
        /// Out of service for maintenance.
        Maintenance,
        /// Out of service, broken down.
        BrokenDown,
    }
}

sim_type! {
    /// Why a run ended. Mirrors `GameState.failed` union (types.ts) and
    /// `mf_protocol::FailReason`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
    pub enum FailReason {
        /// Ran out of money past the grace period.
        Bankrupt,
        /// Approval stayed below the floor too long.
        Approval,
        /// Ran out of time (scenario day limit).
        Time,
        /// Fleet condition collapse.
        Condition,
    }
}

// ── World / fields ───────────────────────────────────────────────────────────

sim_type! {
    /// Scalar fields on a coarse grid. Data, not geometry. Row-major, size
    /// `w*h`. Mirrors `FieldGrid`. The typed-array channels port to `Vec<f32>`
    /// / `Vec<u8>` and hash as byte slices (in P3 field-hashing paths).
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct FieldGrid {
        /// Grid width in cells.
        pub w: u32,
        /// Grid height in cells.
        pub h: u32,
        /// Meters per cell.
        pub cell_size: f64,
        /// World-space X origin of cell (0,0) corner.
        pub origin_x: f64,
        /// World-space Y origin of cell (0,0) corner.
        pub origin_y: f64,
        /// Elevation, 0..1 (`Float32Array`).
        pub terrain: Vec<f32>,
        /// Water flag, 0|1 (`Uint8Array`).
        pub water: Vec<u8>,
        /// Park flag, 0|1 (`Uint8Array`).
        pub parks: Vec<u8>,
        /// Residents per cell (`Float32Array`).
        pub population: Vec<f32>,
        /// Jobs per cell (`Float32Array`).
        pub jobs: Vec<f32>,
        /// Relative land value 0..~3 (`Float32Array`).
        pub land_value: Vec<f32>,
        /// NIMBY resistance 0..100 (`Float32Array`).
        pub nimby: Vec<f32>,
    }
}

sim_type! {
    /// A road segment. Mirrors `RoadEdge`. `grade_level` / `is_bridge` /
    /// `is_tunnel` are static presentation data (from OSM tags), NOT part of
    /// the state hash.
    #[derive(Clone, Debug, PartialEq)]
    pub struct RoadEdge {
        /// Entity id.
        pub id: u32,
        /// Road class.
        pub cls: RoadClass,
        /// Geometry.
        pub polyline: Polyline,
        /// Grade-separation level (signed; 0 ground). Absent = 0.
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub grade_level: Option<i32>,
        /// OSM bridge deck. Absent = false.
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub is_bridge: Option<bool>,
        /// OSM tunnel. Absent = false.
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub is_tunnel: Option<bool>,
    }
}

sim_type! {
    /// Demand aggregation unit: a cluster of field cells. Mirrors `District`.
    #[derive(Clone, Debug, PartialEq)]
    pub struct District {
        /// Entity id.
        pub id: u32,
        /// Display name.
        pub name: String,
        /// Centroid position.
        pub centroid: Vec2,
        /// Member field-cell indices.
        pub cell_indices: Vec<u32>,
        /// Residents.
        pub population: f64,
        /// Jobs.
        pub jobs: f64,
        /// Mean land value.
        pub land_value: f64,
        /// Transient/derived last-period growth delta (not serialized, not
        /// hashed).
        #[cfg_attr(feature = "serde", serde(skip))]
        pub last_growth_delta: Option<f64>,
    }
}

sim_type! {
    /// A named point-of-interest anchor from the OSM city bundle.
    /// Mirrors `PoiAnchor`.
    #[derive(Clone, Debug, PartialEq)]
    pub struct PoiAnchor {
        /// String id.
        pub id: String,
        /// Anchor kind.
        pub kind: PoiKind,
        /// Display name.
        pub name: String,
        /// World-space centroid `[x, y]`.
        pub centroid: [f64; 2],
        /// Optional footprint area.
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub area: Option<f64>,
    }
}

// ── Transit ──────────────────────────────────────────────────────────────────

sim_type! {
    /// A transit station. Mirrors `Station`.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Station {
        /// Entity id.
        pub id: u32,
        /// Display name.
        pub name: String,
        /// World position.
        pub pos: Vec2,
        /// Transit mode.
        pub mode: TransitMode,
        /// Level 1..5.
        pub level: u32,
        /// Rolling daily boardings (from assignment).
        pub ridership: f64,
        /// Rolling daily alightings (from assignment).
        pub alightings: f64,
        /// Tick the station was built.
        pub build_tick: u64,
        /// Underground depth (m); `None` = surface.
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub depth: Option<f64>,
    }
}

sim_type! {
    /// Additive cost breakdown for a track-cost quote (v0.8). Mirrors
    /// `TrackCostBreakdown`.
    #[derive(Clone, Debug, PartialEq)]
    pub struct TrackCostBreakdown {
        /// Cost of the equivalent surface alignment.
        pub surface: f64,
        /// Cost of the equivalent elevated alignment.
        pub elevated: f64,
        /// Total cut-and-cover component.
        pub cut_cover: f64,
        /// Total bored component.
        pub bored: f64,
        /// Dominant strata crossed, e.g. "fill/clay/rock".
        pub strata: String,
        /// Does any part sit below the water table?
        pub below_water_table: bool,
    }
}

sim_type! {
    /// A built track segment. Mirrors `TrackSegment`.
    #[derive(Clone, Debug, PartialEq)]
    pub struct TrackSegment {
        /// Entity id.
        pub id: u32,
        /// Transit mode.
        pub mode: TransitMode,
        /// Grade.
        pub grade: TrackGrade,
        /// From-station id.
        pub from_station_id: u32,
        /// To-station id.
        pub to_station_id: u32,
        /// Geometry.
        pub polyline: Polyline,
        /// Build cost paid.
        pub build_cost: f64,
        /// Cached corridor density at midpoint (derived; refreshed each
        /// assignment). Absent on legacy saves.
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub congestion_density: Option<f64>,
    }
}

sim_type! {
    /// A route definition. Mirrors `RouteDef`. Many fields are derived from the
    /// flow assignment (P3); the v0.9 ops fields are optional/additive.
    #[derive(Clone, Debug, PartialEq)]
    pub struct RouteDef {
        /// Entity id.
        pub id: u32,
        /// Display name.
        pub name: String,
        /// Color string.
        pub color: String,
        /// Transit mode.
        pub mode: TransitMode,
        /// Ordered station ids.
        pub station_ids: Vec<u32>,
        /// Ordered track segment ids (len = station_ids.len() - 1).
        pub segment_ids: Vec<u32>,
        /// Service headway, seconds.
        pub headway_seconds: f64,
        /// Fare.
        pub fare: f64,
        /// Assigned vehicle count.
        pub vehicle_count: u32,
        /// Derived daily ridership.
        pub daily_ridership: f64,
        /// Derived daily revenue.
        pub daily_revenue: f64,
        /// Derived peak-hour capacity.
        pub capacity: f64,
        /// Derived peak-hour load.
        pub load: f64,
        /// Derived load / capacity.
        pub crowding: f64,
        /// Derived per-segment daily loads (aligned to `segment_ids`).
        pub segment_loads: Vec<f64>,
        /// Fraction of route not in tunnel (derived). Absent = 1.
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub surface_exposure: Option<f64>,
        /// Length-weighted day-average grade-effective speed, m/s (derived).
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub move_grade_speed: Option<f64>,
        /// Per-period target headway (ops A1). Keyed by [`Period`].
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub frequency: Option<std::collections::BTreeMap<Period, f64>>,
        /// Command-set base headway (ops neutral value).
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub scheduled_headway: Option<f64>,
        /// In-service unit count right now (derived each ops tick).
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub in_service_vehicles: Option<u32>,
        /// Rolling on-time fraction 0..1 (ops A5). HASHED (see save::state_hash).
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub on_time_pct: Option<f64>,
        /// Rolling average delay per departure, seconds (ops A5).
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub avg_delay_sec: Option<f64>,
        /// Lagged reliability -> ridership multiplier 0..1.
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub reliability_demand_mult: Option<f64>,
    }
}

// ── v0.9 System A (Operations) ────────────────────────────────────────────────

sim_type! {
    /// One rolling-stock unit. Mirrors `FleetUnit`.
    #[derive(Clone, Debug, PartialEq)]
    pub struct FleetUnit {
        /// Entity id.
        pub id: u32,
        /// Transit mode.
        pub mode: TransitMode,
        /// Assigned route id; `None` = idle in the pool.
        #[cfg_attr(feature = "serde", serde(default))]
        pub route_id: Option<u32>,
        /// Age in sim-days.
        pub age_days: f64,
        /// Health 0..1. HASHED (see save::state_hash).
        pub condition: f64,
        /// Operational state. HASHED (0/1/2 by variant).
        pub status: FleetStatus,
        /// Ticks remaining in the current non-active status.
        pub status_ticks_left: u32,
    }
}

sim_type! {
    /// A depot / maintenance facility for one mode. Mirrors `Depot`.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Depot {
        /// Entity id.
        pub id: u32,
        /// Transit mode served.
        pub mode: TransitMode,
        /// World position.
        pub pos: Vec2,
        /// Tick the depot was built.
        pub build_tick: u64,
    }
}

sim_type! {
    /// An active breakdown incident. Mirrors `BreakdownIncident`.
    #[derive(Clone, Debug, PartialEq)]
    pub struct BreakdownIncident {
        /// Entity id.
        pub id: u32,
        /// Affected route id.
        pub route_id: u32,
        /// Disabled unit id.
        pub unit_id: u32,
        /// Index into the route's `segment_ids` that is blocked.
        pub segment_index: u32,
        /// Ticks remaining until the blockage clears.
        pub ticks_left: u32,
    }
}

sim_type! {
    /// A moving vehicle marker. Mirrors `VehicleState`.
    #[derive(Clone, Debug, PartialEq)]
    pub struct VehicleState {
        /// Entity id.
        pub id: u32,
        /// Route id.
        pub route_id: u32,
        /// Distance along the route's out-and-back path. HASHED.
        pub along: f64,
        /// Total out-and-back length cached at spawn.
        pub path_length: f64,
        /// Dwell time remaining.
        pub dwell_remaining: f64,
        /// Crowding-derived occupancy 0..1.
        pub occupancy: f64,
    }
}

sim_type! {
    /// Per-route daily reliability accumulator. Mirrors the `opsDaily` value
    /// shape.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct OpsDaily {
        /// Departures counted today.
        pub departures: f64,
        /// Delayed departures counted today.
        pub delayed_departures: f64,
        /// Accumulated delay this day, seconds.
        pub delay_sec: f64,
    }
}

// ── Demand / flows ────────────────────────────────────────────────────────────

sim_type! {
    /// One assigned origin-destination flow. Mirrors `FlowResult`.
    #[derive(Clone, Debug, PartialEq)]
    pub struct FlowResult {
        /// Origin district id.
        pub origin_district: u32,
        /// Destination district id.
        pub dest_district: u32,
        /// Trips per day choosing transit.
        pub transit_trips: f64,
        /// Trips per day choosing car.
        pub car_trips: f64,
        /// Generalized cost minutes for the transit path.
        pub transit_cost: f64,
        /// Route ids traversed in order.
        pub route_ids: Vec<u32>,
        /// Station ids traversed: `[board, ...transfers..., alight]`.
        pub station_ids: Vec<u32>,
    }
}

// ── Economy ───────────────────────────────────────────────────────────────────

sim_type! {
    /// One day's ledger. Mirrors `DayLedger`.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct DayLedger {
        /// Fare revenue.
        pub fares: f64,
        /// Subsidy income.
        pub subsidy: f64,
        /// Operations cost.
        pub operations: f64,
        /// Maintenance cost.
        pub maintenance: f64,
        /// Interest paid.
        pub interest: f64,
    }
}

sim_type! {
    /// Cumulative lifetime ledger. Mirrors `LifetimeLedger`.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct LifetimeLedger {
        /// Lifetime fare revenue.
        pub fares: f64,
        /// Lifetime subsidy income.
        pub subsidy: f64,
        /// Lifetime operations cost.
        pub operations: f64,
        /// Lifetime maintenance cost.
        pub maintenance: f64,
        /// Lifetime interest paid.
        pub interest: f64,
        /// Days accumulated.
        pub days: u32,
    }
}

sim_type! {
    /// Budget state. Mirrors `Budget`. `cash` is HASHED (see save::state_hash).
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Budget {
        /// Current cash. HASHED.
        pub cash: f64,
        /// Outstanding loan balance.
        pub loan_balance: f64,
        /// Annual loan rate.
        pub loan_rate: f64,
        /// Yesterday's ledger (for UI).
        pub last_day: DayLedger,
        /// Rolling net/day history (oldest -> newest), capped at 7.
        pub net_history: Vec<f64>,
        /// Cumulative lifetime totals (absent on legacy saves).
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub lifetime: Option<LifetimeLedger>,
    }
}

sim_type! {
    /// Headline city statistics. Mirrors `CityStats`. `population` is HASHED.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct CityStats {
        /// Total population. HASHED.
        pub population: f64,
        /// Total jobs.
        pub jobs: f64,
        /// Daily transit trips.
        pub daily_transit_trips: f64,
        /// Daily car trips.
        pub daily_car_trips: f64,
        /// Transit mode share 0..1.
        pub transit_share: f64,
        /// Fraction of population within walk radius of a station.
        pub coverage: f64,
        /// Approval 0..100.
        pub approval: f64,
    }
}

// ── Command log ───────────────────────────────────────────────────────────────

sim_type! {
    /// One stamped command in the replay log. Mirrors the `commandLog[]` entry.
    #[derive(Clone, Debug, PartialEq)]
    pub struct CommandLogEntry {
        /// Tick the command was applied.
        pub tick: u64,
        /// The command applied.
        pub cmd: crate::commands::SimCommand,
    }
}

// ── Scenario / events (P2/P3-owned; minimal placeholders here) ────────────────

sim_type! {
    /// Optional scenario constraints. Mirrors `ScenarioRules`
    /// (scenarioRules.ts). Field set aligned with `mf_protocol::ScenarioRules`.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct ScenarioRules {
        /// Optional scenario id.
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub scenario_id: Option<String>,
        /// Modes available at start.
        pub starting_modes: Vec<TransitMode>,
        /// Lock modes beyond `starting_modes`.
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub lock_modes: Option<bool>,
        /// Optional day limit.
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub max_day: Option<u32>,
        /// Minimum approval before failure (0..1).
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub approval_floor: Option<f64>,
        /// Starting cash override.
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub starting_cash: Option<f64>,
        /// Daily subsidy override.
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub daily_subsidy: Option<f64>,
        /// Optional era label.
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub era_label: Option<String>,
    }
}

// `ActiveEvent` is now [`crate::events::ActiveEvent`] (`{ id, days_left }`),
// reconciled at integration. The P1 placeholder was removed.

sim_type! {
    /// Active data-driven scenario handle.
    ///
    /// The full catalog definition (`scenario::evaluate::ScenarioDef`) is loaded
    /// by id at runtime from `scenario::catalog`; only the stable id persists in
    /// saves/replays.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct ScenarioDef {
        /// Scenario id.
        pub id: String,
    }
}

// ── Game state ────────────────────────────────────────────────────────────────

/// The full game state. Port of `GameState` (types.ts). Everything except the
/// clearly marked TRANSIENT region is JSON-serializable (the save format).
///
/// The determinism hash is defined in [`crate::save::state_hash`] and covers
/// only a narrow subset of these fields (see that function). Everything else,
/// serialized or not, does NOT enter the hash.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[derive(Clone, Debug)]
pub struct GameState {
    // ==== PERSISTED (serialized) ====
    /// Original numeric seed. Mirrors `seed`.
    pub seed: u32,
    /// Tick counter (1 tick = 1 game-second). Mirrors `tick`. HASHED.
    pub tick: u64,
    /// Primary RNG stream state. Mirrors `rngState`. Kept SEPARATE from
    /// `ops_rng_state` so ops randomness cannot reorder other systems.
    pub rng_state: RngState,
    /// Difficulty preset.
    pub difficulty: Difficulty,
    /// City preset key (selects the weather climate profile).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub city_key: Option<String>,
    /// Scalar field grid.
    pub fields: FieldGrid,
    /// Road network.
    pub roads: Vec<RoadEdge>,
    /// Districts.
    pub districts: Vec<District>,
    /// Stations. `len` is HASHED.
    pub stations: Vec<Station>,
    /// Track segments. `len` is HASHED.
    pub tracks: Vec<TrackSegment>,
    /// Routes. `len` is HASHED, plus per-route fields (see save::state_hash).
    pub routes: Vec<RouteDef>,
    /// Vehicle markers. Per-vehicle `along` is HASHED.
    pub vehicles: Vec<VehicleState>,
    /// Assigned OD flows.
    pub flows: Vec<FlowResult>,
    /// Budget. `cash` is HASHED.
    pub budget: Budget,
    /// City stats. `population` is HASHED.
    pub stats: CityStats,
    /// Monotonic entity id counter.
    pub next_id: u32,
    /// Assignment reruns on next demand pass when set.
    pub demand_dirty: bool,
    /// Unlocked transit modes.
    pub unlocked_modes: Vec<TransitMode>,
    /// Active city events (each `{ id, days_left }`; see [`crate::events`]).
    pub active_events: Vec<crate::events::ActiveEvent>,
    /// Earliest day a new event may start.
    pub next_event_day: u32,
    /// Optional scenario constraints.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub scenario_rules: Option<ScenarioRules>,
    /// Active data-driven scenario.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub scenario: Option<ScenarioDef>,
    /// True once the scenario win tree is satisfied.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub scenario_won: Option<bool>,
    /// Ids of mid-run scenario events already fired.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub fired_scenario_events: Option<Vec<String>>,
    /// Per-district travel-demand multipliers (scenario events). `BTreeMap` for
    /// deterministic iteration (mirrors `Record<number, number>`).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub district_demand_mult: Option<std::collections::BTreeMap<u32, f64>>,
    /// Temporary citywide demand multiplier.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub global_demand_mult: Option<f64>,
    /// Days remaining on `global_demand_mult`.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub global_demand_mult_days_left: Option<u32>,
    /// Stamped command stream for replay / anti-cheat.
    pub command_log: Vec<CommandLogEntry>,
    /// Consecutive days at/below the approval floor.
    pub low_approval_days: u32,
    /// Consecutive days below the bankruptcy cash floor. NOT hashed.
    pub bankrupt_days: u32,
    /// Why the run ended, if it failed.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub failed: Option<FailReason>,

    // ── v0.9 System A (Operations); optional/additive ──
    /// Rolling-stock ledger. Per-unit `condition` + `status` are HASHED.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub fleet: Option<Vec<FleetUnit>>,
    /// Placed depots (one per mode).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub depots: Option<Vec<Depot>>,
    /// Active breakdown incidents. `len` is HASHED.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub incidents: Option<Vec<BreakdownIncident>>,
    /// Dedicated ops RNG stream. Kept SEPARATE from `rng_state`.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub ops_rng_state: Option<RngState>,
    /// Service period the last ops tick resolved.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub ops_period: Option<Period>,
    /// Per-route daily reliability accumulators. `BTreeMap` for determinism.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub ops_daily: Option<std::collections::BTreeMap<u32, OpsDaily>>,

    // ==== TRANSIENT / not hashed / not serialized ====
    // These mirror save.ts::serialize's destructured exclusion list exactly.
    // Recomputed on load; carry #[serde(skip)].
    /// Transient per-process instance id (geometry-cache scoping).
    #[cfg_attr(feature = "serde", serde(skip))]
    pub instance_id: u32,
    /// Transient current sky (pure fn of seed+tick+cityKey). See
    /// [`crate::weather::WeatherSnapshot`].
    #[cfg_attr(feature = "serde", serde(skip))]
    pub weather: Option<crate::weather::WeatherSnapshot>,
    /// Transient last headline weather event.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub last_weather_event: Option<crate::weather::WeatherEvent>,
    /// Transient road congestion field. See
    /// [`crate::transit::traffic::TrafficFieldOut`].
    #[cfg_attr(feature = "serde", serde(skip))]
    pub traffic: Option<crate::transit::traffic::TrafficFieldOut>,
    /// Transient unserved-demand overlay data. See
    /// [`crate::transit::assignment::UnservedDesire`].
    #[cfg_attr(feature = "serde", serde(skip))]
    pub unserved: Option<Vec<crate::transit::assignment::UnservedDesire>>,
    /// Transient analytics accumulator (heatmaps / OD / insights). See
    /// [`crate::analytics::AnalyticsState`].
    #[cfg_attr(feature = "serde", serde(skip))]
    pub analytics: Option<crate::analytics::AnalyticsState>,
    /// Transient OSM water mask (real cities).
    #[cfg_attr(feature = "serde", serde(skip))]
    pub osm_water_mask: Option<Vec<u8>>,
    /// Transient OSM park mask.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub osm_park_mask: Option<Vec<u8>>,
    /// Transient OSM building mask.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub osm_building_mask: Option<Vec<u8>>,
    /// Transient OSM mask resolution.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub osm_mask_res: Option<u32>,
    /// Transient real-elevation heightfield (meters, row-major).
    #[cfg_attr(feature = "serde", serde(skip))]
    pub osm_elevation: Option<Vec<i16>>,
    /// Transient elevation resolution.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub osm_elev_res: Option<u32>,
    /// Transient map labels.
    ///
    /// TODO(P2): port `city/osmCity.ts::MapLabel`.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub osm_labels: Option<Vec<MapLabel>>,
    /// Transient named POI anchors from the OSM bundle.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub poi_anchors: Option<Vec<PoiAnchor>>,
}

// ── Transient placeholder types (owned by later phases) ──────────────────────
// These exist only so the transient `GameState` slots have a concrete type.
// None are serialized or hashed. The real shapes land with their systems.

sim_type! {
    /// Map-label category. Mirrors `MapLabel.kind` (`city/osmCity.ts`) and
    /// `mf_protocol::MapLabelKind`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
    pub enum MapLabelKind {
        /// Road-name label (has a baseline `angle`).
        Road,
        /// Water-body label.
        Water,
        /// Park / green-space label.
        Park,
    }
}

sim_type! {
    /// Real OSM place-name label for the map layer. Mirrors
    /// `city/osmCity.ts::MapLabel`. Transient (render-only), not hashed.
    #[derive(Clone, Debug, PartialEq)]
    pub struct MapLabel {
        /// Label category.
        pub kind: MapLabelKind,
        /// Display name (real OSM place name).
        pub name: String,
        /// World-space X of the anchor.
        pub x: f64,
        /// World-space Y of the anchor.
        pub y: f64,
        /// Road labels: baseline angle in radians. Absent for water/park.
        #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
        pub angle: Option<f64>,
        /// Importance 1..~5 (drives zoom-gated visibility + size).
        pub imp: f64,
    }
}

impl GameState {
    /// Build a minimal, valid empty state from a seed. This is a P1 placeholder
    /// for `sim/src/core/newGame.ts` (worldgen lands in P2): it seeds only what
    /// the tick loop, commands, and hash need. Difficulty defaults to Normal.
    pub fn new(seed: u32) -> Self {
        Self::with_difficulty(seed, Difficulty::Normal)
    }

    /// Minimal empty state at a given difficulty.
    pub fn with_difficulty(seed: u32, difficulty: Difficulty) -> Self {
        let rng = crate::rng::Rng::from_seed(seed);
        GameState {
            seed,
            tick: 0,
            rng_state: rng.state(),
            difficulty,
            city_key: None,
            fields: FieldGrid::default(),
            roads: Vec::new(),
            districts: Vec::new(),
            stations: Vec::new(),
            tracks: Vec::new(),
            routes: Vec::new(),
            vehicles: Vec::new(),
            flows: Vec::new(),
            budget: Budget {
                cash: crate::constants::starting_cash(difficulty),
                loan_rate: 0.06,
                ..Budget::default()
            },
            stats: CityStats::default(),
            next_id: 1,
            demand_dirty: false,
            unlocked_modes: vec![TransitMode::Bus],
            active_events: Vec::new(),
            next_event_day: 8,
            scenario_rules: None,
            scenario: None,
            scenario_won: None,
            fired_scenario_events: None,
            district_demand_mult: None,
            global_demand_mult: None,
            global_demand_mult_days_left: None,
            command_log: Vec::new(),
            low_approval_days: 0,
            bankrupt_days: 0,
            failed: None,
            fleet: None,
            depots: None,
            incidents: None,
            ops_rng_state: None,
            ops_period: None,
            ops_daily: None,
            instance_id: 0,
            weather: None,
            last_weather_event: None,
            traffic: None,
            unserved: None,
            analytics: None,
            osm_water_mask: None,
            osm_park_mask: None,
            osm_building_mask: None,
            osm_mask_res: None,
            osm_elevation: None,
            osm_elev_res: None,
            osm_labels: None,
            poi_anchors: None,
        }
    }

    /// Rebuild an [`Rng`](crate::rng::Rng) from the saved primary stream state.
    pub fn rng(&self) -> crate::rng::Rng {
        crate::rng::Rng::from_state(self.rng_state)
    }

    /// The deterministic state fingerprint. Delegates to
    /// [`crate::save::state_hash`], the single source of truth for the hashed
    /// field set + order (mirrored from `save.ts::stateHash`).
    pub fn state_hash(&self) -> u64 {
        crate::save::state_hash(self)
    }
}
