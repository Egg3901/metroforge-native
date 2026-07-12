/**
 * Per-game instance ids. A long-lived process (the Bun sidecar, a worker) runs
 * many games back to back; entity ids reset to 1 each `newGame`, so any process-
 * global memo keyed by a bare entity id would collide across games and leak one
 * city's geometry into another — breaking the (seed, commands) -> state contract
 * that replays and leaderboards depend on. Every GameState gets a unique
 * `instanceId` from this counter so those caches can be scoped to their game.
 *
 * The id is transient: it is never serialized and never enters `stateHash`, so
 * it does not affect the deterministic fingerprint.
 */
let seq = 0;

export function nextInstanceId(): number {
  return ++seq;
}
