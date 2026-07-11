//! Colorblind palette-shift preference. A Bevy `Resource` shared the same
//! way [`crate::Theme`] is: `mf-game` owns the Settings selector and
//! `config.toml` persistence, `mf-render`'s `palette.rs` remaps the vivid
//! route table and mode accents when a shift is active.
//!
//! Kept free of any rendering types (mirrors `theme.rs`) so this crate
//! stays a light dependency.

use bevy_ecs::prelude::*;

/// Palette remapping for common forms of color vision deficiency. `Off`
/// keeps the theme's authored route/accent colors unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Resource, Default)]
pub enum ColorblindMode {
    /// No remapping: the theme's authored colors as-is.
    #[default]
    Off,
    /// Red-green (deuteranopia): green-weak / green-blind.
    Deuteranopia,
    /// Red-green (protanopia): red-weak / red-blind.
    Protanopia,
    /// Blue-yellow (tritanopia).
    Tritanopia,
}

impl ColorblindMode {
    /// Every mode, in the order the Settings selector lists them.
    pub const ALL: [ColorblindMode; 4] = [
        ColorblindMode::Off,
        ColorblindMode::Deuteranopia,
        ColorblindMode::Protanopia,
        ColorblindMode::Tritanopia,
    ];

    /// Stable config/serde key (`off`, `deuteranopia`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            ColorblindMode::Off => "off",
            ColorblindMode::Deuteranopia => "deuteranopia",
            ColorblindMode::Protanopia => "protanopia",
            ColorblindMode::Tritanopia => "tritanopia",
        }
    }

    /// Parse a config/serde key (accepts a few aliases); `None` if unknown.
    pub fn from_str_key(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "off" | "none" => Some(ColorblindMode::Off),
            "deuteranopia" | "deutan" => Some(ColorblindMode::Deuteranopia),
            "protanopia" | "protan" => Some(ColorblindMode::Protanopia),
            "tritanopia" | "tritan" => Some(ColorblindMode::Tritanopia),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_off() {
        assert_eq!(ColorblindMode::default(), ColorblindMode::Off);
    }

    #[test]
    fn roundtrips_known_keys() {
        for mode in ColorblindMode::ALL {
            assert_eq!(ColorblindMode::from_str_key(mode.as_str()), Some(mode));
        }
    }
}
