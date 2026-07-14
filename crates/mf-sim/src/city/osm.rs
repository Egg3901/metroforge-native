//! Real-city OSM data path. Port of `sim/src/core/city/osmCity.ts`
//! (mask/elevation decoders + the `OsmCityData` model) plus the OSM branch of
//! `generateCity` (terrain/water/parks-from-mask + real road network).
//!
//! Determinism: OSM data is STATIC input, not RNG. Every decode is a pure
//! function of the bundle bytes, so loading is deterministic and run-twice
//! identical. The only RNG in the OSM city path is the shared population/
//! subcenter noise (which runs before roads, unchanged from the procedural
//! path) and the district-naming fork; OSM never traces streamlines, so it
//! draws no road RNG.
//!
//! This module stays serde-free (like the rest of `mf-sim`): JSON parsing and
//! the key -> bundle registry live in the host layer (`mf-net`, which mirrors
//! `sim/sidecar/cities.ts` + `simHost.ts`). Here we consume an already-parsed
//! [`OsmCityData`] of raw base64 payloads + geometry.

use crate::fields::cell_center;
use crate::geometry::Noise2D;
use crate::types::{FieldGrid, MapLabel, PoiAnchor, RoadClass};

/// One real OSM road segment, as carried in the bundle. Mirrors the
/// `roads[]` entry shape in `osmCity.ts` (`{ cls, pts, g?, br?, tn? }`).
#[derive(Clone, Debug, PartialEq)]
pub struct OsmRoad {
    /// Road class string (`arterial` / `collector` / anything else -> local).
    pub cls: String,
    /// Flat `[x0, y0, x1, y1, ...]` world-meter vertices.
    pub pts: Vec<f64>,
    /// Grade-separation level (signed; 0 = ground). Default 0.
    pub g: i32,
    /// Bridge deck. Default false.
    pub br: bool,
    /// Tunnel. Default false.
    pub tn: bool,
}

/// A parsed real-city OSM bundle. Mirrors `OsmCityData` (`osmCity.ts`) plus the
/// `poiAnchors` array that rides on the raw JSON. Masks/elevation are kept as
/// the raw base64 strings from the bundle and decoded lazily in [`apply_osm`],
/// exactly like the TS generator does.
#[derive(Clone, Debug, PartialEq)]
pub struct OsmCityData {
    /// Preset key (`"nyc"`, ...).
    pub key: String,
    /// Display label (`"New York"`).
    pub label: String,
    /// World square side length in meters.
    pub world_size: f64,
    /// Mask side length (`mask_res * mask_res` cells).
    pub mask_res: u32,
    /// Masks are 1-bit-per-cell packed (vs legacy 1-byte-per-cell).
    pub mask_packed: bool,
    /// Base64 water mask (1 = water).
    pub water_mask: String,
    /// Base64 park mask (1 = park/green), if baked.
    pub park_mask: Option<String>,
    /// Base64 building-footprint coverage mask, if baked.
    pub building_mask: Option<String>,
    /// Real-elevation heightfield side length, if baked.
    pub elev_res: Option<u32>,
    /// Base64 little-endian Int16 elevation grid (meters), if baked.
    pub elevation: Option<String>,
    /// Real OSM road network.
    pub roads: Vec<OsmRoad>,
    /// Real OSM place-name labels.
    pub labels: Vec<MapLabel>,
    /// Named POI anchors (stadium/airport/university/...).
    pub poi_anchors: Vec<PoiAnchor>,
}

// ── base64 ──────────────────────────────────────────────────────────────────

/// Decode a standard-alphabet base64 string to raw bytes. Mirrors JS `atob`
/// (padding-tolerant; ignores whitespace and stray non-alphabet bytes the way
/// a lenient decoder does). Pure + deterministic.
fn b64_decode(s: &str) -> Vec<u8> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3 + 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &c in s.as_bytes() {
        let Some(v) = val(c) else { continue };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// Decode a base64 mask to a `Vec<u8>` of `n` cells (1 = set). Packed = 1 bit
/// per cell (current format); otherwise 1 byte per cell (legacy). Mirrors
/// `decodeB64Mask`.
pub fn decode_b64_mask(b64: &str, n: usize, packed: bool) -> Vec<u8> {
    let bin = b64_decode(b64);
    if !packed {
        return bin;
    }
    let count = if n > 0 { n } else { bin.len() * 8 };
    let mut out = vec![0u8; count];
    for (i, cell) in out.iter_mut().enumerate() {
        let byte = bin.get(i >> 3).copied().unwrap_or(0);
        *cell = (byte >> (i & 7)) & 1;
    }
    out
}

/// Decode a base64 little-endian Int16 elevation grid to `res*res` meter
/// samples (row-major). Mirrors `decodeElevation`.
pub fn decode_elevation(b64: &str, res: u32) -> Vec<i16> {
    let bin = b64_decode(b64);
    let n = (res as usize) * (res as usize);
    let mut out = vec![0i16; n];
    for (i, h) in out.iter_mut().enumerate() {
        let lo = bin.get(i * 2).copied().unwrap_or(0) as u16;
        let hi = bin.get(i * 2 + 1).copied().unwrap_or(0) as u16;
        *h = ((hi << 8) | lo) as i16;
    }
    out
}

/// Sample a mask at a world point -> true if the containing cell is set.
/// Mirrors `maskAt` (row `r = floor((y+half)/worldSize * res)`).
pub fn mask_at(mask: &[u8], res: u32, world_size: f64, x: f64, y: f64) -> bool {
    let half = world_size / 2.0;
    let c = (((x + half) / world_size) * res as f64).floor() as i64;
    let r = (((y + half) / world_size) * res as f64).floor() as i64;
    if c < 0 || r < 0 || c >= res as i64 || r >= res as i64 {
        return false;
    }
    mask[(r * res as i64 + c) as usize] == 1
}

/// Map a bundle road-class string onto [`RoadClass`] (unknown -> local).
/// Mirrors the `cls` coercion in `generateCity`'s OSM road branch.
pub fn road_class_of(cls: &str) -> RoadClass {
    match cls {
        "arterial" => RoadClass::Arterial,
        "collector" => RoadClass::Collector,
        _ => RoadClass::Local,
    }
}

// ── OSM terrain / water / parks from baked masks ─────────────────────────────

/// The high-resolution static channels decoded from an OSM bundle, threaded
/// out of the generator onto [`crate::city::GeneratedCity`] and, from there,
/// onto the transient `GameState.osm_*` slots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OsmChannels {
    /// High-res water mask (1 = water), `mask_res * mask_res` bytes.
    pub water_mask: Option<Vec<u8>>,
    /// High-res park mask.
    pub park_mask: Option<Vec<u8>>,
    /// High-res building-footprint mask.
    pub building_mask: Option<Vec<u8>>,
    /// Mask side length.
    pub mask_res: Option<u32>,
    /// Real-elevation heightfield (meters, row-major, `elev_res^2`).
    pub elevation: Option<Vec<i16>>,
    /// Elevation side length.
    pub elev_res: Option<u32>,
}

/// Stamp real land/water/parks + subtle shore-faded relief onto the coarse
/// field grid from an OSM bundle, and return the decoded hi-res channels.
/// Faithful port of the `if (osm)` terrain branch of `generateCity`
/// (`generator.ts` lines 183-270): AREA-fraction majority vote of the ~19 m
/// mask over each 125 m field cell, then a multi-source BFS distance-to-water
/// that fades the fbm relief in over ~1.2 km inland.
pub fn apply_osm_terrain(
    fields: &mut FieldGrid,
    osm: &OsmCityData,
    terrain_noise: &Noise2D,
) -> OsmChannels {
    let world_size = fields.w as f64 * fields.cell_size;
    let res = osm.mask_res;
    let n_mask = (res as usize) * (res as usize);
    let packed = osm.mask_packed;

    let water = decode_b64_mask(&osm.water_mask, n_mask, packed);
    let park = osm
        .park_mask
        .as_ref()
        .map(|m| decode_b64_mask(m, n_mask, packed));
    let building = osm
        .building_mask
        .as_ref()
        .map(|m| decode_b64_mask(m, n_mask, packed));
    let elevation = match (&osm.elevation, osm.elev_res) {
        (Some(e), Some(r)) => Some(decode_elevation(e, r)),
        _ => None,
    };

    // AREA fraction of the hi-res mask over each field cell footprint. SUB=7.
    const SUB: usize = 7;
    let half_cell = fields.cell_size / 2.0;
    let mask_frac = |m: &[u8], wx: f64, wy: f64| -> f64 {
        let mut hit = 0u32;
        for sy in 0..SUB {
            let py = wy - half_cell + ((sy as f64 + 0.5) / SUB as f64) * fields.cell_size;
            for sx in 0..SUB {
                let px = wx - half_cell + ((sx as f64 + 0.5) / SUB as f64) * fields.cell_size;
                if mask_at(m, res, world_size, px, py) {
                    hit += 1;
                }
            }
        }
        hit as f64 / (SUB * SUB) as f64
    };

    let w = fields.w as usize;
    let h = fields.h as usize;
    for cy in 0..h {
        for cx in 0..w {
            let i = cy * w + cx;
            let p = cell_center(fields, i);
            let is_water = mask_frac(&water, p.x, p.y) > 0.5;
            fields.water[i] = if is_water { 1 } else { 0 };
            if let Some(pm) = &park {
                if !is_water && mask_frac(pm, p.x, p.y) > 0.5 {
                    fields.parks[i] = 1;
                }
            }
        }
    }

    // Multi-source BFS distance-to-water (4-neighborhood, distance in cells).
    let cells = w * h;
    let mut dist = vec![f32::INFINITY; cells];
    let mut queue: Vec<usize> = Vec::new();
    for (i, d) in dist.iter_mut().enumerate() {
        if fields.water[i] == 1 {
            *d = 0.0;
            queue.push(i);
        }
    }
    let mut qi = 0;
    while qi < queue.len() {
        let i = queue[qi];
        qi += 1;
        let d = dist[i] + 1.0;
        let cx = i % w;
        let cy = i / w;
        let mut neighbors: [i64; 4] = [-1; 4];
        if cx > 0 {
            neighbors[0] = (i - 1) as i64;
        }
        if cx < w - 1 {
            neighbors[1] = (i + 1) as i64;
        }
        if cy > 0 {
            neighbors[2] = (i - w) as i64;
        }
        if cy < h - 1 {
            neighbors[3] = (i + w) as i64;
        }
        for ni in neighbors {
            if ni >= 0 {
                let ni = ni as usize;
                if d < dist[ni] {
                    dist[ni] = d;
                    queue.push(ni);
                }
            }
        }
    }

    let shore_fade_cells = {
        let v = (1200.0 / fields.cell_size) as i64;
        if v == 0 {
            1.0
        } else {
            v as f64
        }
    };
    for cy in 0..h {
        for cx in 0..w {
            let i = cy * w + cx;
            if fields.water[i] == 1 {
                fields.terrain[i] = 0.12;
                continue;
            }
            let p = cell_center(fields, i);
            let elev = terrain_noise.fbm(
                (p.x / world_size) * 4.0 + 10.0,
                (p.y / world_size) * 4.0 + 10.0,
                4,
                2.0,
                0.5,
            );
            let t = (dist[i] as f64 / shore_fade_cells).min(1.0);
            let fade = t * t * (3.0 - 2.0 * t); // smoothstep
            fields.terrain[i] = (0.2 + elev * 0.12 * fade).clamp(0.0, 1.0) as f32;
        }
    }

    OsmChannels {
        water_mask: Some(water),
        park_mask: park,
        building_mask: building,
        mask_res: Some(res),
        elevation,
        elev_res: osm.elev_res,
    }
}
