//! Visual theme selection (issue #32). A `Theme` is a Bevy `Resource` shared
//! the same way [`crate::QualityTier`] is: `mf-game` owns the HUD selector
//! and `config.toml` persistence, `mf-render`'s `palette.rs` is the single
//! consumer that turns a tier into actual colors.
//!
//! Kept free of any rendering types (mirrors `quality.rs`'s rationale) so
//! this crate stays a light dependency.

use bevy_ecs::prelude::*;

/// Which visual theme is active. `Light` is the original/default Mirror's
/// Edge white-city look (art-direction.md, unchanged); `Dark` promotes the
/// existing night rig to a standing theme (near-black city, glowing
/// transit) rather than a time-of-day state; `Purple` is a violet/vaporwave
/// palette variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Resource, Default)]
pub enum Theme {
    /// Default Mirror's Edge white-city look.
    #[default]
    Light,
    /// Standing near-black city with glowing transit (not a time-of-day state).
    Dark,
    /// Violet / vaporwave palette variant.
    Purple,
}

impl Theme {
    /// Every theme in HUD combo order.
    pub const ALL: [Theme; 3] = [Theme::Light, Theme::Dark, Theme::Purple];

    /// Player-facing label for combo boxes.
    pub fn label(self) -> &'static str {
        match self {
            Theme::Light => "Light",
            Theme::Dark => "Dark",
            Theme::Purple => "Purple",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_is_light() {
        assert_eq!(Theme::default(), Theme::Light);
    }

    #[test]
    fn all_covers_every_variant_in_declared_order() {
        assert_eq!(Theme::ALL, [Theme::Light, Theme::Dark, Theme::Purple]);
    }
}
