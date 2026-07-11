//! Atmospheric weather (scrolling volumetric fog/cloud + distance haze).
//!
//! Enabled on Medium/High when [`mf_state::WeatherEffects`] is on. Uses
//! Bevy's `FogVolume` with a procedurally generated repeating 3D noise
//! density texture whose UVW offset scrolls every frame — the same pattern
//! as Bevy's `scrolling_fog` example, without shipping a `.ktx2` asset
//! (this binary has no `assets/` folder; see `reveal.rs` for the same
//! constraint).
//!
//! Potato/Low stay clear: volumetric fog needs directional shadow maps,
//! which those tiers disable.

use bevy::asset::RenderAssetUsages;
use bevy::image::{
    ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor,
};
use bevy::pbr::{FogVolume, VolumetricFog, VolumetricLight};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use mf_state::{CurrentCity, QualityTier, SubwayView, WeatherEffects};

use crate::daynight::{DayNightState, Sun};
use crate::palette;

/// Marker on the single city-scale fog volume we own.
#[derive(Component)]
struct AtmosphereFogVolume;

pub struct MfAtmospherePlugin;

impl Plugin for MfAtmospherePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_atmosphere_system).add_systems(
            Update,
            (
                sync_atmosphere_system.after(crate::daynight::apply_day_night_system),
                scroll_atmosphere_fog_system,
            )
                .chain()
                .in_set(crate::MfRenderSet::Dynamic),
        );
    }
}

/// Noise resolution — 32³ is enough for soft cloud blobs at city scale and
/// keeps the one-time CPU bake cheap (~32 KB).
const NOISE_RES: u32 = 32;

/// Horizontal fog extent as a multiple of city `world_size`.
const FOG_EXTENT_FACTOR: f32 = 1.4;
/// Fog slab thickness (meters) and center altitude.
const FOG_THICKNESS_M: f32 = 700.0;
const FOG_CENTER_Y_M: f32 = 380.0;
/// Base world-space density; kept low so a multi-km volume stays translucent.
const FOG_DENSITY: f32 = 0.00022;
/// Scroll velocity in density-texture UVW units per second (fluid drift).
const FOG_SCROLL_UVW_PER_SEC: Vec3 = Vec3::new(0.012, 0.004, 0.018);
/// Distance-haze visibility (meters) for [`DistanceFog`].
const HAZE_VISIBILITY_M: f32 = 14_000.0;

fn setup_atmosphere_system(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let noise = images.add(build_fog_noise_texture());
    commands.spawn((
        AtmosphereFogVolume,
        FogVolume {
            density_texture: Some(noise),
            density_factor: FOG_DENSITY,
            absorption: 0.25,
            scattering: 0.35,
            scattering_asymmetry: 0.6,
            fog_color: Color::srgb(0.92, 0.95, 0.98),
            light_intensity: 0.85,
            ..default()
        },
        Transform::from_xyz(0.0, FOG_CENTER_Y_M, 0.0)
            .with_scale(Vec3::new(20_000.0, FOG_THICKNESS_M, 20_000.0)),
        Visibility::Hidden,
    ));
}

/// Value-noise 3D density texture with Repeat + Linear sampling so scrolling
/// UVW offsets wrap without seams or pixelation.
fn build_fog_noise_texture() -> Image {
    let n = NOISE_RES as usize;
    let mut data = vec![0u8; n * n * n];
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                // Two octaves of hashed value noise → soft cloud blobs.
                let u = x as f32 / n as f32;
                let v = y as f32 / n as f32;
                let w = z as f32 / n as f32;
                let a = value_noise_3(u * 3.0, v * 2.0, w * 3.0);
                let b = value_noise_3(u * 7.0 + 17.0, v * 5.0 + 9.0, w * 7.0 + 3.0);
                let dens = (a * 0.65 + b * 0.35).clamp(0.0, 1.0);
                // Soft threshold so empty sky stays mostly clear.
                let shaped = ((dens - 0.35).max(0.0) / 0.65).powf(1.4);
                data[z * n * n + y * n + x] = (shaped * 255.0).round() as u8;
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: NOISE_RES,
            height: NOISE_RES,
            depth_or_array_layers: NOISE_RES,
        },
        TextureDimension::D3,
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

fn hash_31(x: i32, y: i32, z: i32) -> f32 {
    let mut n = x
        .wrapping_mul(374761393)
        .wrapping_add(y.wrapping_mul(668265263))
        .wrapping_add(z.wrapping_mul(1274126177));
    n = (n ^ (n >> 13)).wrapping_mul(1274126177);
    (n & 0xffff) as f32 / 65535.0
}

fn value_noise_3(x: f32, y: f32, z: f32) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let z0 = z.floor() as i32;
    let fx = x.fract();
    let fy = y.fract();
    let fz = z.fract();
    // Smoothstep fade.
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

#[allow(clippy::too_many_arguments)]
fn sync_atmosphere_system(
    quality: Res<QualityTier>,
    weather: Res<WeatherEffects>,
    day_night: Res<DayNightState>,
    subway: Res<SubwayView>,
    city: Res<CurrentCity>,
    mut commands: Commands,
    mut cameras_vol: Query<(Entity, Option<&mut VolumetricFog>), With<Camera3d>>,
    mut cameras_haze: Query<(Entity, Option<&mut DistanceFog>), With<Camera3d>>,
    suns: Query<(Entity, Option<&VolumetricLight>), With<Sun>>,
    mut volumes: Query<
        (&mut FogVolume, &mut Transform, &mut Visibility),
        With<AtmosphereFogVolume>,
    >,
    camera_xforms: Query<&GlobalTransform, With<Camera3d>>,
) {
    let knobs = quality.knobs();
    // Subway view is its own lighting language — kill atmosphere while
    // transitioned in so the vignette/metro tube stay readable.
    let active = knobs.atmosphere_enabled
        && weather.enabled
        && subway.t < 0.45
        && knobs.shadow_map_size.is_some();

    if active {
        let steps = knobs.atmosphere_fog_steps.max(16);
        let n = day_night.night_factor;
        let fog_color = palette::sky_day().mix(&palette::sky_night(), n * 0.85);
        let haze_alpha = 0.55 + n * 0.25;
        let density = FOG_DENSITY * (1.0 + n * 0.35);

        let world = city
            .static_city
            .as_ref()
            .map(|c| c.world_size as f32)
            .unwrap_or(20_000.0);
        let extent = (world * FOG_EXTENT_FACTOR).max(8_000.0);
        let cam_xz = camera_xforms
            .iter()
            .next()
            .map(|t| {
                let p = t.translation();
                Vec2::new(p.x, p.z)
            })
            .unwrap_or(Vec2::ZERO);

        for (mut vol, mut transform, mut vis) in &mut volumes {
            *vis = Visibility::Visible;
            vol.density_factor = density;
            vol.fog_color = fog_color;
            vol.light_tint = Color::srgb(1.0, 0.96, 0.9).mix(&Color::srgb(0.55, 0.65, 0.9), n);
            *transform = Transform::from_xyz(cam_xz.x, FOG_CENTER_Y_M, cam_xz.y)
                .with_scale(Vec3::new(extent, FOG_THICKNESS_M, extent));
        }

        let vol_fog = VolumetricFog {
            ambient_color: fog_color,
            ambient_intensity: 0.08 + n * 0.06,
            step_count: steps,
            jitter: 0.35,
        };
        let haze = DistanceFog {
            color: fog_color.with_alpha(haze_alpha),
            directional_light_color: Color::srgba(1.0, 0.95, 0.85, 0.35 * (1.0 - n * 0.7)),
            directional_light_exponent: 24.0,
            falloff: FogFalloff::from_visibility(HAZE_VISIBILITY_M),
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
    } else {
        for (mut vol, _, mut vis) in &mut volumes {
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
    }
}

fn scroll_atmosphere_fog_system(
    time: Res<Time>,
    weather: Res<WeatherEffects>,
    quality: Res<QualityTier>,
    mut volumes: Query<&mut FogVolume, With<AtmosphereFogVolume>>,
) {
    if !quality.knobs().atmosphere_enabled || !weather.enabled {
        return;
    }
    let dt = time.delta_secs();
    for mut vol in &mut volumes {
        vol.density_texture_offset += FOG_SCROLL_UVW_PER_SEC * dt;
    }
}
