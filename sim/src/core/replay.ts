/**
 * Headless replay: (seed, difficulty, rules, commandLog) → final state.
 * Used by golden tests and client-side score verification before submit.
 */
import { applyCommand } from './commands';
import { loadOsmCity } from './city/osmRegistry';
import { newGame, type NewGameOptions } from './newGame';
import { setBankruptDays, simTick } from './sim';
import { stateHash } from './save';
import type { Command, Difficulty, GameState } from './types';
import type { ScenarioRules } from './scenarioRules';

export interface LoggedCommand {
  tick: number;
  cmd: Command;
}

export interface ReplayInput {
  seed: number;
  difficulty: Difficulty;
  size?: NewGameOptions['size'];
  presetKey?: string;
  rules?: ScenarioRules;
  /** commands stamped with the tick they were issued (before the command applied) */
  commandLog: LoggedCommand[];
  /** advance the sim to at least this tick after applying all commands */
  finalTick?: number;
}

export interface ReplayResult {
  state: GameState;
  hash: number;
  failed: GameState['failed'];
}

/**
 * Replay a command stream. OSM cities must be preloaded (or pass osm on opts)
 * because the registry is async — callers that already have osm should pass it.
 */
export function replaySync(input: ReplayInput & { osm?: NewGameOptions['osm'] }): ReplayResult {
  setBankruptDays(0);
  const state = newGame(input.seed, input.difficulty, {
    size: input.size,
    presetKey: input.presetKey,
    osm: input.osm,
    rules: input.rules,
  });
  const log = [...input.commandLog].sort((a, b) => a.tick - b.tick);
  let i = 0;
  const target = Math.max(input.finalTick ?? 0, log.length ? log[log.length - 1]!.tick : 0);
  // Apply commands at their stamped tick: advance sim to cmd.tick, then apply.
  while (state.tick < target || i < log.length) {
    while (i < log.length && log[i]!.tick <= state.tick) {
      applyCommand(state, log[i]!.cmd);
      i++;
    }
    if (state.tick >= target && i >= log.length) break;
    if (state.failed) break;
    simTick(state);
    // safety: don't spin forever on empty logs with huge finalTick in tests
    if (state.tick > target + 1 && i >= log.length) break;
  }
  // drain any commands stamped at/after the final tick
  while (i < log.length) {
    applyCommand(state, log[i]!.cmd);
    i++;
  }
  return { state, hash: stateHash(state), failed: state.failed };
}

/** Async wrapper that loads OSM data when a real-city preset is used. */
export async function replay(input: ReplayInput): Promise<ReplayResult> {
  const osm = input.presetKey ? await loadOsmCity(input.presetKey) : undefined;
  return replaySync({ ...input, osm });
}
