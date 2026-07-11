//! Attract-mode flag shared between `mf-game` (sets it while the MainMenu
//! diorama is up) and `mf-render` (reads it to lock lighting). Lives here
//! for the same crate-split reason as [`crate::WeatherEffects`]: render
//! must not depend on the game shell.

use bevy_ecs::prelude::*;

/// When [`AttractLighting::active`] is true, day/night pins golden-hour
/// targets instead of following the sim clock — the title-screen diorama
/// stays warm and moody even while attract runs the sim at 30×.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AttractLighting {
    pub active: bool,
}
