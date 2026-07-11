//! Persistent client config (spec §3.4 `config.rs`): a `config.toml` under
//! the OS config dir (`directories::ProjectDirs("com","[REDACTED]",
//! "MetroForge")`), holding a quality-tier override, a theme override
//! (issue #32), and the weather-effects toggle. Auto-detection (spec §4) is
//! used whenever no quality override is set; `Theme::Light` is used whenever
//! no theme override is set. Either override always wins over its default.

use bevy::prelude::*;
use mf_state::{QualityTier, Theme};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// TOML-serializable mirror of [`QualityTier`] — kept local to `mf-game` so
/// `mf-state` doesn't need a `serde` dependency for the sake of one config
/// file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigQuality {
    Potato,
    Low,
    Medium,
    High,
}

impl From<ConfigQuality> for QualityTier {
    fn from(q: ConfigQuality) -> Self {
        match q {
            ConfigQuality::Potato => QualityTier::Potato,
            ConfigQuality::Low => QualityTier::Low,
            ConfigQuality::Medium => QualityTier::Medium,
            ConfigQuality::High => QualityTier::High,
        }
    }
}

impl From<QualityTier> for ConfigQuality {
    fn from(q: QualityTier) -> Self {
        match q {
            QualityTier::Potato => ConfigQuality::Potato,
            QualityTier::Low => ConfigQuality::Low,
            QualityTier::Medium => ConfigQuality::Medium,
            QualityTier::High => ConfigQuality::High,
        }
    }
}

/// TOML-serializable mirror of [`Theme`] — same rationale as
/// [`ConfigQuality`] above (keeps `serde` out of `mf-state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigTheme {
    Light,
    Dark,
    Purple,
}

impl From<ConfigTheme> for Theme {
    fn from(t: ConfigTheme) -> Self {
        match t {
            ConfigTheme::Light => Theme::Light,
            ConfigTheme::Dark => Theme::Dark,
            ConfigTheme::Purple => Theme::Purple,
        }
    }
}

impl From<Theme> for ConfigTheme {
    fn from(t: Theme) -> Self {
        match t {
            Theme::Light => ConfigTheme::Light,
            Theme::Dark => ConfigTheme::Dark,
            Theme::Purple => ConfigTheme::Purple,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quality_override: Option<ConfigQuality>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    theme_override: Option<ConfigTheme>,
    /// Whether the first-launch tutorial (`tutorial.rs`) has been completed
    /// or skipped. `false` (the default for a missing key / fresh install)
    /// arms the flow on the next city load; `true` suppresses it. A plain
    /// bool rather than an `Option` since "never seen it" and "explicitly
    /// not done" are the same state to the flow.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    tutorial_completed: bool,
    /// Scrolling fog/cloud weather. Defaults to on when absent so existing
    /// config.toml files keep the new Medium+ atmosphere without an edit.
    #[serde(default = "default_weather_effects")]
    weather_effects: bool,
    /// Whether the bottom-right HUD minimap (`minimap.rs`) is expanded.
    /// Defaults to on so existing config.toml files (which predate the
    /// minimap) still show it without an edit, same rationale as
    /// `weather_effects` above.
    #[serde(default = "default_minimap_open")]
    minimap_open: bool,
}

fn default_weather_effects() -> bool {
    true
}

fn default_minimap_open() -> bool {
    true
}

impl Default for ConfigFile {
    fn default() -> Self {
        ConfigFile {
            quality_override: None,
            theme_override: None,
            tutorial_completed: false,
            weather_effects: true,
            minimap_open: true,
        }
    }
}

/// Loaded/persisted client config. A Bevy `Resource` so `hud.rs`'s quality,
/// theme, and weather selectors can read/write it directly.
#[derive(Resource, Debug, Clone)]
pub struct MfConfig {
    pub quality_override: Option<QualityTier>,
    pub theme_override: Option<Theme>,
    /// Whether the first-launch tutorial has been completed or skipped (see
    /// `tutorial.rs`). Read at `InGame` entry to decide whether to arm the
    /// flow; the "Replay tutorial" setting clears it back to `false`.
    pub tutorial_completed: bool,
    /// Player preference for atmospheric weather (fog/cloud). Still gated
    /// by quality tier at render time.
    pub weather_effects: bool,
    /// Whether the HUD minimap (`minimap.rs`) is expanded. `M` toggles the
    /// top-down map mode (`map_mode.rs`), so the minimap claims `N` instead
    /// (verified unclaimed by grep before wiring it up, same convention
    /// `map_mode.rs`'s module doc uses for `M`).
    pub minimap_open: bool,
    path: Option<PathBuf>,
}

impl Default for MfConfig {
    fn default() -> Self {
        MfConfig {
            quality_override: None,
            theme_override: None,
            tutorial_completed: false,
            weather_effects: true,
            minimap_open: true,
            path: None,
        }
    }
}

impl MfConfig {
    fn config_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("com", "[REDACTED]", "MetroForge")
            .map(|dirs| dirs.config_dir().join("config.toml"))
    }

    /// Load from disk, falling back to defaults (no override) if the file is
    /// absent or unreadable — a missing config is not an error.
    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            tracing::warn!(
                "mf-game: no config dir available on this platform; quality/theme overrides disabled"
            );
            return MfConfig::default();
        };
        let parsed = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| toml::from_str::<ConfigFile>(&s).ok());
        let quality_override = parsed
            .as_ref()
            .and_then(|f| f.quality_override)
            .map(QualityTier::from);
        let theme_override = parsed
            .as_ref()
            .and_then(|f| f.theme_override)
            .map(Theme::from);
        let tutorial_completed = parsed
            .as_ref()
            .map(|f| f.tutorial_completed)
            .unwrap_or(false);
        let weather_effects = parsed.as_ref().map(|f| f.weather_effects).unwrap_or(true);
        let minimap_open = parsed.as_ref().map(|f| f.minimap_open).unwrap_or(true);
        MfConfig {
            quality_override,
            theme_override,
            tutorial_completed,
            weather_effects,
            minimap_open,
            path: Some(path),
        }
    }

    /// Persist the current overrides (or their absence) back to
    /// `config.toml`.
    pub fn save(&self) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            anyhow::bail!("no config path resolved for this platform");
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = ConfigFile {
            quality_override: self.quality_override.map(ConfigQuality::from),
            theme_override: self.theme_override.map(ConfigTheme::from),
            tutorial_completed: self.tutorial_completed,
            weather_effects: self.weather_effects,
            minimap_open: self.minimap_open,
        };
        let toml_str = toml::to_string_pretty(&file)?;
        std::fs::write(path, toml_str)?;
        Ok(())
    }

    pub fn set_quality_override(&mut self, quality: Option<QualityTier>) {
        self.quality_override = quality;
        if let Err(e) = self.save() {
            tracing::warn!("mf-game: failed to persist config.toml: {e}");
        }
    }

    pub fn set_theme_override(&mut self, theme: Option<Theme>) {
        self.theme_override = theme;
        if let Err(e) = self.save() {
            tracing::warn!("mf-game: failed to persist config.toml: {e}");
        }
    }

    /// Persist whether the first-launch tutorial is done. `true` on
    /// completion/skip suppresses the flow; the "Replay tutorial" setting
    /// passes `false` to re-arm it on the next city load.
    pub fn set_tutorial_completed(&mut self, completed: bool) {
        self.tutorial_completed = completed;
        if let Err(e) = self.save() {
            tracing::warn!("mf-game: failed to persist config.toml: {e}");
        }
    }

    pub fn set_weather_effects(&mut self, enabled: bool) {
        self.weather_effects = enabled;
        if let Err(e) = self.save() {
            tracing::warn!("mf-game: failed to persist config.toml: {e}");
        }
    }

    /// Persist the minimap's collapsed/expanded state (`N` toggle, see
    /// `minimap.rs`).
    pub fn set_minimap_open(&mut self, open: bool) {
        self.minimap_open = open;
        if let Err(e) = self.save() {
            tracing::warn!("mf-game: failed to persist config.toml: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_quality_roundtrips_through_toml() {
        let file = ConfigFile {
            quality_override: Some(ConfigQuality::High),
            theme_override: None,
            tutorial_completed: false,
            weather_effects: true,
            minimap_open: true,
        };
        let s = toml::to_string_pretty(&file).unwrap();
        assert!(s.contains("high"));
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert_eq!(back.quality_override, Some(ConfigQuality::High));
    }

    #[test]
    fn missing_override_serializes_weather_default() {
        let file = ConfigFile {
            quality_override: None,
            theme_override: None,
            tutorial_completed: false,
            weather_effects: true,
            minimap_open: true,
        };
        let s = toml::to_string_pretty(&file).unwrap();
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert_eq!(back.quality_override, None);
        assert_eq!(back.theme_override, None);
        assert!(back.weather_effects);
    }

    #[test]
    fn legacy_config_without_weather_defaults_on() {
        let back: ConfigFile = toml::from_str("quality_override = \"medium\"\n").unwrap();
        assert!(back.weather_effects);
        assert_eq!(back.quality_override, Some(ConfigQuality::Medium));
    }

    #[test]
    fn weather_effects_roundtrips_off() {
        let file = ConfigFile {
            quality_override: None,
            theme_override: None,
            tutorial_completed: false,
            weather_effects: false,
            minimap_open: true,
        };
        let s = toml::to_string_pretty(&file).unwrap();
        assert!(s.contains("false"));
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert!(!back.weather_effects);
    }

    #[test]
    fn config_theme_roundtrips_through_toml() {
        let file = ConfigFile {
            quality_override: None,
            theme_override: Some(ConfigTheme::Purple),
            tutorial_completed: false,
            weather_effects: true,
            minimap_open: true,
        };
        let s = toml::to_string_pretty(&file).unwrap();
        assert!(s.contains("purple"));
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert_eq!(back.theme_override, Some(ConfigTheme::Purple));
    }

    #[test]
    fn tutorial_completed_roundtrips_through_toml() {
        let file = ConfigFile {
            quality_override: None,
            theme_override: None,
            tutorial_completed: true,
            weather_effects: true,
            minimap_open: true,
        };
        let s = toml::to_string_pretty(&file).unwrap();
        assert!(s.contains("tutorial_completed"));
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert!(back.tutorial_completed);
    }

    #[test]
    fn tutorial_completed_defaults_false_and_is_omitted_when_unset() {
        let file = ConfigFile::default();
        let s = toml::to_string_pretty(&file).unwrap();
        // Skipped when false, so a fresh install writes no tutorial key.
        assert!(!s.contains("tutorial_completed"));
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert!(!back.tutorial_completed);
    }

    #[test]
    fn theme_conversion_roundtrips_every_variant() {
        for theme in Theme::ALL {
            let cfg: ConfigTheme = theme.into();
            let back: Theme = cfg.into();
            assert_eq!(theme, back);
        }
    }
}
