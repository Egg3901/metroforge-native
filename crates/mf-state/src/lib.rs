//! `mf-state` — Bevy `Resource`s shared between `mf-game` and `mf-render`,
//! filled from `mf-net`'s `Events<FromSimMsg>` stream. This crate exists so
//! `mf-render` (owned by a separate agent/session) can depend on the shared
//! state without depending on — or being depended on by — `mf-game`
//! directly.
//!
//! Resources: [`CurrentCity`], [`LatestFields`], [`LatestUi`],
//! [`LatestFrame`], [`QualityTier`], [`SubwayView`], [`HeightAt`].

pub mod city;
pub mod fields;
pub mod frame;
pub mod height;
pub mod plugin;
pub mod quality;
pub mod subway;
pub mod ui;

pub use city::CurrentCity;
pub use fields::LatestFields;
pub use frame::LatestFrame;
pub use height::HeightAt;
pub use plugin::MfStatePlugin;
pub use quality::{
    detect as detect_quality_tier, GpuDeviceKind, QualityKnobs, QualityTier, VehicleMesh,
};
pub use subway::{SubwayView, SUBWAY_TRANSITION_SECS};
pub use ui::LatestUi;
