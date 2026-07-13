//! `vehicle_models.rs` — loader for the surface transit vehicle kit (bus,
//! articulated tram, commuter rail) scripted glTF scenes (see
//! `tools/blender/gen_bus.py` / `gen_tram.py` / `gen_rail.py`).
//!
//! This is a DELIBERATELY SEPARATE loader module from `models.rs`: the metro
//! consist + bridges + clouds live in `models.rs::ModelHandles`, which a
//! parallel structure-placement lane owns. Rather than edit that resource, the
//! vehicle kit ships its own tiny registry resource `VehicleModelHandles` and
//! its own startup loader, consumed by `vehicles.rs` for the per-mode model
//! swap (mirroring how `models.rs::ModelHandles.train_metro` feeds the metro
//! swap). The ambient street cars are built procedurally in `traffic.rs` for
//! cheap instancing, so their `.glb` is an art record only and is not loaded
//! here.
//!
//! Art direction: the `.glb` files ship flat-shaded, low-poly, palette-material
//! (near-white BODY for per-route tint; neutral windows/roof) — the same
//! Mirror's-Edge white-cel look as the metro consist. See
//! `crates/mf-render/src/palette.rs` for the color source the generators mirror.

use bevy::prelude::*;

/// Asset paths (relative to the Bevy asset root, `crates/mf-game/assets`).
pub const BUS_GLB: &str = "models/bus.glb";
pub const TRAM_GLB: &str = "models/tram.glb";
pub const RAIL_GLB: &str = "models/rail.glb";

/// Startup-loaded `Scene` handles for the surface transit kit. `vehicles.rs`
/// clones the handle matching a route's `TransitMode` and spawns a `SceneRoot`
/// in place of the brick at Medium+ tiers.
#[derive(Resource, Debug, Clone)]
pub struct VehicleModelHandles {
    pub bus: Handle<Scene>,
    pub tram: Handle<Scene>,
    pub rail: Handle<Scene>,
}

pub struct MfVehicleModelsPlugin;

impl Plugin for MfVehicleModelsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_vehicle_models_system);
    }
}

fn load_vehicle_models_system(mut commands: Commands, asset_server: Res<AssetServer>) {
    let scene =
        |path: &str| asset_server.load(GltfAssetLabel::Scene(0).from_asset(path.to_owned()));
    commands.insert_resource(VehicleModelHandles {
        bus: scene(BUS_GLB),
        tram: scene(TRAM_GLB),
        rail: scene(RAIL_GLB),
    });
}
