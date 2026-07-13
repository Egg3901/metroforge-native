//! City events -- periodic, seeded disruptions and boosts.
//!
//! Port of `sim/src/core/events.ts`. Each event runs for a few days and nudges
//! travel demand, approval, and (sometimes) fare revenue. Deterministic: firing
//! rolls come from the sim's seeded RNG and active events are saved with the
//! game.
//!
//! NOTE: `ActiveEvent` here mirrors the TS `{ id, daysLeft }` shape used by the
//! aggregation helpers. The persisted `types::ActiveEvent` uses
//! `{ id, start_day, end_day }`; the orchestrator reconciles the two when
//! wiring the daily event pass. See the P3-ENV report note.

/// UI tone for an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventTone {
    /// Positive event.
    Good,
    /// Warning / negative event.
    Warn,
    /// Neutral / informational event.
    Info,
}

/// A city-event definition. Mirrors `EventDef`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EventDef {
    /// Stable id.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Flavor description.
    pub desc: &'static str,
    /// How many days it lasts.
    pub days: u32,
    /// Travel-demand multiplier while active.
    pub demand_mult: f64,
    /// Approval nudge per day while active.
    pub approval: f64,
    /// Fare-revenue multiplier while active.
    pub fare_mult: f64,
    /// Relative likelihood.
    pub weight: f64,
    /// UI tone.
    pub tone: EventTone,
}

/// A currently active event (mirrors the TS `ActiveEvent` `{ id, daysLeft }`).
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveEvent {
    /// Event id.
    pub id: String,
    /// Days remaining.
    pub days_left: u32,
}

/// The catalog of city-event definitions (order = the weighted-pick order).
pub const EVENT_DEFS: [EventDef; 7] = [
    EventDef {
        id: "festival",
        name: "City Festival",
        desc: "Crowds pour downtown -- ridership surges.",
        days: 3,
        demand_mult: 1.35,
        approval: 3.0,
        fare_mult: 1.0,
        weight: 3.0,
        tone: EventTone::Good,
    },
    EventDef {
        id: "fuel",
        name: "Fuel Price Spike",
        desc: "Gas prices jump -- commuters flock to transit.",
        days: 5,
        demand_mult: 1.25,
        approval: 1.0,
        fare_mult: 1.0,
        weight: 3.0,
        tone: EventTone::Good,
    },
    EventDef {
        id: "roadclosure",
        name: "Major Road Closure",
        desc: "A key artery is shut -- cars gridlock, transit shines.",
        days: 3,
        demand_mult: 1.2,
        approval: 0.0,
        fare_mult: 1.0,
        weight: 2.0,
        tone: EventTone::Info,
    },
    EventDef {
        id: "boom",
        name: "Downtown Boom",
        desc: "A new development opens -- fresh trips all week.",
        days: 6,
        demand_mult: 1.2,
        approval: 1.0,
        fare_mult: 1.0,
        weight: 2.0,
        tone: EventTone::Good,
    },
    EventDef {
        id: "heatwave",
        name: "Heat Wave",
        desc: "Brutal heat -- people stay home.",
        days: 2,
        demand_mult: 0.82,
        approval: -1.0,
        fare_mult: 1.0,
        weight: 2.0,
        tone: EventTone::Warn,
    },
    EventDef {
        id: "shortage",
        name: "Operator Shortage",
        desc: "Staffing gaps disrupt service and patience.",
        days: 2,
        demand_mult: 0.85,
        approval: -3.0,
        fare_mult: 1.0,
        weight: 2.0,
        tone: EventTone::Warn,
    },
    EventDef {
        id: "farefree",
        name: "Fare-Free Week",
        desc: "The mayor waives fares -- approval soars, the farebox suffers.",
        days: 5,
        demand_mult: 1.15,
        approval: 5.0,
        fare_mult: 0.0,
        weight: 1.0,
        tone: EventTone::Good,
    },
];

/// Look up an event def by id.
pub fn event_by_id(id: &str) -> Option<&'static EventDef> {
    EVENT_DEFS.iter().find(|e| e.id == id)
}

/// Combined travel-demand multiplier of all active events.
pub fn event_demand_mult(active: &[ActiveEvent]) -> f64 {
    active.iter().fold(1.0, |m, a| {
        m * event_by_id(&a.id).map_or(1.0, |e| e.demand_mult)
    })
}

/// Combined approval delta of all active events.
pub fn event_approval_delta(active: &[ActiveEvent]) -> f64 {
    active
        .iter()
        .map(|a| event_by_id(&a.id).map_or(0.0, |e| e.approval))
        .sum()
}

/// Combined fare multiplier of all active events.
pub fn event_fare_mult(active: &[ActiveEvent]) -> f64 {
    active.iter().fold(1.0, |m, a| {
        m * event_by_id(&a.id).map_or(1.0, |e| e.fare_mult)
    })
}

/// Weighted pick of a new event def from a roll in `[0,1)`.
pub fn roll_event(pick: f64) -> &'static EventDef {
    let total: f64 = EVENT_DEFS.iter().map(|e| e.weight).sum();
    let mut r = pick * total;
    for e in &EVENT_DEFS {
        r -= e.weight;
        if r < 0.0 {
            return e;
        }
    }
    &EVENT_DEFS[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipliers_combine() {
        let active = vec![
            ActiveEvent {
                id: "festival".into(),
                days_left: 2,
            },
            ActiveEvent {
                id: "farefree".into(),
                days_left: 4,
            },
        ];
        assert!((event_demand_mult(&active) - 1.35 * 1.15).abs() < 1e-9);
        assert!((event_approval_delta(&active) - 8.0).abs() < 1e-9);
        assert_eq!(event_fare_mult(&active), 0.0);
    }

    #[test]
    fn roll_covers_range_deterministically() {
        assert_eq!(roll_event(0.0).id, "festival");
        assert_eq!(roll_event(0.999).id, "farefree");
        // Same pick -> same event.
        assert_eq!(roll_event(0.5).id, roll_event(0.5).id);
    }

    #[test]
    fn unknown_id_is_neutral() {
        let active = vec![ActiveEvent {
            id: "nope".into(),
            days_left: 1,
        }];
        assert_eq!(event_demand_mult(&active), 1.0);
        assert_eq!(event_approval_delta(&active), 0.0);
        assert_eq!(event_fare_mult(&active), 1.0);
    }
}
