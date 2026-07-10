use bevy_ecs::prelude::*;

/// Which unserved-demand / travel-demand overlay is currently on, cycled
/// Off -> Demand -> Unserved -> Off by `mf-game`'s `overlays.rs` (`KeyCode::KeyG`).
/// Lives here rather than in `mf-game` so `mf-render` can read it too: when
/// an overlay is active the transit network's vivid route colors fade so
/// the overlay owns the stage (owner direction: "overlays should reduce the
/// color strength of our network"), and `mf-render` cannot depend on
/// `mf-game` for that. Same crate-split reason as [`crate::subway::SubwayView`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayMode {
    #[default]
    Off,
    /// Client-computed, network-independent gravity model over
    /// `LatestFields`' population/jobs grids: "where the city wants to go",
    /// everywhere, regardless of what's been built.
    Demand,
    /// The sim's own `DemandPayload` (`LatestDemand`): OD pairs the
    /// assignment engine found underserved by the CURRENT network — "trips
    /// being lost to cars right now".
    Unserved,
}

impl OverlayMode {
    /// Advances the cycle by one step, wrapping back to `Off`.
    pub fn next(self) -> OverlayMode {
        match self {
            OverlayMode::Off => OverlayMode::Demand,
            OverlayMode::Demand => OverlayMode::Unserved,
            OverlayMode::Unserved => OverlayMode::Off,
        }
    }
}

/// Overlay toggle state. A struct (not a bare enum `Resource`) so it can
/// grow sibling fields later (e.g. a toolbar-driven "pinned" flag) without
/// another crate-boundary move.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OverlayState {
    pub mode: OverlayMode,
}

impl OverlayState {
    /// Mirrors [`crate::subway::SubwayView::toggle`]: the (dumb) state
    /// transition lives on the resource itself, not scattered across every
    /// system that might want to advance it.
    pub fn cycle(&mut self) {
        self.mode = self.mode.next();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_wraps_off_demand_unserved_off() {
        let mut overlay = OverlayState::default();
        assert_eq!(overlay.mode, OverlayMode::Off);
        overlay.cycle();
        assert_eq!(overlay.mode, OverlayMode::Demand);
        overlay.cycle();
        assert_eq!(overlay.mode, OverlayMode::Unserved);
        overlay.cycle();
        assert_eq!(overlay.mode, OverlayMode::Off);
    }
}
