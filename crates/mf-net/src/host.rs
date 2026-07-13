//! `GameState -> wire` serializer + the command bridge (P4).
//!
//! This is the Rust port of `sim/src/host/protocol.ts` + `sim/src/host/sim.worker.ts`'s
//! `buildUi` / `sendFrame` / `sendStatic` / `sendFields` builders, plus
//! `sim/src/host/uiExtras.ts` (`uiExtras` / `routeExtras`). It lives in `mf-net`
//! (not `mf-sim`) on purpose: `mf-sim` stays bevy-free AND serde-free, so the
//! module that needs BOTH `mf_sim::GameState` and the `mf_protocol` wire DTOs
//! lands here, the one crate that already depends on both. `mf-sim` is unchanged.
//!
//! Everything here is a pure function of a `&GameState` (+ the host-tracked
//! `speed` / `fields_version`), so it introduces no non-determinism.

use mf_protocol as wire;
use mf_sim::types as st;
use mf_sim::GameState;

// ── enum bridges (mf_sim <-> mf_protocol) ────────────────────────────────────

/// Sim transit mode -> wire transit mode.
pub fn mode_to_wire(m: st::TransitMode) -> wire::TransitMode {
    match m {
        st::TransitMode::Bus => wire::TransitMode::Bus,
        st::TransitMode::Tram => wire::TransitMode::Tram,
        st::TransitMode::Metro => wire::TransitMode::Metro,
        st::TransitMode::Rail => wire::TransitMode::Rail,
    }
}

/// Wire transit mode -> sim transit mode.
pub fn mode_from_wire(m: wire::TransitMode) -> st::TransitMode {
    match m {
        wire::TransitMode::Bus => st::TransitMode::Bus,
        wire::TransitMode::Tram => st::TransitMode::Tram,
        wire::TransitMode::Metro => st::TransitMode::Metro,
        wire::TransitMode::Rail => st::TransitMode::Rail,
    }
}

/// Lowercase mode string used where the wire wants a plain string (`UiDepot.mode`).
fn mode_str(m: st::TransitMode) -> String {
    match m {
        st::TransitMode::Bus => "bus",
        st::TransitMode::Tram => "tram",
        st::TransitMode::Metro => "metro",
        st::TransitMode::Rail => "rail",
    }
    .to_string()
}

/// Track grade -> the plain `String` the wire `UiTrack.grade` carries.
fn grade_str(g: st::TrackGrade) -> String {
    match g {
        st::TrackGrade::Surface => "surface",
        st::TrackGrade::Elevated => "elevated",
        st::TrackGrade::Tunnel => "tunnel",
    }
    .to_string()
}

fn grade_from_wire(g: wire::TrackGrade) -> st::TrackGrade {
    match g {
        wire::TrackGrade::Surface => st::TrackGrade::Surface,
        wire::TrackGrade::Elevated => st::TrackGrade::Elevated,
        wire::TrackGrade::Tunnel => st::TrackGrade::Tunnel,
    }
}

/// Wire difficulty -> sim difficulty (used when handling `init`).
pub fn difficulty_from_wire(d: wire::Difficulty) -> st::Difficulty {
    match d {
        wire::Difficulty::Easy => st::Difficulty::Easy,
        wire::Difficulty::Normal => st::Difficulty::Normal,
        wire::Difficulty::Hard => st::Difficulty::Hard,
    }
}

fn fail_to_wire(f: st::FailReason) -> Option<wire::FailReason> {
    match f {
        st::FailReason::Bankrupt => Some(wire::FailReason::Bankrupt),
        st::FailReason::Approval => Some(wire::FailReason::Approval),
        st::FailReason::Time => Some(wire::FailReason::Time),
        // The wire union is `bankrupt|approval|time|null`; `condition` has no
        // wire spelling, so it surfaces as the generic scenario-fail flag via
        // the `failed` bool path (bankrupt=false) rather than a `FailReason`.
        st::FailReason::Condition => None,
    }
}

fn period_str(p: st::Period) -> String {
    match p {
        st::Period::AmPeak => "amPeak",
        st::Period::Midday => "midday",
        st::Period::PmPeak => "pmPeak",
        st::Period::Evening => "evening",
        st::Period::Night => "night",
    }
    .to_string()
}

fn period_from_str(s: &str) -> Option<st::Period> {
    Some(match s {
        "amPeak" => st::Period::AmPeak,
        "midday" => st::Period::Midday,
        "pmPeak" => st::Period::PmPeak,
        "evening" => st::Period::Evening,
        "night" => st::Period::Night,
        _ => return None,
    })
}

fn weather_state_to_wire(s: mf_sim::weather::WeatherState) -> wire::WeatherState {
    use mf_sim::weather::WeatherState as W;
    match s {
        W::Clear => wire::WeatherState::Clear,
        W::Overcast => wire::WeatherState::Overcast,
        W::Rain => wire::WeatherState::Rain,
        W::Fog => wire::WeatherState::Fog,
        W::Snow => wire::WeatherState::Snow,
        W::Storm => wire::WeatherState::Storm,
    }
}

fn season_to_wire(s: mf_sim::weather::Season) -> wire::Season {
    use mf_sim::weather::Season as S;
    match s {
        S::Winter => wire::Season::Winter,
        S::Spring => wire::Season::Spring,
        S::Summer => wire::Season::Summer,
        S::Autumn => wire::Season::Autumn,
    }
}

fn weather_event_to_wire(e: mf_sim::weather::WeatherEvent) -> wire::WeatherEvent {
    use mf_sim::weather::WeatherEvent as E;
    match e {
        E::Blizzard => wire::WeatherEvent::Blizzard,
        E::Heatwave => wire::WeatherEvent::Heatwave,
    }
}

// ── command bridge ───────────────────────────────────────────────────────────

/// Translate an inbound wire [`wire::Command`] into a sim [`mf_sim::SimCommand`].
///
/// The two enums were aligned variant-for-variant in P1; the only real work is
/// the id-width cast (wire `i64` -> sim `u32`) and mapping the wire `period`
/// string onto the [`st::Period`] enum. Returns `None` only if a period string
/// is unrecognized (the sim then never sees a malformed command).
pub fn command_to_sim(cmd: &wire::Command) -> Option<mf_sim::SimCommand> {
    use mf_sim::SimCommand as S;
    let v = |p: &wire::Vec2| mf_sim::geometry::Vec2 { x: p.x, y: p.y };
    Some(match cmd {
        wire::Command::BuildStation { mode, pos } => S::BuildStation {
            mode: mode_from_wire(*mode),
            pos: v(pos),
        },
        wire::Command::BuildTrack {
            mode,
            grade,
            from_station_id,
            to_station_id,
            waypoints,
        } => S::BuildTrack {
            mode: mode_from_wire(*mode),
            grade: grade_from_wire(*grade),
            from_station_id: *from_station_id as u32,
            to_station_id: *to_station_id as u32,
            waypoints: waypoints.iter().map(v).collect(),
        },
        wire::Command::CreateRoute { mode, station_ids } => S::CreateRoute {
            mode: mode_from_wire(*mode),
            station_ids: station_ids.iter().map(|&i| i as u32).collect(),
        },
        wire::Command::EditRoute {
            route_id,
            headway_seconds,
            fare,
            vehicle_count,
            name,
            color,
        } => S::EditRoute {
            route_id: *route_id as u32,
            headway_seconds: *headway_seconds,
            fare: *fare,
            vehicle_count: *vehicle_count,
            name: name.clone(),
            color: color.clone(),
        },
        wire::Command::DeleteRoute { route_id } => S::DeleteRoute {
            route_id: *route_id as u32,
        },
        wire::Command::DemolishStation { station_id } => S::DemolishStation {
            station_id: *station_id as u32,
        },
        wire::Command::DemolishTrack { track_id } => S::DemolishTrack {
            track_id: *track_id as u32,
        },
        wire::Command::UpgradeStation { station_id } => S::UpgradeStation {
            station_id: *station_id as u32,
        },
        wire::Command::TakeLoan { amount } => S::TakeLoan { amount: *amount },
        wire::Command::RepayLoan { amount } => S::RepayLoan { amount: *amount },
        wire::Command::RenameStation { station_id, name } => S::RenameStation {
            station_id: *station_id as u32,
            name: name.clone(),
        },
        wire::Command::SetRouteFrequency {
            route_id,
            period,
            headway_seconds,
        } => S::SetRouteFrequency {
            route_id: *route_id as u32,
            period: period_from_str(period)?,
            headway_seconds: *headway_seconds,
        },
        wire::Command::BuildDepot { mode, pos } => S::BuildDepot {
            mode: mode_from_wire(*mode),
            pos: v(pos),
        },
    })
}

/// Translate a sim [`mf_sim::CommandResult`] to the wire [`wire::CommandResult`]
/// (id-width cast `u32` -> `i64`).
pub fn command_result_to_wire(r: &mf_sim::CommandResult) -> wire::CommandResult {
    wire::CommandResult {
        ok: r.ok,
        error: r.error.clone(),
        created_id: r.created_id.map(|id| id as i64),
    }
}

// ── hello / ready / fields ───────────────────────────────────────────────────

/// The one-shot `hello` the embedded sim sends on connect, mirroring the Bun
/// sidecar's capability advertisement. City list is the procedural preset
/// catalog (`mf_sim::city::presets`); OSM real-city bundles are P5.
pub fn hello_info() -> wire::HelloInfo {
    let city_list = mf_sim::city::presets::CITY_PRESETS
        .iter()
        .map(|p| wire::CityListEntry {
            key: p.key.to_string(),
            label: p.label.to_string(),
            country: None,
            population: None,
            building_count: None,
            size_km: None,
            map_preview: None,
        })
        .collect();
    wire::HelloInfo {
        protocol_version: wire::PROTOCOL_VERSION,
        game_version: env!("CARGO_PKG_VERSION").to_string(),
        city_list,
        default_world_size: mf_sim::city::presets::MapSize::Medium.meters(),
    }
}

fn road_cls_str(cls: st::RoadClass) -> String {
    match cls {
        st::RoadClass::Arterial => "arterial",
        st::RoadClass::Collector => "collector",
        st::RoadClass::Local => "local",
    }
    .to_string()
}

/// Map a sim map-label kind to the wire kind.
fn label_kind_to_wire(k: st::MapLabelKind) -> wire::MapLabelKind {
    match k {
        st::MapLabelKind::Road => wire::MapLabelKind::Road,
        st::MapLabelKind::Water => wire::MapLabelKind::Water,
        st::MapLabelKind::Park => wire::MapLabelKind::Park,
    }
}

/// Map a sim POI-anchor kind to the wire kind.
fn poi_kind_to_wire(k: st::PoiKind) -> wire::PoiAnchorKind {
    match k {
        st::PoiKind::Stadium => wire::PoiAnchorKind::Stadium,
        st::PoiKind::Airport => wire::PoiAnchorKind::Airport,
        st::PoiKind::University => wire::PoiAnchorKind::University,
        st::PoiKind::Hospital => wire::PoiAnchorKind::Hospital,
        st::PoiKind::Museum => wire::PoiAnchorKind::Museum,
    }
}

/// Build the static binary mask frames (water=0, park=1, building=2) for a real
/// (OSM) city, in the sidecar's order. Empty for procedural cities. Mirrors
/// `simHost.ts`'s `encodeStaticMask` calls.
pub fn build_masks(s: &GameState) -> Vec<wire::StaticMask> {
    let Some(res) = s.osm_mask_res else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(m) = &s.osm_water_mask {
        out.push(wire::StaticMask {
            which: wire::MaskWhich::Water,
            res,
            mask: m.clone(),
        });
    }
    if let Some(m) = &s.osm_park_mask {
        out.push(wire::StaticMask {
            which: wire::MaskWhich::Park,
            res,
            mask: m.clone(),
        });
    }
    if let Some(m) = &s.osm_building_mask {
        out.push(wire::StaticMask {
            which: wire::MaskWhich::Building,
            res,
            mask: m.clone(),
        });
    }
    out
}

/// Build the static real-elevation frame (msgType=7) for a real city, or
/// `None`. Mirrors `simHost.ts`'s `encodeStaticElevation` call.
pub fn build_elevation(s: &GameState) -> Option<wire::StaticElevation> {
    match (&s.osm_elevation, s.osm_elev_res) {
        (Some(h), Some(res)) => Some(wire::StaticElevation {
            res,
            heights: h.clone(),
        }),
        _ => None,
    }
}

/// Build the optional static per-building footprint payload (msgType=5) for a
/// real city key. Mirrors sidecar `resolveBuildings + encodeStaticBuildings`.
pub fn build_static_buildings(preset_key: Option<&str>) -> Option<wire::StaticBuildings> {
    crate::cities::resolve_buildings(preset_key)
}

/// Build the `ready` static-city payload. For real (OSM) cities this carries
/// the mask resolution + `has*Mask` flags (the mask BYTES arrive as separate
/// [`wire::StaticMask`] frames), the OSM place-name labels, and the POI
/// anchors. Procedural cities have none of these (flags false, `None`).
pub fn build_ready(s: &GameState) -> wire::ReadyPayload {
    let f = &s.fields;
    let world_size = f.w as f64 * f.cell_size;
    let road_scale = if s.roads.len() > 3000 {
        0.28
    } else if s.roads.len() > 1500 {
        0.5
    } else {
        1.0
    };
    let roads = s
        .roads
        .iter()
        .map(|r| wire::RoadDto {
            cls: road_cls_str(r.cls),
            points: r.polyline.points.iter().flat_map(|p| [p.x, p.y]).collect(),
            grade_level: r.grade_level.unwrap_or(0),
            is_bridge: r.is_bridge.unwrap_or(false),
            is_tunnel: r.is_tunnel.unwrap_or(false),
            name: None,
            wikidata: None,
        })
        .collect();
    let labels = s.osm_labels.as_ref().map(|ls| {
        ls.iter()
            .map(|l| wire::MapLabel {
                kind: label_kind_to_wire(l.kind),
                name: l.name.clone(),
                x: l.x,
                y: l.y,
                angle: l.angle,
                imp: l.imp,
            })
            .collect()
    });
    let poi_anchors = s.poi_anchors.as_ref().map(|as_| {
        as_.iter()
            .map(|a| wire::PoiAnchorDto {
                id: a.id.clone(),
                kind: poi_kind_to_wire(a.kind),
                name: a.name.clone(),
                centroid: a.centroid,
                area: a.area.unwrap_or(0.0),
            })
            .collect()
    });
    wire::ReadyPayload {
        static_city: wire::StaticCityJson {
            field_w: f.w,
            field_h: f.h,
            cell_size: f.cell_size,
            origin_x: f.origin_x,
            origin_y: f.origin_y,
            world_size,
            road_scale,
            mask_res: s.osm_mask_res,
            has_water_mask: s.osm_water_mask.is_some(),
            has_park_mask: s.osm_park_mask.is_some(),
            has_building_mask: s.osm_building_mask.is_some(),
            labels,
            poi_anchors,
            roads,
        },
    }
}

/// Build the binary `Fields` grid frame (msgType=2) at `version`.
pub fn build_fields(s: &GameState, version: u32) -> wire::Fields {
    let f = &s.fields;
    wire::Fields {
        version,
        cell_count: (f.w * f.h),
        terrain: f.terrain.clone(),
        population: f.population.clone(),
        jobs: f.jobs.clone(),
        land_value: f.land_value.clone(),
        water: f.water.clone(),
        parks: f.parks.clone(),
    }
}

// ── UiState ──────────────────────────────────────────────────────────────────

/// Daily fleet running cost (ops + maintenance) for a route. Mirrors
/// `routeOperatingCost` (economy.ts); the sim's copy is private so we recompute
/// from the public `constants::modes` config.
fn route_operating_cost(mode: st::TransitMode, vehicle_count: u32) -> f64 {
    let cfg = mf_sim::constants::modes(mode);
    vehicle_count as f64 * (cfg.ops_per_vehicle_per_day + cfg.maint_per_vehicle_per_day)
}

/// Length-weighted mean of per-segment grade-effective speeds at `tod_factor`.
/// Mirrors `routeAvgEffectiveSpeed` (uiExtras.ts).
fn route_avg_effective_speed(s: &GameState, r: &st::RouteDef, tod_factor: f64) -> f64 {
    use mf_sim::transit::grade_effects::{segment_density01, segment_effective_speed_mps};
    let mut len_sum = 0.0;
    let mut speed_len = 0.0;
    for seg_id in &r.segment_ids {
        let Some(seg) = s.tracks.iter().find(|t| t.id == *seg_id) else {
            continue;
        };
        let len = seg.polyline.length;
        if len <= 0.0 {
            continue;
        }
        let dens = segment_density01(Some(&s.fields), seg);
        let spd = segment_effective_speed_mps(r.mode, seg.grade, tod_factor, dens);
        len_sum += len;
        speed_len += spd * len;
    }
    if len_sum > 0.0 {
        speed_len / len_sum
    } else {
        0.0
    }
}

fn build_route(s: &GameState, r: &st::RouteDef, tod_factor: f64) -> wire::UiRoute {
    let path = mf_sim::transit::route_path::get_route_path(s, r);
    let length_meters = path.map(|p| p.length / 2.0).unwrap_or(0.0);
    let operating_cost = route_operating_cost(r.mode, r.vehicle_count);
    let crowding = r.crowding;

    // per-period target headway schedule (uiExtras `routeExtras.frequency`).
    let mut frequency = std::collections::HashMap::new();
    for &p in mf_sim::ops::periods::PERIODS.iter() {
        frequency.insert(period_str(p), mf_sim::ops::period_target_headway(r, p));
    }

    wire::UiRoute {
        id: r.id as i64,
        name: r.name.clone(),
        color: r.color.clone(),
        mode: mode_to_wire(r.mode),
        station_ids: r.station_ids.iter().map(|&i| i as i64).collect(),
        headway_seconds: r.headway_seconds,
        fare: r.fare,
        vehicle_count: r.vehicle_count,
        daily_ridership: r.daily_ridership,
        daily_revenue: r.daily_revenue,
        length_meters,
        capacity: r.capacity,
        load: r.load,
        crowding,
        segment_loads: r.segment_loads.clone(),
        // ── uiExtras `routeExtras` (v0.4.2 + v0.9) ──
        live_crowding: Some(crowding * tod_factor),
        operating_cost: Some(operating_cost),
        farebox: Some(if operating_cost > 0.0 {
            r.daily_revenue / operating_cost
        } else {
            0.0
        }),
        avg_effective_speed: Some(route_avg_effective_speed(s, r, tod_factor)),
        on_time_pct: Some(r.on_time_pct.unwrap_or(1.0)),
        avg_delay_sec: Some(r.avg_delay_sec.unwrap_or(0.0)),
        in_service_vehicles: Some(r.in_service_vehicles.unwrap_or(0)),
        peak_units_required: Some(mf_sim::ops::peak_units_required(s, r)),
        frequency: Some(frequency),
    }
}

/// Plain-language HUD cues. Port of `computeInsights` (sim.worker.ts).
fn compute_insights(s: &GameState) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut packed: Vec<&st::RouteDef> = s.routes.iter().filter(|r| r.crowding > 1.0).collect();
    packed.sort_by(|a, b| b.crowding.partial_cmp(&a.crowding).unwrap());
    if let Some(r) = packed.first() {
        out.push(format!(
            "{} is over capacity ({}%) and turning riders away. Add vehicles.",
            r.name,
            (r.crowding * 100.0).round()
        ));
    }
    let ld = &s.budget.last_day;
    if ld.fares > 0.0 && ld.fares < ld.operations + ld.maintenance {
        out.push(format!(
            "Fares cover only {}% of running costs.",
            ((ld.fares / (ld.operations + ld.maintenance)) * 100.0).round()
        ));
    }
    if s.stats.coverage < 0.35 && !s.stations.is_empty() {
        out.push(format!(
            "Only {}% of residents live near a stop. Extend your reach.",
            (s.stats.coverage * 100.0).round()
        ));
    }
    let has_gap = s.unserved.as_ref().map(|u| !u.is_empty()).unwrap_or(false);
    if has_gap && s.stats.transit_share < 0.4 {
        out.push(
            "Big travel demand is still driving. Check the Gaps overlay for where to build next."
                .to_string(),
        );
    }
    if let Some(a) = s.analytics.as_ref() {
        out.extend(mf_sim::analytics::analytics_insight_lines(&a.insights, 2));
    }
    for a in &s.active_events {
        if let Some(d) = mf_sim::events::event_by_id(&a.id) {
            out.push(format!("{}: {}", d.name, d.desc));
        }
    }
    out.truncate(4);
    out
}

/// Build the full [`wire::UiState`] from a [`GameState`]. Port of `buildUi`
/// (sim.worker.ts) + `uiExtras`. `speed` is the host's current speed multiplier;
/// `fields_version` is the host-tracked grid bump counter; `bankrupt` is the
/// host's sticky flag (mirrors the worker's `bankrupt || failed==='bankrupt'`).
pub fn build_ui_state(
    s: &GameState,
    speed: f64,
    fields_version: u32,
    bankrupt: bool,
) -> wire::UiState {
    use mf_sim::transit::time_of_day::{diurnal_factor, hour_of_day};
    let tod = diurnal_factor(s.tick);
    let day = (s.tick / mf_sim::constants::TICKS_PER_DAY as u64) as u32 + 1;

    let stations = s
        .stations
        .iter()
        .map(|st| wire::UiStation {
            id: st.id as i64,
            name: st.name.clone(),
            x: st.pos.x,
            y: st.pos.y,
            mode: mode_to_wire(st.mode),
            level: st.level,
            ridership: st.ridership,
            alightings: st.alightings,
        })
        .collect();

    let tracks = s
        .tracks
        .iter()
        .map(|t| wire::UiTrack {
            id: t.id as i64,
            mode: mode_to_wire(t.mode),
            grade: grade_str(t.grade),
            points: t.polyline.points.iter().flat_map(|p| [p.x, p.y]).collect(),
            from_station_id: t.from_station_id as i64,
            to_station_id: t.to_station_id as i64,
        })
        .collect();

    let routes = s.routes.iter().map(|r| build_route(s, r, tod)).collect();

    let active_events = s
        .active_events
        .iter()
        .map(|a| wire::ActiveEventDto {
            id: a.id.clone(),
            name: mf_sim::events::event_by_id(&a.id)
                .map(|d| d.name.to_string())
                .unwrap_or_else(|| a.id.clone()),
            days_left: a.days_left,
        })
        .collect();

    let districts = s
        .districts
        .iter()
        .map(|d| wire::UiDistrict {
            id: d.id as i64,
            name: d.name.clone(),
            x: d.centroid.x,
            y: d.centroid.y,
            population: d.population,
            jobs: d.jobs,
            growth_delta: d.last_growth_delta,
        })
        .collect();

    let last_day = wire::DayLedger {
        fares: s.budget.last_day.fares,
        subsidy: s.budget.last_day.subsidy,
        operations: s.budget.last_day.operations,
        maintenance: s.budget.last_day.maintenance,
        interest: s.budget.last_day.interest,
    };

    let lifetime = s
        .budget
        .lifetime
        .as_ref()
        .map(|l| wire::types::UiLifetimeLedger {
            fares: l.fares,
            subsidy: l.subsidy,
            operations: l.operations,
            maintenance: l.maintenance,
            interest: l.interest,
            days: l.days as f64,
        });

    // v0.9 ops summaries.
    let fs = mf_sim::ops::fleet_summary(s);
    let fleet = Some(wire::UiFleetSummary {
        total: fs.total as u32,
        active: fs.active as u32,
        maintenance: fs.maintenance as u32,
        broken_down: fs.broken_down as u32,
        avg_condition: fs.avg_condition,
        avg_age_days: fs.avg_age_days,
    });
    let depots = s
        .depots
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|d| wire::UiDepot {
            id: d.id as i64,
            mode: mode_str(d.mode),
            x: d.pos.x,
            y: d.pos.y,
        })
        .collect();
    let incidents = s
        .incidents
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|i| wire::UiIncident {
            id: i.id as i64,
            route_id: i.route_id as i64,
            ticks_left: i.ticks_left,
        })
        .collect();
    let period = s.ops_period.unwrap_or(st::Period::Midday);

    // failure surfacing: `condition` has no wire FailReason, so it reads as a
    // generic non-bankrupt fail through `failed=None` while the run is over.
    let failed = s.failed.and_then(fail_to_wire);
    let is_bankrupt = bankrupt || s.failed == Some(st::FailReason::Bankrupt);

    let rules = s.scenario_rules.as_ref();

    wire::UiState {
        tick: s.tick,
        insights: compute_insights(s),
        day,
        speed,
        cash: s.budget.cash,
        loan_balance: s.budget.loan_balance,
        last_day,
        net_history: s.budget.net_history.clone(),
        population: s.stats.population,
        approval: s.stats.approval,
        transit_share: s.stats.transit_share,
        coverage: s.stats.coverage,
        daily_transit_trips: s.stats.daily_transit_trips,
        unlocked_modes: s.unlocked_modes.iter().map(|&m| mode_to_wire(m)).collect(),
        stations,
        tracks,
        routes,
        active_events,
        fields_version,
        bankrupt: is_bankrupt,
        failed,
        max_day: rules.and_then(|r| r.max_day),
        era_label: rules.and_then(|r| r.era_label.clone()),
        command_count: s.command_log.len() as u32,
        // uiExtras (v0.4.2 sim-depth)
        hour_of_day: Some(hour_of_day(s.tick)),
        demand_factor: Some(tod),
        farebox_recovery: Some(mf_sim::scenario::evaluate::farebox_recovery(
            &s.budget.last_day,
        )),
        lifetime,
        districts,
        overcrowded_routes: Some(s.routes.iter().filter(|r| r.crowding > 1.0).count() as u32),
        // weather (v0.7)
        weather_state: s.weather.as_ref().map(|w| weather_state_to_wire(w.state)),
        weather_intensity: s.weather.as_ref().map(|w| w.intensity),
        weather_season: s.weather.as_ref().map(|w| season_to_wire(w.season)),
        weather_event: s
            .weather
            .as_ref()
            .and_then(|w| w.event)
            .map(weather_event_to_wire),
        // cohort demand-by-hour summary: the `cohorts` helper set (cohort_mix /
        // hourly_demand_curve) is not ported to Rust yet, so this additive field
        // stays `None`. TODO(P5): port `transit/cohorts.ts` HUD helpers.
        cohort_demand: None,
        // ops (v0.9 System A)
        fleet,
        depots,
        incidents,
        service_period: Some(period_str(period)),
        service_period_label: Some(mf_sim::ops::periods::period_label(period).to_string()),
    }
}

// ── FrameSnapshot ────────────────────────────────────────────────────────────

/// Parse a `#rrggbb` CSS color into packed `0x00RRGGBB`. Unknown formats fall
/// back to white so a bad color never drops a vehicle.
fn parse_hex_color(s: &str) -> u32 {
    let hex = s.trim_start_matches('#');
    if hex.len() >= 6 {
        if let Ok(v) = u32::from_str_radix(&hex[..6], 16) {
            return v & 0x00ff_ffff;
        }
    }
    0x00ff_ffff
}

/// Build the binary `FrameSnapshot` (msgType=1): vehicles stride-6, a route
/// color table indexed by `routeColorIdx`, and (for now) zero agents. Port of
/// `sendFrame` (sim.worker.ts).
///
/// NOTE(P5): agents are the host-side pedestrian particle pool
/// (`sim/src/host/agents.ts`, resampled from `state.flows`), NOT sim state.
/// That cosmetic layer is not ported yet, so `agent_count = 0`. Vehicles (the
/// gameplay-relevant markers) are fully built.
pub fn build_frame(s: &GameState) -> wire::FrameSnapshot {
    // route id -> color index (position in `routes`), mirroring the TS map.
    let mut route_index: std::collections::HashMap<u32, usize> =
        std::collections::HashMap::with_capacity(s.routes.len());
    let mut color_table: Vec<u32> = Vec::with_capacity(s.routes.len());
    for (i, r) in s.routes.iter().enumerate() {
        route_index.insert(r.id, i);
        color_table.push(parse_hex_color(&r.color));
    }

    let mut vehicles: Vec<f32> = Vec::with_capacity(s.vehicles.len() * 6);
    let mut n: u32 = 0;
    for v in &s.vehicles {
        let Some(route) = s.routes.iter().find(|r| r.id == v.route_id) else {
            continue;
        };
        let Some(path) = mf_sim::transit::route_path::get_route_path(s, route) else {
            continue;
        };
        let (pos, heading) = mf_sim::geometry::point_along(&path, v.along);
        let idx = route_index.get(&v.route_id).copied().unwrap_or(0);
        vehicles.push(v.id as f32);
        vehicles.push(pos.x as f32);
        vehicles.push(pos.y as f32);
        vehicles.push(heading as f32);
        vehicles.push(v.occupancy as f32);
        vehicles.push(idx as f32);
        n += 1;
    }

    wire::FrameSnapshot {
        tick: s.tick as u32,
        vehicle_count: n,
        agent_count: 0,
        color_table,
        vehicles,
        agents: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_sim::types::Difficulty;
    use mf_sim::{apply_command, new_game, sim_tick, NewGameOptions, SimCommand};

    /// Build a 2-station bus route on real road points (so the bus road-snap
    /// always succeeds and the two stops clear the min-spacing rule). Returns
    /// the created route's station ids.
    fn scripted_bus_route(s: &mut GameState) -> (u32, u32) {
        // Collect road points, then pick a pair 400..3000 m apart.
        let pts: Vec<mf_sim::geometry::Vec2> = s
            .roads
            .iter()
            .flat_map(|r| r.polyline.points.iter().copied())
            .collect();
        let p0 = pts[0];
        let p1 = *pts
            .iter()
            .find(|p| {
                let d = mf_sim::geometry::dist(p0, **p);
                (400.0..3000.0).contains(&d)
            })
            .expect("a second road point 400..3000 m away exists");
        let a = apply_command(
            s,
            &SimCommand::BuildStation {
                mode: st::TransitMode::Bus,
                pos: p0,
            },
        );
        let b = apply_command(
            s,
            &SimCommand::BuildStation {
                mode: st::TransitMode::Bus,
                pos: p1,
            },
        );
        assert!(a.ok && b.ok, "stations built: {:?} {:?}", a.error, b.error);
        let (sa, sb) = (a.created_id.unwrap(), b.created_id.unwrap());
        let bt = apply_command(
            s,
            &SimCommand::BuildTrack {
                mode: st::TransitMode::Bus,
                grade: st::TrackGrade::Surface,
                from_station_id: sa,
                to_station_id: sb,
                waypoints: vec![],
            },
        );
        assert!(bt.ok, "track built: {:?}", bt.error);
        let cr = apply_command(
            s,
            &SimCommand::CreateRoute {
                mode: st::TransitMode::Bus,
                station_ids: vec![sa, sb],
            },
        );
        assert!(cr.ok, "route created: {:?}", cr.error);
        (sa, sb)
    }

    /// new_game + a scripted 2-station bus route, then a few ticks, and assert
    /// the serializer produces a UiState with the expected route/station/ops
    /// fields.
    #[test]
    fn ui_state_reflects_a_scripted_network() {
        let mut s = new_game(12345, Difficulty::Normal, NewGameOptions::default());
        let _ = scripted_bus_route(&mut s);

        for _ in 0..50 {
            sim_tick(&mut s);
        }

        let ui = build_ui_state(&s, 30.0, 3, false);
        assert_eq!(ui.speed, 30.0);
        assert_eq!(ui.fields_version, 3);
        assert_eq!(ui.stations.len(), 2);
        assert_eq!(ui.routes.len(), 1);
        assert!(ui.population > 0.0);
        let r = &ui.routes[0];
        assert_eq!(r.station_ids.len(), 2);
        // v0.9 ops fields must be present (additive Some(..)).
        assert!(r.frequency.is_some(), "per-period frequency present");
        assert!(r.on_time_pct.is_some(), "on-time pct present");
        assert!(r.peak_units_required.is_some(), "peak units present");
        // fleet summary + service period always present post-v0.9.
        assert!(ui.fleet.is_some());
        assert!(ui.service_period.is_some());
        assert!(ui.service_period_label.is_some());
    }

    #[test]
    fn frame_snapshot_has_vehicles_and_color_table() {
        let mut s = new_game(999, Difficulty::Normal, NewGameOptions::default());
        let _ = scripted_bus_route(&mut s);
        for _ in 0..30 {
            sim_tick(&mut s);
        }
        let frame = build_frame(&s);
        assert_eq!(frame.tick, s.tick as u32);
        assert_eq!(frame.color_table.len(), s.routes.len());
        // stride-6 vehicle buffer, count consistent.
        assert_eq!(frame.vehicles.len(), frame.vehicle_count as usize * 6);
        assert_eq!(frame.agent_count, 0);
    }

    #[test]
    fn command_bridge_casts_ids_and_periods() {
        let c = wire::Command::SetRouteFrequency {
            route_id: 7,
            period: "amPeak".to_string(),
            headway_seconds: 300.0,
        };
        let sim = command_to_sim(&c).expect("known period");
        match sim {
            SimCommand::SetRouteFrequency {
                route_id, period, ..
            } => {
                assert_eq!(route_id, 7u32);
                assert_eq!(period, st::Period::AmPeak);
            }
            _ => panic!("wrong variant"),
        }
        // unknown period rejected.
        assert!(command_to_sim(&wire::Command::SetRouteFrequency {
            route_id: 1,
            period: "bogus".to_string(),
            headway_seconds: 300.0,
        })
        .is_none());
    }
}
