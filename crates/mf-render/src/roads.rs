//! Road ribbons (spec §3.3 `roads.rs`): one merged mesh per class
//! (arterial/collector/local — "≤3 meshes"), rebuilt once per city load.
//! Local-road visibility is LOD-toggled by camera height. All classes are
//! the same rich-black `ROAD` color per art-direction §2 ("differentiate by
//! width only"); arterials additionally get a 1m `ROAD_EDGE` hairline on
//! medium/high tier.

use bevy::prelude::*;
use bevy::render::mesh::MeshAabb;

use mf_state::{CurrentCity, EffectiveKnobs, HeightAt, Theme};

use crate::mesh_utils::{append_cuboid, MeshBuffers};
use crate::palette;
use crate::RenderCacheStats;

/// Road surface lift above ground. The spec said 0.5, but at overview zoom
/// on near-flat terrain a 0.5m offset loses the depth fight against the
/// terrain mesh at grazing angles (roads visibly vanish from skyline
/// framings; found on the flattened real-city relief). 2m is still
/// imperceptible as elevation at street zoom and keeps the ribbons winning
/// depth at distance.
pub(crate) const ROAD_Y_OFFSET: f32 = 2.0;
/// Water-crossing segments ride a fixed deck height instead of hugging
/// `WATER_LEVEL_Y` — a road at water level renders as a barely-visible black
/// sliver mid-river (owner-flagged on the East River bridges). A flat
/// causeway a few meters up reads as a bridge at city zoom.
pub(crate) const BRIDGE_DECK_Y: f32 = 8.0;
/// Vertical separation per grade level. Raised 6m -> 8m (owner: stacked decks
/// and bridges read "completely flat"): a full 8m of air under each level is
/// the clearance that lets support piers, the slab edge and the blob shadow all
/// have room to read as real grade separation at oblique zoom. A `gradeLevel`
/// of N lifts the deck `N * GRADE_STEP_Y` above ground (N<0 for
/// tunnels/underpasses dips it below), so stacked interchange layers separate.
pub(crate) const GRADE_STEP_Y: f32 = 8.0;
/// Length (meters) over which a bridge deck ramps from ground up to its full
/// grade lift at each end (the approach ramps).
const RAMP_M: f32 = 40.0;
/// A road segment whose peak deck sits at least this far above ground gets a
/// slab skirt + blob shadow (avoids skirting near-ground micro-lifts).
const ELEVATED_EPS: f32 = 1.5;
/// Visible thickness of a bridge/elevated deck: the slab edge drops this far
/// below the deck top on both sides (owner: decks had no thickness, so they
/// read as painted stripes rather than structures).
const SLAB_THICKNESS: f32 = 1.2;
/// Along-deck spacing of support piers under an elevated run. Piers are THE cue
/// that reads at every zoom/angle, so they are placed on every tier.
const PIER_SPACING_M: f32 = 40.0;
/// Widths per spec §3.3 (already includes `roadScale` multiplication).
// Widened ~1.5x from real-world-ish 40/24/13: at overview zoom the true
// widths are a few pixels and vanish into the bright ground (the oldest
// render-backlog item, owner-flagged twice). Slight exaggeration is the
// standard map-style tradeoff.
// `pub(crate)`: `terrain.rs` reuses these as the terrain-grading corridor
// half-width source (see `terrain::grade_terrain`) so the graded corridor
// stays in lockstep with the ribbon width instead of drifting via a
// duplicated constant.
pub(crate) const ARTERIAL_WIDTH: f64 = 60.0;
pub(crate) const COLLECTOR_WIDTH: f64 = 36.0;
pub(crate) const LOCAL_WIDTH: f64 = 20.0;
/// Camera height above which local-road detail is hidden (LOD).
const LOCAL_ROAD_LOD_HEIGHT: f32 = 4_000.0;
/// Collectors hide above this height (arterials stay for skyline structure).
const COLLECTOR_ROAD_LOD_HEIGHT: f32 = 8_000.0;

/// Scale a color's RGB toward black by `k` (keeps alpha), for the darker slab
/// skirt / portal marks — no NEW hue is introduced (art direction §2).
fn scale_rgb(c: Color, k: f32) -> Color {
    let s = c.to_srgba();
    Color::srgba(s.red * k, s.green * k, s.blue * k, s.alpha)
}

/// Per-vertex absolute deck Y for a road polyline: ground/water base plus the
/// grade lift (`gradeLevel * GRADE_STEP_Y`), ramped in over `RAMP_M` from each
/// end so a bridge deck rises from and settles back to the ground at its
/// approaches rather than stepping vertically.
fn deck_heights(pts: &[Vec2], grade: i32, sample: &impl Fn(f32, f32) -> f32) -> Vec<f32> {
    let full_lift = grade as f32 * GRADE_STEP_Y;
    let n = pts.len();
    // Cumulative arc length so the ramp is measured in true meters.
    let mut cum = vec![0.0_f32; n];
    for i in 1..n {
        cum[i] = cum[i - 1] + pts[i - 1].distance(pts[i]);
    }
    let total = cum[n - 1];
    pts.iter()
        .enumerate()
        .map(|(i, p)| {
            let base = sample(p.x, p.y);
            let from_start = cum[i];
            let from_end = total - cum[i];
            // 0 at the very ends, 1 once past RAMP_M in from both ends.
            let ramp = (from_start.min(from_end) / RAMP_M).clamp(0.0, 1.0);
            base + full_lift * ramp
        })
        .collect()
}

/// The visible SLAB EDGE down both sides of an elevated deck: a `SLAB_THICKNESS`
/// tall vertical band hanging under the deck top, so the deck reads as a solid
/// slab with real thickness rather than a painted stripe. The long drop to the
/// ground is carried by [`append_deck_piers`] instead of a continuous skirt
/// wall — piers read as structure at every angle where a solid skirt read as a
/// featureless dark curtain.
fn append_deck_slab(
    buf: &mut MeshBuffers,
    pts: &[Vec2],
    heights: &[f32],
    width: f32,
    y_offset: f32,
    color: Color,
) {
    let half = width * 0.5;
    for i in 0..pts.len() - 1 {
        let a = pts[i];
        let b = pts[i + 1];
        let dir = (b - a).normalize_or_zero();
        if dir == Vec2::ZERO {
            continue;
        }
        let perp = Vec2::new(-dir.y, dir.x) * half;
        let ya = heights[i] + y_offset;
        let yb = heights[i + 1] + y_offset;
        let ya_b = ya - SLAB_THICKNESS;
        let yb_b = yb - SLAB_THICKNESS;
        for s in [1.0_f32, -1.0] {
            let pa = Vec3::new(a.x + perp.x * s, ya, a.y + perp.y * s);
            let pb = Vec3::new(b.x + perp.x * s, yb, b.y + perp.y * s);
            let pa_b = Vec3::new(a.x + perp.x * s, ya_b, a.y + perp.y * s);
            let pb_b = Vec3::new(b.x + perp.x * s, yb_b, b.y + perp.y * s);
            let nrm = Vec3::new(perp.x * s, 0.0, perp.y * s).normalize_or_zero();
            buf.push_flat_quad(pa, pb, pb_b, pa_b, nrm, color);
        }
    }
}

/// Rectangular support piers under an elevated deck, one every ~`PIER_SPACING_M`
/// along its length, each rising from the ground/water up to the deck's
/// underside (deck top minus `SLAB_THICKNESS`). These are THE grade-separation
/// cue that survives every zoom and camera angle, so they are emitted on ALL
/// tiers (cheap axis-aligned boxes into the pooled detail mesh). A slight width
/// taper is faked by making the pier a touch narrower than a full lane pillar.
#[allow(clippy::too_many_arguments)]
fn append_deck_piers(
    buf: &mut MeshBuffers,
    pts: &[Vec2],
    heights: &[f32],
    width: f32,
    y_offset: f32,
    sample: &impl Fn(f32, f32) -> f32,
    color: Color,
) {
    // Pier footprint scales with the road width but stays a slim pillar.
    let pier_half = (width as f64 * 0.14).clamp(1.2, 3.0) as f32;
    // First pier half a span in, then every PIER_SPACING_M of arc length.
    let mut carry = PIER_SPACING_M * 0.5;
    for i in 0..pts.len() - 1 {
        let a = pts[i];
        let b = pts[i + 1];
        let seg = a.distance(b);
        if seg <= 1e-3 {
            continue;
        }
        let mut t = carry;
        while t < seg {
            let f = t / seg;
            let pos = a.lerp(b, f);
            let deck = heights[i] + (heights[i + 1] - heights[i]) * f;
            let ground = sample(pos.x, pos.y);
            // Top of the pier sits just under the slab; bottom on the ground.
            let underside = deck + y_offset - SLAB_THICKNESS;
            let h = underside - ground;
            if h > ELEVATED_EPS {
                append_cuboid(
                    buf, pos, ground, pier_half, pier_half, h, color, color, color,
                );
            }
            t += PIER_SPACING_M;
        }
        carry = t - seg;
    }
}

/// Thin LIGHT hairlines down both deck edges (the cel-outline language, art
/// direction §1) so an elevated deck separates cleanly from its own shadow and
/// from surface roads passing beneath it. Sits a hair above the deck top.
fn append_deck_hairlines(
    buf: &mut MeshBuffers,
    pts: &[Vec2],
    heights: &[f32],
    width: f32,
    y_offset: f32,
    color: Color,
) {
    const HAIRLINE_W: f32 = 1.2;
    let half = width * 0.5;
    let left = crate::mesh_utils::offset_polyline(pts, half);
    let right = crate::mesh_utils::offset_polyline(pts, -half);
    for edge in [left, right] {
        crate::mesh_utils::append_ribbon_at_heights(
            buf,
            &edge,
            heights,
            y_offset + 0.08,
            HAIRLINE_W,
            color,
        );
    }
}

/// A cheap projected blob shadow: a dark translucent ribbon laid on the
/// surface directly beneath an elevated deck (straight-down projection, no
/// shadow-map cost).
fn append_blob_shadow(
    buf: &mut MeshBuffers,
    pts: &[Vec2],
    heights: &[f32],
    width: f32,
    sample: &impl Fn(f32, f32) -> f32,
    color: Color,
) {
    let half = width * 0.5 * 1.15; // a touch wider than the deck
    for i in 0..pts.len() - 1 {
        let a = pts[i];
        let b = pts[i + 1];
        // Only shadow where the deck is actually lifted above the ground here.
        if heights[i] - sample(a.x, a.y) <= ELEVATED_EPS
            && heights[i + 1] - sample(b.x, b.y) <= ELEVATED_EPS
        {
            continue;
        }
        let dir = (b - a).normalize_or_zero();
        if dir == Vec2::ZERO {
            continue;
        }
        let perp = Vec2::new(-dir.y, dir.x) * half;
        let ya = sample(a.x, a.y) + ROAD_Y_OFFSET * 0.5;
        let yb = sample(b.x, b.y) + ROAD_Y_OFFSET * 0.5;
        let a0 = Vec3::new(a.x + perp.x, ya, a.y + perp.y);
        let a1 = Vec3::new(a.x - perp.x, ya, a.y - perp.y);
        let b0 = Vec3::new(b.x + perp.x, yb, b.y + perp.y);
        let b1 = Vec3::new(b.x - perp.x, yb, b.y - perp.y);
        buf.push_flat_quad(a0, b0, b1, a1, Vec3::Y, color);
    }
}

/// A chunky, unmistakable tunnel portal at each end of a buried road: a raised
/// dark portal FRAME (a lintel bar plus two side abutment posts), a near-black
/// recessed mouth quad, and a short fade-ramp of darkening leading up to the
/// mouth so the suppressed surface road reads as diving into a hole in the
/// ground rather than as paint. `frame_color` is the darker-than-road detail
/// tone; the mouth is pure near-black.
fn append_tunnel_portals(
    buf: &mut MeshBuffers,
    pts: &[Vec2],
    width: f32,
    sample: &impl Fn(f32, f32) -> f32,
    frame_color: Color,
) {
    let mouth_color = Color::srgb(0.01, 0.01, 0.015);
    let half = width * 0.5;
    let ends: [(Vec2, Vec2); 2] = [(pts[0], pts[1]), (pts[pts.len() - 1], pts[pts.len() - 2])];
    for (mouth, inward) in ends {
        let dir = (inward - mouth).normalize_or_zero();
        if dir == Vec2::ZERO {
            continue;
        }
        let perp_u = Vec2::new(-dir.y, dir.x);
        let perp = perp_u * half;
        let ground = sample(mouth.x, mouth.y);

        // 1. Fade-ramp: a longer, wider dark apron on the ground leading INTO
        //    the mouth (from ~1.4 lanes back to the mouth line), reading as the
        //    road dropping into shadow before it disappears.
        let ramp_back = dir * (-half * 1.4);
        let wperp = perp_u * (half * 1.12);
        let y_ramp = ground + ROAD_Y_OFFSET + 0.08;
        let r0 = Vec3::new(mouth.x + wperp.x, y_ramp, mouth.y + wperp.y);
        let r1 = Vec3::new(mouth.x - wperp.x, y_ramp, mouth.y - wperp.y);
        let r2 = Vec3::new(
            mouth.x - wperp.x + ramp_back.x,
            y_ramp,
            mouth.y - wperp.y + ramp_back.y,
        );
        let r3 = Vec3::new(
            mouth.x + wperp.x + ramp_back.x,
            y_ramp,
            mouth.y + wperp.y + ramp_back.y,
        );
        buf.push_flat_quad(r0, r1, r2, r3, Vec3::Y, frame_color);

        // 2. Recessed mouth: a short near-black rectangle at the mouth line.
        let depth = dir * (half * 0.9);
        let y_mouth = ground + ROAD_Y_OFFSET + 0.12;
        let m0 = Vec3::new(mouth.x + perp.x, y_mouth, mouth.y + perp.y);
        let m1 = Vec3::new(mouth.x - perp.x, y_mouth, mouth.y - perp.y);
        let m2 = Vec3::new(
            mouth.x - perp.x + depth.x,
            y_mouth,
            mouth.y - perp.y + depth.y,
        );
        let m3 = Vec3::new(
            mouth.x + perp.x + depth.x,
            y_mouth,
            mouth.y + perp.y + depth.y,
        );
        buf.push_flat_quad(m0, m1, m2, m3, Vec3::Y, mouth_color);

        // 3. Portal frame: two abutment posts flanking the mouth + a lintel bar
        //    spanning them, all in the darker frame tone, so the entrance reads
        //    as a built structure at oblique zoom.
        let post_half = (width * 0.16).clamp(1.5, 3.5);
        let post_h = (SLAB_THICKNESS + GRADE_STEP_Y * 0.45).max(4.0);
        for s in [1.0_f32, -1.0] {
            let c = mouth + perp_u * (half + post_half * 0.6) * s;
            append_cuboid(
                buf,
                c,
                ground,
                post_half,
                post_half,
                post_h,
                frame_color,
                frame_color,
                frame_color,
            );
        }
    }
}

pub struct MfRoadsPlugin;

impl Plugin for MfRoadsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoadsState>().add_systems(
            Update,
            (
                build_roads_system.in_set(crate::MfRenderSet::Statics),
                road_lod_system.in_set(crate::MfRenderSet::Dynamic),
                road_shadow_distance_fade_system.in_set(crate::MfRenderSet::Dynamic),
                apply_quality_to_roads_material_system.in_set(crate::MfRenderSet::Dynamic),
                road_tunnel_subway_visibility_system.in_set(crate::MfRenderSet::Dynamic),
            ),
        );
    }
}

#[derive(Resource, Default)]
struct RoadsState {
    /// Cheap structural signature: `(fields version, roads.len(), total
    /// point count, theme, densify step bits)`. Road geometry never changes
    /// after `ready` in v1, but the terrain the ribbons drape over rebuilds
    /// on every fields version — baking only once left roads buried under
    /// relief that arrived in a later version (the residual half of the
    /// roads race). `Theme` rides along so a theme switch forces a full
    /// rebuild (road color is baked into mesh vertex color at build time).
    /// Densify step bits so a quality-tier change rebuilds at the new
    /// ribbon resolution. The trailing `u32` is the DEM elevation-channel
    /// resolution (0 = none): the real-elevation frame (msgType=7) arrives
    /// AFTER `fields` and does NOT bump `fields.version`, so without keying on
    /// it the terrain mesh rebuilds onto real relief (its own key already
    /// includes `elev_res`) while the road ribbons — keyed only on the fields
    /// version — never rebuild and stay baked at the pre-DEM flat heights,
    /// buried under the raised ground. This is the "streets don't render at
    /// all / v0.4.1 streetless" recurrence; keying on `elev_res` forces the
    /// ribbons to re-bake against the DEM the same frame the terrain does.
    signature: Option<(u32, usize, usize, Theme, u32, u32)>,
    /// Class entity ids (arterial/collector/local) — reused across rebuilds.
    class_entities: [Option<Entity>; 3],
    edge_entity: Option<Entity>,
    local_entity: Option<Entity>,
    collector_entity: Option<Entity>,
    /// Long-lived mesh assets reused via [`MeshBuffers::apply_to_mesh`].
    class_meshes: [Option<Handle<Mesh>>; 3],
    edge_mesh: Option<Handle<Mesh>>,
    class_materials: [Option<Handle<StandardMaterial>>; 3],
    edge_material: Option<Handle<StandardMaterial>>,
    /// Scratch buffers kept across rebuilds so vertex Vecs retain capacity.
    scratch_class: [MeshBuffers; 3],
    scratch_edge: MeshBuffers,
    /// Bridge-deck slab skirts (vertical edge quads) + tunnel portal marks —
    /// all rendered in one darker-than-road material so the deck reads as a
    /// slab and the portals read as mouths. Geometry-only (cheap): kept even
    /// on Potato.
    scratch_detail: MeshBuffers,
    detail_entity: Option<Entity>,
    detail_mesh: Option<Handle<Mesh>>,
    detail_material: Option<Handle<StandardMaterial>>,
    /// Light cel-outline hairlines down both edges of every elevated deck —
    /// their own mesh + light material so the deck separates from its shadow
    /// and from surface roads below. Kept on all tiers (cheap thin ribbons).
    scratch_deckedge: MeshBuffers,
    deckedge_entity: Option<Entity>,
    deckedge_mesh: Option<Handle<Mesh>>,
    deckedge_material: Option<Handle<StandardMaterial>>,
    /// Soft blob shadows projected straight down from elevated decks onto the
    /// surface below (dark translucent quads). Skipped on the unlit tiers
    /// (Potato/Low) per the potato-tier budget.
    scratch_shadow: MeshBuffers,
    shadow_entity: Option<Entity>,
    shadow_mesh: Option<Handle<Mesh>>,
    shadow_material: Option<Handle<StandardMaterial>>,
    /// Underground tunnel bodies — hidden in the normal view (surface road is
    /// suppressed between portals), shown as dark tubes at depth in subway
    /// (Tab) view, mirroring the transit tunnel/tube treatment.
    scratch_tunnel: MeshBuffers,
    tunnel_entity: Option<Entity>,
    tunnel_mesh: Option<Handle<Mesh>>,
    tunnel_material: Option<Handle<StandardMaterial>>,
    /// Tracked so `road_lod_system` can hide the arterial mesh once the
    /// camera climbs above the active tier's fog `end` — above that height the
    /// whole network is fully fogged to the sky color anyway, so hiding it is
    /// free and removes the aliased sub-pixel scribbles it would otherwise
    /// draw at the horizon on the no-MSAA fog tiers (Potato/Low).
    arterial_entity: Option<Entity>,
}

#[derive(Component)]
struct LocalRoadMarker;

#[derive(Component)]
struct CollectorRoadMarker;

/// Marker on every road-surface mesh entity (all classes + the arterial
/// hairline edge) so `subway.rs` can fade their alpha toward 0.3 in subway
/// view without reaching into this module's internals.
#[derive(Component)]
pub struct RoadSurface;

/// Marker on just the 3 road-class entities (arterial/collector/local) —
/// NOT the arterial edge, which is intentionally always lit regardless of
/// tier. Lets `apply_quality_to_roads_material_system` retarget only the
/// materials whose `unlit` should track the tier.
#[derive(Component)]
struct RoadClassSurface;

/// Marker on the underground road-tunnel body mesh — hidden in the normal
/// view, revealed at depth in subway (Tab) view by
/// [`road_tunnel_subway_visibility_system`], mirroring the transit tube
/// treatment.
#[derive(Component)]
struct RoadTunnelSurface;

#[allow(clippy::too_many_arguments)]
fn build_roads_system(
    mut commands: Commands,
    city: Res<CurrentCity>,
    fields: Res<mf_state::LatestFields>,
    height_at: Res<HeightAt>,
    mut state: ResMut<RoadsState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    effective: Res<EffectiveKnobs>,
    theme: Res<Theme>,
    mut stats: ResMut<RenderCacheStats>,
    counters: Res<crate::perf::PerfCounters>,
) {
    let Some(city_json) = &city.static_city else {
        return;
    };
    // RACE FIX: `ready` (roads) arrives before `fields` (terrain), and this
    // system builds exactly once per signature - building against the
    // placeholder flat HeightAt buried every road under the real relief
    // (intermittently, per frame timing: the recurring "why are the roads
    // never showing"). Wait for real terrain before baking.
    let Some(f) = &fields.0 else {
        return;
    };
    let total_points: usize = city_json.roads.iter().map(|r| r.points.len()).sum();
    let densify_step = effective.0.ribbon_densify_step_m;
    // Match `terrain.rs`'s rebuild key: the DEM elevation channel (msgType=7)
    // arrives after `fields` without bumping `f.version`, so the ribbons must
    // key on it too or they never re-bake onto the real relief (see the
    // `signature` field doc). 0 when the city ships no DEM.
    let elev_res = city.elevation.as_ref().map(|e| e.res).unwrap_or(0);
    let signature = (
        f.version,
        city_json.roads.len(),
        total_points,
        *theme,
        densify_step.to_bits(),
        elev_res,
    );
    // Re-bake whenever the terrain sampler itself changes, not only when the
    // structural signature does. `terrain.rs` runs in `MfRenderSet::Terrain`
    // (chained BEFORE this system's `Statics`) and replaces `HeightAt` on every
    // ground rebuild — including the DEM-arrival rebuild, whose trigger
    // (`msgType=7` elevation) does not always move any field in `signature` in
    // lockstep. Keying the ribbon rebuild directly on the resource we actually
    // sample closes the "roads baked against the flat pre-DEM ground stay
    // buried" race for good (it was intermittent per run: the render showed the
    // full black grid on some boots and a bare white ground on others, same
    // seed/tier/weather). `is_changed()` is true the frame terrain rewrites the
    // sampler and on first run, so the fresh sampler is always the one baked.
    let height_changed = height_at.is_changed();
    if state.signature == Some(signature) && !height_changed {
        return;
    }
    let _span = tracing::info_span!("roads_rebuild").entered();
    let _timer = crate::perf::PerfSpan::start(&counters.roads_rebuild_us);
    state.signature = Some(signature);

    let road_scale = city_json.road_scale as f32;
    let road_color = palette::road();
    let unlit = effective.0.unlit_material;

    for buf in &mut state.scratch_class {
        buf.clear();
    }
    state.scratch_edge.clear();
    state.scratch_detail.clear();
    state.scratch_deckedge.clear();
    state.scratch_shadow.clear();
    state.scratch_tunnel.clear();

    // Grade-separation tones (no NEW hues per art direction §2 — road black
    // scaled/mixed only). All the aux meshes below bake their FINAL color into
    // vertex colors and render through a WHITE base material, so several
    // distinct tones can share one pooled mesh without a per-tone material.
    //   - `detail_color`: darker-than-road tone for tunnel bodies + portal
    //     frames + the portal fade-ramp.
    //   - `pier_color`: the piers are the same near-black as the roads (art
    //     direction §1) so they read as one dark structure with the deck.
    //   - `slab_color`: the deck's visible slab edge, a touch LIGHTER than the
    //     deck so deck/edge/shadow separate in tone (issue #143).
    //   - `hairline_color`: the light cel-outline (~#e9eae5) down both deck
    //     edges.
    let detail_color = scale_rgb(road_color, 0.55);
    let pier_color = road_color;
    let slab_color = road_color.mix(&palette::ground(), 0.16);
    let hairline_color = Color::srgb(0.914, 0.918, 0.898);
    // Cheap projected blob shadow: pure black, low alpha (issue #143: dropped
    // from ~0.22^2 to a clean 0.25 by rendering through a WHITE base).
    let shadow_color = Color::srgba(0.0, 0.0, 0.0, 0.25);
    // Blob shadows now run on ALL tiers — one translucent quad per elevated
    // deck segment is affordable even on Potato, and the projected shadow is a
    // big part of reading grade separation (owner). They fade with distance in
    // `road_shadow_distance_fade_system`.
    let shadows_enabled = true;

    // Double-render fix (#144): where `bridges.rs` places a scripted bridge
    // model over a span, suppress THIS module's elevated grade structure (slab
    // edge, support piers, blob shadow, deck hairlines) for that road so the
    // model's own towers/piers/deck don't render on top of the flat causeway.
    // Same pure decision function the placer uses, so the two never disagree.
    // The thin road ribbon itself is kept — it rides on the model deck as the
    // black roadway surface and keeps the approaches continuous.
    let bridged_roads: std::collections::HashSet<usize> =
        crate::bridges::plan_bridge_placements(city_json, &height_at)
            .into_iter()
            .map(|p| p.road_idx)
            .collect();

    for (road_idx, road) in city_json.roads.iter().enumerate() {
        let raw: Vec<Vec2> = road
            .points
            .chunks_exact(2)
            .map(|c| Vec2::new(c[0] as f32, c[1] as f32))
            .collect();
        if raw.len() < 2 {
            continue;
        }
        // Follow the terrain, not just the sparse simplified vertices.
        // Step is tiered: Potato/Low use coarser densify to cut rebuild
        // vertices and draw cost.
        let pts = crate::mesh_utils::densify_polyline(&raw, densify_step);
        let (idx, width) = match road.cls.as_str() {
            "arterial" => (0usize, ARTERIAL_WIDTH as f32 * road_scale),
            "collector" => (1usize, COLLECTOR_WIDTH as f32 * road_scale),
            _ => (2usize, LOCAL_WIDTH as f32 * road_scale),
        };
        let sample = |x: f32, z: f32| {
            let h = height_at.sample(x, z);
            if h <= crate::terrain::WATER_LEVEL_Y + 0.01 {
                BRIDGE_DECK_Y
            } else {
                h
            }
        };

        // Per-vertex deck heights: ground/water base + grade lift, ramped up
        // over `RAMP_M` at each end (bridges) so the deck meets the ground at
        // the approaches instead of stepping vertically.
        let grade = road.grade_level;
        let heights = deck_heights(&pts, grade, &sample);

        if road.is_tunnel {
            // Surface road is hidden between portals; render a portal mark at
            // each end on the ground, and the tunnel body as a dark tube shown
            // only in subway view.
            append_tunnel_portals(
                &mut state.scratch_detail,
                &pts,
                width,
                &sample,
                detail_color,
            );
            // Body at depth (grade is negative for tunnels): a dashed dark
            // ribbon reads as underground infrastructure at overview zoom.
            crate::mesh_utils::append_ribbon_at_heights(
                &mut state.scratch_tunnel,
                &pts,
                &heights,
                ROAD_Y_OFFSET,
                width,
                detail_color,
            );
            continue;
        }

        crate::mesh_utils::append_ribbon_at_heights(
            &mut state.scratch_class[idx],
            &pts,
            &heights,
            ROAD_Y_OFFSET,
            width,
            road_color,
        );
        if idx == 0 {
            crate::mesh_utils::append_ribbon_at_heights(
                &mut state.scratch_edge,
                &pts,
                &heights,
                ROAD_Y_OFFSET + 0.05,
                width + 2.0,
                palette::road_edge(),
            );
        }

        // Slab skirt + blob shadow for elevated (bridge / raised) decks.
        let peak_lift = heights
            .iter()
            .zip(pts.iter())
            .map(|(h, p)| h - sample(p.x, p.y))
            .fold(0.0_f32, f32::max);
        if peak_lift > ELEVATED_EPS && !bridged_roads.contains(&road_idx) {
            // Visible slab thickness on both edges...
            append_deck_slab(
                &mut state.scratch_detail,
                &pts,
                &heights,
                width,
                ROAD_Y_OFFSET,
                slab_color,
            );
            // ...support piers down to the ground (THE grade cue, all tiers)...
            append_deck_piers(
                &mut state.scratch_detail,
                &pts,
                &heights,
                width,
                ROAD_Y_OFFSET,
                &sample,
                pier_color,
            );
            // ...light cel hairlines down both deck edges...
            append_deck_hairlines(
                &mut state.scratch_deckedge,
                &pts,
                &heights,
                width,
                ROAD_Y_OFFSET,
                hairline_color,
            );
            if shadows_enabled {
                // Projected straight down onto the surface below.
                append_blob_shadow(
                    &mut state.scratch_shadow,
                    &pts,
                    &heights,
                    width,
                    &sample,
                    shadow_color,
                );
            }
        }
    }

    let names = ["arterial", "collector", "local"];
    state.local_entity = None;
    state.collector_entity = None;

    #[allow(clippy::needless_range_loop)]
    for i in 0..3 {
        if state.scratch_class[i].is_empty() {
            if let Some(e) = state.class_entities[i].take() {
                commands.entity(e).despawn();
            }
            state.class_meshes[i] = None;
            state.class_materials[i] = None;
            continue;
        }
        let mesh_handle = state.class_meshes[i]
            .get_or_insert_with(|| {
                meshes.add(Mesh::new(
                    bevy::render::mesh::PrimitiveTopology::TriangleList,
                    bevy::render::render_asset::RenderAssetUsages::default(),
                ))
            })
            .clone();
        let aabb = {
            let mesh = meshes.get_mut(&mesh_handle).expect("road class mesh");
            state.scratch_class[i].apply_to_mesh(mesh);
            mesh.compute_aabb().unwrap_or_default()
        };
        let material_handle = state.class_materials[i]
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: road_color,
                    unlit,
                    alpha_mode: AlphaMode::Blend,
                    perceptual_roughness: 1.0,
                    reflectance: 0.0,
                    ..default()
                })
            })
            .clone();
        if let Some(mat) = materials.get_mut(&material_handle) {
            mat.base_color = road_color;
            mat.unlit = unlit;
        }
        let entity = if let Some(e) = state.class_entities[i] {
            if let Ok(mut commands_e) = commands.get_entity(e) {
                commands_e.insert((
                    Mesh3d(mesh_handle.clone()),
                    MeshMaterial3d(material_handle.clone()),
                    aabb,
                    Visibility::Visible,
                ));
                e
            } else {
                let mut entity_commands = commands.spawn((
                    Mesh3d(mesh_handle.clone()),
                    MeshMaterial3d(material_handle.clone()),
                    Transform::IDENTITY,
                    Visibility::default(),
                    aabb,
                    RoadSurface,
                    RoadClassSurface,
                    Name::new(format!("roads-{}", names[i])),
                ));
                if names[i] == "local" {
                    entity_commands.insert(LocalRoadMarker);
                } else if names[i] == "collector" {
                    entity_commands.insert(CollectorRoadMarker);
                }
                let id = entity_commands.id();
                state.class_entities[i] = Some(id);
                id
            }
        } else {
            let mut entity_commands = commands.spawn((
                Mesh3d(mesh_handle),
                MeshMaterial3d(material_handle),
                Transform::IDENTITY,
                Visibility::default(),
                aabb,
                RoadSurface,
                RoadClassSurface,
                Name::new(format!("roads-{}", names[i])),
            ));
            if names[i] == "local" {
                entity_commands.insert(LocalRoadMarker);
            } else if names[i] == "collector" {
                entity_commands.insert(CollectorRoadMarker);
            }
            let id = entity_commands.id();
            state.class_entities[i] = Some(id);
            id
        };
        if names[i] == "local" {
            state.local_entity = Some(entity);
        } else if names[i] == "collector" {
            state.collector_entity = Some(entity);
        } else if names[i] == "arterial" {
            state.arterial_entity = Some(entity);
        }
    }

    // Arterial hairline edge, medium/high tier only (art-direction §1).
    if !unlit && !state.scratch_edge.is_empty() {
        let mesh_handle = state
            .edge_mesh
            .get_or_insert_with(|| {
                meshes.add(Mesh::new(
                    bevy::render::mesh::PrimitiveTopology::TriangleList,
                    bevy::render::render_asset::RenderAssetUsages::default(),
                ))
            })
            .clone();
        let aabb = {
            let mesh = meshes.get_mut(&mesh_handle).expect("road edge mesh");
            state.scratch_edge.apply_to_mesh(mesh);
            mesh.compute_aabb().unwrap_or_default()
        };
        let material_handle = state
            .edge_material
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: palette::road_edge(),
                    unlit: false,
                    alpha_mode: AlphaMode::Blend,
                    perceptual_roughness: 1.0,
                    reflectance: 0.0,
                    ..default()
                })
            })
            .clone();
        if let Some(mat) = materials.get_mut(&material_handle) {
            mat.base_color = palette::road_edge();
        }
        if let Some(e) = state.edge_entity {
            if let Ok(mut commands_e) = commands.get_entity(e) {
                commands_e.insert((
                    Mesh3d(mesh_handle),
                    MeshMaterial3d(material_handle),
                    aabb,
                    Visibility::Visible,
                ));
            } else {
                state.edge_entity = Some(
                    commands
                        .spawn((
                            Mesh3d(mesh_handle),
                            MeshMaterial3d(material_handle),
                            Transform::IDENTITY,
                            Visibility::default(),
                            aabb,
                            RoadSurface,
                            Name::new("roads-arterial-edge"),
                        ))
                        .id(),
                );
            }
        } else {
            state.edge_entity = Some(
                commands
                    .spawn((
                        Mesh3d(mesh_handle),
                        MeshMaterial3d(material_handle),
                        Transform::IDENTITY,
                        Visibility::default(),
                        aabb,
                        RoadSurface,
                        Name::new("roads-arterial-edge"),
                    ))
                    .id(),
            );
        }
    } else if let Some(e) = state.edge_entity.take() {
        commands.entity(e).despawn();
        state.edge_mesh = None;
        state.edge_material = None;
    }
    // ── grade-separation aux meshes: deck skirts + tunnel portals, blob
    //    shadows, and (subway-view) tunnel bodies. Each is a single merged
    //    mesh, upserted with a stable material. ──
    // All grade-separation aux meshes bake their final tone into VERTEX colors
    // (slab / pier / portal / hairline / shadow can differ within one pooled
    // mesh), so every aux material is a plain WHITE base the vertex colors
    // modulate — no per-tone material, no double-multiply.
    let detail_mat = state
        .detail_material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 1.0,
                reflectance: 0.0,
                ..default()
            })
        })
        .clone();
    let st = &mut *state;
    let detail_id = upsert_aux_mesh(
        &mut commands,
        &mut meshes,
        &mut st.scratch_detail,
        &mut st.detail_mesh,
        &mut st.detail_entity,
        detail_mat,
        "roads-grade-detail",
    );
    // Slab/piers/portals fade with the rest of the road surface in subway view.
    if let Some(e) = detail_id {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.insert(RoadSurface);
        }
    }

    // Light deck-edge hairlines (own light material, always lit-independent).
    let deckedge_mat = state
        .deckedge_material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 1.0,
                reflectance: 0.0,
                ..default()
            })
        })
        .clone();
    let st = &mut *state;
    let deckedge_id = upsert_aux_mesh(
        &mut commands,
        &mut meshes,
        &mut st.scratch_deckedge,
        &mut st.deckedge_mesh,
        &mut st.deckedge_entity,
        deckedge_mat,
        "roads-deck-edge",
    );
    if let Some(e) = deckedge_id {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.insert(RoadSurface);
        }
    }

    let shadow_mat = state
        .shadow_material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 1.0,
                reflectance: 0.0,
                ..default()
            })
        })
        .clone();
    let st = &mut *state;
    upsert_aux_mesh(
        &mut commands,
        &mut meshes,
        &mut st.scratch_shadow,
        &mut st.shadow_mesh,
        &mut st.shadow_entity,
        shadow_mat,
        "roads-grade-shadow",
    );

    let tunnel_mat = state
        .tunnel_material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 1.0,
                reflectance: 0.0,
                ..default()
            })
        })
        .clone();
    let st = &mut *state;
    let tunnel_id = upsert_aux_mesh(
        &mut commands,
        &mut meshes,
        &mut st.scratch_tunnel,
        &mut st.tunnel_mesh,
        &mut st.tunnel_entity,
        tunnel_mat,
        "roads-tunnel-body",
    );
    // Tunnel bodies are hidden in the normal view; a dedicated system reveals
    // them at depth in subway view. Start hidden on (re)spawn.
    if let Some(e) = tunnel_id {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.insert((RoadTunnelSurface, Visibility::Hidden));
        }
    }

    stats.road_entities = state.class_entities.iter().filter(|e| e.is_some()).count()
        + usize::from(state.edge_entity.is_some())
        + usize::from(state.detail_entity.is_some())
        + usize::from(state.deckedge_entity.is_some())
        + usize::from(state.shadow_entity.is_some())
        + usize::from(state.tunnel_entity.is_some());
}

/// Upsert a single merged aux mesh (skirts / shadows / tunnel bodies): despawn
/// when empty, otherwise (re)apply the scratch buffer to a long-lived mesh and
/// (re)bind it to a reused entity. Returns the live entity, if any.
fn upsert_aux_mesh(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    buf: &mut MeshBuffers,
    mesh_slot: &mut Option<Handle<Mesh>>,
    entity_slot: &mut Option<Entity>,
    material: Handle<StandardMaterial>,
    name: &'static str,
) -> Option<Entity> {
    if buf.is_empty() {
        if let Some(e) = entity_slot.take() {
            commands.entity(e).despawn();
        }
        *mesh_slot = None;
        return None;
    }
    let mesh_handle = mesh_slot
        .get_or_insert_with(|| {
            meshes.add(Mesh::new(
                bevy::render::mesh::PrimitiveTopology::TriangleList,
                bevy::render::render_asset::RenderAssetUsages::default(),
            ))
        })
        .clone();
    let aabb = {
        let mesh = meshes.get_mut(&mesh_handle).expect("aux road mesh");
        buf.apply_to_mesh(mesh);
        mesh.compute_aabb().unwrap_or_default()
    };
    if let Some(e) = *entity_slot {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.insert((
                Mesh3d(mesh_handle),
                MeshMaterial3d(material),
                aabb,
                Visibility::Visible,
            ));
            return Some(e);
        }
    }
    let id = commands
        .spawn((
            Mesh3d(mesh_handle),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            Visibility::default(),
            aabb,
            Name::new(name),
        ))
        .id();
    *entity_slot = Some(id);
    Some(id)
}

/// Hide local/collector road meshes once the camera climbs above their LOD
/// heights (spec: "Local-roads Visibility toggled by camera height";
/// collectors follow at a higher threshold so arterials alone remain for
/// skyline structure). Reads Bevy's own `Camera3d`/`Transform` rather than
/// `mf-game`'s `CameraRig` component, since `mf-render` must not depend on
/// `mf-game` (the dependency runs the other way).
fn road_lod_system(
    state: Res<RoadsState>,
    effective: Res<EffectiveKnobs>,
    cameras: Query<&Transform, With<Camera3d>>,
    mut visibility: Query<&mut Visibility>,
    counters: Res<crate::perf::PerfCounters>,
) {
    let _span = tracing::info_span!("road_lod").entered();
    let _timer = crate::perf::PerfSpan::start(&counters.road_lod_us);
    let Ok(cam_transform) = cameras.single() else {
        return;
    };
    let y = cam_transform.translation.y;

    // On the fog tiers (Potato/Low) everything past the fog `end` distance is
    // fully blended to the sky color, so any road mesh whose nearest point is
    // beyond that is invisible regardless — but with no MSAA those distant
    // sub-pixel ribbons still alias into the black "scribbles" the horizon
    // showed. Clamp each class's hide-height to the fog `end` so it drops out
    // as soon as it's fully fogged, and give arterials (which otherwise never
    // hide, to hold skyline structure on the un-fogged tiers) a hide-height at
    // all. Off the fog tiers the original height-only LOD is unchanged and
    // arterials never hide.
    let fog_end = effective.0.fog.map(|(_, end)| end);
    let local_hide = fog_end.map_or(LOCAL_ROAD_LOD_HEIGHT, |e| e.min(LOCAL_ROAD_LOD_HEIGHT));
    let collector_hide = fog_end.map_or(COLLECTOR_ROAD_LOD_HEIGHT, |e| {
        e.min(COLLECTOR_ROAD_LOD_HEIGHT)
    });

    // Gate the write through `set_visibility_if_changed` (perf pass): Bevy's
    // change detection fires on every `DerefMut` of `Visibility`, so writing
    // it unconditionally each frame would dirty the render world needlessly.
    let set_vis =
        |entity: Option<Entity>, hide_above: Option<f32>, vis: &mut Query<&mut Visibility>| {
            let Some(entity) = entity else {
                return;
            };
            let Ok(mut v) = vis.get_mut(entity) else {
                return;
            };
            let next = match hide_above {
                Some(h) if y > h => Visibility::Hidden,
                _ => Visibility::Visible,
            };
            crate::perf::set_visibility_if_changed(&mut v, next, Some(&counters));
        };

    set_vis(state.local_entity, Some(local_hide), &mut visibility);
    set_vis(
        state.collector_entity,
        Some(collector_hide),
        &mut visibility,
    );
    // Arterials: only cull on the fog tiers (above the fog `end` height);
    // otherwise they stay for skyline structure.
    set_vis(state.arterial_entity, fog_end, &mut visibility);
}

/// Flip the 3 road-class materials' `unlit` flag when the quality tier
/// changes, mirroring `buildings.rs`'s `apply_quality_to_buildings_material_
/// system` and `terrain.rs`'s equivalent. Without this, `unlit` — baked in
/// once at `build_roads_system` time — goes stale after a runtime tier
/// change (e.g. dropping to Potato mid-session): roads keep rendering via
/// the LIT path with a directional light, while terrain/buildings correctly
/// switch to flat unlit vertex colors, and the mismatch is visible (found
/// via A/B screenshot diffing while fixing this crate's winding/culling —
/// see the `append_ribbon` comment in mesh_utils.rs). The arterial edge
/// deliberately stays out of this (`RoadClassSurface` excludes it) since
/// it's always lit by design, independent of tier.
/// Reveal underground road-tunnel bodies only in subway (Tab) view: hidden at
/// `t == 0` (normal view, where the surface road is suppressed between
/// portals), visible once the view eases toward subway — the road analog of
/// the transit tunnel/tube reveal in `subway.rs`.
fn road_tunnel_subway_visibility_system(
    subway: Res<mf_state::SubwayView>,
    tunnels: Query<Entity, With<RoadTunnelSurface>>,
    mut visibility: Query<&mut Visibility>,
    counters: Res<crate::perf::PerfCounters>,
) {
    let want = if subway.t > 0.02 {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    for e in &tunnels {
        if let Ok(mut v) = visibility.get_mut(e) {
            crate::perf::set_visibility_if_changed(&mut v, want, Some(&counters));
        }
    }
}

/// Fade the projected blob shadows out as the camera climbs (issue #115): at
/// street/oblique zoom the shadows sell grade separation, but at high overview
/// they pile into muddy smears under the network, so scale the shadow
/// material's base alpha from full (at/below `FADE_START_Y`) to zero (at/above
/// `FADE_END_Y`). One material write, gated on a real change.
fn road_shadow_distance_fade_system(
    state: Res<RoadsState>,
    cameras: Query<&Transform, With<Camera3d>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut last: Local<f32>,
) {
    const FADE_START_Y: f32 = 1_600.0;
    const FADE_END_Y: f32 = 5_000.0;
    let Some(handle) = &state.shadow_material else {
        return;
    };
    let Ok(cam) = cameras.single() else {
        return;
    };
    let y = cam.translation.y;
    let fade = 1.0 - ((y - FADE_START_Y) / (FADE_END_Y - FADE_START_Y)).clamp(0.0, 1.0);
    if (fade - *last).abs() < 1e-2 {
        return;
    }
    *last = fade;
    if let Some(mat) = materials.get_mut(handle) {
        let mut c = mat.base_color.to_srgba();
        c.alpha = fade;
        mat.base_color = c.into();
    }
}

fn apply_quality_to_roads_material_system(
    effective: Res<EffectiveKnobs>,
    roads: Query<&MeshMaterial3d<StandardMaterial>, With<RoadClassSurface>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !effective.is_changed() {
        return;
    }
    let unlit = effective.0.unlit_material;
    for handle in &roads {
        if let Some(mat) = materials.get_mut(&handle.0) {
            mat.unlit = unlit;
        }
    }
}
