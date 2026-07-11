//! Quality tier (spec §4). Kept free of any `wgpu`/`bevy_render` types so
//! `mf-state` stays a light dependency for `mf-render`: the knob table
//! returns plain data, and `mf-game`/`mf-render` translate it into actual
//! `PresentMode`/`Msaa`/shadow-map settings where those types live.

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
    /// When `true`, soft cloud cards + scrolling ground shadows are eligible
    /// (Medium/High). Potato/Low keep this off. The player can still turn the
    /// effect off via [`crate::WeatherEffects`] even when this is `true`.
    pub atmosphere_enabled: bool,
    /// Reserved after the billboard rewrite (was volumetric raymarch steps).
    /// Kept so quality tables stay stable; unused by the renderer.
    pub atmosphere_fog_steps: u32,
    /// When `true`, Bevy `Bloom` is eligible on the camera (Medium/High).
    /// Intensity still ramps with `DayNightState.night_factor` and is fully
    /// off during day; Potato/Low keep this false so the bloom pass never
    /// runs on weak GPUs / lavapipe.
    pub bloom_enabled: bool,
    /// When `true`, arterial street-lamp glow meshes are built (Low+).
    /// Potato skips them with day/night disabled.
    pub street_lamps_enabled: bool,
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
                bloom_enabled: false,
                street_lamps_enabled: false,
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
                bloom_enabled: false,
                street_lamps_enabled: true,
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
                bloom_enabled: true,
                street_lamps_enabled: true,
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
                bloom_enabled: true,
                street_lamps_enabled: true,
            },
        }
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
        assert!(!QualityTier::Potato.knobs().bloom_enabled);
        assert!(!QualityTier::Low.knobs().bloom_enabled);
        assert!(QualityTier::Medium.knobs().bloom_enabled);
        assert!(QualityTier::High.knobs().bloom_enabled);
        assert!(!QualityTier::Potato.knobs().street_lamps_enabled);
        assert!(QualityTier::Low.knobs().street_lamps_enabled);
        assert!(QualityTier::Medium.knobs().street_lamps_enabled);
        assert!(QualityTier::High.knobs().street_lamps_enabled);
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
}
