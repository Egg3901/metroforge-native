//! Binary hot-path frames (spec §1.2). All little-endian. Scalars are read
//! with `from_le_bytes`; `f32`/`u32` arrays are COPIED out via
//! `chunks_exact(4)` rather than cast in place, since the incoming `&[u8]`
//! (a WS frame buffer) is not guaranteed 4-byte aligned.

use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum BinaryError {
    #[error("frame too short: need at least {need} bytes, got {got}")]
    TooShort { need: usize, got: usize },
    #[error("unknown binary msgType {0}")]
    UnknownMsgType(u8),
    #[error("unsupported version {0}")]
    UnsupportedVersion(u8),
    #[error("unknown StaticMask.which {0}")]
    UnknownMaskWhich(u8),
    /// A `StaticBuildings` building's `vertexCount` byte was outside the
    /// wire's documented 3..=64 range. The sidecar is expected to only ever
    /// emit valid polygons, but decode must not trust that: a future data
    /// bug on the wire must fail closed here, not panic or read garbage.
    #[error("StaticBuildings building has vertexCount {got}, must be 3..=64")]
    InvalidVertexCount { got: u8 },
    /// `StaticBuildings.vertexTotal` (used by the caller for prealloc) did
    /// not match the sum of every building's `vertexCount`. Since the
    /// per-building loop is driven by `buildingCount` alone, this can only
    /// be checked after decoding all buildings, unlike `TooShort` (which
    /// fires mid-loop on truncation).
    #[error("StaticBuildings vertexTotal header says {declared}, buildings summed to {actual}")]
    VertexTotalMismatch { declared: u32, actual: u32 },
}

fn u8_at(b: &[u8], off: usize) -> Result<u8, BinaryError> {
    b.get(off).copied().ok_or(BinaryError::TooShort {
        need: off + 1,
        got: b.len(),
    })
}

fn u16_at(b: &[u8], off: usize) -> Result<u16, BinaryError> {
    let s = b.get(off..off + 2).ok_or(BinaryError::TooShort {
        need: off + 2,
        got: b.len(),
    })?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], off: usize) -> Result<u32, BinaryError> {
    let s = b.get(off..off + 4).ok_or(BinaryError::TooShort {
        need: off + 4,
        got: b.len(),
    })?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn f32_at(b: &[u8], off: usize) -> Result<f32, BinaryError> {
    let s = b.get(off..off + 4).ok_or(BinaryError::TooShort {
        need: off + 4,
        got: b.len(),
    })?;
    Ok(f32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Copy `count` little-endian `f32`s starting at byte offset `off`.
fn read_f32_array(b: &[u8], off: usize, count: usize) -> Result<Vec<f32>, BinaryError> {
    let need = count * 4;
    let slice = b.get(off..off + need).ok_or(BinaryError::TooShort {
        need: off + need,
        got: b.len(),
    })?;
    Ok(slice
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

/// Copy `count` little-endian `u32`s starting at byte offset `off`.
fn read_u32_array(b: &[u8], off: usize, count: usize) -> Result<Vec<u32>, BinaryError> {
    let need = count * 4;
    let slice = b.get(off..off + need).ok_or(BinaryError::TooShort {
        need: off + need,
        got: b.len(),
    })?;
    Ok(slice
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

fn read_u8_array(b: &[u8], off: usize, count: usize) -> Result<Vec<u8>, BinaryError> {
    b.get(off..off + count)
        .map(|s| s.to_vec())
        .ok_or(BinaryError::TooShort {
            need: off + count,
            got: b.len(),
        })
}

fn write_u32_array(out: &mut Vec<u8>, values: &[u32]) {
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

fn write_f32_array(out: &mut Vec<u8>, values: &[f32]) {
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// msgType=1 — every 50 ms simulation step.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameSnapshot {
    pub tick: u32,
    pub vehicle_count: u32,
    pub agent_count: u32,
    /// packed 0x00RRGGBB per route color index; index by `vehicles[i*6+5]`.
    pub color_table: Vec<u32>,
    /// stride 6: `[id, x, y, heading, occupancy, routeColorIdx]`
    pub vehicles: Vec<f32>,
    /// stride 3: `[x, y, phase]` (phase: 0 walk, 1 ride, 2 wait)
    pub agents: Vec<f32>,
}

const FRAME_HEADER_LEN: usize = 24;

impl FrameSnapshot {
    pub fn decode(b: &[u8]) -> Result<Self, BinaryError> {
        if b.len() < FRAME_HEADER_LEN {
            return Err(BinaryError::TooShort {
                need: FRAME_HEADER_LEN,
                got: b.len(),
            });
        }
        check_msg_type(b, 1)?;
        let tick = u32_at(b, 4)?;
        let vehicle_count = u32_at(b, 8)?;
        let agent_count = u32_at(b, 12)?;
        let color_table_len = u32_at(b, 16)? as usize;

        let color_table_off = FRAME_HEADER_LEN;
        let color_table = read_u32_array(b, color_table_off, color_table_len)?;

        let vehicles_off = color_table_off + color_table_len * 4;
        let vehicles = read_f32_array(b, vehicles_off, vehicle_count as usize * 6)?;

        let agents_off = vehicles_off + vehicles.len() * 4;
        let agents = read_f32_array(b, agents_off, agent_count as usize * 3)?;

        Ok(FrameSnapshot {
            tick,
            vehicle_count,
            agent_count,
            color_table,
            vehicles,
            agents,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            FRAME_HEADER_LEN
                + self.color_table.len() * 4
                + self.vehicles.len() * 4
                + self.agents.len() * 4,
        );
        out.push(1); // msgType
        out.push(1); // version
        out.extend_from_slice(&0u16.to_le_bytes()); // reserved
        out.extend_from_slice(&self.tick.to_le_bytes());
        out.extend_from_slice(&self.vehicle_count.to_le_bytes());
        out.extend_from_slice(&self.agent_count.to_le_bytes());
        out.extend_from_slice(&(self.color_table.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // reserved
        write_u32_array(&mut out, &self.color_table);
        write_f32_array(&mut out, &self.vehicles);
        write_f32_array(&mut out, &self.agents);
        out
    }
}

/// msgType=2 — init + every 7 sim-days. Array lengths are `cellCount`
/// (reused from `StaticCityJson.fieldW * fieldH` by the caller).
#[derive(Debug, Clone, PartialEq)]
pub struct Fields {
    pub version: u32,
    pub cell_count: u32,
    pub terrain: Vec<f32>,
    pub population: Vec<f32>,
    pub jobs: Vec<f32>,
    pub land_value: Vec<f32>,
    pub water: Vec<u8>,
    pub parks: Vec<u8>,
}

const FIELDS_HEADER_LEN: usize = 16;

impl Fields {
    pub fn decode(b: &[u8]) -> Result<Self, BinaryError> {
        if b.len() < FIELDS_HEADER_LEN {
            return Err(BinaryError::TooShort {
                need: FIELDS_HEADER_LEN,
                got: b.len(),
            });
        }
        check_msg_type(b, 2)?;
        let version = u32_at(b, 4)?;
        let cell_count = u32_at(b, 8)?;
        let n = cell_count as usize;

        let mut off = FIELDS_HEADER_LEN;
        let terrain = read_f32_array(b, off, n)?;
        off += n * 4;
        let population = read_f32_array(b, off, n)?;
        off += n * 4;
        let jobs = read_f32_array(b, off, n)?;
        off += n * 4;
        let land_value = read_f32_array(b, off, n)?;
        off += n * 4;
        let water = read_u8_array(b, off, n)?;
        off += n;
        let parks = read_u8_array(b, off, n)?;

        Ok(Fields {
            version,
            cell_count,
            terrain,
            population,
            jobs,
            land_value,
            water,
            parks,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let n = self.cell_count as usize;
        let mut out = Vec::with_capacity(FIELDS_HEADER_LEN + n * 4 * 4 + n * 2);
        out.push(2);
        out.push(1);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&self.cell_count.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        write_f32_array(&mut out, &self.terrain);
        write_f32_array(&mut out, &self.population);
        write_f32_array(&mut out, &self.jobs);
        write_f32_array(&mut out, &self.land_value);
        out.extend_from_slice(&self.water);
        out.extend_from_slice(&self.parks);
        out
    }
}

/// msgType=3.
#[derive(Debug, Clone, PartialEq)]
pub struct TrafficHotspot {
    pub x: f32,
    pub y: f32,
    pub severity: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Traffic {
    pub w: u32,
    pub h: u32,
    pub cell_size: f32,
    pub origin_x: f32,
    pub origin_y: f32,
    pub values: Vec<f32>,
    pub hotspots: Vec<TrafficHotspot>,
}

const TRAFFIC_HEADER_LEN: usize = 32;

impl Traffic {
    pub fn decode(b: &[u8]) -> Result<Self, BinaryError> {
        if b.len() < TRAFFIC_HEADER_LEN {
            return Err(BinaryError::TooShort {
                need: TRAFFIC_HEADER_LEN,
                got: b.len(),
            });
        }
        check_msg_type(b, 3)?;
        let hotspot_count = u16_at(b, 2)? as usize;
        let w = u32_at(b, 4)?;
        let h = u32_at(b, 8)?;
        let cell_size = f32_at(b, 12)?;
        let origin_x = f32_at(b, 16)?;
        let origin_y = f32_at(b, 20)?;
        let value_count = u32_at(b, 24)? as usize;

        let mut off = TRAFFIC_HEADER_LEN;
        let values = read_f32_array(b, off, value_count)?;
        off += value_count * 4;

        let need = hotspot_count * 12;
        let hs_bytes = b.get(off..off + need).ok_or(BinaryError::TooShort {
            need: off + need,
            got: b.len(),
        })?;
        let hotspots = hs_bytes
            .chunks_exact(12)
            .map(|c| TrafficHotspot {
                x: f32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                y: f32::from_le_bytes([c[4], c[5], c[6], c[7]]),
                severity: f32::from_le_bytes([c[8], c[9], c[10], c[11]]),
            })
            .collect();

        Ok(Traffic {
            w,
            h,
            cell_size,
            origin_x,
            origin_y,
            values,
            hotspots,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            TRAFFIC_HEADER_LEN + self.values.len() * 4 + self.hotspots.len() * 12,
        );
        out.push(3);
        out.push(1);
        out.extend_from_slice(&(self.hotspots.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.w.to_le_bytes());
        out.extend_from_slice(&self.h.to_le_bytes());
        out.extend_from_slice(&self.cell_size.to_le_bytes());
        out.extend_from_slice(&self.origin_x.to_le_bytes());
        out.extend_from_slice(&self.origin_y.to_le_bytes());
        out.extend_from_slice(&(self.values.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        write_f32_array(&mut out, &self.values);
        for hs in &self.hotspots {
            out.extend_from_slice(&hs.x.to_le_bytes());
            out.extend_from_slice(&hs.y.to_le_bytes());
            out.extend_from_slice(&hs.severity.to_le_bytes());
        }
        out
    }
}

/// msgType=4 — 0-3 frames sent right after `ready`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskWhich {
    Water = 0,
    Park = 1,
    Building = 2,
}

impl MaskWhich {
    fn from_u8(v: u8) -> Result<Self, BinaryError> {
        match v {
            0 => Ok(MaskWhich::Water),
            1 => Ok(MaskWhich::Park),
            2 => Ok(MaskWhich::Building),
            other => Err(BinaryError::UnknownMaskWhich(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StaticMask {
    pub which: MaskWhich,
    pub res: u32,
    /// `res * res` bytes, row-major.
    pub mask: Vec<u8>,
}

const STATIC_MASK_HEADER_LEN: usize = 12;

impl StaticMask {
    pub fn decode(b: &[u8]) -> Result<Self, BinaryError> {
        if b.len() < STATIC_MASK_HEADER_LEN {
            return Err(BinaryError::TooShort {
                need: STATIC_MASK_HEADER_LEN,
                got: b.len(),
            });
        }
        check_msg_type(b, 4)?;
        let which = MaskWhich::from_u8(u8_at(b, 2)?)?;
        let res = u32_at(b, 4)?;
        let count = (res as usize) * (res as usize);
        let mask = read_u8_array(b, STATIC_MASK_HEADER_LEN, count)?;
        Ok(StaticMask { which, res, mask })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(STATIC_MASK_HEADER_LEN + self.mask.len());
        out.push(4);
        out.push(1);
        out.push(self.which as u8);
        out.push(0); // reserved
        out.extend_from_slice(&self.res.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&self.mask);
        out
    }
}

/// msgType=5 — building footprints, sent once (not on the periodic cadence
/// of Frame/Fields/Traffic). Purely additive: a city with no
/// `StaticBuildings` message is valid and unremarkable — `mf-render` falls
/// back to its procedural density formula (see `height_dm` doc below), so
/// this does NOT gate `mf-game`'s `Loading` state the way `StaticMask` does.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildingFootprint {
    /// Height in decimeters, verbatim off the wire (NOT converted to
    /// meters, unlike `verts`): the renderer both interprets `0` as
    /// "unknown, use my density formula" and does its own unit conversion,
    /// so decode has no reason to touch this field.
    pub height_dm: u16,
    /// Outer-ring polygon vertices in world meters, origin-centered.
    /// Converted from the wire's half-meter `i16` fixed point (`/2.0`) at
    /// decode time. The sidecar normalizes winding to CCW in the wire's
    /// y-down convention, but decode does NOT trust or check that — winding
    /// is the renderer's concern, and a future winding/data bug here must
    /// not crash the client.
    pub verts: Vec<[f32; 2]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StaticBuildings {
    pub buildings: Vec<BuildingFootprint>,
}

const STATIC_BUILDINGS_HEADER_LEN: usize = 12;
/// Bytes for one building's fixed header (vertexCount, flags, heightDm),
/// ahead of its `vertexCount` vertex pairs.
const BUILDING_HEADER_LEN: usize = 4;
/// Bytes per vertex: `i16 xHalfM, i16 yHalfM`.
const BUILDING_VERTEX_LEN: usize = 4;

impl StaticBuildings {
    pub fn decode(b: &[u8]) -> Result<Self, BinaryError> {
        if b.len() < STATIC_BUILDINGS_HEADER_LEN {
            return Err(BinaryError::TooShort {
                need: STATIC_BUILDINGS_HEADER_LEN,
                got: b.len(),
            });
        }
        check_msg_type(b, 5)?;
        let building_count = u32_at(b, 4)? as usize;
        let vertex_total = u32_at(b, 8)?;

        // Cap the prealloc at what the remaining buffer could possibly hold
        // (each building needs >= BUILDING_HEADER_LEN bytes) so a corrupt or
        // hostile `buildingCount` can't force a huge allocation before the
        // per-building bounds checks below get a chance to reject it.
        let max_possible = (b.len() - STATIC_BUILDINGS_HEADER_LEN) / BUILDING_HEADER_LEN;
        let mut buildings = Vec::with_capacity(building_count.min(max_possible));

        let mut off = STATIC_BUILDINGS_HEADER_LEN;
        let mut vertex_sum: u32 = 0;
        for _ in 0..building_count {
            let vertex_count = u8_at(b, off)?;
            let _flags = u8_at(b, off + 1)?; // reserved, always 0 for now
            let height_dm = u16_at(b, off + 2)?;
            off += BUILDING_HEADER_LEN;

            if !(3..=64).contains(&vertex_count) {
                return Err(BinaryError::InvalidVertexCount { got: vertex_count });
            }

            let vc = vertex_count as usize;
            let need = vc * BUILDING_VERTEX_LEN;
            let vert_bytes = b.get(off..off + need).ok_or(BinaryError::TooShort {
                need: off + need,
                got: b.len(),
            })?;
            let verts = vert_bytes
                .chunks_exact(BUILDING_VERTEX_LEN)
                .map(|c| {
                    let x_half = i16::from_le_bytes([c[0], c[1]]);
                    let y_half = i16::from_le_bytes([c[2], c[3]]);
                    [x_half as f32 / 2.0, y_half as f32 / 2.0]
                })
                .collect();
            off += need;

            vertex_sum += vertex_count as u32;
            buildings.push(BuildingFootprint { height_dm, verts });
        }

        if vertex_sum != vertex_total {
            return Err(BinaryError::VertexTotalMismatch {
                declared: vertex_total,
                actual: vertex_sum,
            });
        }

        Ok(StaticBuildings { buildings })
    }

    pub fn encode(&self) -> Vec<u8> {
        let vertex_total: u32 = self.buildings.iter().map(|bd| bd.verts.len() as u32).sum();
        let body_len: usize = self
            .buildings
            .iter()
            .map(|bd| BUILDING_HEADER_LEN + bd.verts.len() * BUILDING_VERTEX_LEN)
            .sum();
        let mut out = Vec::with_capacity(STATIC_BUILDINGS_HEADER_LEN + body_len);
        out.push(5); // msgType
        out.push(1); // version
        out.extend_from_slice(&0u16.to_le_bytes()); // reserved
        out.extend_from_slice(&(self.buildings.len() as u32).to_le_bytes());
        out.extend_from_slice(&vertex_total.to_le_bytes());
        for building in &self.buildings {
            out.push(building.verts.len() as u8); // vertexCount
            out.push(0); // flags, reserved
            out.extend_from_slice(&building.height_dm.to_le_bytes());
            for v in &building.verts {
                // Inverse of decode's `/2.0`; half-meter quantization means
                // this is exact for values that came from decode, but rounds
                // arbitrary floats built by hand (documented on the type).
                let x_half = (v[0] * 2.0).round() as i16;
                let y_half = (v[1] * 2.0).round() as i16;
                out.extend_from_slice(&x_half.to_le_bytes());
                out.extend_from_slice(&y_half.to_le_bytes());
            }
        }
        out
    }
}

fn check_msg_type(b: &[u8], expected: u8) -> Result<(), BinaryError> {
    let msg_type = u8_at(b, 0)?;
    if msg_type != expected {
        return Err(BinaryError::UnknownMsgType(msg_type));
    }
    let version = u8_at(b, 1)?;
    if version != 1 {
        return Err(BinaryError::UnsupportedVersion(version));
    }
    Ok(())
}

/// Dispatch on byte 0 (`msgType`).
#[derive(Debug, Clone, PartialEq)]
pub enum BinaryMsg {
    Frame(FrameSnapshot),
    Fields(Fields),
    Traffic(Traffic),
    Mask(StaticMask),
    Buildings(StaticBuildings),
}

pub fn decode_binary(b: &[u8]) -> Result<BinaryMsg, BinaryError> {
    let msg_type = u8_at(b, 0)?;
    match msg_type {
        1 => Ok(BinaryMsg::Frame(FrameSnapshot::decode(b)?)),
        2 => Ok(BinaryMsg::Fields(Fields::decode(b)?)),
        3 => Ok(BinaryMsg::Traffic(Traffic::decode(b)?)),
        4 => Ok(BinaryMsg::Mask(StaticMask::decode(b)?)),
        5 => Ok(BinaryMsg::Buildings(StaticBuildings::decode(b)?)),
        other => Err(BinaryError::UnknownMsgType(other)),
    }
}
