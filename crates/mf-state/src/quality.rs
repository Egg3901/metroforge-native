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
    /// Weakest tier (software raster / UHD-class): unlit, no day/night, fogged.
    Potato,
    /// Integrated-GPU baseline: unlit, day/night on, limited draw distance.
    Low,
    /// Default: lit materials, MSAA, atmosphere, bloom.
    #[default]
    Medium,
    /// Discrete-GPU high: larger shadows, unlimited draw distance, denser fog steps.
    High,
}

/// Coarse GPU classification `mf-game`'s `quality_boot.rs` extracts from
/// `RenderAdapterInfo` and feeds into [`detect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuDeviceKind {
    /// Discrete GPU adapter.
    Discrete,
    /// Integrated GPU adapter.
    Integrated,
    /// CPU / software device kind from the adapter info.
    Cpu,
    /// Anything else (unknown / other).
    Other,
}

/// Player-facing shadow quality (maps to [`QualityKnobs::shadow_map_size`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ShadowQuality {
    /// Shadows disabled (no shadow map).
    Off,
    /// 2048px shadow map.
    #[default]
    Medium,
    /// 4096px shadow map.
    High,
}

impl ShadowQuality {
    /// Player-facing label for the Settings combo box.
    pub fn label(self) -> &'static str {
        match self {
            ShadowQuality::Off => "Off",
            ShadowQuality::Medium => "Medium",
            ShadowQuality::High => "High",
        }
    }

    /// Classify a knob-table shadow map size back into a quality bucket.
    pub fn from_map_size(size: Option<u32>) -> Self {
        match size {
            None => ShadowQuality::Off,
            Some(s) if s >= 4096 => ShadowQuality::High,
            Some(_) => ShadowQuality::Medium,
        }
    }

    /// The shadow map size this bucket selects (`None` = shadows off).
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
    /// Max agents drawn from the latest frame (0 = none on Potato).
    pub agent_cap: u32,
    /// Terrain mesh subdivision divisor (higher = coarser mesh).
    pub terrain_subdiv_divisor: u32,
    /// When `false`, day/night is pinned to noon (Potato).
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
    /// When `true`, soft cloud cards + scrolling ground shadows are eligible
    /// (Medium/High). Potato/Low keep this off. The player can still turn the
    /// effect off via [`crate::WeatherEffects`] even when this is `true`.
    pub atmosphere_enabled: bool,
    /// Reserved after the billboard rewrite (was volumetric raymarch steps).
    /// Kept so quality tables stay stable; unused by the renderer.
    pub atmosphere_fog_steps: u32,
    /// Stylized water shader tier (mf-render `water.rs`):
    /// - `0` = flat vertex-color water baked into the terrain mesh (Potato;
    ///   zero extra draw / fill — required for the llvmpipe release smoke)
    /// - `1` = separate water mesh, single static ripple layer (Low)
    /// - `2` = full dual-layer scrolling ripples + specular + fresnel + foam
    ///   + night shimmer (Medium/High)
    pub water_quality: u8,
    /// When `true`, Bevy `Bloom` is eligible on the camera (Medium/High).
    /// Intensity still ramps with `DayNightState.night_factor` and is fully
    /// off during day; Potato/Low keep this false so the bloom pass never
    /// runs on weak GPUs / lavapipe.
    pub bloom_enabled: bool,
    /// When `true`, arterial street-lamp glow meshes are built (Low+).
    /// Potato skips them with day/night disabled.
    pub street_lamps_enabled: bool,
    /// When `true`, the cheap inverted-hull cel outline is drawn on the
    /// dense-center building chunk (see `mf-render`'s `outline.rs`).
    ///
    /// Originally High-only (spec §art-direction), but the tier-truth pass
    /// (owner feedback: the low tiers "look like shit") found this is the
    /// single biggest readability lever for the *unlit* tiers: with no
    /// lighting and near-white building colors, Potato/Low read as flat
    /// white mush with no edge definition. The outline is a bounded cost —
    /// exactly ONE of the 64 building chunks, inverted-hull, regardless of
    /// tier — so even Potato (agent_cap 0, no trees) has the budget for it,
    /// and it is what makes the "white cel blocks with crisp black edges"
    /// art direction land on every tier instead of only High. Enabled on all
    /// four tiers; the per-chunk scoping in `outline.rs` keeps it cheap.
    pub outline_enabled: bool,
}

/// Per-knob deltas on top of the active [`QualityTier`] preset. Every field
/// is `None` when the player has not overridden that control ("use preset").
/// Persisted under `[graphics]` in `config.toml` by `mf-game`.
#[derive(Debug, Clone, Copy, PartialEq, Resource, Default)]
pub struct QualityOverrides {
    /// Shadow-quality override.
    pub shadows: Option<ShadowQuality>,
    /// Building (and matching tree) draw distance in meters. `Some(m)` with
    /// `m >= `[`DRAW_DISTANCE_UNLIMITED_M`] forces unlimited (`None` on the
    /// knob). `None` on this field means "use the preset".
    pub draw_distance_m: Option<f32>,
    /// Park-tree rendering override.
    pub trees: Option<bool>,
    /// Distance-fog override.
    pub fog: Option<bool>,
    /// Sky cloud/atmosphere override.
    pub volumetric_clouds: Option<bool>,
    /// Building cel-outline override.
    pub outlines: Option<bool>,
    /// VSync (present-mode) override.
    pub vsync: Option<bool>,
}

/// Slider / preset sentinel: at or above this meters value, draw distance
/// is treated as unlimited.
pub const DRAW_DISTANCE_UNLIMITED_M: f32 = 20_000.0;
/// Inclusive lower bound for the Advanced draw-distance slider.
pub const DRAW_DISTANCE_MIN_M: f32 = 2_000.0;

impl QualityOverrides {
    /// `true` when no control is overridden (effective == preset).
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
    /// The merged knob set (preset + Advanced overrides).
    pub fn get(&self) -> &QualityKnobs {
        &self.0
    }
}

/// Merge a preset row with player deltas. Volumetric clouds forced on also
/// ensure a minimum shadow map (atmosphere requires shadows); fog forced on
/// synthesizes start/end from the effective draw distance when the preset
/// had no fog.
///
/// ## Live vs rebuild
/// Most knobs apply the same frame `EffectiveKnobs` updates (MSAA, fog,
/// shadows, draw-distance culling, vsync, outlines, atmosphere eligibility).
/// These Advanced-facing knobs **rebuild scene meshes** automatically because
/// their bake keys already include the knob value:
/// - [`QualityKnobs::tree_enabled`] — park tree chunk rebuild
///
/// Preset-only knobs that also rebuild (not exposed in Advanced UI):
/// - [`QualityKnobs::terrain_subdiv_divisor`] — terrain mesh
/// - [`QualityKnobs::ribbon_densify_step_m`] — roads / transit ribbons
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
            k.fog = Some(
                k.fog
                    .unwrap_or_else(|| synthesize_fog_range(k.building_draw_distance_m)),
            );
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
        k.outline_enabled = outlines;
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
                fog: Some((900.0, 2_600.0)),
                atmosphere_enabled: false,
                atmosphere_fog_steps: 0,
                water_quality: 0,
                bloom_enabled: false,
                street_lamps_enabled: false,
                // The one cheap thing that saves Potato from flat white mush:
                // crisp black edges on the dense core turn "broken High" into
                // a deliberate flat-shaded minimal style. One chunk draw; the
                // tier has the budget (agent_cap 0, no trees, no shadows).
                outline_enabled: true,
            },
            QualityTier::Low => QualityKnobs {
                vsync: true,
                // MSAA off on Low. 2x looked tempting as "cheap edge-AA" for
                // the newly-added outlines/route stripes, but WebGPU only
                // GUARANTEES sample counts [1, 4] for the Depth32Float target
                // Bevy uses here — 2x panics outright on lavapipe (software)
                // and can be unsupported on real integrated GPUs too, exactly
                // the hardware this tier targets. 4x is the next portable
                // step but too costly for the weak-GPU tier, so this stays 1
                // (off); the outline crispness is the win, not AA.
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
                water_quality: 1,
                bloom_enabled: false,
                street_lamps_enabled: true,
                // The headline Low fix: outlines give the unlit white massing
                // the edge definition it was completely missing, so Low reads
                // as a city instead of "Potato with a longer draw distance".
                outline_enabled: true,
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
                water_quality: 2,
                bloom_enabled: true,
                street_lamps_enabled: true,
                // Medium reads via shadows already, but the cel outline is the
                // signature art direction and one chunk is trivial next to the
                // shadow/bloom/atmosphere passes it already runs — keep the
                // look consistent from Low all the way to High.
                outline_enabled: true,
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
                water_quality: 2,
                bloom_enabled: true,
                street_lamps_enabled: true,
                outline_enabled: true,
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
        // Outline is enabled on every tier after the tier-truth pass.
        assert!(QualityTier::Potato.knobs().outline_enabled);
        assert!(QualityTier::High.knobs().outline_enabled);
        assert_eq!(QualityTier::Potato.knobs().water_quality, 0);
        assert_eq!(QualityTier::Low.knobs().water_quality, 1);
        assert_eq!(QualityTier::Medium.knobs().water_quality, 2);
        assert_eq!(QualityTier::High.knobs().water_quality, 2);
        assert!(!QualityTier::Potato.knobs().bloom_enabled);
        assert!(!QualityTier::Low.knobs().bloom_enabled);
        assert!(QualityTier::Medium.knobs().bloom_enabled);
        assert!(QualityTier::High.knobs().bloom_enabled);
        assert!(!QualityTier::Potato.knobs().street_lamps_enabled);
        assert!(QualityTier::Low.knobs().street_lamps_enabled);
        assert!(QualityTier::Medium.knobs().street_lamps_enabled);
        assert!(QualityTier::High.knobs().street_lamps_enabled);
        // Cel outline is now on for every tier (the tier-truth readability
        // fix): one bounded dense-center chunk draw, cheap enough even for
        // Potato, and the signature "white cel blocks" look end to end.
        assert!(QualityTier::Potato.knobs().outline_enabled);
        assert!(QualityTier::Low.knobs().outline_enabled);
        assert!(QualityTier::Medium.knobs().outline_enabled);
        assert!(QualityTier::High.knobs().outline_enabled);
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
        let o = QualityOverrides {
            trees: Some(true),
            vsync: Some(true),
            ..Default::default()
        };
        assert!(!o.is_empty());
        let potato = QualityTier::Potato.effective_knobs(&o);
        assert!(potato.tree_enabled);
        assert!(potato.vsync);
        // Untouched knobs stay on the Potato preset.
        assert!(potato.unlit_material);
        assert_eq!(potato.shadow_map_size, None);
    }

    #[test]
    fn volumetric_clouds_override_enables_shadows() {
        let o = QualityOverrides {
            volumetric_clouds: Some(true),
            ..Default::default()
        };
        let k = QualityTier::Potato.effective_knobs(&o);
        assert!(k.atmosphere_enabled);
        assert!(k.shadow_map_size.is_some());
        assert!(k.atmosphere_fog_steps > 0);
    }

    #[test]
    fn draw_distance_unlimited_sentinel() {
        let o = QualityOverrides {
            draw_distance_m: Some(DRAW_DISTANCE_UNLIMITED_M),
            ..Default::default()
        };
        let k = QualityTier::Low.effective_knobs(&o);
        assert!(k.building_draw_distance_m.is_none());
        assert!(k.tree_draw_distance_m.is_none());
    }

    #[test]
    fn fog_override_off_clears_preset_fog() {
        let o = QualityOverrides {
            fog: Some(false),
            ..Default::default()
        };
        assert!(QualityTier::Potato.effective_knobs(&o).fog.is_none());
    }

    #[test]
    fn recommend_tier_thresholds() {
        assert_eq!(recommend_tier_from_frame_times(8.0, 9.0), QualityTier::High);
        assert_eq!(
            recommend_tier_from_frame_times(12.0, 13.0),
            QualityTier::Medium
        );
        assert_eq!(
            recommend_tier_from_frame_times(18.0, 20.0),
            QualityTier::Low
        );
        assert_eq!(
            recommend_tier_from_frame_times(30.0, 40.0),
            QualityTier::Potato
        );
    }

    /// Property: every numeric knob is monotone across Potato → Low →
    /// Medium → High (non-decreasing for "more is better", non-increasing
    /// for "coarser is cheaper"). Catches a future tier-table edit that
    /// accidentally makes Medium worse than Low on some axis.
    #[test]
    fn numeric_knobs_are_monotone_across_tiers() {
        let tiers = [
            QualityTier::Potato,
            QualityTier::Low,
            QualityTier::Medium,
            QualityTier::High,
        ];
        let knobs: Vec<QualityKnobs> = tiers.iter().map(|t| t.knobs()).collect();

        // "More is better" (or equal): non-decreasing.
        for w in knobs.windows(2) {
            assert!(w[0].msaa_samples <= w[1].msaa_samples);
            assert!(w[0].agent_cap <= w[1].agent_cap);
            assert!(w[0].atmosphere_fog_steps <= w[1].atmosphere_fog_steps);
            assert!(w[0].water_quality <= w[1].water_quality);
        }

        // Draw distances: None = unlimited = greatest. Treat as +∞.
        let draw = |d: Option<f32>| d.unwrap_or(f32::INFINITY);
        for w in knobs.windows(2) {
            assert!(draw(w[0].building_draw_distance_m) <= draw(w[1].building_draw_distance_m));
            assert!(draw(w[0].tree_draw_distance_m) <= draw(w[1].tree_draw_distance_m));
        }

        // Shadow map: None < Some(n), and sizes non-decreasing when present.
        let shadow_rank = |s: Option<u32>| s.map(|n| n as i64).unwrap_or(-1);
        for w in knobs.windows(2) {
            assert!(shadow_rank(w[0].shadow_map_size) <= shadow_rank(w[1].shadow_map_size));
        }

        // "Coarser is cheaper": non-increasing.
        for w in knobs.windows(2) {
            assert!(w[0].terrain_subdiv_divisor >= w[1].terrain_subdiv_divisor);
            assert!(w[0].ribbon_densify_step_m >= w[1].ribbon_densify_step_m);
        }

        // Bool knobs that flip false→true (or stay) as tier rises.
        for w in knobs.windows(2) {
            assert!(!w[0].vsync || w[1].vsync);
            assert!(!w[0].day_night_enabled || w[1].day_night_enabled);
            assert!(!w[0].tree_enabled || w[1].tree_enabled);
            assert!(!w[0].atmosphere_enabled || w[1].atmosphere_enabled);
            assert!(!w[0].bloom_enabled || w[1].bloom_enabled);
            assert!(!w[0].street_lamps_enabled || w[1].street_lamps_enabled);
            // unlit is the inverse: true on weak tiers, false on strong.
            assert!(!w[1].unlit_material || w[0].unlit_material);
        }
    }
}
