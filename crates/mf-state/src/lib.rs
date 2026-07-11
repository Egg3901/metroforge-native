//! `mf-state` — Bevy `Resource`s shared between `mf-game` and `mf-render`,
//! filled from `mf-net`'s `Events<FromSimMsg>` stream. This crate exists so
//! `mf-render` (owned by a separate agent/session) can depend on the shared
//! state without depending on — or being depended on by — `mf-game`
//! directly.
//!
//! Resources: [`CurrentCity`], [`LatestFields`], [`LatestUi`],
//! [`LatestFrame`], [`QualityTier`], [`SubwayView`], [`HeightAt`],
//! [`RevealState`], [`LatestDemand`], [`OverlayState`], [`RouteFocus`],
//! [`WeatherEffects`], [`AttractLighting`].
//!
//! Crate map and pipeline: `docs/ARCHITECTURE.md`.

#![warn(missing_docs)]

/// Attract-mode cinematic lighting state.
pub mod attract;
/// Loaded city statics + masks + optional building footprints.
pub mod city;
/// Latest unserved-demand payload from the sidecar.
pub mod demand;
/// Latest fields grid (terrain/population/jobs/…).
pub mod fields;
/// Latest per-tick vehicle/agent frame snapshot.
pub mod frame;
/// Shared ground-height sampler (`HeightAt`).
pub mod height;
/// Overlay mode resource (`Off` / `Demand` / `Unserved`).
pub mod overlay;
/// Bevy plugin that registers resources and applies `SimEvent`s.
pub mod plugin;
/// Quality tier + knob table.
pub mod quality;
/// Cursor/camera building-reveal hole state.
pub mod reveal;
pub mod route_focus;
/// Subway-view toggle + eased progress.
pub mod subway;
/// Visual theme selection (Light/Dark/Purple).
pub mod theme;
/// Latest 2 Hz `UiState`.
pub mod ui;
/// Player weather-effects Settings toggle.
pub mod weather;

pub use attract::AttractLighting;
pub use city::CurrentCity;
pub use demand::LatestDemand;
pub use fields::LatestFields;
pub use frame::LatestFrame;
pub use height::HeightAt;
pub use overlay::{OverlayMode, OverlayState};
pub use plugin::MfStatePlugin;
pub use quality::{detect as detect_quality_tier, GpuDeviceKind, QualityKnobs, QualityTier};
pub use reveal::RevealState;
pub use route_focus::RouteFocus;
pub use subway::{SubwayView, SUBWAY_TRANSITION_SECS};
pub use theme::Theme;
pub use ui::LatestUi;
pub use weather::WeatherEffects;
