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
    #[default]
    Light,
    Dark,
    Purple,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::Light, Theme::Dark, Theme::Purple];

    pub fn label(self) -> &'static str {
        match self {
            Theme::Light => "Light",
            Theme::Dark => "Dark",
            Theme::Purple => "Purple",
        }
    }
}

/// Ordering anchor shared across crates for the one-shot boot resolution of
/// [`Theme`] (`config.toml` override beats `MF_THEME` env beats the `Light`
/// default). `mf-game`'s `theme_boot::resolve_theme_system` is the sole
/// writer and runs `.in_set(ThemeBootSet)`; `mf-render`'s
/// `sync_theme_system` (which publishes `Res<Theme>` into `palette.rs`'s
/// process-global atomic every static-geometry bake reads through) runs
/// `.after(ThemeBootSet)` and before any geometry bakes.
///
/// Without this, the two systems live in different crates/plugins with no
/// ordering relationship at all — correctness would depend on the default
/// scheduler's insertion-order tie-breaking rather than on a real
/// constraint, i.e. a latent bake-vs-boot-resolution race (see issue #44).
/// Living in `mf-state` (rather than either `mf-game` or `mf-render`) since
/// both of those crates already depend on this one and neither should have
/// to depend on the other just to order against this set.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThemeBootSet;

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
