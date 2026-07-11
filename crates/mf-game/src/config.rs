//! Persistent client config (spec §3.4 `config.rs`): a `config.toml` under
//! the OS config dir (see [`crate::paths`]), holding a quality-tier
//! override, a theme override (issue #32), the weather-effects toggle,
//! window chrome (size/position/borderless-fullscreen), graphics deltas,
//! and HUD prefs. Auto-detection (spec §4) is used whenever no quality
//! override is set; `Theme::Light` is used whenever no theme override is
//! set. Either override always wins over its default.
//!
//! Advanced graphics controls persist as **deltas** under `[graphics]`:
//! omitted keys mean "use the selected preset". Old config.toml files
//! without `[graphics]` keep parsing via serde defaults.

use bevy::prelude::*;
use mf_state::{QualityOverrides, QualityTier, ShadowQuality, Theme};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::paths;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ConfigShadowQuality {
    Off,
    Medium,
    High,
}

impl From<ConfigShadowQuality> for ShadowQuality {
    fn from(s: ConfigShadowQuality) -> Self {
        match s {
            ConfigShadowQuality::Off => ShadowQuality::Off,
            ConfigShadowQuality::Medium => ShadowQuality::Medium,
            ConfigShadowQuality::High => ShadowQuality::High,
        }
    }
}

impl From<ShadowQuality> for ConfigShadowQuality {
    fn from(s: ShadowQuality) -> Self {
        match s {
            ShadowQuality::Off => ConfigShadowQuality::Off,
            ShadowQuality::Medium => ConfigShadowQuality::Medium,
            ShadowQuality::High => ConfigShadowQuality::High,
        }
    }
}

/// Persisted Advanced graphics deltas. Every field is optional so old
/// configs and "use preset" both serialize as omitted keys.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct GraphicsOverridesFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shadows: Option<ConfigShadowQuality>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    draw_distance_m: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    trees: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fog: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volumetric_clouds: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outlines: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vsync: Option<bool>,
}

impl GraphicsOverridesFile {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn to_overrides(&self) -> QualityOverrides {
        QualityOverrides {
            shadows: self.shadows.map(ShadowQuality::from),
            draw_distance_m: self.draw_distance_m,
            trees: self.trees,
            fog: self.fog,
            volumetric_clouds: self.volumetric_clouds,
            outlines: self.outlines,
            vsync: self.vsync,
        }
    }

    fn from_overrides(o: &QualityOverrides) -> Self {
        GraphicsOverridesFile {
            shadows: o.shadows.map(ConfigShadowQuality::from),
            draw_distance_m: o.draw_distance_m,
            trees: o.trees,
            fog: o.fog,
            volumetric_clouds: o.volumetric_clouds,
            outlines: o.outlines,
            vsync: o.vsync,
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
    /// Still honored when `[graphics].volumetric_clouds` is unset; an
    /// explicit volumetric_clouds delta wins.
    #[serde(default = "default_weather_effects")]
    weather_effects: bool,
    /// Advanced graphics deltas on top of the selected quality preset.
    #[serde(default, skip_serializing_if = "GraphicsOverridesFile::is_empty")]
    graphics: GraphicsOverridesFile,
    /// On-screen FPS / frame-time counter. Off by default; omitted when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    show_fps: bool,
    /// Borderless-fullscreen preference (F11 / Alt+Enter). Defaults off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    borderless_fullscreen: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_height: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_y: Option<i32>,
    /// Autosave cadence in sim-days. `0` disables autosave. Defaults to
    /// [`crate::saves::DEFAULT_AUTOSAVE_INTERVAL_DAYS`] for legacy configs.
    #[serde(default = "default_autosave_interval_days")]
    autosave_interval_days: u32,
    /// Whether the bottom-right HUD minimap (`minimap.rs`) is expanded.
    /// Defaults to on so existing config.toml files (which predate the
    /// minimap) still show it without an edit, same rationale as
    /// `weather_effects` above.
    #[serde(default = "default_minimap_open")]
    minimap_open: bool,
    /// Master output gain in `[0, 1]`. Defaults to 1.0 for legacy configs
    /// that predate the audio settings row.
    #[serde(default = "default_master_volume")]
    master_volume: f32,
    /// When true, all procedural SFX and ambience are silent. Omitted from
    /// TOML when false (fresh-install default), same pattern as
    /// `tutorial_completed`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    mute: bool,
}

fn default_weather_effects() -> bool {
    true
}

fn default_autosave_interval_days() -> u32 {
    crate::saves::DEFAULT_AUTOSAVE_INTERVAL_DAYS
}

fn default_minimap_open() -> bool {
    true
}

fn default_master_volume() -> f32 {
    1.0
}

impl Default for ConfigFile {
    fn default() -> Self {
        ConfigFile {
            quality_override: None,
            theme_override: None,
            tutorial_completed: false,
            weather_effects: true,
            graphics: GraphicsOverridesFile::default(),
            show_fps: false,
            borderless_fullscreen: false,
            window_width: None,
            window_height: None,
            window_x: None,
            window_y: None,
            autosave_interval_days: default_autosave_interval_days(),
            minimap_open: true,
            master_volume: default_master_volume(),
            mute: false,
        }
    }
}

/// Loaded/persisted client config. A Bevy `Resource` so `hud.rs`'s quality,
/// theme, and graphics selectors can read/write it directly.
#[derive(Resource, Debug, Clone)]
pub struct MfConfig {
    pub quality_override: Option<QualityTier>,
    pub theme_override: Option<Theme>,
    /// Whether the first-launch tutorial has been completed or skipped (see
    /// `tutorial.rs`). Read at `InGame` entry to decide whether to arm the
    /// flow; the "Replay tutorial" setting clears it back to `false`.
    pub tutorial_completed: bool,
    /// Player preference for atmospheric weather (fog/cloud). Still gated
    /// by quality tier / graphics overrides at render time.
    pub weather_effects: bool,
    /// Advanced graphics deltas (mirrors `[graphics]` in config.toml).
    pub graphics: QualityOverrides,
    /// On-screen FPS counter toggle.
    pub show_fps: bool,
    /// Borderless-fullscreen toggle (persisted; applied at window create
    /// and when the player hits F11 / Alt+Enter).
    pub borderless_fullscreen: bool,
    /// Last windowed logical size / position. `None` means "use defaults /
    /// let the OS place the window".
    pub window_width: Option<f32>,
    pub window_height: Option<f32>,
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    /// Autosave every N sim-days (`0` = off). See
    /// [`crate::saves::DEFAULT_AUTOSAVE_INTERVAL_DAYS`].
    pub autosave_interval_days: u32,
    /// Whether the HUD minimap (`minimap.rs`) is expanded. `M` toggles the
    /// top-down map mode (`map_mode.rs`), so the minimap claims `N` instead
    /// (verified unclaimed by grep before wiring it up, same convention
    /// `map_mode.rs`'s module doc uses for `M`).
    pub minimap_open: bool,
    /// Master output gain in `[0, 1]` for procedural SFX + ambience.
    pub master_volume: f32,
    /// When true, all audio is silent regardless of `master_volume`.
    pub mute: bool,
    path: Option<PathBuf>,
}

impl Default for MfConfig {
    fn default() -> Self {
        MfConfig {
            quality_override: None,
            theme_override: None,
            tutorial_completed: false,
            weather_effects: true,
            graphics: QualityOverrides::default(),
            show_fps: false,
            borderless_fullscreen: false,
            window_width: None,
            window_height: None,
            window_x: None,
            window_y: None,
            autosave_interval_days: crate::saves::DEFAULT_AUTOSAVE_INTERVAL_DAYS,
            minimap_open: true,
            master_volume: 1.0,
            mute: false,
            path: None,
        }
    }
}

impl MfConfig {
    fn config_path() -> Option<PathBuf> {
        paths::config_toml_path()
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
        let file = parsed.unwrap_or_default();
        MfConfig {
            quality_override: file.quality_override.map(QualityTier::from),
            theme_override: file.theme_override.map(Theme::from),
            tutorial_completed: file.tutorial_completed,
            weather_effects: file.weather_effects,
            graphics: file.graphics.to_overrides(),
            show_fps: file.show_fps,
            borderless_fullscreen: file.borderless_fullscreen,
            window_width: file.window_width,
            window_height: file.window_height,
            window_x: file.window_x,
            window_y: file.window_y,
            autosave_interval_days: file.autosave_interval_days,
            minimap_open: file.minimap_open,
            master_volume: file.master_volume.clamp(0.0, 1.0),
            mute: file.mute,
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
            graphics: GraphicsOverridesFile::from_overrides(&self.graphics),
            show_fps: self.show_fps,
            borderless_fullscreen: self.borderless_fullscreen,
            window_width: self.window_width,
            window_height: self.window_height,
            window_x: self.window_x,
            window_y: self.window_y,
            autosave_interval_days: self.autosave_interval_days,
            minimap_open: self.minimap_open,
            master_volume: self.master_volume.clamp(0.0, 1.0),
            mute: self.mute,
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

    /// Persist the Advanced graphics deltas.
    pub fn set_graphics_overrides(&mut self, graphics: QualityOverrides) {
        self.graphics = graphics;
        if let Err(e) = self.save() {
            tracing::warn!("mf-game: failed to persist config.toml: {e}");
        }
    }

    pub fn set_borderless_fullscreen(&mut self, enabled: bool) {
        self.borderless_fullscreen = enabled;
        if let Err(e) = self.save() {
            tracing::warn!("mf-game: failed to persist config.toml: {e}");
        }
    }

    pub fn set_autosave_interval_days(&mut self, days: u32) {
        self.autosave_interval_days = days;
        if let Err(e) = self.save() {
            tracing::warn!("mf-game: failed to persist config.toml: {e}");
        }
    }

    pub fn set_show_fps(&mut self, show: bool) {
        self.show_fps = show;
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

    /// Persist master volume in `[0, 1]` (Settings slider).
    pub fn set_master_volume(&mut self, volume: f32) {
        self.master_volume = volume.clamp(0.0, 1.0);
        if let Err(e) = self.save() {
            tracing::warn!("mf-game: failed to persist config.toml: {e}");
        }
    }

    /// Persist mute (Settings checkbox). When muted, procedural SFX and
    /// ambience are silent regardless of `master_volume`.
    pub fn set_mute(&mut self, mute: bool) {
        self.mute = mute;
        if let Err(e) = self.save() {
            tracing::warn!("mf-game: failed to persist config.toml: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_file() -> ConfigFile {
        ConfigFile {
            quality_override: Some(ConfigQuality::High),
            theme_override: None,
            tutorial_completed: false,
            weather_effects: true,
            graphics: GraphicsOverridesFile::default(),
            show_fps: false,
            borderless_fullscreen: false,
            window_width: None,
            window_height: None,
            window_x: None,
            window_y: None,
            autosave_interval_days: 10,
            minimap_open: true,
            master_volume: 1.0,
            mute: false,
        }
    }

    #[test]
    fn config_quality_roundtrips_through_toml() {
        let file = sample_file();
        let s = toml::to_string_pretty(&file).unwrap();
        assert!(s.contains("high"));
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert_eq!(back.quality_override, Some(ConfigQuality::High));
    }

    #[test]
    fn missing_override_serializes_weather_default() {
        let file = ConfigFile::default();
        let s = toml::to_string_pretty(&file).unwrap();
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert_eq!(back.quality_override, None);
        assert_eq!(back.theme_override, None);
        assert!(back.weather_effects);
        assert!(back.graphics.is_empty());
        assert!(!back.show_fps);
        assert!(!back.borderless_fullscreen);
    }

    #[test]
    fn legacy_config_without_weather_defaults_on() {
        let back: ConfigFile = toml::from_str("quality_override = \"medium\"\n").unwrap();
        assert!(back.weather_effects);
        assert_eq!(back.quality_override, Some(ConfigQuality::Medium));
        assert!(back.graphics.is_empty());
        assert!(!back.show_fps);
        assert!(!back.borderless_fullscreen);
        assert_eq!(back.window_width, None);
        assert_eq!(back.autosave_interval_days, 10);
        assert!((back.master_volume - 1.0).abs() < f32::EPSILON);
        assert!(!back.mute);
    }

    #[test]
    fn weather_effects_roundtrips_off() {
        let file = ConfigFile {
            weather_effects: false,
            ..ConfigFile::default()
        };
        let s = toml::to_string_pretty(&file).unwrap();
        assert!(s.contains("false"));
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert!(!back.weather_effects);
    }

    #[test]
    fn config_theme_roundtrips_through_toml() {
        let file = ConfigFile {
            theme_override: Some(ConfigTheme::Purple),
            ..ConfigFile::default()
        };
        let s = toml::to_string_pretty(&file).unwrap();
        assert!(s.contains("purple"));
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert_eq!(back.theme_override, Some(ConfigTheme::Purple));
    }

    #[test]
    fn tutorial_completed_roundtrips_through_toml() {
        let file = ConfigFile {
            tutorial_completed: true,
            ..ConfigFile::default()
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
    fn window_geometry_and_fullscreen_roundtrip() {
        let file = ConfigFile {
            borderless_fullscreen: true,
            window_width: Some(1920.0),
            window_height: Some(1080.0),
            window_x: Some(100),
            window_y: Some(50),
            ..ConfigFile::default()
        };
        let s = toml::to_string_pretty(&file).unwrap();
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert!(back.borderless_fullscreen);
        assert_eq!(back.window_width, Some(1920.0));
        assert_eq!(back.window_height, Some(1080.0));
        assert_eq!(back.window_x, Some(100));
        assert_eq!(back.window_y, Some(50));
    }

    #[test]
    fn theme_conversion_roundtrips_every_variant() {
        for theme in Theme::ALL {
            let cfg: ConfigTheme = theme.into();
            let back: Theme = cfg.into();
            assert_eq!(theme, back);
        }
    }

    #[test]
    fn graphics_overrides_roundtrip_as_deltas() {
        let file = ConfigFile {
            quality_override: Some(ConfigQuality::Medium),
            theme_override: None,
            tutorial_completed: false,
            weather_effects: true,
            graphics: GraphicsOverridesFile {
                shadows: Some(ConfigShadowQuality::Off),
                draw_distance_m: Some(8_000.0),
                trees: Some(false),
                fog: Some(true),
                volumetric_clouds: Some(false),
                outlines: Some(true),
                vsync: Some(false),
            },
            show_fps: true,
            autosave_interval_days: 10,
            minimap_open: true,
        };
        let s = toml::to_string_pretty(&file).unwrap();
        assert!(s.contains("[graphics]"));
        assert!(s.contains("show_fps"));
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert_eq!(back.graphics.shadows, Some(ConfigShadowQuality::Off));
        assert_eq!(back.graphics.draw_distance_m, Some(8_000.0));
        assert_eq!(back.graphics.trees, Some(false));
        assert_eq!(back.graphics.fog, Some(true));
        assert_eq!(back.graphics.volumetric_clouds, Some(false));
        assert_eq!(back.graphics.outlines, Some(true));
        assert_eq!(back.graphics.vsync, Some(false));
        assert!(back.show_fps);
        let overrides = back.graphics.to_overrides();
        assert_eq!(overrides.shadows, Some(ShadowQuality::Off));
        assert_eq!(overrides.draw_distance_m, Some(8_000.0));
    }

    #[test]
    fn empty_graphics_section_omitted_on_serialize() {
        let file = ConfigFile::default();
        let s = toml::to_string_pretty(&file).unwrap();
        assert!(!s.contains("[graphics]"));
        assert!(!s.contains("show_fps"));
    }

    #[test]
    fn legacy_config_parses_without_graphics_keys() {
        let toml = r#"
quality_override = "low"
theme_override = "dark"
weather_effects = false
tutorial_completed = true
"#;
        let back: ConfigFile = toml::from_str(toml).unwrap();
        assert_eq!(back.quality_override, Some(ConfigQuality::Low));
        assert_eq!(back.theme_override, Some(ConfigTheme::Dark));
        assert!(!back.weather_effects);
        assert!(back.tutorial_completed);
        assert!(back.graphics.is_empty());
        assert!(!back.show_fps);
    }

    #[test]
    fn autosave_interval_roundtrips_and_defaults() {
        let file = ConfigFile {
            autosave_interval_days: 5,
            ..ConfigFile::default()
        };
        let s = toml::to_string_pretty(&file).unwrap();
        assert!(s.contains("5"));
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert_eq!(back.autosave_interval_days, 5);

        let legacy: ConfigFile = toml::from_str("weather_effects = false\n").unwrap();
        assert_eq!(legacy.autosave_interval_days, 10);
    }

    #[test]
    fn master_volume_and_mute_roundtrip_and_default() {
        let file = ConfigFile {
            master_volume: 0.35,
            mute: true,
            ..sample_file()
        };
        let s = toml::to_string_pretty(&file).unwrap();
        assert!(s.contains("master_volume"), "serialized:\n{s}");
        assert!(s.contains("mute"), "serialized:\n{s}");
        let back: ConfigFile = toml::from_str(&s).unwrap();
        assert!((back.master_volume - 0.35).abs() < 1e-5);
        assert!(back.mute);

        let legacy: ConfigFile = toml::from_str("weather_effects = false\n").unwrap();
        assert!((legacy.master_volume - 1.0).abs() < f32::EPSILON);
        assert!(!legacy.mute);

        let unmuted = sample_file();
        let s2 = toml::to_string_pretty(&unmuted).unwrap();
        assert!(!s2.contains("mute"));
    }
}
