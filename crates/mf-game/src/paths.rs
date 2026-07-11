//! OS-standard data directories for config, saves, and crash reports.
//!
//! All client-persisted files go through [`directories::ProjectDirs`] with
//! qualifier/organization/application matching the rest of `mf-game`
//! (`"com"`, the MetroForge org id, `"MetroForge"`). On Windows that resolves to:
//!
//! | helper | Windows folder |
//! |---|---|
//! | [`config_dir`] | `%AppData%\Roaming\<org>\MetroForge` |
//! | [`data_dir`] | `%AppData%\Roaming\<org>\MetroForge` |
//! | [`data_local_dir`] | `%AppData%\Local\<org>\MetroForge` |
//!
//! (`ProjectDirs::from` returns `None` only when the home/profile directory
//! cannot be determined.) When that happens we fall back to a
//! `metroforge-userdata/` folder next to the running executable so a
//! portable zip install still has somewhere writable. Callers that already
//! treated a missing dir as "feature disabled" keep that behavior when even
//! the exe-adjacent fallback is unavailable (e.g. broken `current_exe`).

use std::path::PathBuf;

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "ahousedivided"; // pragma: allowlist secret
const APPLICATION: &str = "MetroForge";

/// Preferred project dirs from the `directories` crate, if the OS profile
/// path is available.
pub fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}

/// Next-to-exe fallback root used when [`project_dirs`] is `None`.
pub fn exe_fallback_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("metroforge-userdata"))
}

/// Roaming-style config directory (`config.toml`, `campaign.toml`, …).
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dirs) = project_dirs() {
        return Some(dirs.config_dir().to_path_buf());
    }
    Some(exe_fallback_root()?.join("config"))
}

/// Roaming-style data directory (save slots live under `saves/`).
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dirs) = project_dirs() {
        return Some(dirs.data_dir().to_path_buf());
    }
    Some(exe_fallback_root()?.join("data"))
}

/// Local (non-roaming) data directory — crash reports on Windows.
pub fn data_local_dir() -> Option<PathBuf> {
    if let Some(dirs) = project_dirs() {
        return Some(dirs.data_local_dir().to_path_buf());
    }
    Some(exe_fallback_root()?.join("local"))
}

pub fn config_toml_path() -> Option<PathBuf> {
    Some(config_dir()?.join("config.toml"))
}

pub fn saves_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("saves"))
}

pub fn crash_reports_dir() -> Option<PathBuf> {
    Some(data_local_dir()?.join("crash-reports"))
}

pub fn campaign_toml_path() -> Option<PathBuf> {
    Some(config_dir()?.join("campaign.toml"))
}

pub fn goals_toml_path() -> Option<PathBuf> {
    Some(config_dir()?.join("goals.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_dirs_or_fallback_always_yields_config_when_exe_resolves() {
        // On CI / developer machines ProjectDirs succeeds; the important
        // contract is that config_dir is Some whenever either source works.
        let cfg = config_dir();
        assert!(
            cfg.is_some(),
            "config_dir must resolve via ProjectDirs or exe fallback"
        );
        let path = cfg.unwrap();
        assert!(
            path.ends_with("MetroForge")
                || path.ends_with("config")
                || path.components().any(
                    |c| c.as_os_str() == "MetroForge" || c.as_os_str() == "metroforge-userdata"
                ),
            "unexpected config path layout: {}",
            path.display()
        );
    }

    #[test]
    fn saves_and_crash_dirs_nest_under_data_roots() {
        let saves = saves_dir().expect("saves_dir");
        assert!(saves.ends_with("saves"));
        let crashes = crash_reports_dir().expect("crash_reports_dir");
        assert!(crashes.ends_with("crash-reports"));
    }

    #[test]
    fn config_toml_and_sibling_tomls_share_config_dir() {
        let config = config_toml_path().expect("config");
        let campaign = campaign_toml_path().expect("campaign");
        let goals = goals_toml_path().expect("goals");
        assert_eq!(config.parent(), campaign.parent());
        assert_eq!(config.parent(), goals.parent());
        assert!(config.ends_with("config.toml"));
        assert!(campaign.ends_with("campaign.toml"));
        assert!(goals.ends_with("goals.toml"));
    }

    #[test]
    fn exe_fallback_root_is_next_to_current_exe() {
        let root = exe_fallback_root().expect("current_exe");
        assert!(root.ends_with("metroforge-userdata"));
        let exe = std::env::current_exe().unwrap();
        assert_eq!(root.parent(), exe.parent());
    }
}
