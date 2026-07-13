/**
 * City events — periodic, seeded disruptions and boosts that make a running city
 * feel alive. Each event runs for a few days and nudges travel demand, approval,
 * and (sometimes) fare revenue. Deterministic: firing rolls come from the sim's
 * seeded RNG and active events are saved with the game.
 */

export interface EventDef {
  id: string;
  name: string;
  desc: string;
  /** how many days it lasts */
  days: number;
  /** travel-demand multiplier while active */
  demandMult: number;
  /** approval nudge per day while active */
  approval: number;
  /** fare-revenue multiplier while active (fare-free days) */
  fareMult: number;
  /** relative likelihood */
  weight: number;
  tone: 'good' | 'warn' | 'info';
}

export interface ActiveEvent {
  id: string;
  daysLeft: number;
}

export const EVENT_DEFS: EventDef[] = [
  { id: 'festival', name: 'City Festival', desc: 'Crowds pour downtown — ridership surges.', days: 3, demandMult: 1.35, approval: 3, fareMult: 1, weight: 3, tone: 'good' },
  { id: 'fuel', name: 'Fuel Price Spike', desc: 'Gas prices jump — commuters flock to transit.', days: 5, demandMult: 1.25, approval: 1, fareMult: 1, weight: 3, tone: 'good' },
  { id: 'roadclosure', name: 'Major Road Closure', desc: 'A key artery is shut — cars gridlock, transit shines.', days: 3, demandMult: 1.2, approval: 0, fareMult: 1, weight: 2, tone: 'info' },
  { id: 'boom', name: 'Downtown Boom', desc: 'A new development opens — fresh trips all week.', days: 6, demandMult: 1.2, approval: 1, fareMult: 1, weight: 2, tone: 'good' },
  { id: 'heatwave', name: 'Heat Wave', desc: 'Brutal heat — people stay home.', days: 2, demandMult: 0.82, approval: -1, fareMult: 1, weight: 2, tone: 'warn' },
  { id: 'shortage', name: 'Operator Shortage', desc: 'Staffing gaps disrupt service and patience.', days: 2, demandMult: 0.85, approval: -3, fareMult: 1, weight: 2, tone: 'warn' },
  { id: 'farefree', name: 'Fare-Free Week', desc: 'The mayor waives fares — approval soars, the farebox suffers.', days: 5, demandMult: 1.15, approval: 5, fareMult: 0, weight: 1, tone: 'good' },
];

const byId = (id: string): EventDef | undefined => EVENT_DEFS.find((e) => e.id === id);

export function eventDemandMult(active: ActiveEvent[]): number {
  let m = 1;
  for (const a of active) m *= byId(a.id)?.demandMult ?? 1;
  return m;
}
export function eventApprovalDelta(active: ActiveEvent[]): number {
  let s = 0;
  for (const a of active) s += byId(a.id)?.approval ?? 0;
  return s;
}
export function eventFareMult(active: ActiveEvent[]): number {
  let m = 1;
  for (const a of active) m *= byId(a.id)?.fareMult ?? 1;
  return m;
}

/** Weighted pick of a new event def (or null if the roll declines). */
export function rollEvent(pick: number): EventDef {
  const total = EVENT_DEFS.reduce((a, e) => a + e.weight, 0);
  let r = pick * total;
  for (const e of EVENT_DEFS) {
    r -= e.weight;
    if (r < 0) return e;
  }
  return EVENT_DEFS[0]!;
}
