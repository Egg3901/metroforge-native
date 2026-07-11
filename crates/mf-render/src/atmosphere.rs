//! Atmospheric weather — sparse drifting cloud volumes + ground shadows.
//!
//! Replaces the old dual-slab "gray soup" volumetric fog with:
//! - **2–3 discrete soft [`FogVolume`] blobs** that drift with wind and leave
//!   clear air between them (city always readable)
//! - **Scrolling cloud shadows** on terrain/buildings via a shared 2D noise
//!   texture ([`CloudShadowParams`]) sampled in material extensions
//! - **Golden-hour tinting** from [`DayNightState::sun_elevation`] / twilight
//! - **Hard density clamp** so weather never washes out the whole frame
//!
//! Bevy's [`VolumetricFog`] is kept: sparse discrete volumes + clamped density
//! read as weather rather than webcam fog. Gated to Medium/High +
//! [`WeatherEffects`]; Potato/Low stay clear (needs directional shadow maps).

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::{FogVolume, VolumetricFog, VolumetricLight};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::f32::consts::TAU;

use mf_state::{QualityTier, SubwayView, WeatherEffects};

use crate::daynight::{DayNightState, Sun};
use crate::palette;

/// Index of one of the sparse drifting cloud blobs.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
struct CloudBlob {
    index: u8,
}

/// Shared wind that cloud volumes and ground shadows advect along.
#[derive(Resource, Clone, Copy)]
pub struct AtmosphereWind {
    /// Radians; creeps so the city doesn't get a permanent wind heading.
    pub heading: f32,
    /// Multiplier on base scroll speed, ~0.7..1.45.
    pub gust: f32,
    gust_phase: f32,
}

impl Default for AtmosphereWind {
    fn default() -> Self {
        AtmosphereWind {
            heading: 0.35,
            gust: 1.0,
            gust_phase: 0.0,
        }
    }
}

/// Scrolling cloud-shadow projection for terrain/building shaders.
///
/// `strength == 0` when weather is off or quality is below Medium — materials
/// still bind the texture but the multiply is a no-op.
#[derive(Resource, Clone)]
pub struct CloudShadowParams {
    pub texture: Handle<Image>,
    /// UV scroll in noise-space (advances with wind).
    pub offset: Vec2,
    /// 0..1 darkening amount (already quality/weather gated).
    pub strength: f32,
    /// `1 / world_meters_per_tile` — multiply world XZ then add offset.
    pub inv_scale: f32,
}

impl Default for CloudShadowParams {
    fn default() -> Self {
        CloudShadowParams {
            texture: Handle::default(),
            offset: Vec2::ZERO,
            strength: 0.0,
            inv_scale: 1.0 / SHADOW_TILE_M,
        }
    }
}

pub struct MfAtmospherePlugin;

/// Runs after wind/volume/shadow params are written for the frame — material
/// consumers (terrain/buildings) should schedule after this set.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AtmosphereReady;

impl Plugin for MfAtmospherePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AtmosphereWind>()
            .init_resource::<CloudShadowParams>()
            .configure_sets(Update, AtmosphereReady.in_set(crate::MfRenderSet::Dynamic))
            .add_systems(Startup, setup_atmosphere_system)
            .add_systems(
                Update,
                (
                    update_atmosphere_wind_system,
                    sync_atmosphere_system.after(crate::daynight::apply_day_night_system),
                    drift_cloud_volumes_system,
                    scroll_cloud_shadows_system,
                )
                    .chain()
                    .in_set(AtmosphereReady),
            );
    }
}

const NUM_CLOUDS: u8 = 3;
const NOISE_RES_3D: u32 = 32;
const SHADOW_RES: u32 = 256;
/// World meters covered by one repeat of the shadow noise.
const SHADOW_TILE_M: f32 = 1_100.0;
/// Soft cloud AABB size (XZ) — large enough to read as weather, small enough
/// that three of them leave clear corridors between.
const CLOUD_SIZE_XZ_M: f32 = 4_200.0;
const CLOUD_THICKNESS_M: f32 = 380.0;
const CLOUD_CENTER_Y_M: f32 = 640.0;
/// Hard ceiling — weather must never wash the city. Tuned so even stacked
/// volumes + twilight boost stay readable.
const CLOUD_DENSITY_MAX: f32 = 0.00010;
const CLOUD_DENSITY_BASE: f32 = 0.000055;
/// How far cloud centers roam relative to the camera before wrapping.
const CLOUD_DOMAIN_HALF_M: f32 = 7_500.0;
const CLOUD_DRIFT_SPEED: f32 = 28.0;
const SHADOW_SCROLL: f32 = 0.012;
const HAZE_VISIBILITY_DAY_M: f32 = 28_000.0;
const HAZE_VISIBILITY_NIGHT_M: f32 = 16_000.0;
const WIND_HEADING_RATE: f32 = 0.003;
const WIND_GUST_RATE: f32 = 0.55;
/// Peak ground-shadow darkening (day). Night is quieter.
const SHADOW_STRENGTH_DAY: f32 = 0.22;
const SHADOW_STRENGTH_NIGHT: f32 = 0.08;

fn setup_atmosphere_system(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let blob_noise = images.add(build_soft_blob_texture());
    let shadow_noise = images.add(build_shadow_noise_texture());

    commands.insert_resource(CloudShadowParams {
        texture: shadow_noise,
        offset: Vec2::ZERO,
        strength: 0.0,
        inv_scale: 1.0 / SHADOW_TILE_M,
    });

    // Three discrete soft volumes at staggered starting offsets so the first
    // frame already has clear air between them (not one fused slab).
    for i in 0..NUM_CLOUDS {
        let angle = (i as f32) * (TAU / NUM_CLOUDS as f32) + 0.4;
        let radius = 2_800.0 + (i as f32) * 900.0;
        let x = angle.cos() * radius;
        let z = angle.sin() * radius;
        commands.spawn((
            CloudBlob { index: i },
            FogVolume {
                density_texture: Some(blob_noise.clone()),
                density_factor: 0.0,
                absorption: 0.20,
                scattering: 0.32,
                scattering_asymmetry: 0.70,
                fog_color: Color::srgb(0.92, 0.94, 0.97),
                light_intensity: 0.85,
                ..default()
            },
            Transform::from_xyz(x, CLOUD_CENTER_Y_M, z).with_scale(Vec3::new(
                CLOUD_SIZE_XZ_M,
                CLOUD_THICKNESS_M,
                CLOUD_SIZE_XZ_M,
            )),
            Visibility::Hidden,
        ));
    }
}

fn update_atmosphere_wind_system(time: Res<Time>, mut wind: ResMut<AtmosphereWind>) {
    let dt = time.delta_secs();
    wind.heading = (wind.heading + WIND_HEADING_RATE * dt).rem_euclid(TAU);
    wind.gust_phase += WIND_GUST_RATE * dt;
    let g =
        0.5 + 0.5 * (0.65 * wind.gust_phase.sin() + 0.35 * (wind.gust_phase * 1.73 + 1.1).sin());
    wind.gust = 0.72 + g * 0.70;
}

/// Soft ellipsoid density — dense near the volume center, zero at the edges
/// so discrete FogVolumes read as separate clouds, not a filled slab.
fn build_soft_blob_texture() -> Image {
    let n = NOISE_RES_3D as usize;
    let mut data = vec![0u8; n * n * n];
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let u = x as f32 / n as f32;
                let v = y as f32 / n as f32;
                let w = z as f32 / n as f32;
                // Ellipsoid: flatter vertically so the deck reads as a layer.
                let dx = (u - 0.5) * 2.0;
                let dy = (v - 0.5) * 2.6;
                let dz = (w - 0.5) * 2.0;
                let r = (dx * dx + dy * dy + dz * dz).sqrt();
                let warp = fbm3(u * 3.0 + 1.7, v * 2.0, w * 3.0 + 0.4, 3) * 0.22;
                let soft = (1.0 - ((r + warp - 0.15) / 0.85).clamp(0.0, 1.0)).powf(1.8);
                // Hard floor so the outer shell is truly empty (clear air).
                let shaped = if soft < 0.08 {
                    0.0
                } else {
                    ((soft - 0.08) / 0.92).powf(1.35)
                };
                data[z * n * n + y * n + x] = (shaped * 255.0).round() as u8;
            }
        }
    }
    finish_noise_image_3d(data)
}

/// Large soft 2D blobs for ground-projected cloud shadows (tiling).
fn build_shadow_noise_texture() -> Image {
    let n = SHADOW_RES as usize;
    let mut data = vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            let u = x as f32 / n as f32;
            let v = y as f32 / n as f32;
            // Two octaves of large-scale FBM → a few soft patches per tile.
            let a = fbm2(u * 2.2, v * 2.2, 4);
            let b = fbm2(u * 3.5 + 5.1, v * 3.5 + 2.3, 3);
            let dens = (a * 0.65 + b * 0.35).clamp(0.0, 1.0);
            // Threshold so most of the tile is clear (matching sparse volumes).
            let shaped = ((dens - 0.48).max(0.0) / 0.52).powf(1.6);
            data[y * n + x] = (shaped * 255.0).round() as u8;
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: SHADOW_RES,
            height: SHADOW_RES,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

fn finish_noise_image_3d(data: Vec<u8>) -> Image {
    let mut image = Image::new(
        Extent3d {
            width: NOISE_RES_3D,
            height: NOISE_RES_3D,
            depth_or_array_layers: NOISE_RES_3D,
        },
        TextureDimension::D3,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

fn hash_21(x: i32, y: i32) -> f32 {
    let mut n = x
        .wrapping_mul(374761393)
        .wrapping_add(y.wrapping_mul(668265263));
    n = (n ^ (n >> 13)).wrapping_mul(1274126177);
    (n & 0xffff) as f32 / 65535.0
}

fn hash_31(x: i32, y: i32, z: i32) -> f32 {
    let mut n = x
        .wrapping_mul(374761393)
        .wrapping_add(y.wrapping_mul(668265263))
        .wrapping_add(z.wrapping_mul(1274126177));
    n = (n ^ (n >> 13)).wrapping_mul(1274126177);
    (n & 0xffff) as f32 / 65535.0
}

fn value_noise_2(x: f32, y: f32) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x.fract();
    let fy = y.fract();
    let ux = fx * fx * (3.0 - 2.0 * fx);
    let uy = fy * fy * (3.0 - 2.0 * fy);
    let c00 = hash_21(x0, y0);
    let c10 = hash_21(x0 + 1, y0);
    let c01 = hash_21(x0, y0 + 1);
    let c11 = hash_21(x0 + 1, y0 + 1);
    let x0v = c00 + (c10 - c00) * ux;
    let x1v = c01 + (c11 - c01) * ux;
    x0v + (x1v - x0v) * uy
}

fn value_noise_3(x: f32, y: f32, z: f32) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let z0 = z.floor() as i32;
    let fx = x.fract();
    let fy = y.fract();
    let fz = z.fract();
    let ux = fx * fx * (3.0 - 2.0 * fx);
    let uy = fy * fy * (3.0 - 2.0 * fy);
    let uz = fz * fz * (3.0 - 2.0 * fz);
    let c000 = hash_31(x0, y0, z0);
    let c100 = hash_31(x0 + 1, y0, z0);
    let c010 = hash_31(x0, y0 + 1, z0);
    let c110 = hash_31(x0 + 1, y0 + 1, z0);
    let c001 = hash_31(x0, y0, z0 + 1);
    let c101 = hash_31(x0 + 1, y0, z0 + 1);
    let c011 = hash_31(x0, y0 + 1, z0 + 1);
    let c111 = hash_31(x0 + 1, y0 + 1, z0 + 1);
    let x00 = c000 + (c100 - c000) * ux;
    let x10 = c010 + (c110 - c010) * ux;
    let x01 = c001 + (c101 - c001) * ux;
    let x11 = c011 + (c111 - c011) * ux;
    let y0v = x00 + (x10 - x00) * uy;
    let y1v = x01 + (x11 - x01) * uy;
    y0v + (y1v - y0v) * uz
}

fn fbm2(x: f32, y: f32, octaves: u32) -> f32 {
    let mut amp = 0.5;
    let mut freq = 1.0;
    let mut sum = 0.0;
    let mut norm = 0.0;
    for _ in 0..octaves {
        sum += amp * value_noise_2(x * freq, y * freq);
        norm += amp;
        amp *= 0.5;
        freq *= 2.03;
    }
    sum / norm.max(1e-4)
}

fn fbm3(x: f32, y: f32, z: f32, octaves: u32) -> f32 {
    let mut amp = 0.5;
    let mut freq = 1.0;
    let mut sum = 0.0;
    let mut norm = 0.0;
    for _ in 0..octaves {
        sum += amp * value_noise_3(x * freq, y * freq, z * freq);
        norm += amp;
        amp *= 0.5;
        freq *= 2.03;
    }
    sum / norm.max(1e-4)
}

/// Dawn/dusk peak: 1 at the horizon transition, 0 at noon and deep night.
fn twilight_factor(night: f32) -> f32 {
    let t = 1.0 - ((night - 0.5).abs() * 2.0);
    t.clamp(0.0, 1.0).powf(1.2)
}

/// Extra golden-hour weight from low sun elevation (independent of night
/// ramp) so dawn/dusk tint even when `night_factor` is still near 0 or 1.
fn golden_hour_factor(sun_elevation: f32, night: f32) -> f32 {
    // Peak when sun is near the horizon but not fully night.
    let low_sun = (1.0 - (sun_elevation / 0.35).clamp(0.0, 1.0)).clamp(0.0, 1.0);
    let not_deep_night = (1.0 - ((night - 0.55).max(0.0) / 0.45)).clamp(0.0, 1.0);
    (low_sun * not_deep_night).clamp(0.0, 1.0)
}

fn wrap_xz(pos: Vec2, cam: Vec2, half: f32) -> Vec2 {
    let mut d = pos - cam;
    d.x = (d.x + half).rem_euclid(half * 2.0) - half;
    d.y = (d.y + half).rem_euclid(half * 2.0) - half;
    cam + d
}

#[allow(clippy::too_many_arguments)]
fn sync_atmosphere_system(
    quality: Res<QualityTier>,
    weather: Res<WeatherEffects>,
    day_night: Res<DayNightState>,
    subway: Res<SubwayView>,
    mut shadows: ResMut<CloudShadowParams>,
    mut commands: Commands,
    mut cameras_vol: Query<(Entity, Option<&mut VolumetricFog>), With<Camera3d>>,
    mut cameras_haze: Query<(Entity, Option<&mut DistanceFog>), With<Camera3d>>,
    suns: Query<(Entity, Option<&VolumetricLight>), With<Sun>>,
    mut volumes: Query<(&CloudBlob, &mut FogVolume, &mut Visibility)>,
) {
    let knobs = quality.knobs();
    let active = knobs.atmosphere_enabled
        && weather.enabled
        && subway.t < 0.45
        && knobs.shadow_map_size.is_some();

    if !active {
        shadows.strength = 0.0;
        for (_, mut vol, mut vis) in &mut volumes {
            *vis = Visibility::Hidden;
            vol.density_factor = 0.0;
        }
        for (entity, existing) in &mut cameras_vol {
            if existing.is_some() {
                commands.entity(entity).remove::<VolumetricFog>();
            }
        }
        for (entity, existing) in &mut cameras_haze {
            if existing.is_some() {
                commands.entity(entity).remove::<DistanceFog>();
            }
        }
        for (sun, has_vol) in &suns {
            if has_vol.is_some() {
                commands.entity(sun).remove::<VolumetricLight>();
            }
        }
        return;
    }

    let steps = knobs.atmosphere_fog_steps.max(16);
    let n = day_night.night_factor;
    let elev = day_night.sun_elevation;
    let twilight = twilight_factor(n);
    let golden = golden_hour_factor(elev, n).max(twilight * 0.85);

    // Warm gold at golden hour / twilight; cool blue at night; near-white midday.
    let day_fog = Color::srgb(0.94, 0.96, 0.98);
    let golden_fog = Color::srgb(1.0, 0.78, 0.52);
    let night_fog = palette::sky_night().mix(&Color::srgb(0.28, 0.34, 0.50), 0.30);
    let fog_color = day_fog
        .mix(&golden_fog, golden * 0.90)
        .mix(&night_fog, n * 0.85);

    let light_tint = Color::srgb(1.0, 0.97, 0.92)
        .mix(&Color::srgb(1.0, 0.68, 0.38), golden * 0.75)
        .mix(&Color::srgb(0.55, 0.65, 0.9), n);

    // Density reacts to time of day but is HARD-CLAMPED so the city always
    // reads — never a full-frame wash.
    let density_mul = 1.0 + golden * 0.35 + n * 0.20 + (1.0 - elev) * 0.10;
    let density = (CLOUD_DENSITY_BASE * density_mul).min(CLOUD_DENSITY_MAX);
    let asymmetry = (0.58 + (1.0 - elev) * 0.28 + golden * 0.08).clamp(0.3, 0.92);

    for (blob, mut vol, mut vis) in &mut volumes {
        *vis = Visibility::Visible;
        // Tiny per-index density jitter so overlapping edges don't stack to
        // a hard double-density band.
        let jitter = 1.0 - (blob.index as f32) * 0.06;
        vol.density_factor = (density * jitter).min(CLOUD_DENSITY_MAX);
        vol.scattering_asymmetry = asymmetry;
        vol.light_intensity = 0.75 + golden * 0.30;
        vol.fog_color = fog_color;
        vol.light_tint = light_tint;
        // Absorption stays modest — high absorption + density = gray soup.
        vol.absorption = 0.18 + n * 0.06;
        vol.scattering = 0.30 + golden * 0.08;
    }

    // Ground shadows: strongest in day/golden hour, quiet at night.
    shadows.strength =
        (SHADOW_STRENGTH_DAY * (1.0 - n * 0.55) + SHADOW_STRENGTH_NIGHT * n + golden * 0.06)
            .clamp(0.0, 0.28)
            * (1.0 - subway.t);

    // Ambient must stay tiny — this was a major "webcam fog" contributor.
    let vol_fog = VolumetricFog {
        ambient_color: fog_color,
        ambient_intensity: 0.015 + n * 0.025 + golden * 0.01,
        step_count: steps,
        jitter: if matches!(*quality, QualityTier::High) {
            0.40
        } else {
            0.28
        },
    };

    // Horizon haze only — long visibility, low alpha, never a milky veil.
    let haze_vis = HAZE_VISIBILITY_DAY_M + (HAZE_VISIBILITY_NIGHT_M - HAZE_VISIBILITY_DAY_M) * n
        - golden * 1_500.0;
    let haze_alpha = 0.22 + n * 0.18 + golden * 0.06;
    let extinction = fog_color.mix(&golden_fog, golden * 0.4);
    let inscatter = Color::srgb(0.88, 0.92, 1.0)
        .mix(&Color::srgb(1.0, 0.75, 0.48), golden)
        .mix(&Color::srgb(0.35, 0.42, 0.65), n);
    let haze = DistanceFog {
        color: fog_color.with_alpha(haze_alpha),
        directional_light_color: light_tint.with_alpha(0.30 * (1.0 - n * 0.65) + golden * 0.20),
        directional_light_exponent: 20.0 + elev * 12.0,
        falloff: FogFalloff::from_visibility_colors(haze_vis.max(8_000.0), extinction, inscatter),
    };

    for (entity, existing) in &mut cameras_vol {
        if let Some(mut existing) = existing {
            *existing = vol_fog;
        } else {
            commands.entity(entity).insert(vol_fog);
        }
    }
    for (entity, existing) in &mut cameras_haze {
        if let Some(mut existing) = existing {
            *existing = haze.clone();
        } else {
            commands.entity(entity).insert(haze.clone());
        }
    }
    for (sun, has_vol) in &suns {
        if has_vol.is_none() {
            commands.entity(sun).insert(VolumetricLight);
        }
    }
}

fn drift_cloud_volumes_system(
    time: Res<Time>,
    weather: Res<WeatherEffects>,
    quality: Res<QualityTier>,
    wind: Res<AtmosphereWind>,
    camera_xforms: Query<&GlobalTransform, With<Camera3d>>,
    mut volumes: Query<(&CloudBlob, &mut Transform), With<FogVolume>>,
) {
    if !quality.knobs().atmosphere_enabled || !weather.enabled {
        return;
    }
    let dt = time.delta_secs();
    let dir = Vec2::new(wind.heading.cos(), wind.heading.sin());
    let cam_xz = camera_xforms
        .iter()
        .next()
        .map(|t| {
            let p = t.translation();
            Vec2::new(p.x, p.z)
        })
        .unwrap_or(Vec2::ZERO);

    for (blob, mut transform) in &mut volumes {
        let speed = CLOUD_DRIFT_SPEED * wind.gust * (1.0 - blob.index as f32 * 0.08);
        // Slight cross-wind so the three blobs don't lock-step.
        let shear = Vec2::new(-dir.y, dir.x) * (0.18 + blob.index as f32 * 0.05);
        let flow = (dir + shear) * speed * dt;
        let mut xz = Vec2::new(transform.translation.x, transform.translation.z) + flow;
        xz = wrap_xz(xz, cam_xz, CLOUD_DOMAIN_HALF_M);
        // Per-blob scale jitter (stable) so they don't look identical.
        let scale_mul = 0.88 + (blob.index as f32) * 0.10;
        transform.translation.x = xz.x;
        transform.translation.y = CLOUD_CENTER_Y_M + (blob.index as f32) * 40.0;
        transform.translation.z = xz.y;
        transform.scale = Vec3::new(
            CLOUD_SIZE_XZ_M * scale_mul,
            CLOUD_THICKNESS_M,
            CLOUD_SIZE_XZ_M * scale_mul,
        );
    }
}

fn scroll_cloud_shadows_system(
    time: Res<Time>,
    weather: Res<WeatherEffects>,
    quality: Res<QualityTier>,
    wind: Res<AtmosphereWind>,
    mut shadows: ResMut<CloudShadowParams>,
) {
    if !quality.knobs().atmosphere_enabled || !weather.enabled {
        return;
    }
    let dt = time.delta_secs();
    let dir = Vec2::new(wind.heading.cos(), wind.heading.sin());
    shadows.offset += dir * SHADOW_SCROLL * wind.gust * dt;
    // Keep UV offsets bounded for precision.
    shadows.offset.x = shadows.offset.x.rem_euclid(1.0);
    shadows.offset.y = shadows.offset.y.rem_euclid(1.0);
    shadows.inv_scale = 1.0 / SHADOW_TILE_M;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twilight_peaks_near_half_night_factor() {
        assert!(twilight_factor(0.5) > twilight_factor(0.0));
        assert!(twilight_factor(0.5) > twilight_factor(1.0));
        assert!(twilight_factor(0.5) > 0.9);
    }

    #[test]
    fn golden_hour_peaks_at_low_sun() {
        assert!(golden_hour_factor(0.05, 0.3) > golden_hour_factor(0.9, 0.0));
        assert!(golden_hour_factor(0.1, 0.95) < golden_hour_factor(0.1, 0.3));
    }

    #[test]
    fn density_clamp_is_hard_ceiling() {
        let mul = 1.0 + 0.35 + 0.20 + 0.10;
        let d = (CLOUD_DENSITY_BASE * mul).min(CLOUD_DENSITY_MAX);
        assert!(d <= CLOUD_DENSITY_MAX);
        const {
            assert!(CLOUD_DENSITY_BASE < CLOUD_DENSITY_MAX);
        }
    }

    #[test]
    fn fbm_stays_in_unit_range() {
        for i in 0..8 {
            let t = i as f32 * 0.37;
            let f = fbm3(t, t * 1.3, t * 0.7, 4);
            assert!((0.0..=1.0).contains(&f), "fbm3={f}");
            let f2 = fbm2(t, t * 1.3, 4);
            assert!((0.0..=1.0).contains(&f2), "fbm2={f2}");
        }
    }

    #[test]
    fn wrap_keeps_offset_inside_domain() {
        let cam = Vec2::new(100.0, -50.0);
        let far = cam + Vec2::new(CLOUD_DOMAIN_HALF_M * 3.0, -CLOUD_DOMAIN_HALF_M * 2.5);
        let w = wrap_xz(far, cam, CLOUD_DOMAIN_HALF_M);
        let d = w - cam;
        assert!(d.x.abs() <= CLOUD_DOMAIN_HALF_M + 1e-3);
        assert!(d.y.abs() <= CLOUD_DOMAIN_HALF_M + 1e-3);
    }
}
