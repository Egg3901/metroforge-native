//! Focused-route highlight for the routes panel editor.
//!
//! Lives in `mf-state` (not `mf-game`) so `mf-render` can dim non-focused
//! route stripes the same way it dims the network under a demand overlay —
//! without depending on the game shell.

use bevy_ecs::prelude::*;

/// Which route (if any) the routes panel has selected for in-world highlight.
/// When `route_id` is `Some`, `mf-render` keeps that route's stripe at full
/// strength and mixes every other stripe toward white (same math as the
/// demand-overlay dim). `editing` is a soft hint for gizmos (stop numbers /
/// stronger chevrons); dimming itself only keys off `route_id`.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RouteFocus {
    /// Selected route id, or `None` when nothing is focused.
    pub route_id: Option<i64>,
    /// Soft hint that the routes panel is in edit mode (gizmos only).
    pub editing: bool,
}

impl RouteFocus {
    /// Drop focus and leave edit mode.
    pub fn clear(&mut self) {
        self.route_id = None;
        self.editing = false;
    }

    pub fn focus(&mut self, route_id: i64, editing: bool) {
        self.route_id = Some(route_id);
        self.editing = editing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_drops_focus_and_editing() {
        let mut focus = RouteFocus {
            route_id: Some(7),
            editing: true,
        };
        focus.clear();
        assert_eq!(focus.route_id, None);
        assert!(!focus.editing);
    }

    #[test]
    fn focus_sets_both_fields() {
        let mut focus = RouteFocus::default();
        focus.focus(3, true);
        assert_eq!(focus.route_id, Some(3));
        assert!(focus.editing);
        focus.focus(9, false);
        assert_eq!(focus.route_id, Some(9));
        assert!(!focus.editing);
    }
}
