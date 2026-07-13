//! Headless replay runner. Port of `sim/src/core/replay.ts`.

use crate::city::osm::OsmCityData;
use crate::city::presets::MapSize;
use crate::new_game::{new_game, NewGameOptions};
use crate::types::{Difficulty, FailReason, GameState, ScenarioDef, ScenarioRules};
use crate::{apply_command, sim_tick, state_hash};

/// Replay input payload.
#[derive(Clone, Debug)]
pub struct ReplayInput {
    pub seed: u32,
    pub difficulty: Difficulty,
    pub size: Option<MapSize>,
    pub preset_key: Option<String>,
    pub rules: Option<ScenarioRules>,
    /// Commands stamped with the tick they were issued at.
    pub command_log: Vec<crate::types::CommandLogEntry>,
    /// Advance sim to at least this tick after commands.
    pub final_tick: Option<u64>,
    /// Optional preloaded OSM city bundle for real-city replays.
    pub osm: Option<OsmCityData>,
}

/// Replay output.
#[derive(Clone, Debug)]
pub struct ReplayResult {
    pub state: GameState,
    pub hash: u64,
    pub failed: Option<FailReason>,
}

/// Replay a command stream deterministically.
pub fn replay_sync(input: ReplayInput) -> ReplayResult {
    let scenario = input
        .rules
        .as_ref()
        .and_then(|r| r.scenario_id.clone())
        .map(|id| ScenarioDef { id });
    let mut state = new_game(
        input.seed,
        input.difficulty,
        NewGameOptions {
            size: input.size,
            preset_key: input.preset_key,
            rules: input.rules,
            osm: input.osm,
            scenario,
        },
    );
    let mut log = input.command_log;
    log.sort_by_key(|e| e.tick);
    let mut i = 0usize;
    let target = std::cmp::max(
        input.final_tick.unwrap_or(0),
        log.last().map(|c| c.tick).unwrap_or(0),
    );

    while state.tick < target || i < log.len() {
        while i < log.len() && log[i].tick <= state.tick {
            let cmd = log[i].cmd.clone();
            apply_command(&mut state, &cmd);
            i += 1;
        }
        if state.tick >= target && i >= log.len() {
            break;
        }
        if state.failed.is_some() {
            break;
        }
        sim_tick(&mut state);
        if state.tick > target.saturating_add(1) && i >= log.len() {
            break;
        }
    }
    while i < log.len() {
        let cmd = log[i].cmd.clone();
        apply_command(&mut state, &cmd);
        i += 1;
    }

    ReplayResult {
        hash: state_hash(&state),
        failed: state.failed,
        state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TransitMode;
    use crate::SimCommand;

    #[test]
    fn replay_matches_direct_run_without_commands() {
        let mut direct = new_game(5, Difficulty::Normal, NewGameOptions::default());
        for _ in 0..120 {
            sim_tick(&mut direct);
        }
        let replayed = replay_sync(ReplayInput {
            seed: 5,
            difficulty: Difficulty::Normal,
            size: None,
            preset_key: None,
            rules: None,
            command_log: Vec::new(),
            final_tick: Some(120),
            osm: None,
        });
        assert_eq!(replayed.hash, direct.state_hash());
    }

    #[test]
    fn replay_applies_stamped_commands_in_order() {
        let mut state = new_game(7, Difficulty::Normal, NewGameOptions::default());
        // Build two stations at tick 0 and connect at tick 1.
        let c1 = SimCommand::BuildStation {
            mode: TransitMode::Bus,
            pos: crate::geometry::Vec2 { x: 0.0, y: 0.0 },
        };
        let c2 = SimCommand::BuildStation {
            mode: TransitMode::Bus,
            pos: crate::geometry::Vec2 { x: 600.0, y: 0.0 },
        };
        let r1 = apply_command(&mut state, &c1);
        let r2 = apply_command(&mut state, &c2);
        assert!(r1.ok && r2.ok);
        sim_tick(&mut state);
        let c3 = SimCommand::BuildTrack {
            mode: TransitMode::Bus,
            grade: crate::types::TrackGrade::Surface,
            from_station_id: r1.created_id.unwrap(),
            to_station_id: r2.created_id.unwrap(),
            waypoints: Vec::new(),
        };
        let r3 = apply_command(&mut state, &c3);
        assert!(r3.ok);
        let log = state.command_log.clone();
        let replayed = replay_sync(ReplayInput {
            seed: 7,
            difficulty: Difficulty::Normal,
            size: None,
            preset_key: None,
            rules: None,
            command_log: log,
            final_tick: Some(state.tick),
            osm: None,
        });
        assert_eq!(replayed.hash, state.state_hash());
    }
}
