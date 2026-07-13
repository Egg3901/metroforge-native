//! Procedural city generation (P2). Port of `sim/src/core/city/`.
//!
//! Submodules mirror the TS files one-for-one:
//! * [`presets`] <- `presets.ts` (city knobs + map sizes)
//! * [`names`] <- `names.ts` (deterministic place-name banks)
//! * [`tensor`] <- `tensor.ts` (street-orientation tensor field)
//! * [`streamlines`] <- `streamlines.ts` (evenly-spaced streamline tracing)
//! * [`generator`] <- `generator.ts` (the full terrain/pop/road pipeline)
//!
//! The OSM real-city data path (`osmCity.ts` / `osmRegistry.ts`) is NOT ported
//! in P2; worldgen runs the fully procedural path. See `PORT.md`.

pub mod generator;
pub mod names;
pub mod presets;
pub mod streamlines;
pub mod tensor;

pub use generator::{generate_city, GeneratedCity};
pub use presets::{preset_by_key, CityPreset, MapSize};
