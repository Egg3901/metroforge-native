//! Disk save slots: 3 numbered slots plus a ring of 3 autosaves, each a
//! JSON file under `paths::saves_dir()` (OS data dir via `ProjectDirs`,
//! with an exe-adjacent fallback — see `paths.rs`).
//!
//! # Versioned wrapper format
//!
//! Every on-disk document is a thin client wrapper around the opaque sim
//! blob the TS sidecar owns (`serialize`/`deserialize` in
//! `metroforge/src/core/save.ts`). The wrapper carries an explicit
//! `schema_version` so any save written by v0.4.x loads in every future
//! client via the migration registry below. The sim blob itself is never
//! parsed here — this client only round-trips it.
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "meta": {
//!     "city_label": "nyc",
//!     "day": 42,
//!     "cash": 1234567.5,
//!     "saved_at_epoch_secs": 1700000000,
//!     "network_size": 12,
//!     "playtime_secs": 3600,
//!     "thumbnail_png_base64": null
//!   },
//!   "sim": { /* opaque */ }
//! }
//! ```
//!
//! # Atomic writes + recovery
//!
//! Writes go to `<slot>.json.tmp`, then rename over the live file after
//! rotating the previous live file to `<slot>.json.bak`. A crash mid-write
//! therefore never leaves a half-written live slot. Loads try the live
//! file first, then the newest valid backup (`.bak`, then a leftover
//! `.tmp` if it parses).
//!
//! # CRITICAL load-path ordering
//!
//! `SaveManager::load` only stages sim JSON in `pending_load`;
//! [`send_pending_load_system`] puts `ToSim::LoadSave` on the wire after
//! `OnEnter(Loading)` so a fresh `Init` cannot clobber the restore. See
//! the original module doc in git history for the full schedule analysis;
//! `state.rs` also skips `Init` when [`SaveManager::has_pending_load`].

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use mf_net::{SimEvent, SimLink};
use mf_protocol::envelope::{FromSimJson, LoadSavePayload};
use mf_protocol::{FromSimMsg, ToSim, ToastTone, UiState};
use mf_state::LatestUi;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::audio::{PlaySfx, Sfx};
use crate::config::MfConfig;
use crate::hud::ToastLog;
use crate::state::{AppState, PendingInit};

/// Number of player-addressable numbered slots (in addition to the
/// autosave ring). Player-facing numbering is 1-based.
pub const SLOT_COUNT: u8 = 3;

/// Autosave ring depth: the three most recent autosaves are kept.
pub const AUTOSAVE_RING_SIZE: u8 = 3;

/// Default autosave cadence in sim-days when config has no override.
pub const DEFAULT_AUTOSAVE_INTERVAL_DAYS: u32 = 10;

/// Current on-disk wrapper schema. Bump + add a migration when the
/// wrapper shape changes. Sim-blob versioning stays in the TS sidecar.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// One save destination: a numbered slot (1..=[`SLOT_COUNT`]) or one
/// entry in the autosave ring (0..[`AUTOSAVE_RING_SIZE`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SaveSlot {
    Slot(u8),
    Autosave(u8),
}

impl SaveSlot {
    /// Filename stem (sans `.json`) this slot is stored under.
    fn file_stem(self) -> String {
        match self {
            SaveSlot::Slot(n) => format!("slot{n}"),
            SaveSlot::Autosave(n) => format!("autosave{n}"),
        }
    }

    /// Human-readable label for menus.
    pub fn label(self) -> String {
        match self {
            SaveSlot::Autosave(n) => format!("Autosave {}", n + 1),
            SaveSlot::Slot(n) => format!("Slot {n}"),
        }
    }
}

/// Metadata captured alongside the opaque sim state for the save browser
/// — never fed back into the sim itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveMeta {
    /// City preset key active at save time (`PendingInit::preset_key`).
    pub city_label: Option<String>,
    pub day: u32,
    pub cash: f64,
    pub saved_at_epoch_secs: u64,
    /// Stations + tracks at save time — a cheap "network size" proxy.
    #[serde(default)]
    pub network_size: u32,
    /// Accumulated wall-clock playtime for this city, in seconds.
    #[serde(default)]
    pub playtime_secs: u64,
    /// Optional PNG thumbnail as base64. Capturing a real Bevy screenshot
    /// is not cheap, so this stays `None` unless a future path fills it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_png_base64: Option<String>,
}

impl SaveMeta {
    /// Snapshot display metadata from the live UI + playtime tracker.
    pub fn from_ui(city_label: Option<String>, ui: &UiState, playtime_secs: u64) -> Self {
        SaveMeta {
            city_label,
            day: ui.day,
            cash: ui.cash,
            saved_at_epoch_secs: epoch_secs(),
            network_size: (ui.stations.len() + ui.tracks.len()) as u32,
            playtime_secs,
            thumbnail_png_base64: None,
        }
    }
}

/// On-disk document shape after migration to [`CURRENT_SCHEMA_VERSION`].
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SaveWrapper {
    schema_version: u32,
    meta: SaveMeta,
    sim: Value,
}

/// One entry in [`list`]'s result: a slot plus its metadata, if occupied.
#[derive(Debug, Clone)]
pub struct SlotEntry {
    pub slot: SaveSlot,
    pub meta: Option<SaveMeta>,
}

/// Wall-clock playtime accumulator for the current InGame session.
/// Baseline is restored from a loaded save so continue keeps counting.
#[derive(Resource, Debug, Clone)]
pub struct PlaytimeTracker {
    pub secs: f64,
}

impl Default for PlaytimeTracker {
    fn default() -> Self {
        PlaytimeTracker { secs: 0.0 }
    }
}

impl PlaytimeTracker {
    pub fn whole_secs(&self) -> u64 {
        self.secs.max(0.0) as u64
    }
}

// ---------------------------------------------------------------------------
// Migration registry
// ---------------------------------------------------------------------------

type MigrationFn = fn(Value) -> anyhow::Result<Value>;

/// Ordered (from_version, migrate_to_from_plus_one) steps. Each entry
/// advances the document by exactly one schema version.
fn migration_registry() -> &'static [(u32, MigrationFn)] {
    &[(0, migrate_v0_to_v1)]
}

/// Legacy v0.4 wrapper: `{"meta": {...}, "sim": ...}` with no
/// `schema_version` and without the v1 meta fields.
fn migrate_v0_to_v1(mut doc: Value) -> anyhow::Result<Value> {
    let obj = doc
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("save document is not a JSON object"))?;
    if !obj.contains_key("meta") || !obj.contains_key("sim") {
        anyhow::bail!("legacy save missing meta/sim");
    }
    if let Some(meta) = obj.get_mut("meta").and_then(|m| m.as_object_mut()) {
        meta.entry("network_size".to_string()).or_insert(json!(0));
        meta.entry("playtime_secs".to_string()).or_insert(json!(0));
        // thumbnail stays absent → None via serde default
    }
    obj.insert("schema_version".to_string(), json!(1));
    Ok(doc)
}

/// Run the migration chain until [`CURRENT_SCHEMA_VERSION`]. Documents
/// already at the current version pass through unchanged. Unknown future
/// versions are rejected so we never silently drop fields.
pub fn migrate_to_current(mut doc: Value) -> anyhow::Result<Value> {
    let mut version = doc
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    if version > CURRENT_SCHEMA_VERSION {
        anyhow::bail!(
            "save schema version {version} is newer than this client ({CURRENT_SCHEMA_VERSION})"
        );
    }

    for &(from, migrate) in migration_registry() {
        if version == from {
            doc = migrate(doc)?;
            version = version.saturating_add(1);
            // Keep the stamped version honest even if a migration forgot.
            if let Some(obj) = doc.as_object_mut() {
                obj.insert("schema_version".to_string(), json!(version));
            }
        }
    }

    if version != CURRENT_SCHEMA_VERSION {
        anyhow::bail!(
            "no migration path from schema version {version} to {CURRENT_SCHEMA_VERSION}"
        );
    }
    Ok(doc)
}

// ---------------------------------------------------------------------------
// Paths + pure JSON helpers
// ---------------------------------------------------------------------------

fn saves_dir() -> Option<PathBuf> {
    crate::paths::saves_dir()
}

fn slot_path(slot: SaveSlot) -> Option<PathBuf> {
    saves_dir().map(|dir| dir.join(format!("{}.json", slot.file_stem())))
}

/// Legacy single-autosave path from pre-ring builds (`autosave.json`).
fn legacy_autosave_path() -> Option<PathBuf> {
    saves_dir().map(|dir| dir.join("autosave.json"))
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Parse `sim_json` and wrap it with `meta` into the on-disk document
/// string at [`CURRENT_SCHEMA_VERSION`]. Pure (no filesystem).
fn build_wrapper_json(meta: &SaveMeta, sim_json: &str) -> anyhow::Result<String> {
    let sim: Value = serde_json::from_str(sim_json)?;
    let wrapper = SaveWrapper {
        schema_version: CURRENT_SCHEMA_VERSION,
        meta: meta.clone(),
        sim,
    };
    Ok(serde_json::to_string(&wrapper)?)
}

/// Inverse of [`build_wrapper_json`]: parse + migrate a wrapper document
/// back into metadata plus re-serialized opaque sim JSON.
fn parse_wrapper_json(contents: &str) -> anyhow::Result<(SaveMeta, String)> {
    let raw: Value = serde_json::from_str(contents)?;
    let migrated = migrate_to_current(raw)?;
    let wrapper: SaveWrapper = serde_json::from_value(migrated)?;
    if wrapper.schema_version != CURRENT_SCHEMA_VERSION {
        anyhow::bail!(
            "internal error: migrated schema_version {} != current",
            wrapper.schema_version
        );
    }
    let sim_json = serde_json::to_string(&wrapper.sim)?;
    Ok((wrapper.meta, sim_json))
}

/// Pure autosave-cadence check.
fn should_autosave(last_autosave_day: Option<u32>, current_day: u32, interval_days: u32) -> bool {
    if interval_days == 0 {
        return false;
    }
    let baseline = last_autosave_day.unwrap_or(0);
    current_day.saturating_sub(baseline) >= interval_days
}

// ---------------------------------------------------------------------------
// Atomic I/O + recovery
// ---------------------------------------------------------------------------

fn tmp_path(path: &Path) -> PathBuf {
    path.with_extension("json.tmp")
}

fn bak_path(path: &Path) -> PathBuf {
    path.with_extension("json.bak")
}

/// Write `contents` atomically: temp file → rotate live to `.bak` → rename
/// temp over live. A crash mid-write leaves either the previous live file
/// or a complete temp/bak that [`read_with_recovery`] can pick up.
fn atomic_write(path: &Path, contents: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path(path);
    let bak = bak_path(path);

    std::fs::write(&tmp, contents)
        .map_err(|e| anyhow::anyhow!("writing {}: {e}", tmp.display()))?;

    if path.exists() {
        // Best-effort rotate; a failure here still lets the rename below
        // replace the live file — we just lose the previous backup.
        let _ = std::fs::rename(path, &bak);
    }

    std::fs::rename(&tmp, path).map_err(|e| {
        // If rename failed after rotating live→bak, try to put bak back so
        // the player keeps *something* loadable.
        if bak.exists() && !path.exists() {
            let _ = std::fs::rename(&bak, path);
        }
        anyhow::anyhow!("renaming {} -> {}: {e}", tmp.display(), path.display())
    })?;
    Ok(())
}

/// Read the live file when it parses; otherwise fall back to the newest
/// valid `.bak` / leftover `.tmp`. When a backup is the only valid copy,
/// restore it onto the live path.
fn read_with_recovery(path: &Path) -> anyhow::Result<String> {
    if let Ok(contents) = std::fs::read_to_string(path) {
        if parse_wrapper_json(&contents).is_ok() {
            return Ok(contents);
        }
    }

    let backups = [bak_path(path), tmp_path(path)];
    let mut newest_valid: Option<(std::time::SystemTime, PathBuf, String)> = None;

    for candidate in &backups {
        let Ok(meta) = std::fs::metadata(candidate) else {
            continue;
        };
        let modified = meta.modified().unwrap_or(UNIX_EPOCH);
        let Ok(contents) = std::fs::read_to_string(candidate) else {
            continue;
        };
        if parse_wrapper_json(&contents).is_err() {
            continue;
        }
        let replace = match &newest_valid {
            None => true,
            Some((prev_mod, _, _)) => modified >= *prev_mod,
        };
        if replace {
            newest_valid = Some((modified, candidate.clone(), contents));
        }
    }

    let Some((_, source, contents)) = newest_valid else {
        anyhow::bail!("no valid save at {} (or .bak/.tmp)", path.display());
    };

    tracing::warn!(
        "mf-game: recovering save from {} onto {}",
        source.display(),
        path.display()
    );
    let _ = atomic_write(path, &contents);
    Ok(contents)
}

fn write_slot_at(path: &Path, meta: &SaveMeta, sim_json: &str) -> anyhow::Result<()> {
    let wrapper_json = build_wrapper_json(meta, sim_json)?;
    atomic_write(path, &wrapper_json)
}

fn try_load_at(path: &Path) -> anyhow::Result<(SaveMeta, String)> {
    let contents = read_with_recovery(path)?;
    parse_wrapper_json(&contents)
}

fn write_slot(slot: SaveSlot, meta: &SaveMeta, sim_json: &str) -> anyhow::Result<()> {
    let path = slot_path(slot)
        .ok_or_else(|| anyhow::anyhow!("no data directory available on this platform"))?;
    write_slot_at(&path, meta, sim_json)
}

// ---------------------------------------------------------------------------
// SaveManager
// ---------------------------------------------------------------------------

struct PendingSave {
    slot: SaveSlot,
    meta: SaveMeta,
}

/// Owns in-flight save/load bookkeeping.
#[derive(Resource, Default)]
pub struct SaveManager {
    pending_save: Option<PendingSave>,
    pending_load: Option<String>,
    /// City key staged alongside `pending_load` so `PendingInit` can be
    /// corrected before goals/campaign key off the wrong preset.
    pending_load_city: Option<String>,
    /// Playtime from the slot being loaded — applied once Loading starts.
    pending_load_playtime_secs: Option<u64>,
    last_autosave_day: Option<u32>,
    /// Next ring index to write (0..[`AUTOSAVE_RING_SIZE`]).
    autosave_ring_index: u8,
}

impl SaveManager {
    pub fn has_pending_load(&self) -> bool {
        self.pending_load.is_some()
    }

    /// Start a save into `slot`: sends `ToSim::RequestSave` and remembers
    /// the metadata snapshotted at click time.
    pub fn request_save(
        &mut self,
        slot: SaveSlot,
        meta: SaveMeta,
        link: &SimLink,
        toasts: &mut ToastLog,
        sfx: &mut EventWriter<PlaySfx>,
    ) {
        match link.transport.send(ToSim::RequestSave) {
            Ok(()) => {
                self.pending_save = Some(PendingSave { slot, meta });
            }
            Err(e) => {
                tracing::warn!("mf-game: failed to send requestSave: {e}");
                toasts.push(format!("Save failed: {e}"), ToastTone::Warn);
                sfx.write(PlaySfx(Sfx::Error));
            }
        }
    }

    /// Read `slot` from disk and queue its sim JSON for
    /// [`send_pending_load_system`].
    pub fn load(
        &mut self,
        slot: SaveSlot,
        toasts: &mut ToastLog,
        sfx: &mut EventWriter<PlaySfx>,
    ) -> Option<SaveMeta> {
        match Self::try_load(slot) {
            Ok((meta, sim_json)) => {
                self.pending_load_city = meta.city_label.clone();
                self.pending_load_playtime_secs = Some(meta.playtime_secs);
                self.pending_load = Some(sim_json);
                sfx.write(PlaySfx(Sfx::Confirm));
                Some(meta)
            }
            Err(e) => {
                tracing::warn!("mf-game: failed to load save slot: {e}");
                toasts.push(format!("Load failed: {e}"), ToastTone::Warn);
                sfx.write(PlaySfx(Sfx::Error));
                None
            }
        }
    }

    fn try_load(slot: SaveSlot) -> anyhow::Result<(SaveMeta, String)> {
        let path = slot_path(slot)
            .ok_or_else(|| anyhow::anyhow!("no data directory available on this platform"))?;
        match try_load_at(&path) {
            Ok(pair) => Ok(pair),
            Err(primary_err) => {
                // Pre-ring builds wrote a single `autosave.json`. Treat it
                // as ring index 0 so old installs keep their continue slot.
                if matches!(slot, SaveSlot::Autosave(0)) {
                    if let Some(legacy) = legacy_autosave_path() {
                        if let Ok(pair) = try_load_at(&legacy) {
                            return Ok(pair);
                        }
                    }
                }
                Err(primary_err)
            }
        }
    }

    fn next_autosave_slot(&mut self) -> SaveSlot {
        let idx = self.autosave_ring_index % AUTOSAVE_RING_SIZE;
        self.autosave_ring_index = (idx + 1) % AUTOSAVE_RING_SIZE;
        SaveSlot::Autosave(idx)
    }
}

/// List every slot (autosave ring first, then numbered slots) with
/// metadata if occupied. Corrupt/unreadable slots read as empty.
pub fn list() -> Vec<SlotEntry> {
    let mut out = Vec::with_capacity(SLOT_COUNT as usize + AUTOSAVE_RING_SIZE as usize);
    for n in 0..AUTOSAVE_RING_SIZE {
        out.push(SlotEntry {
            slot: SaveSlot::Autosave(n),
            meta: read_meta(SaveSlot::Autosave(n)),
        });
    }
    // Surface a legacy `autosave.json` under ring 0 when the new path is empty.
    if out[0].meta.is_none() {
        if let Some(legacy) = legacy_autosave_path() {
            if let Ok((meta, _)) = try_load_at(&legacy) {
                out[0].meta = Some(meta);
            }
        }
    }
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
    try_load_at(&path).ok().map(|(meta, _)| meta)
}

fn slot_saved_message(slot: SaveSlot) -> String {
    match slot {
        SaveSlot::Autosave(_) => "Autosaved".to_string(),
        SaveSlot::Slot(n) => format!("Saved to slot {n}"),
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

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
            continue;
        };
        match write_slot(pending.slot, &pending.meta, &payload.json) {
            Ok(()) => {
                toasts.push(slot_saved_message(pending.slot), ToastTone::Good);
                sfx.write(PlaySfx(Sfx::Confirm));
            }
            Err(e) => {
                tracing::warn!("mf-game: failed to write save slot: {e}");
                toasts.push(format!("Save failed: {e}"), ToastTone::Warn);
                sfx.write(PlaySfx(Sfx::Error));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn autosave_system(
    mut manager: ResMut<SaveManager>,
    ui: Res<LatestUi>,
    pending_init: Res<PendingInit>,
    playtime: Res<PlaytimeTracker>,
    config: Res<MfConfig>,
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
    if last_seen_day.is_some_and(|prev| state.day < prev) {
        manager.last_autosave_day = None;
    }
    *last_seen_day = Some(state.day);

    let interval = config.autosave_interval_days;
    if !should_autosave(manager.last_autosave_day, state.day, interval) {
        return;
    }
    let Some(link) = link else {
        return;
    };
    let slot = manager.next_autosave_slot();
    let meta = SaveMeta::from_ui(
        Some(pending_init.preset_key.clone()),
        state,
        playtime.whole_secs(),
    );
    manager.request_save(slot, meta, &link, &mut toasts, &mut sfx);
    manager.last_autosave_day = Some(state.day);
}

fn send_pending_load_system(mut manager: ResMut<SaveManager>, link: Option<Res<SimLink>>) {
    if manager.pending_load.is_none() {
        return;
    }
    let Some(link) = link else {
        return;
    };
    if let Some(sim_json) = manager.pending_load.take() {
        let _ = link
            .transport
            .send(ToSim::LoadSave(LoadSavePayload { json: sim_json }));
    }
}

/// Accumulate wall-clock playtime while InGame (and not paused — pause
/// freezes the sim clock; wall time during the pause overlay still counts
/// as "session time" the way most games do).
fn tick_playtime_system(mut playtime: ResMut<PlaytimeTracker>, time: Res<Time>) {
    playtime.secs += f64::from(time.delta_secs());
}

/// When a load is staged, mirror city + playtime into `PendingInit` /
/// `PlaytimeTracker` so goals/campaign/subsequent saves key off the
/// restored city and the playtime counter continues from the save.
fn apply_pending_load_meta_system(
    mut manager: ResMut<SaveManager>,
    mut pending: ResMut<PendingInit>,
    mut playtime: ResMut<PlaytimeTracker>,
    mut applied: Local<bool>,
) {
    let has_staged =
        manager.pending_load_city.is_some() || manager.pending_load_playtime_secs.is_some();
    if !has_staged {
        *applied = false;
        return;
    }
    if *applied {
        return;
    }
    *applied = true;
    if let Some(city) = manager.pending_load_city.take() {
        pending.preset_key = city;
    }
    if let Some(secs) = manager.pending_load_playtime_secs.take() {
        playtime.secs = secs as f64;
    }
}

/// Reset playtime when starting a fresh city (not a continue).
pub fn reset_playtime(playtime: &mut PlaytimeTracker) {
    playtime.secs = 0.0;
}

pub struct MfSavesPlugin;

impl Plugin for MfSavesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SaveManager>()
            .init_resource::<PlaytimeTracker>()
            .add_systems(
                Update,
                (
                    capture_saved_system,
                    autosave_system.run_if(in_state(AppState::InGame)),
                    tick_playtime_system.run_if(in_state(AppState::InGame)),
                    send_pending_load_system.run_if(in_state(AppState::Loading)),
                    apply_pending_load_meta_system.run_if(in_state(AppState::Loading)),
                ),
            );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // --- slot path mapping -------------------------------------------------

    #[test]
    fn file_stem_numbered_slots_are_one_indexed() {
        assert_eq!(SaveSlot::Slot(1).file_stem(), "slot1");
        assert_eq!(SaveSlot::Slot(2).file_stem(), "slot2");
        assert_eq!(SaveSlot::Slot(3).file_stem(), "slot3");
    }

    #[test]
    fn file_stem_autosave_ring_is_zero_indexed() {
        assert_eq!(SaveSlot::Autosave(0).file_stem(), "autosave0");
        assert_eq!(SaveSlot::Autosave(1).file_stem(), "autosave1");
        assert_eq!(SaveSlot::Autosave(2).file_stem(), "autosave2");
    }

    #[test]
    fn autosave_ring_stems_are_distinct_from_numbered_slots() {
        for a in 0..AUTOSAVE_RING_SIZE {
            for n in 1..=SLOT_COUNT {
                assert_ne!(
                    SaveSlot::Autosave(a).file_stem(),
                    SaveSlot::Slot(n).file_stem()
                );
            }
        }
    }

    // --- wrapper round-trip --------------------------------------------------

    fn sample_meta() -> SaveMeta {
        SaveMeta {
            city_label: Some("nyc".to_string()),
            day: 42,
            cash: 1_234_567.5,
            saved_at_epoch_secs: 1_700_000_000,
            network_size: 12,
            playtime_secs: 3_600,
            thumbnail_png_base64: None,
        }
    }

    #[test]
    fn wrapper_round_trips_meta_and_sim_json() {
        let sim_json = r#"{"tick":100,"budget":{"cash":1234.5},"stations":[]}"#;
        let wrapped = build_wrapper_json(&sample_meta(), sim_json).expect("build wrapper");
        let (meta, sim_back) = parse_wrapper_json(&wrapped).expect("parse wrapper");
        assert_eq!(meta, sample_meta());

        let original: Value = serde_json::from_str(sim_json).unwrap();
        let round_tripped: Value = serde_json::from_str(&sim_back).unwrap();
        assert_eq!(original, round_tripped);

        let doc: Value = serde_json::from_str(&wrapped).unwrap();
        assert_eq!(
            doc.get("schema_version").and_then(|v| v.as_u64()),
            Some(CURRENT_SCHEMA_VERSION as u64)
        );
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
        assert!(parse_wrapper_json(r#"{"meta": {}}"#).is_err());
    }

    #[test]
    fn build_wrapper_rejects_non_json_sim_string() {
        assert!(build_wrapper_json(&sample_meta(), "not json").is_err());
    }

    #[test]
    fn migrate_rejects_future_schema_version() {
        let future = json!({
            "schema_version": CURRENT_SCHEMA_VERSION + 99,
            "meta": sample_meta(),
            "sim": {}
        });
        assert!(migrate_to_current(future).is_err());
    }

    // --- schema fixture matrix -----------------------------------------------

    /// Canonical opaque sim blob used in fixtures (not a real sidecar
    /// save — just enough JSON to prove the client round-trips `sim`).
    const FIXTURE_SIM: &str = r#"{"version":2,"bankruptDays":0,"state":{"tick":100}}"#;

    fn fixture_v0_legacy() -> String {
        // Pre-versioning v0.4.x shape: no schema_version, no v1 meta fields.
        json!({
            "meta": {
                "city_label": "nyc",
                "day": 10,
                "cash": 500_000.0,
                "saved_at_epoch_secs": 1_700_000_000_u64
            },
            "sim": serde_json::from_str::<Value>(FIXTURE_SIM).unwrap()
        })
        .to_string()
    }

    fn fixture_v1_canonical() -> String {
        build_wrapper_json(
            &SaveMeta {
                city_label: Some("cleveland".to_string()),
                day: 25,
                cash: 750_000.0,
                saved_at_epoch_secs: 1_700_100_000,
                network_size: 8,
                playtime_secs: 1_200,
                thumbnail_png_base64: None,
            },
            FIXTURE_SIM,
        )
        .unwrap()
    }

    #[test]
    fn schema_fixture_matrix_loads_every_version() {
        let fixtures: &[(u32, String)] = &[(0, fixture_v0_legacy()), (1, fixture_v1_canonical())];
        assert_eq!(
            fixtures.len(),
            CURRENT_SCHEMA_VERSION as usize + 1,
            "fixture matrix must cover schema versions 0..=CURRENT"
        );

        for &(written_as, ref contents) in fixtures {
            let (meta, sim_back) =
                parse_wrapper_json(contents).unwrap_or_else(|e| panic!("v{written_as} load: {e}"));

            // Migrated document must be current-schema-shaped.
            let rewrapped = build_wrapper_json(&meta, &sim_back).expect("rewrapped");
            let doc: Value = serde_json::from_str(&rewrapped).unwrap();
            assert_eq!(
                doc["schema_version"].as_u64(),
                Some(CURRENT_SCHEMA_VERSION as u64),
                "v{written_as} did not migrate to current schema"
            );

            let original_sim: Value = serde_json::from_str(FIXTURE_SIM).unwrap();
            let round_tripped: Value = serde_json::from_str(&sim_back).unwrap();
            assert_eq!(
                original_sim, round_tripped,
                "v{written_as} sim blob must round-trip"
            );

            assert!(meta.day > 0, "v{written_as} should carry a sim day");
        }
    }

    #[test]
    fn v0_legacy_migration_fills_v1_defaults() {
        let (meta, _) = parse_wrapper_json(&fixture_v0_legacy()).unwrap();
        assert_eq!(meta.city_label.as_deref(), Some("nyc"));
        assert_eq!(meta.day, 10);
        assert_eq!(meta.network_size, 0);
        assert_eq!(meta.playtime_secs, 0);
        assert_eq!(meta.thumbnail_png_base64, None);
    }

    #[test]
    fn on_disk_fixture_files_match_matrix() {
        // Snapshotted copies under fixtures/saves/ — the round-trip matrix
        // also regenerates them in-memory; this asserts the checked-in
        // files stay loadable so CI catches accidental fixture drift.
        let v0 = include_str!("../fixtures/saves/v0_legacy.json");
        let v1 = include_str!("../fixtures/saves/v1_canonical.json");
        for (label, contents) in [("v0", v0), ("v1", v1)] {
            let (meta, sim) =
                parse_wrapper_json(contents).unwrap_or_else(|e| panic!("{label}: {e}"));
            assert!(meta.day > 0, "{label}");
            let _: Value = serde_json::from_str(&sim).unwrap();
        }
    }

    // --- atomic write + recovery ---------------------------------------------

    fn temp_slot_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mf-save-test-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("slot1.json")
    }

    #[test]
    fn atomic_write_leaves_valid_live_file() {
        let path = temp_slot_path("atomic_ok");
        let meta = sample_meta();
        write_slot_at(&path, &meta, FIXTURE_SIM).unwrap();
        assert!(path.exists());
        assert!(!tmp_path(&path).exists());
        let (loaded, _) = try_load_at(&path).unwrap();
        assert_eq!(loaded, meta);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn atomic_write_rotates_previous_to_bak() {
        let path = temp_slot_path("atomic_bak");
        let mut meta = sample_meta();
        write_slot_at(&path, &meta, FIXTURE_SIM).unwrap();
        meta.day = 99;
        write_slot_at(&path, &meta, FIXTURE_SIM).unwrap();

        let bak = bak_path(&path);
        assert!(bak.exists());
        let (bak_meta, _) = parse_wrapper_json(&fs::read_to_string(&bak).unwrap()).unwrap();
        assert_eq!(bak_meta.day, 42);
        let (live_meta, _) = try_load_at(&path).unwrap();
        assert_eq!(live_meta.day, 99);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn recovery_loads_bak_when_live_is_corrupt() {
        let path = temp_slot_path("recover_bak");
        write_slot_at(&path, &sample_meta(), FIXTURE_SIM).unwrap();
        // Second write creates .bak with the first meta, live with day=99.
        let mut newer = sample_meta();
        newer.day = 99;
        write_slot_at(&path, &newer, FIXTURE_SIM).unwrap();
        // Corrupt the live file as if a non-atomic writer crashed.
        fs::write(&path, "CORRUPT{{{").unwrap();

        // After second write: live=day99, bak=day42. Corrupt live → recover
        // newest valid among live(corrupt), bak(day42), tmp(none) → bak.
        let (meta, _) = try_load_at(&path).unwrap();
        assert_eq!(meta.day, 42);
        // Live path should be healed.
        let healed = fs::read_to_string(&path).unwrap();
        assert!(parse_wrapper_json(&healed).is_ok());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn recovery_prefers_newer_valid_tmp_over_stale_bak() {
        let path = temp_slot_path("recover_tmp");
        write_slot_at(&path, &sample_meta(), FIXTURE_SIM).unwrap();
        // Simulate crash after writing tmp but before rename: leave a valid
        // tmp that is newer than bak, and corrupt/missing live.
        let mut tmp_meta = sample_meta();
        tmp_meta.day = 77;
        let tmp_json = build_wrapper_json(&tmp_meta, FIXTURE_SIM).unwrap();
        fs::write(tmp_path(&path), &tmp_json).unwrap();
        fs::write(&path, "broken").unwrap();

        // Ensure tmp mtime is >= bak/live for the "newest valid" pick.
        let (meta, _) = try_load_at(&path).unwrap();
        assert_eq!(meta.day, 77);
        let _ = fs::remove_dir_all(path.parent().unwrap());
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
        assert!(!should_autosave(Some(50), 5, 10));
    }

    #[test]
    fn autosave_ring_advances_modulo_size() {
        let mut manager = SaveManager::default();
        assert_eq!(manager.next_autosave_slot(), SaveSlot::Autosave(0));
        assert_eq!(manager.next_autosave_slot(), SaveSlot::Autosave(1));
        assert_eq!(manager.next_autosave_slot(), SaveSlot::Autosave(2));
        assert_eq!(manager.next_autosave_slot(), SaveSlot::Autosave(0));
    }
}
