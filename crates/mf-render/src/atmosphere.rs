//! Atmospheric weather — level 2.
//!
//! Two stacked [`FogVolume`]s (ground mist + high cloud deck) share a slow
//! wind field so density scrolls like fluid rather than a single sliding
//! slab. Each layer gets its own procedural 3D noise (soft FBM mist vs
//! cellular cloud blobs). Density, tint, and god-ray asymmetry react to
//! sun elevation from [`DayNightState`] — thicker/warmer at dawn and dusk,
//! cooler and thinner at noon, denser at night.
//!
//! Gated to Medium/High + [`WeatherEffects`]; Potato/Low stay clear because
//! volumetric fog needs directional shadow maps.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::{FogVolume, VolumetricFog, VolumetricLight};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::f32::consts::TAU;

use mf_state::{CurrentCity, QualityTier, SubwayView, WeatherEffects};

use crate::daynight::{DayNightState, Sun};
use crate::palette;

/// Which slab of the dual-layer atmosphere this volume is.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum AtmosphereLayer {
    /// Low, soft ground mist — slow drift, fills valleys.
    Mist,
    /// High cloud deck — faster wind, cellular blobs, god-ray medium.
    Clouds,
}

/// Shared wind that both layers advect along. Heading creeps; gust breathes.
#[derive(Resource, Clone, Copy)]
struct AtmosphereWind {
    /// Radians, slowly rotates so the city doesn't get a permanent "wind from
    /// the west" look over a long session.
    heading: f32,
    /// Multiplier on base scroll speed, ~0.7..1.45.
    gust: f32,
    /// Phase accumulator for the gust oscillator.
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

pub struct MfAtmospherePlugin;

impl Plugin for MfAtmospherePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AtmosphereWind>()
            .add_systems(Startup, setup_atmosphere_system)
            .add_systems(
                Update,
                (
                    update_atmosphere_wind_system,
                    sync_atmosphere_system.after(crate::daynight::apply_day_night_system),
                    scroll_atmosphere_fog_system,
                )
                    .chain()
                    .in_set(crate::MfRenderSet::Dynamic),
            );
    }
}

const NOISE_RES: u32 = 40;
const FOG_EXTENT_FACTOR: f32 = 1.5;
/// Mist slab: near-ground, thinner vertically.
const MIST_THICKNESS_M: f32 = 280.0;
const MIST_CENTER_Y_M: f32 = 120.0;
const MIST_DENSITY: f32 = 0.00028;
const MIST_SCROLL: f32 = 0.010;
/// Cloud deck: higher, thicker, faster.
const CLOUD_THICKNESS_M: f32 = 900.0;
const CLOUD_CENTER_Y_M: f32 = 720.0;
const CLOUD_DENSITY: f32 = 0.00018;
const CLOUD_SCROLL: f32 = 0.022;
/// Cross-wind shear so layers don't lock-step.
const CLOUD_SHEAR: f32 = 0.35;
const HAZE_VISIBILITY_DAY_M: f32 = 16_000.0;
const HAZE_VISIBILITY_NIGHT_M: f32 = 9_000.0;
/// Heading creep (rad/s) — a full circle every ~35 minutes of wall time.
const WIND_HEADING_RATE: f32 = 0.003;
const WIND_GUST_RATE: f32 = 0.55;

fn setup_atmosphere_system(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mist_noise = images.add(build_mist_noise_texture());
    let cloud_noise = images.add(build_cloud_noise_texture());

    commands.spawn((
        AtmosphereLayer::Mist,
        FogVolume {
            density_texture: Some(mist_noise),
            density_factor: MIST_DENSITY,
            absorption: 0.22,
            scattering: 0.40,
            scattering_asymmetry: 0.45,
            fog_color: Color::srgb(0.94, 0.96, 0.98),
            light_intensity: 0.7,
            ..default()
        },
        Transform::from_xyz(0.0, MIST_CENTER_Y_M, 0.0).with_scale(Vec3::new(
            20_000.0,
            MIST_THICKNESS_M,
            20_000.0,
        )),
        Visibility::Hidden,
    ));

    commands.spawn((
        AtmosphereLayer::Clouds,
        FogVolume {
            density_texture: Some(cloud_noise),
            density_factor: CLOUD_DENSITY,
            absorption: 0.28,
            scattering: 0.38,
            scattering_asymmetry: 0.72,
            fog_color: Color::srgb(0.90, 0.93, 0.97),
            light_intensity: 1.0,
            ..default()
        },
        Transform::from_xyz(0.0, CLOUD_CENTER_Y_M, 0.0).with_scale(Vec3::new(
            20_000.0,
            CLOUD_THICKNESS_M,
            20_000.0,
        )),
        Visibility::Hidden,
    ));
}

fn update_atmosphere_wind_system(time: Res<Time>, mut wind: ResMut<AtmosphereWind>) {
    let dt = time.delta_secs();
    wind.heading = (wind.heading + WIND_HEADING_RATE * dt).rem_euclid(TAU);
    wind.gust_phase += WIND_GUST_RATE * dt;
    // Two incommensurate sines → irregular breathing, not a metronome.
    let g =
        0.5 + 0.5 * (0.65 * wind.gust_phase.sin() + 0.35 * (wind.gust_phase * 1.73 + 1.1).sin());
    wind.gust = 0.72 + g * 0.70;
}

/// Soft, filled FBM — reads as ground mist / haze banks.
fn build_mist_noise_texture() -> Image {
    let n = NOISE_RES as usize;
    let mut data = vec![0u8; n * n * n];
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let u = x as f32 / n as f32;
                let v = y as f32 / n as f32;
                let w = z as f32 / n as f32;
                // Vertical falloff bias: denser toward the bottom of the slab.
                let height_w = 1.0 - (v - 0.15).abs().clamp(0.0, 1.0) * 0.55;
                let fbm = fbm3(u * 2.5, v * 1.4, w * 2.5, 4) * height_w;
                let shaped = (fbm * 1.15).clamp(0.0, 1.0).powf(1.15);
                data[z * n * n + y * n + x] = (shaped * 255.0).round() as u8;
            }
        }
    }
    finish_noise_image(data)
}

/// Cellular + FBM hybrid — discrete cloud puffs with soft edges.
fn build_cloud_noise_texture() -> Image {
    let n = NOISE_RES as usize;
    let mut data = vec![0u8; n * n * n];
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let u = x as f32 / n as f32;
                let v = y as f32 / n as f32;
                let w = z as f32 / n as f32;
                let cells = 1.0 - worley3(u * 3.5, v * 2.2, w * 3.5);
                let detail = fbm3(u * 6.0 + 3.0, v * 4.0, w * 6.0 + 1.0, 3);
                let dens = (cells * 0.72 + detail * 0.28).clamp(0.0, 1.0);
                // Harder threshold → more empty sky between clouds.
                let shaped = ((dens - 0.42).max(0.0) / 0.58).powf(1.55);
                data[z * n * n + y * n + x] = (shaped * 255.0).round() as u8;
            }
        }
    }
    finish_noise_image(data)
}

fn finish_noise_image(data: Vec<u8>) -> Image {
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

/// Chebyshev-ish Worley: distance to nearest hashed cell feature point.
fn worley3(x: f32, y: f32, z: f32) -> f32 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let zi = z.floor() as i32;
    let fx = x.fract();
    let fy = y.fract();
    let fz = z.fract();
    let mut min_d = 1.0_f32;
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let cx = xi + dx;
                let cy = yi + dy;
                let cz = zi + dz;
                let px = dx as f32 + hash_31(cx, cy, cz);
                let py = dy as f32 + hash_31(cx + 19, cy - 7, cz + 3);
                let pz = dz as f32 + hash_31(cx - 11, cy + 5, cz + 29);
                let d = (Vec3::new(px, py, pz) - Vec3::new(fx, fy, fz)).length();
                min_d = min_d.min(d);
            }
        }
    }
    min_d.clamp(0.0, 1.0)
}

/// Dawn/dusk peak: 1 at the horizon transition, 0 at noon and deep night.
fn twilight_factor(night: f32) -> f32 {
    // night_factor ramps 0→1 across dusk; peak the band around 0.35..0.65.
    let t = 1.0 - ((night - 0.5).abs() * 2.0);
    t.clamp(0.0, 1.0).powf(1.2)
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
    mut volumes: Query<(
        &AtmosphereLayer,
        &mut FogVolume,
        &mut Transform,
        &mut Visibility,
    )>,
    camera_xforms: Query<&GlobalTransform, With<Camera3d>>,
) {
    let knobs = quality.knobs();
    let active = knobs.atmosphere_enabled
        && weather.enabled
        && subway.t < 0.45
        && knobs.shadow_map_size.is_some();

    if !active {
        for (_, mut vol, _, mut vis) in &mut volumes {
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

    // Warm gold at twilight, cool blue at night, near-white midday.
    let day_fog = Color::srgb(0.93, 0.95, 0.97);
    let twilight_fog = Color::srgb(1.0, 0.82, 0.62);
    let night_fog = palette::sky_night().mix(&Color::srgb(0.25, 0.32, 0.48), 0.35);
    let fog_color = day_fog
        .mix(&twilight_fog, twilight * 0.85)
        .mix(&night_fog, n * 0.9);

    let light_tint = Color::srgb(1.0, 0.97, 0.90)
        .mix(&Color::srgb(1.0, 0.72, 0.45), twilight * 0.7)
        .mix(&Color::srgb(0.55, 0.65, 0.9), n);

    // Mist thickens at twilight + night; clouds thin slightly at noon so the
    // white city stays readable under high sun.
    let mist_mul = 1.0 + twilight * 0.55 + n * 0.40;
    let cloud_mul = 0.85 + twilight * 0.50 + n * 0.25 + (1.0 - elev) * 0.15;
    // Low sun → stronger forward scatter (god rays punch through clouds).
    let asymmetry = 0.55 + (1.0 - elev) * 0.30 + twilight * 0.08;

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

    for (layer, mut vol, mut transform, mut vis) in &mut volumes {
        *vis = Visibility::Visible;
        match *layer {
            AtmosphereLayer::Mist => {
                vol.density_factor = MIST_DENSITY * mist_mul;
                vol.scattering_asymmetry = (asymmetry * 0.75).clamp(0.2, 0.9);
                vol.light_intensity = 0.65 + twilight * 0.25;
                *transform = Transform::from_xyz(cam_xz.x, MIST_CENTER_Y_M, cam_xz.y)
                    .with_scale(Vec3::new(extent, MIST_THICKNESS_M, extent));
            }
            AtmosphereLayer::Clouds => {
                vol.density_factor = CLOUD_DENSITY * cloud_mul;
                vol.scattering_asymmetry = asymmetry.clamp(0.3, 0.95);
                vol.light_intensity = 0.9 + twilight * 0.35;
                *transform = Transform::from_xyz(cam_xz.x, CLOUD_CENTER_Y_M, cam_xz.y)
                    .with_scale(Vec3::new(extent * 1.1, CLOUD_THICKNESS_M, extent * 1.1));
            }
        }
        vol.fog_color = fog_color;
        vol.light_tint = light_tint;
    }

    let vol_fog = VolumetricFog {
        ambient_color: fog_color,
        ambient_intensity: 0.06 + n * 0.08 + twilight * 0.04,
        step_count: steps,
        // Jitter softens banding; pairs well even without TAA.
        jitter: if matches!(*quality, QualityTier::High) {
            0.45
        } else {
            0.30
        },
    };

    let haze_vis = HAZE_VISIBILITY_DAY_M + (HAZE_VISIBILITY_NIGHT_M - HAZE_VISIBILITY_DAY_M) * n
        - twilight * 2_500.0;
    let haze_alpha = 0.42 + n * 0.30 + twilight * 0.12;
    let extinction = fog_color.mix(&twilight_fog, twilight * 0.5);
    let inscatter = Color::srgb(0.85, 0.90, 1.0)
        .mix(&Color::srgb(1.0, 0.78, 0.50), twilight)
        .mix(&Color::srgb(0.35, 0.42, 0.65), n);
    let haze = DistanceFog {
        color: fog_color.with_alpha(haze_alpha),
        directional_light_color: light_tint.with_alpha(0.45 * (1.0 - n * 0.65) + twilight * 0.25),
        directional_light_exponent: 18.0 + elev * 14.0,
        falloff: FogFalloff::from_visibility_colors(haze_vis.max(4_000.0), extinction, inscatter),
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

fn scroll_atmosphere_fog_system(
    time: Res<Time>,
    weather: Res<WeatherEffects>,
    quality: Res<QualityTier>,
    wind: Res<AtmosphereWind>,
    mut volumes: Query<(&AtmosphereLayer, &mut FogVolume)>,
) {
    if !quality.knobs().atmosphere_enabled || !weather.enabled {
        return;
    }
    let dt = time.delta_secs();
    let dir = Vec2::new(wind.heading.cos(), wind.heading.sin());
    let gust = wind.gust;
    // Perpendicular shear so the cloud deck slides at an angle to the mist.
    let shear = Vec2::new(-dir.y, dir.x);

    for (layer, mut vol) in &mut volumes {
        let (speed, vertical, lateral) = match *layer {
            AtmosphereLayer::Mist => (MIST_SCROLL * gust, 0.0025, 0.0),
            AtmosphereLayer::Clouds => (CLOUD_SCROLL * gust, 0.006, CLOUD_SHEAR),
        };
        let flow = dir * speed + shear * (speed * lateral);
        vol.density_texture_offset += Vec3::new(flow.x, vertical * gust, flow.y) * dt;
    }
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
    fn fbm_and_worley_stay_in_unit_range() {
        for i in 0..8 {
            let t = i as f32 * 0.37;
            let f = fbm3(t, t * 1.3, t * 0.7, 4);
            assert!((0.0..=1.0).contains(&f), "fbm={f}");
            let w = worley3(t * 2.0, t, t * 1.5);
            assert!((0.0..=1.0).contains(&w), "worley={w}");
        }
    }
}
