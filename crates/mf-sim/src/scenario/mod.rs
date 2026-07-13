//! Scenario system (v0.x). Port of `sim/src/core/scenario/*`.
//!
//! Only the deterministic win/lose evaluation + UI snapshot builder
//! (`evaluate.ts`) is ported in P3-ENV. The data-driven catalog / progression
//! manifest (`catalog.ts`, `progression.ts`, content JSON) are content-lane
//! concerns wired later; `evaluate` takes the win/lose trees as inputs so it
//! stands alone.

pub mod evaluate;
