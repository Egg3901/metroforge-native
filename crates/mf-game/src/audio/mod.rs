//! Procedural audio layer for MetroForge.
//!
//! Zero committed sound assets: every effect is PCM synthesized once at
//! [`Startup`] (see [`synth`]), wrapped in a custom Bevy [`Decodable`]
//! source, and played through the normal `AudioPlayer` /
//! `PlaybackSettings::DESPAWN` path. City ambience is a quiet looping
//! band-passed noise bed that starts on city load and ducks while paused.
//!
//! Public surface for the rest of the game: fire [`PlaySfx`] with an
//! [`Sfx`] variant. Master volume + mute live on [`crate::config::MfConfig`]
//! and are applied at spawn (and continuously for the ambience sink).

mod synth;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bevy::audio::{AddAudioSource, AudioSink, AudioSinkPlayback, Source, Volume};
use bevy::prelude::*;
use cpal::traits::HostTrait;

use mf_net::SimEvent;
use mf_protocol::{FromSimJson, FromSimMsg};

use crate::config::MfConfig;
use crate::state::{AppState, PauseState};

pub use synth::SAMPLE_RATE;

/// Relative gain applied on top of the player's master volume for one-shot
/// SFX. Kept modest: these are UI ticks, not fanfares.
const SFX_GAIN: f32 = 0.55;

/// Base linear gain for the looping city ambience (before master + duck).
const AMBIENCE_GAIN: f32 = 1.0;

/// Ambience volume multiplier while the pause overlay is up.
const AMBIENCE_PAUSE_DUCK: f32 = 0.5;

/// Logs the missing-device warning at most once per process, even if probe
/// systems re-run.
static MISSING_DEVICE_LOGGED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------
// Sfx palette
// ---------------------------------------------------------------------

/// Every synthesized one-shot the game can play. `Hash`/`Eq` so it works as
/// a [`SfxBank`] key. Extend this enum rather than inventing a parallel
/// playback path — HUD/tools/goals already speak [`PlaySfx`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sfx {
    /// Short filtered tick (hover / light UI click).
    Hover,
    /// Confirm / primary click.
    Confirm,
    Cancel,
    Pause,
    Unpause,
    /// Low square error buzz.
    Error,
    /// Toast notification pop.
    Toast,
    SpeedTick,
    /// Station/build placement thunk (low sine + noise burst).
    Placement,
    /// Two-note goal-complete chime.
    GoalComplete,
}

const ALL_SFX: [Sfx; 10] = [
    Sfx::Hover,
    Sfx::Confirm,
    Sfx::Cancel,
    Sfx::Pause,
    Sfx::Unpause,
    Sfx::Error,
    Sfx::Toast,
    Sfx::SpeedTick,
    Sfx::Placement,
    Sfx::GoalComplete,
];

fn render_sfx(sfx: Sfx) -> Vec<f32> {
    match sfx {
        Sfx::Hover => synth::render_ui_click(),
        Sfx::Confirm => synth::render_confirm(),
        Sfx::Cancel => synth::render_cancel(),
        Sfx::Pause => synth::render_pause(),
        Sfx::Unpause => synth::render_unpause(),
        Sfx::Error => synth::render_error_buzz(),
        Sfx::Toast => synth::render_toast_pop(),
        Sfx::SpeedTick => synth::render_speed_tick(),
        Sfx::Placement => synth::render_placement_thunk(),
        Sfx::GoalComplete => synth::render_goal_chime(),
    }
}

// ---------------------------------------------------------------------
// Bevy Decodable source over a precomputed PCM buffer
// ---------------------------------------------------------------------

/// A rendered PCM buffer registered as a custom Bevy audio asset. `Arc<[f32]>`
/// so cloning a decoder is a refcount bump, not a buffer copy.
#[derive(Asset, TypePath, Clone)]
pub struct PcmSfx {
    samples: Arc<[f32]>,
}

impl PcmSfx {
    pub fn new(samples: Vec<f32>) -> Self {
        PcmSfx {
            samples: Arc::from(samples),
        }
    }
}

/// Iterator/`Source` over a [`PcmSfx`] buffer.
pub struct PcmSfxDecoder {
    samples: Arc<[f32]>,
    pos: usize,
}

impl Iterator for PcmSfxDecoder {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = self.samples.get(self.pos).copied();
        if sample.is_some() {
            self.pos += 1;
        }
        sample
    }
}

impl Source for PcmSfxDecoder {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.samples.len().saturating_sub(self.pos))
    }

    fn channels(&self) -> u16 {
        1
    }

    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f32(
            self.samples.len() as f32 / SAMPLE_RATE as f32,
        ))
    }
}

impl bevy::audio::Decodable for PcmSfx {
    type DecoderItem = f32;
    type Decoder = PcmSfxDecoder;

    fn decoder(&self) -> Self::Decoder {
        PcmSfxDecoder {
            samples: self.samples.clone(),
            pos: 0,
        }
    }
}

/// Handles into the `Assets<PcmSfx>` bank, one per [`Sfx`] variant, built
/// once at `Startup`.
#[derive(Resource, Default)]
pub struct SfxBank(pub HashMap<Sfx, Handle<PcmSfx>>);

/// Handle for the looping city ambience buffer (separate from the SFX bank
/// so it can be spawned with `PlaybackSettings::LOOP`).
#[derive(Resource, Clone)]
pub struct AmbienceHandle(pub Handle<PcmSfx>);

/// Marker on the ambience player entity so pause-duck / teardown can find it.
#[derive(Component)]
pub struct CityAmbience;

/// Whether a usable default output device was present at probe time.
/// When `false`, play systems no-op (Bevy itself also skips sinks).
#[derive(Resource, Debug, Clone, Copy)]
pub struct AudioDevicePresent(pub bool);

/// Public event: fire `PlaySfx(Sfx::Confirm)` etc. from anywhere with
/// `EventWriter<PlaySfx>` to play a sound. Playback despawns its entity when
/// done (`PlaybackSettings::DESPAWN`).
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlaySfx(pub Sfx);

/// Master volume in `[0, 1]` after applying mute. Used at SFX spawn and for
/// continuous ambience sink updates (Bevy's `GlobalVolume` does not affect
/// already-playing audio).
pub fn effective_master_volume(config: &MfConfig) -> f32 {
    if config.mute {
        0.0
    } else {
        config.master_volume.clamp(0.0, 1.0)
    }
}

/// Probe for a default output device without opening a stream. Logs once
/// when absent so headless CI/servers stay quiet after the first warning.
fn probe_audio_device() -> bool {
    let present = cpal::default_host().default_output_device().is_some();
    if !present
        && MISSING_DEVICE_LOGGED
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        tracing::warn!(
            "mf-game audio: no output device found; SFX and ambience disabled (ok for headless CI)"
        );
    }
    present
}

fn build_audio_bank_system(mut assets: ResMut<Assets<PcmSfx>>, mut commands: Commands) {
    let present = probe_audio_device();
    commands.insert_resource(AudioDevicePresent(present));

    let mut map = HashMap::with_capacity(ALL_SFX.len());
    for sfx in ALL_SFX {
        let samples = render_sfx(sfx);
        map.insert(sfx, assets.add(PcmSfx::new(samples)));
    }
    commands.insert_resource(SfxBank(map));

    let ambience = assets.add(PcmSfx::new(synth::render_city_ambience()));
    commands.insert_resource(AmbienceHandle(ambience));
}

fn play_sfx_system(
    mut events: EventReader<PlaySfx>,
    bank: Option<Res<SfxBank>>,
    device: Option<Res<AudioDevicePresent>>,
    config: Option<Res<MfConfig>>,
    mut commands: Commands,
) {
    if device.is_some_and(|d| !d.0) {
        // Drain so events don't pile up forever on headless hosts.
        events.clear();
        return;
    }
    let Some(bank) = bank else {
        return;
    };
    let master = config
        .as_deref()
        .map(effective_master_volume)
        .unwrap_or(0.5);
    if master <= 0.0 {
        events.clear();
        return;
    }
    for PlaySfx(sfx) in events.read() {
        if let Some(handle) = bank.0.get(sfx) {
            commands.spawn((
                AudioPlayer(handle.clone()),
                PlaybackSettings::DESPAWN.with_volume(Volume::Linear(master * SFX_GAIN)),
            ));
        }
    }
}

/// Every sim `toast` message plays the toast pop.
fn toast_sfx_system(mut sim_events: EventReader<SimEvent>, mut sfx_events: EventWriter<PlaySfx>) {
    for SimEvent(msg) in sim_events.read() {
        if let FromSimMsg::Json(FromSimJson::Toast(_)) = msg {
            sfx_events.write(PlaySfx(Sfx::Toast));
        }
    }
}

fn start_ambience_system(
    mut commands: Commands,
    ambience: Option<Res<AmbienceHandle>>,
    device: Option<Res<AudioDevicePresent>>,
    config: Option<Res<MfConfig>>,
    existing: Query<Entity, With<CityAmbience>>,
) {
    // Ambience bed is opt-in (config `ambience_enabled`, default off): the
    // looping noise bed read as harsh. One-shot SFX are unaffected. A missing
    // config resolves to off, matching the default.
    if !config.as_deref().is_some_and(|c| c.ambience_enabled) {
        return;
    }
    if device.is_some_and(|d| !d.0) {
        return;
    }
    let Some(ambience) = ambience else {
        return;
    };
    // Idempotent: don't stack loops if InGame is re-entered oddly.
    if !existing.is_empty() {
        return;
    }
    let master = config
        .as_deref()
        .map(effective_master_volume)
        .unwrap_or(0.5);
    commands.spawn((
        AudioPlayer(ambience.0.clone()),
        PlaybackSettings::LOOP.with_volume(Volume::Linear(master * AMBIENCE_GAIN)),
        CityAmbience,
    ));
}

fn stop_ambience_system(mut commands: Commands, existing: Query<Entity, With<CityAmbience>>) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }
}

/// Keep the ambience sink's volume in sync with master/mute and pause duck.
/// Bevy's `GlobalVolume` does not update already-playing audio, so this is
/// the authoritative path for the looping bed.
fn sync_ambience_volume_system(
    pause: Res<PauseState>,
    config: Res<MfConfig>,
    mut sinks: Query<&mut AudioSink, With<CityAmbience>>,
) {
    let master = effective_master_volume(&config);
    let duck = if pause.active {
        AMBIENCE_PAUSE_DUCK
    } else {
        1.0
    };
    let vol = master * AMBIENCE_GAIN * duck;
    for mut sink in &mut sinks {
        sink.set_volume(Volume::Linear(vol));
    }
}

/// Mirror config master/mute into `GlobalVolume` so newly spawned one-shots
/// pick up the latest setting even before our explicit spawn volume runs.
/// `MfConfig` is inserted on Boot (after Startup), so this system simply
/// does not run until then — Bevy skips systems whose `Res` params are missing.
fn sync_global_volume_system(config: Res<MfConfig>, mut global: ResMut<bevy::audio::GlobalVolume>) {
    if config.is_changed() {
        global.volume = Volume::Linear(effective_master_volume(&config));
    }
}

pub struct MfAudioPlugin;

impl Plugin for MfAudioPlugin {
    fn build(&self, app: &mut App) {
        app.add_audio_source::<PcmSfx>()
            .add_event::<PlaySfx>()
            .add_systems(Startup, build_audio_bank_system)
            .add_systems(
                Update,
                (
                    play_sfx_system,
                    toast_sfx_system,
                    sync_ambience_volume_system,
                    sync_global_volume_system,
                ),
            )
            .add_systems(OnEnter(AppState::InGame), start_ambience_system)
            .add_systems(OnExit(AppState::InGame), stop_ambience_system);
    }
}
