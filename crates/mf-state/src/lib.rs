//! `mf-state` — Bevy `Resource`s shared between `mf-game` and `mf-render`,
//! filled from `mf-net`'s `Events<FromSimMsg>` stream. This crate exists so
//! `mf-render` (owned by a separate agent/session) can depend on the shared
//! state without depending on — or being depended on by — `mf-game`
//! directly.
//!
//! Resources: [`CurrentCity`], [`LatestFields`], [`LatestUi`],
//! [`LatestFrame`], [`QualityTier`], [`SubwayView`], [`HeightAt`],
//! [`RevealState`], [`LatestDemand`], [`OverlayState`].

pub mod city;
pub mod demand;
pub mod fields;
pub mod frame;
pub mod height;
pub mod overlay;
pub mod plugin;
pub mod quality;
pub mod reveal;
pub mod subway;
pub mod ui;

pub use city::CurrentCity;
pub use demand::LatestDemand;
pub use fields::LatestFields;
pub use frame::LatestFrame;
pub use height::HeightAt;
pub use overlay::{OverlayMode, OverlayState};
pub use plugin::MfStatePlugin;
pub use quality::{detect as detect_quality_tier, GpuDeviceKind, QualityKnobs, QualityTier};
pub use reveal::RevealState;
pub use subway::{SubwayView, SUBWAY_TRANSITION_SECS};
pub use ui::LatestUi;
