//! `mf-state` — Bevy `Resource`s shared between `mf-game` and `mf-render`,
//! filled from `mf-net`'s `Events<FromSimMsg>` stream. This crate exists so
//! `mf-render` (owned by a separate agent/session) can depend on the shared
//! state without depending on — or being depended on by — `mf-game`
//! directly.
//!
//! Resources: [`CurrentCity`], [`LatestFields`], [`LatestUi`],
//! [`LatestFrame`], [`QualityTier`], [`SubwayView`], [`HeightAt`],
//! [`RevealState`], [`LatestDemand`], [`OverlayState`], [`WeatherEffects`].

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
pub mod theme;
pub mod ui;
pub mod weather;

pub use city::CurrentCity;
pub use demand::LatestDemand;
pub use fields::LatestFields;
pub use frame::LatestFrame;
pub use height::HeightAt;
pub use overlay::{OverlayMode, OverlayState};
pub use plugin::MfStatePlugin;
pub use quality::{
    detect as detect_quality_tier, merge_knobs, recommend_tier_from_frame_times,
    sync_effective_knobs_system, DetectedQuality, EffectiveKnobs, GpuDeviceKind, QualityKnobs,
    QualityOverrides, QualityTier, ShadowQuality, DRAW_DISTANCE_MIN_M, DRAW_DISTANCE_UNLIMITED_M,
};
pub use reveal::RevealState;
pub use subway::{SubwayView, SUBWAY_TRANSITION_SECS};
pub use theme::Theme;
pub use ui::LatestUi;
pub use weather::WeatherEffects;
