//! One-shot quality-tier resolution at boot (spec §4): `--safe-mode` beats
//! `config.toml` override beats `MF_QUALITY` beats GPU auto-detect beats the
//! `QualityTier` resource's own `Medium` default. Resolves exactly once,
//! then gets out of the way — from that point on `hud.rs`'s quality
//! selector owns the `QualityTier` resource, and this module must never
//! write to it again or the two would fight every time the player picks a
//! tier from the HUD.
//!
//! Auto-detect (env / GPU) is always latched into [`DetectedQuality`] when
//! known, including the config-override path (GPU detect still runs so
//! Settings "Auto" can re-apply it). Vsync follows [`EffectiveKnobs`] so
//! Advanced overrides apply live.
//!
//! `bevy::render::renderer::RenderAdapterInfo` (wrapping `wgpu::AdapterInfo`)
//! is inserted into the *main* world by `RenderPlugin::finish`, which Bevy
//! runs before the app's first `Update`, so in practice it is already
//! present by the time this system first runs. It is read as an
//! `Option<Res<_>>` and retried on a later frame rather than assumed,
//! since "already present in practice" is exactly the kind of engine
//! internal that can shift between Bevy point releases without notice.

use bevy::prelude::*;
use bevy::render::renderer::RenderAdapterInfo;
use bevy::window::{PresentMode, PrimaryWindow, Window};
use mf_state::{detect_quality_tier, DetectedQuality, EffectiveKnobs, GpuDeviceKind, QualityTier};

use crate::config::MfConfig;
use crate::crash::{self, SafeMode};

pub struct MfQualityBootPlugin;

impl Plugin for MfQualityBootPlugin {
    fn build(&self, app: &mut App) {
        // resolve → sync_effective_knobs → apply_vsync so the first frame
        // after tier resolution already sees merged knobs (including vsync).
        app.add_systems(
            Update,
            (
                record_adapter_for_crash_system,
                resolve_quality_system.before(mf_state::KnobSyncSet),
                apply_vsync_system.after(mf_state::KnobSyncSet),
            ),
        );
    }
}

/// Stash GPU adapter details for the panic hook as soon as Bevy exposes them,
/// independent of which quality source wins (including `--safe-mode`).
fn record_adapter_for_crash_system(
    mut done: Local<bool>,
    adapter_info: Option<Res<RenderAdapterInfo>>,
) {
    if *done {
        return;
    }
    let Some(adapter_info) = adapter_info else {
        return;
    };
    let kind = map_device_kind(adapter_info.device_type);
    crash::record_gpu_adapter(&adapter_info.name, kind);
    *done = true;
}

/// Runs every `Update` until it resolves (config / env / GPU detect can each
/// need to wait on a resource that isn't there yet), then no-ops forever via
/// the `done` latch.
fn resolve_quality_system(
    mut done: Local<bool>,
    mut env_invalid_warned: Local<bool>,
    mut quality: ResMut<QualityTier>,
    mut detected: ResMut<DetectedQuality>,
    safe_mode: Res<SafeMode>,
    config: Option<Res<MfConfig>>,
    adapter_info: Option<Res<RenderAdapterInfo>>,
) {
    if *done {
        return;
    }

    // `--safe-mode` / crash-dialog Safe mode: Potato wins over every other
    // source so a bad GPU/driver combo can boot without weather/bloom/outlines.
    if safe_mode.0 {
        resolve(&mut quality, QualityTier::Potato, "--safe-mode");
        *done = true;
        return;
    }

    // `MfConfig` is inserted in `main` before the window is created (so
    // size/position/fullscreen apply on first frame). Still read as
    // `Option` and retried in case plugin ordering ever shifts.
    let Some(config) = config else {
        return;
    };

    // Config override: still wait for the adapter so DetectedQuality gets
    // the GPU auto-detect value, then apply the override to QualityTier.
    if let Some(override_tier) = config.quality_override {
        let Some(adapter_info) = adapter_info else {
            return; // RenderPlugin hasn't finished initializing yet; retry next frame
        };
        let kind = map_device_kind(adapter_info.device_type);
        let gpu_tier = detect_quality_tier(&adapter_info.name, kind);
        detected.0 = gpu_tier;
        resolve(&mut quality, override_tier, "config.toml override");
        *done = true;
        return;
    }

    if let Ok(raw) = std::env::var("MF_QUALITY") {
        match parse_mf_quality_env(&raw) {
            Some(tier) => {
                detected.0 = tier;
                resolve(&mut quality, tier, "MF_QUALITY env var");
                *done = true;
                return;
            }
            None => {
                if !*env_invalid_warned {
                    tracing::warn!(
                        "mf-game: MF_QUALITY={raw:?} is not potato, low, medium, or high; ignoring it"
                    );
                    *env_invalid_warned = true;
                }
                // Fall through to GPU auto-detect this same pass instead of
                // re-checking the same bad env var every frame forever.
            }
        }
    }

    let Some(adapter_info) = adapter_info else {
        return; // RenderPlugin hasn't finished initializing yet; retry next frame
    };
    let kind = map_device_kind(adapter_info.device_type);
    let tier = detect_quality_tier(&adapter_info.name, kind);
    detected.0 = tier;
    resolve(
        &mut quality,
        tier,
        &format!(
            "GPU auto-detect (adapter {:?}, kind {kind:?})",
            adapter_info.name
        ),
    );
    *done = true;
}

fn resolve(quality: &mut QualityTier, tier: QualityTier, source: &str) {
    *quality = tier;
    tracing::info!("mf-game: quality tier resolved to {tier:?} via {source}");
}

fn parse_mf_quality_env(raw: &str) -> Option<QualityTier> {
    match raw.trim().to_lowercase().as_str() {
        "potato" => Some(QualityTier::Potato),
        "low" => Some(QualityTier::Low),
        "medium" => Some(QualityTier::Medium),
        "high" => Some(QualityTier::High),
        _ => None,
    }
}

/// `RenderAdapterInfo` derefs to `wgpu::AdapterInfo`, but `mf-game` doesn't
/// take a direct `wgpu` dependency of its own (mirroring `mf-state`'s design
/// of keeping `wgpu`/`bevy_render` types out except at the boundary Bevy
/// itself already provides), so this maps off `Debug`'s stable derived
/// variant names rather than importing `wgpu::DeviceType` for one match.
fn map_device_kind(device_type: impl std::fmt::Debug) -> GpuDeviceKind {
    match format!("{device_type:?}").as_str() {
        "DiscreteGpu" => GpuDeviceKind::Discrete,
        "IntegratedGpu" => GpuDeviceKind::Integrated,
        "Cpu" => GpuDeviceKind::Cpu,
        _ => GpuDeviceKind::Other, // VirtualGpu, Other, or any future wgpu variant
    }
}

/// Applies the `vsync` knob whenever effective knobs change, including
/// this module's own initial resolution and any later HUD / Advanced
/// override pick — `EffectiveKnobs` is a plain `Res` read here.
fn apply_vsync_system(
    effective: Res<EffectiveKnobs>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if !effective.is_changed() {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    window.present_mode = if effective.0.vsync {
        PresentMode::AutoVsync
    } else {
        PresentMode::AutoNoVsync
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_env_values_case_insensitively() {
        assert_eq!(parse_mf_quality_env("Potato"), Some(QualityTier::Potato));
        assert_eq!(parse_mf_quality_env("LOW"), Some(QualityTier::Low));
        assert_eq!(parse_mf_quality_env("Medium"), Some(QualityTier::Medium));
        assert_eq!(parse_mf_quality_env("high"), Some(QualityTier::High));
    }

    #[test]
    fn rejects_unknown_env_values() {
        assert_eq!(parse_mf_quality_env("ultra"), None);
        assert_eq!(parse_mf_quality_env(""), None);
    }

    // Mirrors `wgpu::DeviceType`'s variant names exactly (not the real type,
    // since `mf-game` deliberately avoids a direct `wgpu` dependency — see
    // `map_device_kind`'s doc comment) so the derived `Debug` output this
    // test feeds in matches what `map_device_kind` sees in [REDACTED].
    #[derive(Debug)]
    enum FakeWgpuDeviceType {
        Other,
        IntegratedGpu,
        DiscreteGpu,
        VirtualGpu,
        Cpu,
    }

    #[test]
    fn maps_device_kind_from_debug_names() {
        assert_eq!(
            map_device_kind(FakeWgpuDeviceType::DiscreteGpu),
            GpuDeviceKind::Discrete
        );
        assert_eq!(
            map_device_kind(FakeWgpuDeviceType::IntegratedGpu),
            GpuDeviceKind::Integrated
        );
        assert_eq!(map_device_kind(FakeWgpuDeviceType::Cpu), GpuDeviceKind::Cpu);
        assert_eq!(
            map_device_kind(FakeWgpuDeviceType::VirtualGpu),
            GpuDeviceKind::Other
        );
        assert_eq!(
            map_device_kind(FakeWgpuDeviceType::Other),
            GpuDeviceKind::Other
        );
    }
}
