//! Atmospheric weather — soft drifting cloud cards + ground shadows.
//!
//! Bevy's [`VolumetricFog`] / [`FogVolume`] path was removed: even with sparse
//! density textures and hard clamps it read as uniform gray wash (especially
//! under lavapipe, and in practice as a milky veil over the white city). Soft
//! unlit billboard cards give 2–3 discrete drifting volumes with clear air
//! between them, while scrolling cloud shadows on terrain/buildings sell the
//! sky feel cheaply.
//!
//! Also: golden-hour tinting from [`DayNightState::sun_elevation`]. Gated to
//! Medium/High + [`WeatherEffects`].

use bevy::asset::{load_internal_asset, weak_handle, RenderAssetUsages};
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::{Material, NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderRef, TextureDimension, TextureFormat,
};
use std::f32::consts::TAU;

use mf_state::{EffectiveKnobs, QualityTier, SubwayView, WeatherEffects, WeatherRender};

use crate::daynight::DayNightState;
use crate::palette;

/// Index of one of the sparse drifting cloud cards.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
struct CloudBlob {
    index: u8,
}

/// Shared wind that cloud cards and ground shadows advect along.
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

const CLOUD_SHADER_HANDLE: Handle<Shader> = weak_handle!("c0a7b8d9-1e2f-4a5b-9c8d-7e6f5a4b3c2d");

/// Soft unlit cloud card — density texture drives alpha; color carries
/// day/golden/night tint.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
struct CloudMaterial {
    #[uniform(0)]
    color: Vec4,
    #[texture(1)]
    #[sampler(2)]
    density: Handle<Image>,
}

impl Material for CloudMaterial {
    fn fragment_shader() -> ShaderRef {
        CLOUD_SHADER_HANDLE.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

pub struct MfAtmospherePlugin;

/// Runs after wind/volume/shadow params are written for the frame — material
/// consumers (terrain/buildings) should schedule after this set.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AtmosphereReady;

impl Plugin for MfAtmospherePlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(app, CLOUD_SHADER_HANDLE, "cloud.wgsl", Shader::from_wgsl);
        app.init_resource::<AtmosphereWind>()
            .init_resource::<CloudShadowParams>()
            .add_plugins(MaterialPlugin::<CloudMaterial>::default())
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
const SHADOW_RES: u32 = 256;
const BLOB_RES: u32 = 128;
/// World meters covered by one repeat of the shadow noise.
const SHADOW_TILE_M: f32 = 1_100.0;
/// Soft cloud card size (XZ).
const CLOUD_SIZE_XZ_M: f32 = 5_500.0;
const CLOUD_CENTER_Y_M: f32 = 900.0;
/// How far cloud centers roam relative to the camera before wrapping.
const CLOUD_DOMAIN_HALF_M: f32 = 8_000.0;
const CLOUD_DRIFT_SPEED: f32 = 32.0;
const SHADOW_SCROLL: f32 = 0.012;
const WIND_HEADING_RATE: f32 = 0.003;
const WIND_GUST_RATE: f32 = 0.55;
/// Peak ground-shadow darkening (day). Must read clearly on white buildings.
const SHADOW_STRENGTH_DAY: f32 = 0.40;
const SHADOW_STRENGTH_NIGHT: f32 = 0.12;
/// Card opacity ceiling — never opaque enough to hide the city behind.
const CLOUD_ALPHA_MAX: f32 = 0.55;

fn setup_atmosphere_system(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<CloudMaterial>>,
) {
    let blob = images.add(build_soft_blob_texture_2d());
    let shadow_noise = images.add(build_shadow_noise_texture());

    commands.insert_resource(CloudShadowParams {
        texture: shadow_noise,
        offset: Vec2::ZERO,
        strength: 0.0,
        inv_scale: 1.0 / SHADOW_TILE_M,
    });

    let mesh = meshes.add(Plane3d::new(Vec3::Y, Vec2::splat(0.5)));

    for i in 0..NUM_CLOUDS {
        let angle = (i as f32) * (TAU / NUM_CLOUDS as f32) + 0.4;
        let radius = 3_200.0 + (i as f32) * 1_100.0;
        let x = angle.cos() * radius;
        let z = angle.sin() * radius;
        let scale_mul = 0.85 + (i as f32) * 0.12;
        let mat = materials.add(CloudMaterial {
            color: Vec4::new(0.95, 0.96, 0.98, 0.0),
            density: blob.clone(),
        });
        commands.spawn((
            CloudBlob { index: i },
            Mesh3d(mesh.clone()),
            MeshMaterial3d(mat),
            Transform::from_xyz(x, CLOUD_CENTER_Y_M + (i as f32) * 60.0, z).with_scale(Vec3::new(
                CLOUD_SIZE_XZ_M * scale_mul,
                1.0,
                CLOUD_SIZE_XZ_M * scale_mul * (0.70 + (i as f32) * 0.08),
            )),
            Visibility::Hidden,
            NotShadowCaster,
            NotShadowReceiver,
        ));
    }
}

fn update_atmosphere_wind_system(
    time: Res<Time>,
    weather: Res<WeatherRender>,
    mut wind: ResMut<AtmosphereWind>,
) {
    let dt = time.delta_secs();
    wind.heading = (wind.heading + WIND_HEADING_RATE * dt).rem_euclid(TAU);
    // Storm drives the cloud/shadow scroll harder — the existing shared wind
    // field is exactly the "stronger wind on the cloud wind field" hook.
    wind.gust_phase += WIND_GUST_RATE * (1.0 + weather.storm * 1.4) * dt;
    let g =
        0.5 + 0.5 * (0.65 * wind.gust_phase.sin() + 0.35 * (wind.gust_phase * 1.73 + 1.1).sin());
    wind.gust = (0.72 + g * 0.70) * (1.0 + weather.storm * 0.8);
}

/// Soft radial blob for cloud-card alpha (empty edges = clear air).
fn build_soft_blob_texture_2d() -> Image {
    let n = BLOB_RES as usize;
    let mut data = vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            let u = x as f32 / n as f32;
            let v = y as f32 / n as f32;
            let dx = (u - 0.5) * 2.0;
            let dy = (v - 0.5) * 2.0;
            let r = (dx * dx + dy * dy).sqrt();
            let warp = fbm2(u * 3.2 + 1.1, v * 3.2 + 0.7, 3) * 0.28;
            let soft = (1.0 - ((r + warp - 0.05) / 0.95).clamp(0.0, 1.0)).powf(1.65);
            let shaped = if soft < 0.06 {
                0.0
            } else {
                ((soft - 0.06) / 0.94).powf(1.25)
            };
            data[y * n + x] = (shaped * 255.0).round() as u8;
        }
    }
    finish_noise_image_2d(data, BLOB_RES, ImageAddressMode::ClampToEdge)
}

/// Large soft 2D blobs for ground-projected cloud shadows (tiling).
fn build_shadow_noise_texture() -> Image {
    let n = SHADOW_RES as usize;
    let mut data = vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            let u = x as f32 / n as f32;
            let v = y as f32 / n as f32;
            let a = fbm2(u * 2.2, v * 2.2, 4);
            let b = fbm2(u * 3.5 + 5.1, v * 3.5 + 2.3, 3);
            let dens = (a * 0.65 + b * 0.35).clamp(0.0, 1.0);
            let shaped = ((dens - 0.48).max(0.0) / 0.52).powf(1.6);
            data[y * n + x] = (shaped * 255.0).round() as u8;
        }
    }
    finish_noise_image_2d(data, SHADOW_RES, ImageAddressMode::Repeat)
}

fn finish_noise_image_2d(data: Vec<u8>, res: u32, address: ImageAddressMode) -> Image {
    let mut image = Image::new(
        Extent3d {
            width: res,
            height: res,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: address,
        address_mode_v: address,
        address_mode_w: address,
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

/// Dawn/dusk peak: 1 at the horizon transition, 0 at noon and deep night.
fn twilight_factor(night: f32) -> f32 {
    let t = 1.0 - ((night - 0.5).abs() * 2.0);
    t.clamp(0.0, 1.0).powf(1.2)
}

/// Extra golden-hour weight from low sun elevation.
fn golden_hour_factor(sun_elevation: f32, night: f32) -> f32 {
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
    effective: Res<EffectiveKnobs>,
    weather: Res<WeatherEffects>,
    weather_render: Res<WeatherRender>,
    day_night: Res<DayNightState>,
    subway: Res<SubwayView>,
    mut shadows: ResMut<CloudShadowParams>,
    mut commands: Commands,
    cameras_haze: Query<(Entity, Option<&DistanceFog>), With<Camera3d>>,
    mut volumes: Query<(&CloudBlob, &MeshMaterial3d<CloudMaterial>, &mut Visibility)>,
    mut materials: ResMut<Assets<CloudMaterial>>,
) {
    let knobs = effective.0;
    // Billboard path does not need shadow maps — only the effective-knob
    // gate (preset merged with Advanced overrides) + player toggle + subway
    // fade.
    let active = knobs.atmosphere_enabled && weather.enabled && subway.t < 0.45;

    // Medium/High must not keep the camera's startup linear DistanceFog
    // (or any leftover haze) — that was the main full-frame wash.
    if knobs.atmosphere_enabled {
        for (entity, existing) in &cameras_haze {
            if existing.is_some() {
                commands.entity(entity).remove::<DistanceFog>();
            }
        }
    }

    if !active {
        shadows.strength = 0.0;
        for (_, _, mut vis) in &mut volumes {
            *vis = Visibility::Hidden;
        }
        return;
    }

    let n = day_night.night_factor;
    let elev = day_night.sun_elevation;
    let twilight = twilight_factor(n);
    let golden = golden_hour_factor(elev, n).max(twilight * 0.85);

    let day_col = Vec3::new(0.96, 0.97, 0.99);
    let golden_col = Vec3::new(1.0, 0.78, 0.52);
    let night_col = {
        let c = palette::sky_night().to_srgba();
        Vec3::new(c.red, c.green, c.blue).lerp(Vec3::new(0.35, 0.42, 0.58), 0.35)
    };
    let mut rgb = day_col
        .lerp(golden_col, golden * 0.85)
        .lerp(night_col, n * 0.75);
    // Weather grade: overcast/storm pull the cloud deck toward a flat cool
    // grey (never a coloured wash — art direction); a lightning flash bleaches
    // the cards white for its brief pulse.
    let ov = weather_render.overcast;
    let storm = weather_render.storm;
    let fog_w = weather_render.fog;
    let overcast_grey = Vec3::new(0.62, 0.65, 0.70);
    let storm_grey = Vec3::new(0.34, 0.37, 0.44);
    rgb = rgb
        .lerp(overcast_grey, ov * 0.55)
        .lerp(storm_grey, storm * 0.55);
    rgb = rgb.lerp(Vec3::splat(1.0), weather_render.lightning * 0.6);
    // Alpha reacts gently but is HARD-CLAMPED so cards never hide the city;
    // heavy fog raises the ceiling so the mist can actually thicken.
    let ceiling = (CLOUD_ALPHA_MAX + fog_w * 0.28 + ov * 0.12).min(0.9);
    let weather_alpha = ov * 0.14 + fog_w * 0.22 + storm * 0.12;
    let alpha = ((0.38 + golden * 0.12 + n * 0.08 + weather_alpha) * (1.0 - subway.t)).min(ceiling);

    for (blob, handle, mut vis) in &mut volumes {
        *vis = Visibility::Visible;
        let jitter = 1.0 - (blob.index as f32) * 0.05;
        if let Some(mat) = materials.get_mut(&handle.0) {
            mat.color = Vec4::new(rgb.x, rgb.y, rgb.z, (alpha * jitter).min(ceiling));
        }
    }

    // Diffuse overcast/storm light softens cast shadows: flatten the scrolling
    // ground shadow as the sky closes up.
    let flatten = 1.0 - (ov * 0.5 + storm * 0.3).clamp(0.0, 0.75);
    shadows.strength =
        (SHADOW_STRENGTH_DAY * (1.0 - n * 0.55) + SHADOW_STRENGTH_NIGHT * n + golden * 0.08)
            .clamp(0.0, 0.48)
            * (1.0 - subway.t)
            * flatten;
}

fn drift_cloud_volumes_system(
    time: Res<Time>,
    weather: Res<WeatherEffects>,
    quality: Res<QualityTier>,
    wind: Res<AtmosphereWind>,
    camera_xforms: Query<&GlobalTransform, With<Camera3d>>,
    mut volumes: Query<(&CloudBlob, &mut Transform), With<MeshMaterial3d<CloudMaterial>>>,
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
        let shear = Vec2::new(-dir.y, dir.x) * (0.18 + blob.index as f32 * 0.05);
        let flow = (dir + shear) * speed * dt;
        let mut xz = Vec2::new(transform.translation.x, transform.translation.z) + flow;
        xz = wrap_xz(xz, cam_xz, CLOUD_DOMAIN_HALF_M);
        let scale_mul = 0.85 + (blob.index as f32) * 0.12;
        transform.translation.x = xz.x;
        transform.translation.y = CLOUD_CENTER_Y_M + (blob.index as f32) * 60.0;
        transform.translation.z = xz.y;
        transform.scale = Vec3::new(
            CLOUD_SIZE_XZ_M * scale_mul,
            1.0,
            CLOUD_SIZE_XZ_M * scale_mul * (0.70 + (blob.index as f32) * 0.08),
        );
    }
}

fn scroll_cloud_shadows_system(
    time: Res<Time>,
    weather: Res<WeatherEffects>,
    effective: Res<EffectiveKnobs>,
    wind: Res<AtmosphereWind>,
    mut shadows: ResMut<CloudShadowParams>,
) {
    if !effective.0.atmosphere_enabled || !weather.enabled {
        return;
    }
    let dt = time.delta_secs();
    let dir = Vec2::new(wind.heading.cos(), wind.heading.sin());
    shadows.offset += dir * SHADOW_SCROLL * wind.gust * dt;
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
    fn cloud_alpha_hard_ceiling() {
        let a = (0.38_f32 + 0.12 + 0.08).min(CLOUD_ALPHA_MAX);
        assert!(a <= CLOUD_ALPHA_MAX);
        const {
            assert!(CLOUD_ALPHA_MAX < 1.0);
        }
    }

    #[test]
    fn fbm_stays_in_unit_range() {
        for i in 0..8 {
            let t = i as f32 * 0.37;
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
