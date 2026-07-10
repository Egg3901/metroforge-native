//! Subway-view transition (spec §3.3 `subway.rs`, art-direction §7): steps
//! `mf_state::SubwayView::step` each frame (this crate is the one with
//! per-frame `Time` access and the actual animation to drive — `mf-game`'s
//! `input.rs` only flips `.active` on Tab), then drives the ~400ms-eased
//! transition: building chunks squash to slabs, road/stripe materials fade
//! to alpha 0.3 (except metro, which swaps to a bold emissive tube), and a
//! procedurally-generated radial-gradient vignette fades in over the UI.

use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use mf_state::SubwayView;

use crate::buildings::BuildingChunk;
use crate::palette;
use crate::roads::RoadSurface;
use crate::transit::{MetroBoldTube, RouteStripe};

/// Squashed building Y-scale at full subway view (art-direction §7).
const SQUASH_SCALE_Y: f32 = 0.04;
/// Faded road/stripe alpha at full subway view. Raised from the art doc's
/// 0.3: with high-key lighting the grid washed out entirely at 0.3
/// (verified via headless screenshots) — the subway view's whole point is
/// reading the street grid under the transit network.
const FADED_ALPHA: f32 = 0.55;

/// Ground dim factor at full subway view (art-direction §7).
const GROUND_DIM: f32 = 0.28;

pub struct MfSubwayPlugin;

impl Plugin for MfSubwayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VignetteState>()
            .init_resource::<SubwayLastApplied>()
            .add_systems(
                Update,
                (
                    step_subway_system,
                    squash_buildings_system,
                    fade_road_and_stripe_alpha_system,
                    metro_bold_tube_visibility_system,
                    update_vignette_system,
                )
                    .chain()
                    .in_set(crate::MfRenderSet::Dynamic),
            );
    }
}

fn step_subway_system(time: Res<Time>, mut subway: ResMut<SubwayView>) {
    subway.step(time.delta_secs());
}

/// Last `SubwayView::t` these systems actually applied to their entities —
/// once the transition settles (`step()` clamps `t` to exactly 0.0 or 1.0),
/// the transform/material/visibility writes below would otherwise repeat
/// forever for a view that never moves again. `update_vignette_system`, last
/// in the chain, is the sole writer: it updates this once per frame, after
/// the three systems above it have already read this frame's (i.e. the
/// previous frame's) value — each of those has its own `Added<_>` escape
/// hatch for entities rebuilt elsewhere while `t` sat steady.
#[derive(Resource, Default)]
struct SubwayLastApplied {
    t: Option<f32>,
}

fn squash_buildings_system(
    subway: Res<SubwayView>,
    last_applied: Res<SubwayLastApplied>,
    mut chunks: Query<&mut Transform, With<BuildingChunk>>,
    fresh_chunks: Query<Entity, Added<BuildingChunk>>,
) {
    // `buildings.rs` periodically despawns+respawns chunks on a data rebuild
    // (new entities at the default unsquashed scale); a chunk born while `t`
    // is steady still needs one squash pass, hence the `Added` check
    // alongside the steady-t skip.
    if last_applied.t == Some(subway.t) && fresh_chunks.is_empty() {
        return;
    }
    let scale_y = 1.0 - subway.t * (1.0 - SQUASH_SCALE_Y);
    for mut transform in &mut chunks {
        transform.scale.y = scale_y;
    }
}

#[allow(clippy::too_many_arguments)]
fn fade_road_and_stripe_alpha_system(
    subway: Res<SubwayView>,
    last_applied: Res<SubwayLastApplied>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    roads: Query<&MeshMaterial3d<StandardMaterial>, With<RoadSurface>>,
    stripes: Query<(&RouteStripe, &MeshMaterial3d<StandardMaterial>)>,
    terrain: Query<&MeshMaterial3d<StandardMaterial>, With<crate::terrain::TerrainSurface>>,
    fresh_roads: Query<Entity, Added<RoadSurface>>,
    fresh_stripes: Query<Entity, Added<RouteStripe>>,
    fresh_terrain: Query<Entity, Added<crate::terrain::TerrainSurface>>,
) {
    // Roads, terrain and route stripes are each independently rebuilt
    // (despawn+respawn with brand-new materials) by their own modules; a
    // surface born while `t` is steady still needs one fade pass, hence the
    // three `Added` checks alongside the steady-t skip.
    let steady = last_applied.t == Some(subway.t)
        && fresh_roads.is_empty()
        && fresh_stripes.is_empty()
        && fresh_terrain.is_empty();
    if steady {
        return;
    }
    // Roads and the normal-width route stripe stay `AlphaMode::Blend`
    // unconditionally (set at creation in roads.rs/transit.rs) — only their
    // `base_color` alpha moves here. See the long comment on the road-class
    // materials in `roads.rs` for why this doesn't also flip `alpha_mode` to
    // `Opaque` when steady at alpha 1.0: that optimization was attempted and
    // broke rendering (verified via headless screenshot A/B diffing), so
    // this crate still pays the transparent-pass cost for these two
    // material families specifically, unlike the rest of issue #5's fix.
    let alpha = 1.0 - subway.t * (1.0 - FADED_ALPHA);
    for handle in &roads {
        if let Some(mat) = materials.get_mut(&handle.0) {
            mat.base_color = mat.base_color.with_alpha(alpha);
        }
    }
    // Dim the ground so the faded grid and the vivid metro network pop
    // against it (base_color multiplies the terrain's vertex colors). Only
    // the RGB channels move here, never alpha, so the terrain material's
    // `AlphaMode` is untouched (it's created, and stays, `Opaque`).
    let dim = 1.0 - subway.t * GROUND_DIM;
    for handle in &terrain {
        if let Some(mat) = materials.get_mut(&handle.0) {
            mat.base_color = Color::srgb(dim, dim, dim);
        }
    }
    for (stripe, handle) in &stripes {
        // Metro's normal-width stripe fades further toward invisible, since
        // its bold tube (a separate entity) takes over as the hero once
        // `t > 0.5` (see `metro_bold_tube_visibility_system`).
        let target_alpha = if stripe.mode == mf_protocol::TransitMode::Metro {
            1.0 - subway.t * 0.85
        } else {
            alpha
        };
        if let Some(mat) = materials.get_mut(&handle.0) {
            mat.base_color = mat.base_color.with_alpha(target_alpha);
        }
    }
}

/// Metro routes swap their thin surface stripe for a bold, 2x-width,
/// emissive tube once the transition is more than half-eased (art-direction
/// §7: "metro tunnels/lines render as BOLD tubes ... with metro trains as
/// bricks moving along them").
fn metro_bold_tube_visibility_system(
    subway: Res<SubwayView>,
    last_applied: Res<SubwayLastApplied>,
    mut bold_tubes: Query<&mut Visibility, With<MetroBoldTube>>,
    fresh_tubes: Query<Entity, Added<MetroBoldTube>>,
) {
    // `transit.rs` (re)spawns tubes hidden whenever routes rebuild; one born
    // while `t` is already past the reveal threshold still needs this pass
    // to flip it visible, hence the `Added` check alongside the steady-t
    // skip.
    if last_applied.t == Some(subway.t) && fresh_tubes.is_empty() {
        return;
    }
    let visible = subway.t > 0.5;
    for mut vis in &mut bold_tubes {
        *vis = if visible {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

#[derive(Resource, Default)]
struct VignetteState {
    entity: Option<Entity>,
    image: Option<Handle<Image>>,
}

/// Procedural radial gradient (transparent center -> `vignette_edge(0.55)`
/// at the corners) — a UI-level overlay, zero post-processing cost, works
/// on potato (art-direction §7).
fn build_vignette_image() -> Image {
    const SIZE: u32 = 256;
    let center = SIZE as f32 / 2.0 - 0.5;
    let max_dist = (2.0f32).sqrt() * center;
    let mut data = vec![0u8; (SIZE * SIZE * 4) as usize];
    let edge = palette::vignette_edge(1.0).to_srgba();
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt() / max_dist;
            let t = dist.clamp(0.0, 1.0).powf(1.6);
            let idx = ((y * SIZE + x) * 4) as usize;
            data[idx] = (edge.red * 255.0) as u8;
            data[idx + 1] = (edge.green * 255.0) as u8;
            data[idx + 2] = (edge.blue * 255.0) as u8;
            // Edge alpha caps at 0.55 per art-direction §7.
            data[idx + 3] = (t * 0.55 * 255.0) as u8;
        }
    }
    Image::new(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

fn update_vignette_system(
    subway: Res<SubwayView>,
    mut last_applied: ResMut<SubwayLastApplied>,
    mut state: ResMut<VignetteState>,
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut image_nodes: Query<&mut ImageNode>,
) {
    let entity = if let Some(e) = state.entity {
        e
    } else {
        let handle = images.add(build_vignette_image());
        let e = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(0.0),
                    top: Val::Percent(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                ImageNode {
                    image: handle.clone(),
                    color: Color::WHITE.with_alpha(0.0),
                    ..default()
                },
                Name::new("subway-vignette"),
            ))
            .id();
        state.entity = Some(e);
        state.image = Some(handle);
        e
    };
    // This system is last in the chain and is the sole writer of
    // `SubwayLastApplied` — see its doc comment.
    if last_applied.t == Some(subway.t) {
        return;
    }
    if let Ok(mut node) = image_nodes.get_mut(entity) {
        node.color = Color::WHITE.with_alpha(subway.t);
    }
    last_applied.t = Some(subway.t);
}
