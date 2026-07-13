//! The command mutation API. Port of `sim/src/core/commands.ts`.
//!
//! Commands are the ONLY sanctioned way to mutate a [`GameState`] from outside
//! the tick loop. [`SimCommand`] mirrors the wire `Command` enum in
//! `mf-protocol` variant-for-variant (and the TS `Command` union) so a P4
//! bridge is a trivial `match`; the one representational difference is entity
//! ids, which are `u32` here (sim-internal) versus `i64` on the wire.
//!
//! # P1 scope
//!
//! Fully implemented: the pure state-edit commands (rename, loans, route
//! delete/edit-metadata, demolish, upgrade, per-period frequency). STUBBED with
//! a clear error + TODO: commands that need P2 worldgen (field water tests,
//! road-graph routing) or P3 systems (geology cost, ops fleet sync, headway
//! derivation). Those return `ok: false` rather than panicking so callers and
//! tests stay well-behaved until the systems land.

use crate::constants::{modes, REFUND_FRACTION};
use crate::geometry::Vec2;
use crate::types::{GameState, Period, TrackGrade, TransitMode};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// The command union. Mirrors `Command` (types.ts:212) and
/// `mf_protocol::Command`. Ids are sim-internal `u32`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(
        tag = "kind",
        rename_all = "camelCase",
        rename_all_fields = "camelCase"
    )
)]
#[derive(Clone, Debug, PartialEq)]
pub enum SimCommand {
    /// Place a new station. STUB (needs P2 field/road logic).
    BuildStation {
        /// Transit mode.
        mode: TransitMode,
        /// World position.
        pos: Vec2,
    },
    /// Build a track between two stations. STUB (needs P2/P3 routing + geology).
    BuildTrack {
        /// Transit mode.
        mode: TransitMode,
        /// Grade.
        grade: TrackGrade,
        /// From-station id.
        from_station_id: u32,
        /// To-station id.
        to_station_id: u32,
        /// Intermediate waypoints.
        waypoints: Vec<Vec2>,
    },
    /// Create a route through the given stations. STUB (needs P3 fleet sync).
    CreateRoute {
        /// Transit mode.
        mode: TransitMode,
        /// Ordered station ids.
        station_ids: Vec<u32>,
    },
    /// Edit mutable route properties (unset fields unchanged).
    EditRoute {
        /// Route id.
        route_id: u32,
        /// New headway (ignored: headway is derived from the fleet in P3).
        headway_seconds: Option<f64>,
        /// New fare.
        fare: Option<f64>,
        /// New vehicle count.
        vehicle_count: Option<u32>,
        /// New display name.
        name: Option<String>,
        /// New color.
        color: Option<String>,
    },
    /// Delete a route by id.
    DeleteRoute {
        /// Route id.
        route_id: u32,
    },
    /// Demolish a station by id.
    DemolishStation {
        /// Station id.
        station_id: u32,
    },
    /// Demolish a track by id.
    DemolishTrack {
        /// Track id.
        track_id: u32,
    },
    /// Upgrade a station's level.
    UpgradeStation {
        /// Station id.
        station_id: u32,
    },
    /// Take out a loan.
    TakeLoan {
        /// Principal to borrow.
        amount: f64,
    },
    /// Repay loan principal.
    RepayLoan {
        /// Principal to repay.
        amount: f64,
    },
    /// Rename a station.
    RenameStation {
        /// Station id.
        station_id: u32,
        /// New name.
        name: String,
    },
    /// Ops (v0.9 A1): set a route's target headway for one service period.
    SetRouteFrequency {
        /// Route id.
        route_id: u32,
        /// Service period.
        period: Period,
        /// Target headway, seconds.
        headway_seconds: f64,
    },
    /// Ops (v0.9 A4): place a maintenance depot. STUB (needs field water test +
    /// ops tunables).
    BuildDepot {
        /// Mode served.
        mode: TransitMode,
        /// World position.
        pos: Vec2,
    },
}

/// Outcome of applying a command. Mirrors `CommandResult` (types.ts:225).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandResult {
    /// Whether the command succeeded.
    pub ok: bool,
    /// Error message when `ok` is false.
    pub error: Option<String>,
    /// Id of a newly created entity, when applicable.
    pub created_id: Option<u32>,
}

impl CommandResult {
    fn ok() -> Self {
        CommandResult {
            ok: true,
            ..Default::default()
        }
    }
    // Used by the entity-creating commands once they land (buildStation /
    // buildTrack / createRoute / buildDepot are stubbed in P1).
    #[allow(dead_code)]
    fn created(id: u32) -> Self {
        CommandResult {
            ok: true,
            created_id: Some(id),
            ..Default::default()
        }
    }
    fn err(msg: &str) -> Self {
        CommandResult {
            ok: false,
            error: Some(msg.to_string()),
            ..Default::default()
        }
    }
}

/// Truncate to at most 40 characters. Mirrors `name.slice(0, 40)` (char-based
/// rather than UTF-16-code-unit-based; a documented, cosmetic-only difference).
fn clamp_name(s: &str) -> String {
    s.chars().take(40).collect()
}

/// Apply a command to the state. Mirrors `applyCommand`: rejects when the run
/// has already failed, and on success appends to the replay `command_log`.
pub fn apply_command(state: &mut GameState, cmd: &SimCommand) -> CommandResult {
    if state.failed.is_some() {
        return CommandResult::err("This run is over");
    }
    let result = apply_command_inner(state, cmd);
    if result.ok {
        state.command_log.push(crate::types::CommandLogEntry {
            tick: state.tick,
            cmd: cmd.clone(),
        });
    }
    result
}

fn apply_command_inner(state: &mut GameState, cmd: &SimCommand) -> CommandResult {
    match cmd {
        // ── STUBBED: need P2 worldgen / P3 systems (fields, road graph,
        //    geology cost, ops fleet sync). See commands.ts for the full body. ──
        SimCommand::BuildStation { .. } => {
            // TODO(P2/P3): port `buildStation` (needs isWaterAt, nearestRoadPoint,
            // stationCost, station push). commands.ts case 'buildStation'.
            CommandResult::err("buildStation not yet ported (P2/P3)")
        }
        SimCommand::BuildTrack { .. } => {
            // TODO(P2/P3): port `buildTrack` (road routing + trackCost + geology
            // depth surcharge). commands.ts case 'buildTrack'.
            CommandResult::err("buildTrack not yet ported (P2/P3)")
        }
        SimCommand::CreateRoute { .. } => {
            // TODO(P3): port `createRoute` (segment resolution + starter fleet +
            // syncVehicles + deriveHeadway). commands.ts case 'createRoute'.
            CommandResult::err("createRoute not yet ported (P3)")
        }
        SimCommand::BuildDepot { .. } => {
            // TODO(P3): port `buildDepot` (isWaterAt + opsTunables depotBuildCost).
            // commands.ts case 'buildDepot'.
            CommandResult::err("buildDepot not yet ported (P3)")
        }

        // ── Fully ported: pure state edits ──
        SimCommand::RenameStation { station_id, name } => {
            match state.stations.iter_mut().find(|s| s.id == *station_id) {
                Some(station) => {
                    station.name = clamp_name(name);
                    CommandResult::ok()
                }
                None => CommandResult::err("Station not found"),
            }
        }

        SimCommand::TakeLoan { amount } => {
            let amount = amount.max(0.0);
            let max_loan = 20_000_000.0;
            if state.budget.loan_balance + amount > max_loan {
                return CommandResult::err("Loan limit reached ($20M)");
            }
            state.budget.loan_balance += amount;
            state.budget.cash += amount;
            CommandResult::ok()
        }

        SimCommand::RepayLoan { amount } => {
            let amount = amount.min(state.budget.loan_balance).min(state.budget.cash);
            if amount <= 0.0 {
                return CommandResult::err("Nothing to repay");
            }
            state.budget.loan_balance -= amount;
            state.budget.cash -= amount;
            CommandResult::ok()
        }

        SimCommand::DeleteRoute { route_id } => {
            let Some(idx) = state.routes.iter().position(|r| r.id == *route_id) else {
                return CommandResult::err("Route not found");
            };
            let route = &state.routes[idx];
            state.budget.cash += route.vehicle_count as f64 * modes(route.mode).vehicle_cost * 0.4;
            state.routes.remove(idx);
            state.vehicles.retain(|v| v.route_id != *route_id);
            if let Some(fleet) = state.fleet.as_mut() {
                fleet.retain(|u| u.route_id != Some(*route_id));
            }
            if let Some(incidents) = state.incidents.as_mut() {
                incidents.retain(|i| i.route_id != *route_id);
            }
            state.demand_dirty = true;
            CommandResult::ok()
        }

        SimCommand::DemolishStation { station_id } => {
            let Some(idx) = state.stations.iter().position(|s| s.id == *station_id) else {
                return CommandResult::err("Station not found");
            };
            if state
                .routes
                .iter()
                .any(|r| r.station_ids.contains(station_id))
            {
                return CommandResult::err("Remove routes serving this station first");
            }
            if state
                .tracks
                .iter()
                .any(|t| t.from_station_id == *station_id || t.to_station_id == *station_id)
            {
                return CommandResult::err("Demolish connected tracks first");
            }
            let station_mode = state.stations[idx].mode;
            state.budget.cash += modes(station_mode).station_cost * REFUND_FRACTION;
            state.stations.remove(idx);
            state.demand_dirty = true;
            CommandResult::ok()
        }

        SimCommand::DemolishTrack { track_id } => {
            let Some(idx) = state.tracks.iter().position(|t| t.id == *track_id) else {
                return CommandResult::err("Track not found");
            };
            if state
                .routes
                .iter()
                .any(|r| r.segment_ids.contains(track_id))
            {
                return CommandResult::err("Remove routes using this track first");
            }
            state.budget.cash += state.tracks[idx].build_cost * REFUND_FRACTION;
            state.tracks.remove(idx);
            state.demand_dirty = true;
            CommandResult::ok()
        }

        SimCommand::UpgradeStation { station_id } => {
            let Some(station) = state.stations.iter_mut().find(|s| s.id == *station_id) else {
                return CommandResult::err("Station not found");
            };
            if station.level >= 5 {
                return CommandResult::err("Station already at max level");
            }
            let cost = modes(station.mode).station_cost * 0.5 * station.level as f64;
            if state.budget.cash < cost {
                return CommandResult::err("Insufficient funds");
            }
            station.level += 1;
            state.budget.cash -= cost;
            state.demand_dirty = true;
            CommandResult::ok()
        }

        SimCommand::EditRoute {
            route_id,
            fare,
            vehicle_count,
            name,
            color,
            headway_seconds: _,
        } => {
            let Some(route_idx) = state.routes.iter().position(|r| r.id == *route_id) else {
                return CommandResult::err("Route not found");
            };
            let mode = state.routes[route_idx].mode;
            let cfg = modes(mode);
            // fare / name / color: pure edits (fully ported)
            if let Some(f) = fare {
                state.routes[route_idx].fare = f.clamp(0.0, 10.0);
            }
            if let Some(n) = name {
                state.routes[route_idx].name = clamp_name(n);
            }
            if let Some(c) = color {
                state.routes[route_idx].color = c.clone();
            }
            // vehicle count: cash accounting is pure and fully ported here.
            if let Some(vc) = vehicle_count {
                let target = (*vc).min(40);
                let current = state.routes[route_idx].vehicle_count;
                if target > current {
                    let cost = (target - current) as f64 * cfg.vehicle_cost;
                    if state.budget.cash < cost {
                        return CommandResult::err("Insufficient funds for vehicles");
                    }
                    state.budget.cash -= cost;
                } else if target < current {
                    state.budget.cash += (current - target) as f64 * cfg.vehicle_cost * 0.4;
                }
                state.routes[route_idx].vehicle_count = target;
                // TODO(P3): syncVehicles + syncFleetForRoute (needs geometry +
                // ops). commands.ts case 'editRoute'.
            }
            // TODO(P3): route.headwaySeconds = deriveHeadway(...) (needs
            // routeCycleSeconds / gradeEffects). Left unchanged until P3.
            state.demand_dirty = true;
            CommandResult::ok()
        }

        SimCommand::SetRouteFrequency {
            route_id,
            period,
            headway_seconds,
        } => {
            let Some(route) = state.routes.iter_mut().find(|r| r.id == *route_id) else {
                return CommandResult::err("Route not found");
            };
            let cfg = modes(route.mode);
            let clamped = headway_seconds
                .round()
                .clamp(cfg.min_headway, crate::constants::MAX_HEADWAY);
            route
                .frequency
                .get_or_insert_with(Default::default)
                .insert(*period, clamped);
            state.demand_dirty = true;
            CommandResult::ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Difficulty, RouteDef, Station};

    fn base_state() -> GameState {
        GameState::with_difficulty(1, Difficulty::Normal)
    }

    fn push_station(state: &mut GameState, id: u32, name: &str) {
        state.stations.push(Station {
            id,
            name: name.to_string(),
            pos: Vec2 { x: 0.0, y: 0.0 },
            mode: TransitMode::Bus,
            level: 1,
            ridership: 0.0,
            alightings: 0.0,
            build_tick: 0,
            depth: None,
        });
    }

    #[test]
    fn rename_station_truncates_and_logs() {
        let mut s = base_state();
        push_station(&mut s, 10, "Old");
        let long = "x".repeat(60);
        let r = apply_command(
            &mut s,
            &SimCommand::RenameStation {
                station_id: 10,
                name: long,
            },
        );
        assert!(r.ok);
        assert_eq!(s.stations[0].name.chars().count(), 40);
        assert_eq!(s.command_log.len(), 1);
    }

    #[test]
    fn loan_take_and_repay() {
        let mut s = base_state();
        let cash0 = s.budget.cash;
        assert!(
            apply_command(
                &mut s,
                &SimCommand::TakeLoan {
                    amount: 1_000_000.0
                }
            )
            .ok
        );
        assert_eq!(s.budget.cash, cash0 + 1_000_000.0);
        assert_eq!(s.budget.loan_balance, 1_000_000.0);
        assert!(apply_command(&mut s, &SimCommand::RepayLoan { amount: 400_000.0 }).ok);
        assert_eq!(s.budget.loan_balance, 600_000.0);
    }

    #[test]
    fn loan_limit_enforced() {
        let mut s = base_state();
        let r = apply_command(
            &mut s,
            &SimCommand::TakeLoan {
                amount: 25_000_000.0,
            },
        );
        assert!(!r.ok);
    }

    #[test]
    fn upgrade_station_costs_and_levels() {
        let mut s = base_state();
        push_station(&mut s, 5, "A");
        let r = apply_command(&mut s, &SimCommand::UpgradeStation { station_id: 5 });
        assert!(r.ok);
        assert_eq!(s.stations[0].level, 2);
    }

    #[test]
    fn stubbed_build_station_errors_not_panics() {
        let mut s = base_state();
        let r = apply_command(
            &mut s,
            &SimCommand::BuildStation {
                mode: TransitMode::Bus,
                pos: Vec2 { x: 0.0, y: 0.0 },
            },
        );
        assert!(!r.ok);
    }

    #[test]
    fn set_route_frequency_clamps() {
        let mut s = base_state();
        s.routes.push(RouteDef {
            id: 1,
            name: "R".into(),
            color: "#fff".into(),
            mode: TransitMode::Bus,
            station_ids: vec![],
            segment_ids: vec![],
            headway_seconds: 600.0,
            fare: 2.5,
            vehicle_count: 0,
            daily_ridership: 0.0,
            daily_revenue: 0.0,
            capacity: 0.0,
            load: 0.0,
            crowding: 0.0,
            segment_loads: vec![],
            surface_exposure: None,
            move_grade_speed: None,
            frequency: None,
            scheduled_headway: None,
            in_service_vehicles: None,
            on_time_pct: None,
            avg_delay_sec: None,
            reliability_demand_mult: None,
        });
        // below bus min_headway (120) -> clamped up to 120
        let r = apply_command(
            &mut s,
            &SimCommand::SetRouteFrequency {
                route_id: 1,
                period: Period::AmPeak,
                headway_seconds: 10.0,
            },
        );
        assert!(r.ok);
        assert_eq!(
            *s.routes[0]
                .frequency
                .as_ref()
                .unwrap()
                .get(&Period::AmPeak)
                .unwrap(),
            120.0
        );
    }
}
