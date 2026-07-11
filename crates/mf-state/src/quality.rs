//! Quality tier (spec §4). Kept free of any `wgpu`/`bevy_render` types so
//! `mf-state` stays a light dependency for `mf-render`: the knob table
//! returns plain data, and `mf-game`/`mf-render` translate it into actual
//! `PresentMode`/`Msaa`/shadow-map settings where those types live.
//!
//! Player-facing Advanced graphics controls persist as [`QualityOverrides`]
//! deltas on top of the selected preset. Render systems read
//! [`EffectiveKnobs`] (preset merged with overrides), which updates live
//! whenever the tier or any override changes.

use bevy_ecs::prelude::*;

/// Quality tier, auto-detected at Boot (see [`detect`]) with a
/// `config.toml` override always winning (spec §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Resource, Default)]
pub enum QualityTier {
    Potato,
    Low,
    #[default]
    Medium,
    High,
}

/// Coarse GPU classification `mf-game`'s `quality_boot.rs` extracts from
/// `RenderAdapterInfo` and feeds into [`detect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuDeviceKind {
    Discrete,
    Integrated,
    Cpu,
    Other,
}

/// Player-facing shadow quality (maps to [`QualityKnobs::shadow_map_size`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ShadowQuality {
    Off,
    #[default]
    Medium,
    High,
}

impl ShadowQuality {
    pub fn label(self) -> &'static str {
        match self {
            ShadowQuality::Off => "Off",
            ShadowQuality::Medium => "Medium",
            ShadowQuality::High => "High",
        }
    }

    pub fn from_map_size(size: Option<u32>) -> Self {
        match size {
            None => ShadowQuality::Off,
            Some(s) if s >= 4096 => ShadowQuality::High,
            Some(_) => ShadowQuality::Medium,
        }
    }

    pub fn map_size(self) -> Option<u32> {
        match self {
            ShadowQuality::Off => None,
            ShadowQuality::Medium => Some(2048),
            ShadowQuality::High => Some(4096),
        }
    }
}

/// One row of the spec §4 knob table, as plain data.
///
/// `render_scale` and `vehicle_mesh` (plus the `VehicleMesh` enum) were
/// removed here: nothing ever read either knob (no low-res render target
/// exists, and `vehicles.rs` always builds its fixed box/tram meshes
/// regardless of tier), so they were dead data rather than a knob anyone
/// could turn. Re-add a knob only alongside the code that actually
/// implements it.
///
/// `ribbon_densify_step_m` / `tree_enabled` / `tree_draw_distance_m` are
/// client-only perf knobs (perf audit): coarser ribbons and tree culling on
/// weak tiers, mirroring the existing `terrain_subdiv_divisor` /
/// `building_draw_distance_m` pattern.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualityKnobs {
    /// `true` = `AutoVsync`, `false` = `AutoNoVsync` (only Potato disables vsync).
    pub vsync: bool,
    /// MSAA sample count; `1` means "off".
    pub msaa_samples: u8,
    /// Shadow cascade map resolution; `None` means shadows off.
    pub shadow_map_size: Option<u32>,
    /// `true` = unlit (vertex-color-only) material, `false` = lit `StandardMaterial`.
    pub unlit_material: bool,
    /// Building draw distance in meters; `None` means unlimited ("full").
    pub building_draw_distance_m: Option<f32>,
    pub agent_cap: u32,
    /// Terrain mesh subdivision divisor (higher = coarser mesh).
    pub terrain_subdiv_divisor: u32,
    pub day_night_enabled: bool,
    /// Max spacing (meters) between densified ribbon samples for roads /
    /// transit tracks / route stripes. Higher = fewer vertices on rebuild
    /// and at draw time (Potato/Low).
    pub ribbon_densify_step_m: f32,
    /// When `false`, park trees are not built (Potato).
    pub tree_enabled: bool,
    /// Tree-chunk draw distance in meters; `None` means unlimited.
    pub tree_draw_distance_m: Option<f32>,
    /// Distance fog `(start_m, end_m)` to mask draw-distance pop-in on weak
    /// tiers; `None` means fog disabled. `end_m` sits just inside
    /// `building_draw_distance_m` so buildings/trees fade into fog before
    /// they hard-cull, rather than popping at the culling edge. Denser on
    /// Potato (shortest draw distance, most pop-in to hide), lighter on Low,
    /// off on Medium/High where draw distance is unlimited or generous
    /// enough that fog would just look like a boring haze over open sky.
    pub fog: Option<(f32, f32)>,
    /// When `true`, scrolling volumetric fog/cloud + distance haze are
    /// eligible (Medium/High). Potato/Low keep this off — volumetric fog
    /// needs shadow maps, which those tiers disable. The player can still
    /// turn the effect off via [`crate::WeatherEffects`] even when this is
    /// `true`.
    pub atmosphere_enabled: bool,
    /// Raymarch step count for [`bevy::pbr::VolumetricFog`] when atmosphere
    /// is active. Higher = less banding, more GPU.
    pub atmosphere_fog_steps: u32,
    /// Cel-shading building outlines (dense-center chunk). High preset only;
    /// Advanced settings can override.
    pub outlines_enabled: bool,
}

/// Per-knob deltas on top of the active [`QualityTier`] preset. Every field
/// is `None` when the player has not overridden that control ("use preset").
/// Persisted under `[graphics]` in `config.toml` by `mf-game`.
#[derive(Debug, Clone, Copy, PartialEq, Resource, Default)]
pub struct QualityOverrides {
    pub shadows: Option<ShadowQuality>,
    /// Building (and matching tree) draw distance in meters. `Some(m)` with
    /// `m >= `[`DRAW_DISTANCE_UNLIMITED_M`] forces unlimited (`None` on the
    /// knob). `None` on this field means "use the preset".
    pub draw_distance_m: Option<f32>,
    pub trees: Option<bool>,
    pub fog: Option<bool>,
    pub volumetric_clouds: Option<bool>,
    pub outlines: Option<bool>,
    pub vsync: Option<bool>,
}

/// Slider / preset sentinel: at or above this meters value, draw distance
/// is treated as unlimited.
pub const DRAW_DISTANCE_UNLIMITED_M: f32 = 20_000.0;
/// Inclusive lower bound for the Advanced draw-distance slider.
pub const DRAW_DISTANCE_MIN_M: f32 = 2_000.0;

impl QualityOverrides {
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }

    /// Clear every delta so effective knobs match the preset exactly.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// GPU auto-detect result latched at boot so the Settings "Auto" option can
/// re-apply detection without re-querying the adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Resource)]
pub struct DetectedQuality(pub QualityTier);

impl Default for DetectedQuality {
    fn default() -> Self {
        DetectedQuality(QualityTier::Medium)
    }
}

/// Preset knobs merged with [`QualityOverrides`]. Render / game systems that
/// care about live graphics settings should read this (not
/// `QualityTier::knobs()` alone) so Advanced overrides apply without a
/// restart.
#[derive(Debug, Clone, Copy, PartialEq, Resource)]
pub struct EffectiveKnobs(pub QualityKnobs);

impl Default for EffectiveKnobs {
    fn default() -> Self {
        EffectiveKnobs(QualityTier::Medium.knobs())
    }
}

impl EffectiveKnobs {
    pub fn get(&self) -> &QualityKnobs {
        &self.0
    }
}

/// Merge a preset row with player deltas. Volumetric clouds forced on also
/// ensure a minimum shadow map (atmosphere requires shadows); fog forced on
/// synthesizes start/end from the effective draw distance when the preset
/// had no fog.
pub fn merge_knobs(base: QualityKnobs, overrides: &QualityOverrides) -> QualityKnobs {
    let mut k = base;

    if let Some(shadows) = overrides.shadows {
        k.shadow_map_size = shadows.map_size();
    }

    if let Some(meters) = overrides.draw_distance_m {
        if meters >= DRAW_DISTANCE_UNLIMITED_M {
            k.building_draw_distance_m = None;
            k.tree_draw_distance_m = None;
        } else {
            let m = meters.max(DRAW_DISTANCE_MIN_M);
            k.building_draw_distance_m = Some(m);
            k.tree_draw_distance_m = Some(m);
        }
    }

    if let Some(trees) = overrides.trees {
        k.tree_enabled = trees;
    }

    if let Some(fog_on) = overrides.fog {
        if fog_on {
            k.fog = Some(k.fog.unwrap_or_else(|| synthesize_fog_range(k.building_draw_distance_m)));
        } else {
            k.fog = None;
        }
    }

    if let Some(clouds) = overrides.volumetric_clouds {
        k.atmosphere_enabled = clouds;
        if clouds {
            // Volumetric fog needs shadow maps; bump to Medium if still off.
            if k.shadow_map_size.is_none() {
                k.shadow_map_size = ShadowQuality::Medium.map_size();
            }
            if k.atmosphere_fog_steps == 0 {
                k.atmosphere_fog_steps = 32;
            }
        }
    }

    if let Some(outlines) = overrides.outlines {
        k.outlines_enabled = outlines;
    }

    if let Some(vsync) = overrides.vsync {
        k.vsync = vsync;
    }

    k
}

fn synthesize_fog_range(building_draw: Option<f32>) -> (f32, f32) {
    // Mirror Potato/Low intent: end sits inside the cull distance when one
    // exists; otherwise use a generous Medium-like haze band.
    let end = building_draw
        .map(|d| (d * 0.85).max(DRAW_DISTANCE_MIN_M))
        .unwrap_or(10_000.0);
    let start = (end * 0.45).min(end - 100.0).max(500.0);
    (start, end)
}

/// Recompute [`EffectiveKnobs`] whenever the preset or overrides change.
pub fn sync_effective_knobs_system(
    tier: Res<QualityTier>,
    overrides: Res<QualityOverrides>,
    mut effective: ResMut<EffectiveKnobs>,
) {
    if !(tier.is_changed() || overrides.is_changed() || effective.is_added()) {
        return;
    }
    let merged = merge_knobs(tier.knobs(), &overrides);
    if effective.0 != merged {
        effective.0 = merged;
    }
}

impl QualityTier {
    /// Player-facing label for combo boxes (not `Debug` — "Potato" is fine,
    /// but this keeps HUD copy intentional if variants are renamed later).
    pub fn label(self) -> &'static str {
        match self {
            QualityTier::Potato => "Potato",
            QualityTier::Low => "Low",
            QualityTier::Medium => "Medium",
            QualityTier::High => "High",
        }
    }

    /// The full knob table (spec §4), one method call per tier.
    pub fn knobs(self) -> QualityKnobs {
        match self {
            QualityTier::Potato => QualityKnobs {
                vsync: false,
                msaa_samples: 1,
                shadow_map_size: None,
                unlit_material: true,
                building_draw_distance_m: Some(3_000.0),
                agent_cap: 0,
                terrain_subdiv_divisor: 3,
                day_night_enabled: false,
                ribbon_densify_step_m: 48.0,
                tree_enabled: false,
                tree_draw_distance_m: Some(3_000.0),
                // Dense-ish: shortest draw distance (3km) means the most
                // pop-in to hide, so fog closes in early and finishes well
                // inside the 3km cull.
                fog: Some((1_200.0, 2_600.0)),
                atmosphere_enabled: false,
                atmosphere_fog_steps: 0,
                outlines_enabled: false,
            },
            QualityTier::Low => QualityKnobs {
                vsync: true,
                msaa_samples: 1,
                shadow_map_size: None,
                unlit_material: true,
                building_draw_distance_m: Some(6_000.0),
                agent_cap: 100,
                terrain_subdiv_divisor: 2,
                day_night_enabled: true,
                ribbon_densify_step_m: 36.0,
                tree_enabled: true,
                tree_draw_distance_m: Some(6_000.0),
                // Lighter than Potato: draw distance doubled to 6km, so fog
                // can start further out and still finish inside the cull.
                fog: Some((3_000.0, 5_500.0)),
                atmosphere_enabled: false,
                atmosphere_fog_steps: 0,
                outlines_enabled: false,
            },
            QualityTier::Medium => QualityKnobs {
                vsync: true,
                msaa_samples: 4,
                shadow_map_size: Some(2048),
                unlit_material: false,
                building_draw_distance_m: Some(12_000.0),
                agent_cap: 250,
                terrain_subdiv_divisor: 1,
                day_night_enabled: true,
                ribbon_densify_step_m: 24.0,
                tree_enabled: true,
                tree_draw_distance_m: Some(12_000.0),
                // Draw distance generous enough (12km) that fog would just
                // haze open sky rather than mask any real pop-in.
                fog: None,
                atmosphere_enabled: true,
                atmosphere_fog_steps: 32,
                outlines_enabled: false,
            },
            QualityTier::High => QualityKnobs {
                vsync: true,
                msaa_samples: 4,
                shadow_map_size: Some(4096),
                unlit_material: false,
                building_draw_distance_m: None,
                agent_cap: 400,
                terrain_subdiv_divisor: 1,
                day_night_enabled: true,
                ribbon_densify_step_m: 24.0,
                tree_enabled: true,
                tree_draw_distance_m: None,
                fog: None,
                atmosphere_enabled: true,
                atmosphere_fog_steps: 56,
                outlines_enabled: true,
            },
        }
    }

    /// Effective knobs for this preset with the given overrides applied.
    pub fn effective_knobs(self, overrides: &QualityOverrides) -> QualityKnobs {
        merge_knobs(self.knobs(), overrides)
    }
}

/// Auto-detect rule (spec §4): DiscreteGpu -> High; IntegratedGpu -> Low
/// (Medium left to the caller/config for "clearly recent" iGPUs — no data
/// source here distinguishes GPU generations); name matches UHD/HD
/// Graphics/llvmpipe/lavapipe/SwiftShader, or `Cpu` kind -> Potato;
/// otherwise Medium. A `config.toml` override always wins over this.
pub fn detect(adapter_name: &str, kind: GpuDeviceKind) -> QualityTier {
    let lower = adapter_name.to_lowercase();
    let looks_like_software = ["uhd", "hd graphics", "llvmpipe", "lavapipe", "swiftshader"]
        .iter()
        .any(|needle| lower.contains(needle));
    if looks_like_software || kind == GpuDeviceKind::Cpu {
        return QualityTier::Potato;
    }
    match kind {
        GpuDeviceKind::Discrete => QualityTier::High,
        GpuDeviceKind::Integrated => QualityTier::Low,
        GpuDeviceKind::Cpu | GpuDeviceKind::Other => QualityTier::Medium,
    }
}

/// Recommend a preset from a 10s benchmark's average and 1%-low frame times
/// (milliseconds). Thresholds are intentionally conservative so a borderline
/// GPU lands on the next-lower tier rather than a slideshow.
pub fn recommend_tier_from_frame_times(avg_ms: f32, low_1pct_ms: f32) -> QualityTier {
    // Prefer the worse of avg / 1% low so hitching pulls the recommendation down.
    let budget = avg_ms.max(low_1pct_ms * 0.85);
    if budget <= 10.0 {
        QualityTier::High
    } else if budget <= 14.5 {
        QualityTier::Medium
    } else if budget <= 22.0 {
        QualityTier::Low
    } else {
        QualityTier::Potato
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_lavapipe_as_potato() {
        assert_eq!(
            detect("llvmpipe (LLVM 15.0.0, 256 bits)", GpuDeviceKind::Cpu),
            QualityTier::Potato
        );
        assert_eq!(
            detect("Mesa Lavapipe", GpuDeviceKind::Other),
            QualityTier::Potato
        );
    }

    #[test]
    fn detects_discrete_as_high() {
        assert_eq!(
            detect("NVIDIA GeForce RTX 4070", GpuDeviceKind::Discrete),
            QualityTier::High
        );
    }

    #[test]
    fn detects_intel_uhd_as_potato_even_if_reported_integrated() {
        assert_eq!(
            detect("Intel(R) UHD Graphics 620", GpuDeviceKind::Integrated),
            QualityTier::Potato
        );
    }

    #[test]
    fn unknown_adapter_defaults_medium() {
        assert_eq!(
            detect("Some Weird Adapter", GpuDeviceKind::Other),
            QualityTier::Medium
        );
    }

    #[test]
    fn knob_table_matches_spec_shape() {
        assert_eq!(QualityTier::Potato.knobs().agent_cap, 0);
        assert_eq!(QualityTier::High.knobs().agent_cap, 400);
        assert!(
            QualityTier::Potato
                .knobs()
                .building_draw_distance_m
                .unwrap()
                < QualityTier::Low.knobs().building_draw_distance_m.unwrap()
        );
        assert!(QualityTier::High.knobs().building_draw_distance_m.is_none());
        assert!(!QualityTier::Potato.knobs().tree_enabled);
        assert!(QualityTier::Low.knobs().tree_enabled);
        assert!(
            QualityTier::Potato.knobs().ribbon_densify_step_m
                > QualityTier::High.knobs().ribbon_densify_step_m
        );
        assert!(!QualityTier::Potato.knobs().atmosphere_enabled);
        assert!(!QualityTier::Low.knobs().atmosphere_enabled);
        assert!(QualityTier::Medium.knobs().atmosphere_enabled);
        assert!(QualityTier::High.knobs().atmosphere_enabled);
        assert!(
            QualityTier::High.knobs().atmosphere_fog_steps
                > QualityTier::Medium.knobs().atmosphere_fog_steps
        );
        assert!(!QualityTier::Medium.knobs().outlines_enabled);
        assert!(QualityTier::High.knobs().outlines_enabled);
    }

    /// Fog `end_m` must sit strictly inside `building_draw_distance_m`
    /// wherever both are set, so buildings/trees fade into fog before they
    /// hard-pop out of existence at the draw-distance cull — otherwise fog
    /// would do nothing to mask pop-in. Also: `start_m < end_m` for every
    /// tier that has fog at all, and Medium/High (generous or unlimited
    /// draw distance) carry no fog.
    #[test]
    fn fog_end_sits_inside_building_draw_distance() {
        for tier in [
            QualityTier::Potato,
            QualityTier::Low,
            QualityTier::Medium,
            QualityTier::High,
        ] {
            let knobs = tier.knobs();
            if let Some((start, end)) = knobs.fog {
                assert!(start < end, "{tier:?}: fog start must be < end");
                if let Some(building_draw) = knobs.building_draw_distance_m {
                    assert!(
                        end < building_draw,
                        "{tier:?}: fog end ({end}) must sit inside building draw distance ({building_draw})"
                    );
                }
            }
        }
        assert!(QualityTier::Medium.knobs().fog.is_none());
        assert!(QualityTier::High.knobs().fog.is_none());
        assert!(QualityTier::Potato.knobs().fog.is_some());
        assert!(QualityTier::Low.knobs().fog.is_some());
    }

    #[test]
    fn overrides_are_deltas_on_preset() {
        let mut o = QualityOverrides::default();
        assert!(o.is_empty());
        o.trees = Some(true);
        o.vsync = Some(true);
        let potato = QualityTier::Potato.effective_knobs(&o);
        assert!(potato.tree_enabled);
        assert!(potato.vsync);
        // Untouched knobs stay on the Potato preset.
        assert!(potato.unlit_material);
        assert_eq!(potato.shadow_map_size, None);
    }

    #[test]
    fn volumetric_clouds_override_enables_shadows() {
        let mut o = QualityOverrides::default();
        o.volumetric_clouds = Some(true);
        let k = QualityTier::Potato.effective_knobs(&o);
        assert!(k.atmosphere_enabled);
        assert!(k.shadow_map_size.is_some());
        assert!(k.atmosphere_fog_steps > 0);
    }

    #[test]
    fn draw_distance_unlimited_sentinel() {
        let mut o = QualityOverrides::default();
        o.draw_distance_m = Some(DRAW_DISTANCE_UNLIMITED_M);
        let k = QualityTier::Low.effective_knobs(&o);
        assert!(k.building_draw_distance_m.is_none());
        assert!(k.tree_draw_distance_m.is_none());
    }

    #[test]
    fn fog_override_off_clears_preset_fog() {
        let mut o = QualityOverrides::default();
        o.fog = Some(false);
        assert!(QualityTier::Potato.effective_knobs(&o).fog.is_none());
    }

    #[test]
    fn recommend_tier_thresholds() {
        assert_eq!(recommend_tier_from_frame_times(8.0, 9.0), QualityTier::High);
        assert_eq!(
            recommend_tier_from_frame_times(12.0, 13.0),
            QualityTier::Medium
        );
        assert_eq!(recommend_tier_from_frame_times(18.0, 20.0), QualityTier::Low);
        assert_eq!(
            recommend_tier_from_frame_times(30.0, 40.0),
            QualityTier::Potato
        );
    }
}
