//! Persistent client config (spec §3.4 `config.rs`): a `config.toml` under
//! the OS config dir (`directories::ProjectDirs("com","ahousedivided",
//! "MetroForge")`), holding a quality-tier override. Auto-detection (spec
//! §4) is used whenever no override is set; the override always wins.

use bevy::prelude::*;
use mf_state::QualityTier;
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quality_override: Option<ConfigQuality>,
}

/// Loaded/persisted client config. A Bevy `Resource` so `hud.rs`'s quality
/// selector can read/write it directly.
#[derive(Resource, Debug, Clone, Default)]
pub struct MfConfig {
    pub quality_override: Option<QualityTier>,
    path: Option<PathBuf>,
}

impl MfConfig {
    fn config_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("com", "ahousedivided", "MetroForge")
            .map(|dirs| dirs.config_dir().join("config.toml"))
    }

    /// Load from disk, falling back to defaults (no override) if the file is
    /// absent or unreadable — a missing config is not an error.
    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            tracing::warn!(
                "mf-game: no config dir available on this platform; quality override disabled"
            );
            return MfConfig::default();
        };
        let quality_override = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| toml::from_str::<ConfigFile>(&s).ok())
            .and_then(|f| f.quality_override)
            .map(QualityTier::from);
        MfConfig {
            quality_override,
            path: Some(path),
        }
    }

    /// Persist the current override (or its absence) back to `config.toml`.
    pub fn save(&self) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            anyhow::bail!("no config path resolved for this platform");
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = ConfigFile {
            quality_override: self.quality_override.map(ConfigQuality::from),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_quality_roundtrips_through_toml() {
        let file = ConfigFile {
            quality_override: Some(ConfigQuality::High),
        };
        let s = toml::to_string_pretty(&file).unwrap();
        assert!(s.contains("high"));
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert_eq!(back.quality_override, Some(ConfigQuality::High));
    }

    #[test]
    fn missing_override_serializes_to_empty_document() {
        let file = ConfigFile {
            quality_override: None,
        };
        let s = toml::to_string_pretty(&file).unwrap();
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert_eq!(back.quality_override, None);
    }
}
