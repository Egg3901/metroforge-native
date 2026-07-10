//! One-shot theme resolution at boot (issue #32), mirroring
//! `quality_boot.rs`'s precedence chain: `config.toml` override beats
//! `MF_THEME` beats the `Theme` resource's own `Light` default. Resolves
//! exactly once, then gets out of the way — from that point on `hud.rs`'s
//! theme selector owns the `Theme` resource, and this module must never
//! write to it again or the two would fight every time the player picks a
//! theme from the HUD.

use bevy::prelude::*;
use mf_state::Theme;

use crate::config::MfConfig;

pub struct MfThemeBootPlugin;

impl Plugin for MfThemeBootPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, resolve_theme_system.in_set(mf_state::ThemeBootSet));
    }
}

/// Runs every `Update` until it resolves (config isn't guaranteed to have
/// landed by the first tick — see `quality_boot.rs`'s identical rationale),
/// then no-ops forever via the `done` latch.
fn resolve_theme_system(
    mut done: Local<bool>,
    mut env_invalid_warned: Local<bool>,
    mut theme: ResMut<Theme>,
    config: Option<Res<MfConfig>>,
) {
    if *done {
        return;
    }

    // `MfConfig` is inserted by `state.rs`'s `boot_system` on `OnEnter(Boot)`,
    // which should land before this system's first `Update` tick, but there
    // is no hard guarantee of that ordering, so wait rather than assume.
    let Some(config) = config else {
        return;
    };

    if let Some(t) = config.theme_override {
        resolve(&mut theme, t, "config.toml override");
        *done = true;
        return;
    }

    if let Ok(raw) = std::env::var("MF_THEME") {
        match parse_mf_theme_env(&raw) {
            Some(t) => {
                resolve(&mut theme, t, "MF_THEME env var");
                *done = true;
                return;
            }
            None => {
                if !*env_invalid_warned {
                    tracing::warn!(
                        "mf-game: MF_THEME={raw:?} is not light, dark, or purple; ignoring it"
                    );
                    *env_invalid_warned = true;
                }
                // Fall through to the Light default this same pass instead
                // of re-checking the same bad env var every frame forever.
            }
        }
    }

    // No override present anywhere: `Theme`'s own `Light` default already
    // holds (nothing to write), just latch done.
    *done = true;
}

fn resolve(theme: &mut Theme, value: Theme, source: &str) {
    *theme = value;
    tracing::info!("mf-game: theme resolved to {value:?} via {source}");
}

fn parse_mf_theme_env(raw: &str) -> Option<Theme> {
    match raw.trim().to_lowercase().as_str() {
        "light" => Some(Theme::Light),
        "dark" => Some(Theme::Dark),
        "purple" => Some(Theme::Purple),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_env_values_case_insensitively() {
        assert_eq!(parse_mf_theme_env("Light"), Some(Theme::Light));
        assert_eq!(parse_mf_theme_env("DARK"), Some(Theme::Dark));
        assert_eq!(parse_mf_theme_env("Purple"), Some(Theme::Purple));
    }

    #[test]
    fn rejects_unknown_env_values() {
        assert_eq!(parse_mf_theme_env("vaporwave"), None);
        assert_eq!(parse_mf_theme_env(""), None);
    }
}
