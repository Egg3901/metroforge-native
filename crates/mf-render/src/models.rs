//! `models.rs` — loader + registry for the scripted Blender glTF assets
//! (see `tools/blender/`). This is the Bevy half of the asset pipeline: it
//! loads the committed `.glb` scenes at startup and exposes their `Scene`
//! handles as a resource for other render modules (bridges, vehicles) to
//! instantiate.
//!
//! Art direction: the `.glb` files ship flat-shaded, low-poly, palette-material
//! (near-white structure, black deck, transit stays neutral for per-route
//! tint) — the Mirror's-Edge white-cel look, no textures. See
//! `crates/mf-render/src/palette.rs` for the color source of truth the
//! generators mirror.
//!
//! Cel-outline compatibility: the inverted-hull outline pass (`outline.rs`)
//! keys off procedurally built meshes with known handles. glTF scenes spawn
//! their meshes asynchronously as child entities we don't own, so applying
//! the inverted-hull cheaply is not practical here — models render WITHOUT
//! outlines. Their flat-shaded low-poly silhouette carries the cel read on
//! its own (reported in tools/blender/README.md).

use bevy::prelude::*;
use bevy::scene::SceneRoot;

use mf_state::QualityTier;

/// Asset paths (relative to the Bevy asset root, `crates/mf-game/assets`).
pub const BRIDGE_SUSPENSION_GLB: &str = "models/bridge_suspension.glb";
pub const BRIDGE_BROOKLYN_GLB: &str = "models/bridge_brooklyn.glb";
pub const BRIDGE_TRUSS_GLB: &str = "models/bridge_truss.glb";
pub const TRAIN_METRO_GLB: &str = "models/train_metro.glb";
pub const CLOUD_PUFFS_GLB: &str = "models/cloud_puffs.glb";

/// Cloud puffs: how many drifting instances to spawn on Medium+. Kept tiny
/// per the art/perf budget (<40).
pub const CLOUD_PUFF_INSTANCES: usize = 24;
/// Altitude (Bevy Y, meters) the cloud puffs drift at — well above the
/// volumetric deck, complementary not a replacement.
const CLOUD_PUFF_ALTITUDE_M: f32 = 1400.0;
/// Half-extent of the square drift domain (meters) centered on world origin.
const CLOUD_PUFF_DOMAIN_HALF_M: f32 = 6000.0;
/// Slow high-altitude drift speed (m/s).
const CLOUD_PUFF_DRIFT_SPEED: f32 = 6.0;
/// Uniform scale applied to a puff clump scene (the clumps are authored at
/// ~120-260m; this keeps them reading at city scale).
const CLOUD_PUFF_SCALE: f32 = 1.0;

/// Startup-loaded `Scene` handles for every scripted model. Consumers clone
/// the handle they need and spawn a `SceneRoot`.
#[derive(Resource, Debug, Clone)]
pub struct ModelHandles {
    pub bridge_suspension: Handle<Scene>,
    pub bridge_brooklyn: Handle<Scene>,
    pub bridge_truss: Handle<Scene>,
    pub train_metro: Handle<Scene>,
    pub cloud_puffs: Handle<Scene>,
}

/// Marker for a spawned drifting cloud puff instance.
#[derive(Component)]
struct CloudPuffInstance {
    /// Per-instance drift direction (unit-ish), varied so the field doesn't
    /// move as one rigid sheet.
    dir: Vec2,
}

pub struct MfModelsPlugin;

impl Plugin for MfModelsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_models_system).add_systems(
            Update,
            (
                spawn_cloud_puffs_system.in_set(crate::MfRenderSet::Dynamic),
                drift_cloud_puffs_system.in_set(crate::MfRenderSet::Dynamic),
            ),
        );
    }
}

fn load_models_system(mut commands: Commands, asset_server: Res<AssetServer>) {
    let scene =
        |path: &str| asset_server.load(GltfAssetLabel::Scene(0).from_asset(path.to_owned()));
    commands.insert_resource(ModelHandles {
        bridge_suspension: scene(BRIDGE_SUSPENSION_GLB),
        bridge_brooklyn: scene(BRIDGE_BROOKLYN_GLB),
        bridge_truss: scene(BRIDGE_TRUSS_GLB),
        train_metro: scene(TRAIN_METRO_GLB),
        cloud_puffs: scene(CLOUD_PUFFS_GLB),
    });
}

/// True on Medium/High (`QualityTier` has no `Ord`; match explicitly — the
/// same idiom the rest of the crate uses for a "Medium+" gate).
fn is_medium_plus(tier: QualityTier) -> bool {
    matches!(tier, QualityTier::Medium | QualityTier::High)
}

/// Spawns the fixed pool of drifting cloud puffs once, on Medium+. Each puff
/// picks one of the 4 authored clumps (they live side-by-side in the one
/// scene, 600m apart on X) — we just instance the whole scene and let each
/// instance drift; the clumps read as distinct shapes as they move.
#[allow(clippy::type_complexity)]
fn spawn_cloud_puffs_system(
    mut commands: Commands,
    quality: Res<QualityTier>,
    handles: Option<Res<ModelHandles>>,
    existing: Query<(), With<CloudPuffInstance>>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    if !is_medium_plus(*quality) {
        return;
    }
    let Some(handles) = handles else { return };
    if existing.iter().next().is_some() {
        *done = true;
        return;
    }
    let n = CLOUD_PUFF_INSTANCES;
    for i in 0..n {
        // Deterministic scatter via a golden-angle spiral, so the field is
        // even without an RNG dependency.
        let t = i as f32 / n as f32;
        let ang = i as f32 * 2.399_963_2; // golden angle (rad)
        let radius = CLOUD_PUFF_DOMAIN_HALF_M * t.sqrt();
        let x = radius * ang.cos();
        let z = radius * ang.sin();
        let y = CLOUD_PUFF_ALTITUDE_M + (i % 4) as f32 * 45.0;
        let dir = Vec2::new(ang.cos(), ang.sin()).normalize_or_zero();
        commands.spawn((
            SceneRoot(handles.cloud_puffs.clone()),
            Transform::from_xyz(x, y, z).with_scale(Vec3::splat(CLOUD_PUFF_SCALE)),
            Visibility::default(),
            CloudPuffInstance { dir },
        ));
    }
    *done = true;
}

/// Slow toroidal drift; wraps within the domain so the pool is stable.
fn drift_cloud_puffs_system(
    time: Res<Time>,
    quality: Res<QualityTier>,
    mut puffs: Query<(&mut Transform, &CloudPuffInstance)>,
) {
    if !is_medium_plus(*quality) {
        return;
    }
    let dt = time.delta_secs();
    let span = CLOUD_PUFF_DOMAIN_HALF_M;
    for (mut tf, puff) in &mut puffs {
        tf.translation.x += puff.dir.x * CLOUD_PUFF_DRIFT_SPEED * dt;
        tf.translation.z += puff.dir.y * CLOUD_PUFF_DRIFT_SPEED * dt;
        // wrap to [-span, span]
        if tf.translation.x > span {
            tf.translation.x -= 2.0 * span;
        } else if tf.translation.x < -span {
            tf.translation.x += 2.0 * span;
        }
        if tf.translation.z > span {
            tf.translation.z -= 2.0 * span;
        } else if tf.translation.z < -span {
            tf.translation.z += 2.0 * span;
        }
    }
}
