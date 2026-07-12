/**
 * Operating-cost / fare economy helpers. Pure, deterministic, and shared by the
 * daily economy pass (sim.ts) and the UI layer so the number a player sees is
 * the same number the ledger charges. No RNG / Date.
 */
import { MODES } from './constants';
import type { DayLedger, TransitMode } from './types';

/** Daily running cost of a route's fleet: crews + fuel + rolling-stock upkeep,
 *  per vehicle. (Track & station upkeep is charged separately, per km / level.) */
export function routeOperatingCost(mode: TransitMode, vehicleCount: number): number {
  const cfg = MODES[mode];
  return vehicleCount * (cfg.opsPerVehiclePerDay + cfg.maintPerVehiclePerDay);
}

/** Farebox recovery ratio: fare revenue / running costs (operations +
 *  maintenance). 0 when there are no running costs; above 1 the network pays
 *  its own way before any subsidy. */
export function fareboxRecovery(ledger: DayLedger): number {
  const running = ledger.operations + ledger.maintenance;
  return running > 0 ? ledger.fares / running : 0;
}
