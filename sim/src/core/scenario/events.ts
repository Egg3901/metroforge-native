/**
 * Apply scripted mid-run scenario events. Deterministic: density rank is
 * computed from the live district list with a stable sort (pop+jobs desc,
 * then id asc) — never Math.random / Date.
 */
import type { GameState } from '../types';
import type { ScenarioDef, ScenarioEvent } from './types';

function districtsByDensity(state: GameState): GameState['districts'] {
  return [...state.districts].sort((a, b) => {
    const da = a.population + a.jobs;
    const db = b.population + b.jobs;
    if (db !== da) return db - da;
    return a.id - b.id;
  });
}

export interface ScenarioEventResult {
  messages: string[];
  toasts: { message: string; tone: 'info' | 'warn' | 'good' }[];
}

/** Fire any events scheduled for `day` that have not already fired. */
export function applyScenarioEvents(state: GameState, def: ScenarioDef, day: number): ScenarioEventResult {
  const out: ScenarioEventResult = { messages: [], toasts: [] };
  const events = def.events;
  if (!events || events.length === 0) return out;
  if (!state.firedScenarioEvents) state.firedScenarioEvents = [];

  for (const ev of events) {
    if (ev.day !== day) continue;
    if (state.firedScenarioEvents.includes(ev.id)) continue;
    state.firedScenarioEvents.push(ev.id);
    fireOne(state, ev, out);
  }
  return out;
}

function fireOne(state: GameState, ev: ScenarioEvent, out: ScenarioEventResult): void {
  switch (ev.kind) {
    case 'districtDemandMult': {
      const ranked = districtsByDensity(state);
      const d = ranked[ev.densityRank];
      if (!d) {
        out.messages.push(`Scenario event ${ev.id}: no district at density rank ${ev.densityRank}`);
        break;
      }
      if (!state.districtDemandMult) state.districtDemandMult = {};
      const prev = state.districtDemandMult[d.id] ?? 1;
      state.districtDemandMult[d.id] = prev * ev.mult;
      state.demandDirty = true;
      out.toasts.push({ message: ev.message, tone: 'info' });
      out.messages.push(ev.message);
      break;
    }
    case 'globalDemandMult': {
      state.globalDemandMult = ev.mult;
      state.globalDemandMultDaysLeft = ev.durationDays;
      state.demandDirty = true;
      out.toasts.push({ message: ev.message, tone: 'info' });
      out.messages.push(ev.message);
      break;
    }
    case 'cashDelta': {
      state.budget.cash += ev.amount;
      const tone = ev.amount >= 0 ? 'good' : 'warn';
      out.toasts.push({ message: ev.message, tone });
      out.messages.push(ev.message);
      break;
    }
  }
}

/** Tick down temporary global demand multipliers at day boundary. */
export function tickGlobalDemandMult(state: GameState): void {
  if (state.globalDemandMultDaysLeft === undefined) return;
  if (state.globalDemandMultDaysLeft <= 0) {
    delete state.globalDemandMult;
    delete state.globalDemandMultDaysLeft;
    state.demandDirty = true;
    return;
  }
  state.globalDemandMultDaysLeft -= 1;
  if (state.globalDemandMultDaysLeft <= 0) {
    delete state.globalDemandMult;
    delete state.globalDemandMultDaysLeft;
    state.demandDirty = true;
  }
}
