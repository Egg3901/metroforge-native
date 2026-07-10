//! Disk save slots (ship-plan #25 v0.4 `saves.rs`): 3 numbered slots plus one
//! autosave slot, each a JSON file under `ProjectDirs("com","ahousedivided",
//! "MetroForge").data_dir()/saves/<slot>.json` — same `directories` crate
//! pattern `config.rs` uses for `config.toml`.
//!
//! The sim's own save format (`mf_protocol::envelope::SavedPayload.json` /
//! `LoadSavePayload.json`) is opaque to this client — see `deserialize`/
//! `serialize` in `metroforge/src/core/save.ts` — so every file on disk is a
//! thin wrapper: `{"meta": {...}, "sim": <opaque sim state>}`. `meta` is
//! populated from client-side context (`LatestUi`/`PendingInit`) at save
//! time rather than parsed back out of the opaque blob, because the sim
//! state itself carries no city identifier at all (`GameState` in
//! `core/types.ts` has no `presetKey` field — the sidecar's own
//! `initMeta.presetKey` is transient, sidecar-process-local memory, never
//! serialized) and no `day`/`cash` shortcut worth re-deriving when
//! `LatestUi.day`/`.cash` already have both, freshly, at zero parsing cost.
//!
//! # CRITICAL integration finding — read before touching the load path
//!
//! `simHost.ts`'s `handleLoadSave` calls `sendStatic`/`sendUi` just like
//! `handleInit` does, so a load DOES walk the client back through the same
//! `ready`/`fields`/`ui` stream `Loading`'s gate (`state.rs`'s
//! `loading_gate_system`) already waits on — jumping to `AppState::Loading`
//! after a load is sound *in isolation*.
//!
//! The landmine is `state.rs`'s `send_init_system`, which is unconditionally
//! wired to `OnEnter(AppState::Loading)` and sends a *fresh* `ToSim::Init`
//! every time that state is entered, with no way (from this read-only file)
//! to suppress it for a "resume a save" entry. Sent naively, a `LoadSave`
//! issued right before `next_state.set(Loading)` would have its restored
//! state immediately clobbered: the sidecar processes messages in send
//! order, so `Init` (queued by `OnEnter`, which Bevy's `StateTransition`
//! schedule runs before `Update` every frame per `MainScheduleOrder`) would
//! land on the wire *after* our `LoadSave` and overwrite it with
//! `newGame(...)`.
//!
//! The fix here is to never send `LoadSave` from [`SaveManager::load`]
//! directly. `load` only reads/parses the slot and stashes the sim JSON in
//! `pending_load`; [`send_pending_load_system`] — an ordinary `Update`
//! system gated on `in_state(AppState::Loading)` — is what actually puts
//! `ToSim::LoadSave` on the wire, and by the schedule ordering above that is
//! *guaranteed* to run after this frame's `OnEnter(Loading)` `send_init_
//! system`, so `LoadSave` is always the last word. The sidecar still briefly
//! constructs and throws away a fresh, wrong game in between (wasted work,
//! not a correctness bug — `CurrentCity::set_static_city` clears stale
//! masks/buildings on every `ready`, so the final client-side state is
//! clean), and none of it is visible: `loading_hud_system` covers the whole
//! window with a `CentralPanel` for the entire `Loading` state. A fully
//! clean fix would teach `send_init_system` to skip sending `Init` when
//! `Loading` was entered via a load-continue rather than a fresh Start —
//! that's a `state.rs` change outside this wave's ownership; flagged for a
//! follow-up.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use mf_net::{SimEvent, SimLink};
use mf_protocol::envelope::{FromSimJson, LoadSavePayload};
use mf_protocol::{FromSimMsg, ToSim, ToastTone};
use mf_state::LatestUi;
use serde::{Deserialize, Serialize};

use crate::audio::{PlaySfx, Sfx};
use crate::hud::ToastLog;
use crate::state::{AppState, PendingInit};

/// Number of player-addressable numbered slots (in addition to the one
/// autosave slot). Player-facing numbering is 1-based (`Slot(1)`, not
/// `Slot(0)`) so slot numbers in code match what the HUD prints.
pub const SLOT_COUNT: u8 = 3;

/// Autosave cadence, in in-game days (`LatestUi.day` ticks), per the ship
/// plan's "every 10 game days" spec.
pub const AUTOSAVE_INTERVAL_DAYS: u32 = 10;

/// One save destination: a numbered slot (1..=[`SLOT_COUNT`]) or the single
/// autosave slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SaveSlot {
    Slot(u8),
    Autosave,
}

impl SaveSlot {
    /// Filename stem (sans `.json`) this slot is stored under.
    fn file_stem(self) -> String {
        match self {
            SaveSlot::Slot(n) => format!("slot{n}"),
            SaveSlot::Autosave => "autosave".to_string(),
        }
    }
}

/// Metadata captured alongside the opaque sim state, purely for the menu's
/// "Continue" cards — never fed back into the sim itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveMeta {
    /// The city preset key active at save time (`PendingInit::preset_key`),
    /// if one was known. `None` rather than a guess when it wasn't (should
    /// only happen for a save file from before this field existed).
    pub city_label: Option<String>,
    pub day: u32,
    pub cash: f64,
    pub saved_at_epoch_secs: u64,
}

/// One slot's on-disk shape: `{"meta": {...}, "sim": <opaque sim JSON>}`.
/// `sim` is kept as a generic [`serde_json::Value`] rather than re-parsed
/// into any typed shape — this client has no business understanding the
/// sim's own save schema, only round-tripping it byte-for-byte-equivalent
/// (key order may differ after a `Value` round-trip; JSON object key order
/// is never semantically meaningful, so that's fine).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SaveWrapper {
    meta: SaveMeta,
    sim: serde_json::Value,
}

/// One entry in [`list`]'s result: a slot plus its metadata, if occupied.
#[derive(Debug, Clone)]
pub struct SlotEntry {
    pub slot: SaveSlot,
    pub meta: Option<SaveMeta>,
}

fn saves_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("com", "ahousedivided", "MetroForge")
        .map(|dirs| dirs.data_dir().join("saves"))
}

fn slot_path(slot: SaveSlot) -> Option<PathBuf> {
    saves_dir().map(|dir| dir.join(format!("{}.json", slot.file_stem())))
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Parse `sim_json` and wrap it with `meta` into the on-disk document
/// string. Pure (no filesystem) so it's unit-testable directly.
fn build_wrapper_json(meta: &SaveMeta, sim_json: &str) -> anyhow::Result<String> {
    let sim: serde_json::Value = serde_json::from_str(sim_json)?;
    let wrapper = SaveWrapper {
        meta: meta.clone(),
        sim,
    };
    Ok(serde_json::to_string(&wrapper)?)
}

/// Inverse of [`build_wrapper_json`]: parse a wrapper document back into its
/// metadata plus a re-serialized opaque sim JSON string (suitable to hand
/// straight to `ToSim::LoadSave`). Pure (no filesystem).
fn parse_wrapper_json(contents: &str) -> anyhow::Result<(SaveMeta, String)> {
    let wrapper: SaveWrapper = serde_json::from_str(contents)?;
    let sim_json = serde_json::to_string(&wrapper.sim)?;
    Ok((wrapper.meta, sim_json))
}

/// Pure autosave-cadence check: has at least `interval_days` passed since
/// `last_autosave_day` (treating "never autosaved" as day 0)? Split out from
/// the system that calls it so the cadence logic is unit-testable without a
/// Bevy `App`.
fn should_autosave(last_autosave_day: Option<u32>, current_day: u32, interval_days: u32) -> bool {
    if interval_days == 0 {
        return false;
    }
    let baseline = last_autosave_day.unwrap_or(0);
    current_day.saturating_sub(baseline) >= interval_days
}

/// A save write kicked off by [`SaveManager::request_save`], captured at
/// request time and completed once the sidecar's `Saved` reply arrives
/// (see [`capture_saved_system`]).
struct PendingSave {
    slot: SaveSlot,
    meta: SaveMeta,
}

/// Owns in-flight save/load bookkeeping. A `Resource` so both `hud.rs`'s
/// main menu (Continue section) and pause overlay can drive it.
#[derive(Resource, Default)]
pub struct SaveManager {
    pending_save: Option<PendingSave>,
    /// Sim JSON queued for [`send_pending_load_system`] to actually put on
    /// the wire — see the module doc's CRITICAL section for why this isn't
    /// sent directly from [`SaveManager::load`].
    pending_load: Option<String>,
    last_autosave_day: Option<u32>,
}

impl SaveManager {
    /// True when a slot load is staged for `send_pending_load_system`;
    /// `state.rs` consults this to skip the throwaway fresh `Init` that
    /// would otherwise briefly build a city the `LoadSave` then replaces.
    pub fn has_pending_load(&self) -> bool {
        self.pending_load.is_some()
    }

    /// Start a save into `slot`: sends `ToSim::RequestSave` and remembers
    /// `slot` plus the metadata snapshotted right now (`city_label`/`day`/
    /// `cash` — whatever's true the instant the player clicked, not
    /// whenever the sidecar's reply happens to land) so
    /// [`capture_saved_system`] can write it once `Saved` arrives.
    ///
    /// A second call before the first completes silently replaces the
    /// pending request (last request wins) rather than queuing both —
    /// there is exactly one in-flight `requestSave`/`saved` round trip on
    /// the wire at a time, so trying to remember two pending slots could
    /// only ever misattribute the single reply to the wrong one.
    ///
    /// Never panics: a transport send failure is toasted + [`Sfx::Error`]
    /// immediately (there will be no `Saved` reply to react to later).
    #[allow(clippy::too_many_arguments)]
    pub fn request_save(
        &mut self,
        slot: SaveSlot,
        city_label: Option<String>,
        day: u32,
        cash: f64,
        link: &SimLink,
        toasts: &mut ToastLog,
        sfx: &mut EventWriter<PlaySfx>,
    ) {
        match link.transport.send(ToSim::RequestSave) {
            Ok(()) => {
                self.pending_save = Some(PendingSave {
                    slot,
                    meta: SaveMeta {
                        city_label,
                        day,
                        cash,
                        saved_at_epoch_secs: epoch_secs(),
                    },
                });
            }
            Err(e) => {
                tracing::warn!("mf-game: failed to send requestSave: {e}");
                toasts
                    .0
                    .push((format!("Save failed: {e}"), ToastTone::Warn));
                sfx.write(PlaySfx(Sfx::Error));
            }
        }
    }

    /// Read `slot`'s wrapper from disk and queue its sim JSON for
    /// [`send_pending_load_system`] to send once `Loading` is entered (see
    /// the module doc's CRITICAL section). Returns the slot's metadata on
    /// success so the caller can e.g. log it; on failure, toasts +
    /// [`Sfx::Error`] itself and returns `None` — callers don't need their
    /// own error-handling branch, just an `if let Some(_) = ...`.
    pub fn load(
        &mut self,
        slot: SaveSlot,
        toasts: &mut ToastLog,
        sfx: &mut EventWriter<PlaySfx>,
    ) -> Option<SaveMeta> {
        match Self::try_load(slot) {
            Ok((meta, sim_json)) => {
                self.pending_load = Some(sim_json);
                sfx.write(PlaySfx(Sfx::Confirm));
                Some(meta)
            }
            Err(e) => {
                tracing::warn!("mf-game: failed to load save slot: {e}");
                toasts
                    .0
                    .push((format!("Load failed: {e}"), ToastTone::Warn));
                sfx.write(PlaySfx(Sfx::Error));
                None
            }
        }
    }

    fn try_load(slot: SaveSlot) -> anyhow::Result<(SaveMeta, String)> {
        let path = slot_path(slot)
            .ok_or_else(|| anyhow::anyhow!("no data directory available on this platform"))?;
        let contents = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        parse_wrapper_json(&contents)
    }
}

/// List every slot (autosave first, then numbered slots 1..=[`SLOT_COUNT`])
/// with its metadata if occupied — for the main menu's Continue section and
/// the pause overlay's occupied/empty indicators. Never panics: an
/// unreadable or corrupt slot file just reads back as empty (`meta: None`)
/// rather than surfacing a parse error into the menu.
pub fn list() -> Vec<SlotEntry> {
    let mut out = Vec::with_capacity(SLOT_COUNT as usize + 1);
    out.push(SlotEntry {
        slot: SaveSlot::Autosave,
        meta: read_meta(SaveSlot::Autosave),
    });
    for n in 1..=SLOT_COUNT {
        out.push(SlotEntry {
            slot: SaveSlot::Slot(n),
            meta: read_meta(SaveSlot::Slot(n)),
        });
    }
    out
}

fn read_meta(slot: SaveSlot) -> Option<SaveMeta> {
    let path = slot_path(slot)?;
    let contents = std::fs::read_to_string(path).ok()?;
    parse_wrapper_json(&contents).ok().map(|(meta, _)| meta)
}

fn write_slot(slot: SaveSlot, meta: &SaveMeta, sim_json: &str) -> anyhow::Result<()> {
    let path = slot_path(slot)
        .ok_or_else(|| anyhow::anyhow!("no data directory available on this platform"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let wrapper_json = build_wrapper_json(meta, sim_json)?;
    std::fs::write(&path, wrapper_json)?;
    Ok(())
}

fn slot_saved_message(slot: SaveSlot) -> String {
    match slot {
        SaveSlot::Autosave => "Autosaved".to_string(),
        SaveSlot::Slot(n) => format!("Saved to slot {n}"),
    }
}

/// Captures the sidecar's `saved` reply to whichever `request_save` is
/// currently pending and writes it to disk. Never panics: a write failure
/// (disk full, permissions, ...) is toasted + [`Sfx::Error`] rather than
/// propagated.
fn capture_saved_system(
    mut manager: ResMut<SaveManager>,
    mut events: EventReader<SimEvent>,
    mut toasts: ResMut<ToastLog>,
    mut sfx: EventWriter<PlaySfx>,
) {
    for SimEvent(msg) in events.read() {
        let FromSimMsg::Json(FromSimJson::Saved(payload)) = msg else {
            continue;
        };
        let Some(pending) = manager.pending_save.take() else {
            // A `saved` reply with nothing pending (e.g. some other client
            // path calling ToSim::RequestSave directly) — nothing to write.
            continue;
        };
        match write_slot(pending.slot, &pending.meta, &payload.json) {
            Ok(()) => {
                toasts
                    .0
                    .push((slot_saved_message(pending.slot), ToastTone::Good));
                sfx.write(PlaySfx(Sfx::Confirm));
            }
            Err(e) => {
                tracing::warn!("mf-game: failed to write save slot: {e}");
                toasts
                    .0
                    .push((format!("Save failed: {e}"), ToastTone::Warn));
                sfx.write(PlaySfx(Sfx::Error));
            }
        }
    }
}

/// Every 10 game days (`AUTOSAVE_INTERVAL_DAYS`), autosaves into the
/// dedicated autosave slot. Only runs `InGame` — `LatestUi` can hold a
/// stale `Some` from a previous session while `Loading` a new one, and
/// autosaving mid-load would misattribute the still-loading city.
fn autosave_system(
    mut manager: ResMut<SaveManager>,
    ui: Res<LatestUi>,
    pending_init: Res<PendingInit>,
    link: Option<Res<SimLink>>,
    mut toasts: ResMut<ToastLog>,
    mut sfx: EventWriter<PlaySfx>,
    mut last_seen_day: Local<Option<u32>>,
) {
    let Some(state) = &ui.0 else {
        return;
    };
    if *last_seen_day == Some(state.day) {
        return;
    }
    // A day counter that goes backwards means a new game started since we
    // last looked (this `Local`/`last_autosave_day` survive across
    // Loading -> InGame transitions since they aren't reset by the state
    // machine) — rebase to a fresh baseline rather than inheriting the
    // previous session's day count, which would otherwise delay the new
    // session's first autosave until it caught back up.
    if last_seen_day.is_some_and(|prev| state.day < prev) {
        manager.last_autosave_day = None;
    }
    *last_seen_day = Some(state.day);
    if !should_autosave(manager.last_autosave_day, state.day, AUTOSAVE_INTERVAL_DAYS) {
        return;
    }
    let Some(link) = link else {
        return;
    };
    manager.request_save(
        SaveSlot::Autosave,
        Some(pending_init.preset_key.clone()),
        state.day,
        state.cash,
        &link,
        &mut toasts,
        &mut sfx,
    );
    manager.last_autosave_day = Some(state.day);
}

/// Sends the actual `ToSim::LoadSave` for whatever [`SaveManager::load`]
/// queued, once `Loading` is entered. See the module doc's CRITICAL section
/// for why this is deferred here instead of sent directly from `load`.
fn send_pending_load_system(mut manager: ResMut<SaveManager>, link: Option<Res<SimLink>>) {
    if manager.pending_load.is_none() {
        return;
    }
    let Some(link) = link else {
        // No transport yet (shouldn't happen — Loading requires a SimLink
        // to have gotten this far) — retry next frame rather than dropping
        // the queued load.
        return;
    };
    if let Some(sim_json) = manager.pending_load.take() {
        let _ = link
            .transport
            .send(ToSim::LoadSave(LoadSavePayload { json: sim_json }));
    }
}

pub struct MfSavesPlugin;

impl Plugin for MfSavesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SaveManager>().add_systems(
            Update,
            (
                capture_saved_system,
                autosave_system.run_if(in_state(AppState::InGame)),
                send_pending_load_system.run_if(in_state(AppState::Loading)),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- slot path mapping -------------------------------------------------

    #[test]
    fn file_stem_numbered_slots_are_one_indexed() {
        assert_eq!(SaveSlot::Slot(1).file_stem(), "slot1");
        assert_eq!(SaveSlot::Slot(2).file_stem(), "slot2");
        assert_eq!(SaveSlot::Slot(3).file_stem(), "slot3");
    }

    #[test]
    fn file_stem_autosave_is_distinct_from_numbered_slots() {
        let autosave = SaveSlot::Autosave.file_stem();
        assert_eq!(autosave, "autosave");
        for n in 1..=SLOT_COUNT {
            assert_ne!(autosave, SaveSlot::Slot(n).file_stem());
        }
    }

    // --- wrapper round-trip --------------------------------------------------

    fn sample_meta() -> SaveMeta {
        SaveMeta {
            city_label: Some("nyc".to_string()),
            day: 42,
            cash: 1_234_567.5,
            saved_at_epoch_secs: 1_700_000_000,
        }
    }

    #[test]
    fn wrapper_round_trips_meta_and_sim_json() {
        let sim_json = r#"{"tick":100,"budget":{"cash":1234.5},"stations":[]}"#;
        let wrapped = build_wrapper_json(&sample_meta(), sim_json).expect("build wrapper");
        let (meta, sim_back) = parse_wrapper_json(&wrapped).expect("parse wrapper");
        assert_eq!(meta, sample_meta());

        // JSON `Value` round-trips may reorder object keys (JSON object key
        // order carries no semantic meaning), so compare parsed values
        // rather than raw strings.
        let original: serde_json::Value = serde_json::from_str(sim_json).unwrap();
        let round_tripped: serde_json::Value = serde_json::from_str(&sim_back).unwrap();
        assert_eq!(original, round_tripped);
    }

    #[test]
    fn wrapper_round_trip_preserves_none_city_label() {
        let meta = SaveMeta {
            city_label: None,
            ..sample_meta()
        };
        let wrapped = build_wrapper_json(&meta, "{}").expect("build wrapper");
        let (parsed_meta, _) = parse_wrapper_json(&wrapped).expect("parse wrapper");
        assert_eq!(parsed_meta.city_label, None);
    }

    #[test]
    fn parse_wrapper_rejects_garbage() {
        assert!(parse_wrapper_json("not json").is_err());
        assert!(parse_wrapper_json(r#"{"meta": {}}"#).is_err()); // missing sim + required meta fields
    }

    #[test]
    fn build_wrapper_rejects_non_json_sim_string() {
        assert!(build_wrapper_json(&sample_meta(), "not json").is_err());
    }

    // --- autosave day-trigger logic ------------------------------------------

    #[test]
    fn should_autosave_never_saved_triggers_at_the_interval() {
        assert!(!should_autosave(None, 9, 10));
        assert!(should_autosave(None, 10, 10));
        assert!(should_autosave(None, 11, 10));
    }

    #[test]
    fn should_autosave_measures_from_last_autosave_day() {
        assert!(!should_autosave(Some(10), 19, 10));
        assert!(should_autosave(Some(10), 20, 10));
    }

    #[test]
    fn should_autosave_zero_interval_never_triggers() {
        assert!(!should_autosave(None, 1000, 0));
    }

    #[test]
    fn should_autosave_does_not_panic_on_day_going_backwards() {
        // Defensive: a day counter that somehow decreases (e.g. a fresh
        // `Init` after a reconnect) must not underflow/panic.
        assert!(!should_autosave(Some(50), 5, 10));
    }
}
