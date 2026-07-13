//! Transit domain (P3 lane A): the routable road graph, cohort demand, the
//! gravity + Dijkstra + logit assignment, the traffic overlay, grade effects,
//! route geometry, and the station/track/route build command logic.
//!
//! Port of `sim/src/core/transit/*` plus the build-command bodies from
//! `sim/src/core/commands.ts` that P1 left stubbed. Delivered as standalone
//! module functions; the integration owner wires them into the tick orchestrator
//! and the `apply_command` dispatch (both frozen for lane isolation).

pub mod assignment;
pub mod build;
pub mod cohorts;
pub mod grade_effects;
pub mod road_graph;
pub mod route_path;
pub mod time_of_day;
pub mod traffic;
