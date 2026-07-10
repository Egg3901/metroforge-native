//! Procedural chiptune SFX (spec: Mirror's Edge minimalism extends to sound
//! design - dry, short, clean chip blips, no reverb, no noise-wash tails).
//!
//! The crate ships zero sound assets. Every effect is a handful of numbers
//! (`ChipSpec`) rendered to a raw sample buffer once at `Startup`, wrapped in
//! a Bevy `Decodable` asset, and played through the normal `AudioPlayer` /
//! `PlaybackSettings::DESPAWN` path so nothing leaks entities over a long
//! session. This mirrors Bevy's own "decodable" example (a synthesized sine
//! wave registered as a custom audio source) - see
//! `bevy-0.16.1/examples/audio/decodable.rs` in the vendored registry source
//! that this module was checked against, except our decoder replays a
//! precomputed buffer instead of synthesizing per-sample at playback time
//! (the synthesis cost is paid once, at boot, not on every frame audio is
//! read).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bevy::audio::{AddAudioSource, Source, Volume};
use bevy::prelude::*;

use mf_net::SimEvent;
use mf_protocol::{FromSimJson, FromSimMsg};

/// Bevy's audio backend expects a fixed sample rate per source; 44.1 kHz is
/// the standard "just render at this and don't think about it" rate.
pub const SAMPLE_RATE: u32 = 44_100;

/// Single place to tune how loud SFX are relative to whatever `Volume` a
/// future settings menu applies on top. Kept low: these are UI ticks, not
/// fanfares, per the sonic palette brief.
pub const MASTER_VOLUME: f32 = 0.5;

// ---------------------------------------------------------------------
// Synth core (pure functions, unit-tested below without touching Bevy).
// ---------------------------------------------------------------------

/// Oscillator shape. `Noise` is a deterministic LFSR (no OS randomness, so
/// renders - and the tests that assert on them - are reproducible byte for
/// byte). None of the current `Sfx` specs reach for it (`Error` uses a low
/// square buzz instead, to keep the "no noise-wash tails" brief unambiguous)
/// but it is part of the synth core's public surface for a future effect
/// that wants it, and is exercised directly by `noise_render_is_deterministic`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipWave {
    Square,
    Triangle,
    #[allow(dead_code)]
    Noise,
}

/// A single procedural note: one oscillator, a linear pitch sweep, and a
/// standard ADSR envelope. Every `Sfx` variant is exactly one of these -
/// deliberately no layering/reverb/tails, per the "stark minimalism" brief.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChipSpec {
    pub wave: ChipWave,
    /// Pitch sweep start, in Hz. Equal to `end_hz` for a flat tone.
    pub start_hz: f32,
    /// Pitch sweep end, in Hz. Interpolated linearly across `duration_s`.
    pub end_hz: f32,
    pub duration_s: f32,
    pub attack_s: f32,
    pub decay_s: f32,
    /// Level the envelope decays to after `decay_s`, held until release.
    /// In `[0, 1]`.
    pub sustain_level: f32,
    pub release_s: f32,
    /// Linear gain applied after the envelope. Keep small (0.08-0.2): these
    /// are quiet-by-default UI ticks.
    pub gain: f32,
}

/// Soft clip so a spec with an over-aggressive gain can never produce a
/// sample outside `[-1, 1]` (and thus never an audible click at the DAC).
/// `tanh` is ~identity near zero and only bends near the rails, so it is
/// inaudible at the gains this module actually uses (peak envelope * gain
/// is well under 0.3 for every `Sfx` below) and is here purely as a safety
/// net rather than as a deliberate "drive" effect.
fn soft_clip(x: f32) -> f32 {
    x.tanh()
}

/// Render a `ChipSpec` to mono `f32` samples at `SAMPLE_RATE`, in `[-1, 1]`.
///
/// One `Vec` is allocated up front and filled in place - no per-sample
/// reallocation, no intermediate buffers.
pub fn render(spec: &ChipSpec) -> Vec<f32> {
    let sample_rate = SAMPLE_RATE as f32;
    let n = ((spec.duration_s * sample_rate).round().max(1.0)) as usize;
    let mut out = Vec::with_capacity(n);

    // Clamp attack+decay+release to fit inside duration_s: short UI blips
    // (tens of milliseconds) don't have room for the "suggested" envelope
    // shape verbatim, so scale all three phases down proportionally rather
    // than silently overrunning into (or past) the release tail.
    let total_adsr = spec.attack_s + spec.decay_s + spec.release_s;
    let scale = if total_adsr > spec.duration_s && total_adsr > 0.0 {
        spec.duration_s / total_adsr
    } else {
        1.0
    };
    let attack_s = spec.attack_s * scale;
    let decay_s = spec.decay_s * scale;
    let release_s = spec.release_s * scale;
    let decay_end = attack_s + decay_s;
    let sustain_s = (spec.duration_s - attack_s - decay_s - release_s).max(0.0);
    let sustain_end = decay_end + sustain_s;

    // Phase accumulator in cycles (0..1), not radians: lets Square/Triangle
    // be simple piecewise functions of phase instead of calling sin/cos
    // per sample.
    let mut phase = 0.0_f32;
    // Fixed nonzero seed (0xACE1, the classic NES-noise-channel seed) so the
    // LFSR is fully deterministic: same spec in, same samples out, every
    // time, with no OS RNG involved anywhere in this module.
    let mut lfsr: u16 = 0xACE1;

    for i in 0..n {
        let t = i as f32 / sample_rate;

        // ADSR envelope value at time t.
        let env = if t < attack_s {
            if attack_s > 0.0 {
                t / attack_s
            } else {
                1.0
            }
        } else if t < decay_end {
            if decay_s > 0.0 {
                let f = (t - attack_s) / decay_s;
                1.0 + (spec.sustain_level - 1.0) * f
            } else {
                spec.sustain_level
            }
        } else if t < sustain_end {
            spec.sustain_level
        } else if release_s > 0.0 {
            let f = ((t - sustain_end) / release_s).min(1.0);
            spec.sustain_level * (1.0 - f)
        } else {
            0.0
        };

        // Linear pitch sweep across the note's duration.
        let sweep_frac = if spec.duration_s > 0.0 {
            (t / spec.duration_s).min(1.0)
        } else {
            0.0
        };
        let freq = spec.start_hz + (spec.end_hz - spec.start_hz) * sweep_frac;

        let raw = match spec.wave {
            ChipWave::Square => {
                phase = (phase + freq / sample_rate).fract();
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            ChipWave::Triangle => {
                phase = (phase + freq / sample_rate).fract();
                4.0 * (phase - 0.5).abs() - 1.0
            }
            ChipWave::Noise => {
                // Fibonacci LFSR, taps at bits 0 and 2: a cheap, deterministic
                // pseudo-noise generator with no dependency on OS randomness.
                let bit = (lfsr ^ (lfsr >> 2)) & 1;
                lfsr = (lfsr >> 1) | (bit << 15);
                (lfsr as f32 / 32_767.5) - 1.0
            }
        };

        out.push(soft_clip(raw * env * spec.gain));
    }

    out
}

// ---------------------------------------------------------------------
// Sfx palette: one ChipSpec per UI sound.
// ---------------------------------------------------------------------

/// Every synthesized UI sound the game can play. `Hash`/`Eq` so it works as
/// a `SfxBank` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sfx {
    Hover,
    Confirm,
    Cancel,
    Pause,
    Unpause,
    Error,
    Toast,
    SpeedTick,
}

// Note names below are just documentation for the frequencies chosen (equal
// temperament, A4 = 440 Hz); nothing in this module depends on music theory,
// they're picked to sit in a clean "UI chip" register, not to be musically
// correct in any deeper sense.
const A4: f32 = 440.0;
const E4: f32 = 329.63;
const E5: f32 = 659.25;
const A5: f32 = 880.00;

/// The full `Sfx -> ChipSpec` table. Every duration/gain here is a
/// deliberate design choice within the "quiet, dry, no tails" brief; see the
/// per-variant comments for the reasoning.
fn spec_for(sfx: Sfx) -> ChipSpec {
    match sfx {
        // Hover: the most frequent sound in the game (fires on every menu
        // item the cursor crosses), so it has to be the cheapest possible
        // "did something happen" tick - flat pitch, very short, very quiet.
        Sfx::Hover => ChipSpec {
            wave: ChipWave::Square,
            start_hz: 2200.0,
            end_hz: 2200.0,
            duration_s: 0.045,
            attack_s: 0.002,
            decay_s: 0.010,
            sustain_level: 0.4,
            release_s: 0.020,
            gain: 0.10,
        },
        // Confirm: a fast upward sweep reads as "yes/forward" without
        // needing two discrete notes - E5 -> A5 is a rising fourth, bright
        // and short.
        Sfx::Confirm => ChipSpec {
            wave: ChipWave::Square,
            start_hz: E5,
            end_hz: A5,
            duration_s: 0.090,
            attack_s: 0.005,
            decay_s: 0.020,
            sustain_level: 0.5,
            release_s: 0.030,
            gain: 0.18,
        },
        // Cancel: the mirror image of Confirm - a falling fourth (A4 -> E4)
        // reads as "back/no" for free, purely by inverting the sweep
        // direction and dropping an octave for a slightly darker timbre.
        Sfx::Cancel => ChipSpec {
            wave: ChipWave::Square,
            start_hz: A4,
            end_hz: E4,
            duration_s: 0.090,
            attack_s: 0.005,
            decay_s: 0.020,
            sustain_level: 0.5,
            release_s: 0.030,
            gain: 0.15,
        },
        // Pause: triangle (softer than square) sweeping down an octave over
        // ~180ms - a slow "settling" gesture, matched by Unpause below.
        Sfx::Pause => ChipSpec {
            wave: ChipWave::Triangle,
            start_hz: 660.0,
            end_hz: 220.0,
            duration_s: 0.180,
            attack_s: 0.010,
            decay_s: 0.030,
            sustain_level: 0.6,
            release_s: 0.060,
            gain: 0.15,
        },
        // Unpause: exact frequency mirror of Pause (sweeping back up) so the
        // pair reads as one reversible action, not two unrelated sounds.
        Sfx::Unpause => ChipSpec {
            wave: ChipWave::Triangle,
            start_hz: 220.0,
            end_hz: 660.0,
            duration_s: 0.180,
            attack_s: 0.010,
            decay_s: 0.030,
            sustain_level: 0.6,
            release_s: 0.060,
            gain: 0.15,
        },
        // Error: low square buzz with a slight downward sweep (110 -> 90 Hz)
        // - low pitch reads as "wrong" without needing the noise oscillator
        // (which would risk the "noise-wash tail" the brief explicitly
        // rules out). Loudest of the palette since it flags something the
        // player needs to notice, but still well short of a fanfare.
        Sfx::Error => ChipSpec {
            wave: ChipWave::Square,
            start_hz: 110.0,
            end_hz: 90.0,
            duration_s: 0.140,
            attack_s: 0.005,
            decay_s: 0.020,
            sustain_level: 0.6,
            release_s: 0.050,
            gain: 0.20,
        },
        // Toast: a small, bright triangle ping - a passive notification, so
        // it stays quiet and flat-pitched rather than demanding attention.
        Sfx::Toast => ChipSpec {
            wave: ChipWave::Triangle,
            start_hz: 1320.0,
            end_hz: 1320.0,
            duration_s: 0.120,
            attack_s: 0.003,
            decay_s: 0.020,
            sustain_level: 0.4,
            release_s: 0.040,
            gain: 0.12,
        },
        // SpeedTick: fires repeatedly while scrubbing simulation speed, so
        // it has to be the shortest, quietest sound in the palette - a
        // single high square click with almost no sustain.
        Sfx::SpeedTick => ChipSpec {
            wave: ChipWave::Square,
            start_hz: 1760.0,
            end_hz: 1760.0,
            duration_s: 0.030,
            attack_s: 0.001,
            decay_s: 0.005,
            sustain_level: 0.3,
            release_s: 0.012,
            gain: 0.08,
        },
    }
}

/// All variants, for building the bank and for exhaustive unit tests.
const ALL_SFX: [Sfx; 8] = [
    Sfx::Hover,
    Sfx::Confirm,
    Sfx::Cancel,
    Sfx::Pause,
    Sfx::Unpause,
    Sfx::Error,
    Sfx::Toast,
    Sfx::SpeedTick,
];

// ---------------------------------------------------------------------
// Bevy integration: a custom Decodable audio source over the rendered
// buffer, following the "decodable" pattern from Bevy's own examples
// (bevy-0.16.1/examples/audio/decodable.rs) but replaying a precomputed
// buffer instead of synthesizing per sample at decode time.
// ---------------------------------------------------------------------

/// A rendered chip sound, registered as a custom Bevy audio asset. `Arc<[f32]>`
/// so cloning a handle's underlying decoder (done once per playback, see
/// `ChipSfx::decoder`) is a refcount bump, not a buffer copy.
#[derive(Asset, TypePath, Clone)]
pub struct ChipSfx {
    samples: Arc<[f32]>,
}

impl ChipSfx {
    pub fn new(samples: Vec<f32>) -> Self {
        ChipSfx {
            samples: Arc::from(samples),
        }
    }
}

/// Iterator/`Source` over a `ChipSfx`'s buffer. One of these is created per
/// playback (`Decodable::decoder`), each with its own read cursor over the
/// shared `Arc<[f32]>`.
pub struct ChipSfxDecoder {
    samples: Arc<[f32]>,
    pos: usize,
}

impl Iterator for ChipSfxDecoder {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = self.samples.get(self.pos).copied();
        if sample.is_some() {
            self.pos += 1;
        }
        sample
    }
}

impl Source for ChipSfxDecoder {
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

impl bevy::audio::Decodable for ChipSfx {
    type DecoderItem = f32;
    type Decoder = ChipSfxDecoder;

    fn decoder(&self) -> Self::Decoder {
        ChipSfxDecoder {
            samples: self.samples.clone(),
            pos: 0,
        }
    }
}

/// Handles into the `Assets<ChipSfx>` bank, one per `Sfx` variant, built once
/// at `Startup`. This (plus `PlaySfx`) is the public API the rest of the
/// game integrates against - HUD hover/click/pause wiring is a later pass;
/// this module just needs to exist and work end to end.
#[derive(Resource, Default)]
pub struct SfxBank(pub HashMap<Sfx, Handle<ChipSfx>>);

/// Public event: fire `PlaySfx(Sfx::Confirm)` etc. from anywhere with
/// `EventWriter<PlaySfx>` to play a sound. Playback despawns its entity when
/// done (`PlaybackSettings::DESPAWN`), so firing this repeatedly (e.g.
/// `SpeedTick` while scrubbing) never leaks entities over a long session.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlaySfx(pub Sfx);

fn build_sfx_bank_system(mut assets: ResMut<Assets<ChipSfx>>, mut commands: Commands) {
    let mut map = HashMap::with_capacity(ALL_SFX.len());
    for sfx in ALL_SFX {
        let samples = render(&spec_for(sfx));
        map.insert(sfx, assets.add(ChipSfx::new(samples)));
    }
    commands.insert_resource(SfxBank(map));
}

/// Reads `PlaySfx` and spawns a one-shot, self-despawning audio player at
/// `MASTER_VOLUME`. If the bank isn't ready yet (shouldn't happen - it's
/// built at `Startup`, before `Update` ever runs) the event is just dropped
/// rather than panicking.
fn play_sfx_system(
    mut events: EventReader<PlaySfx>,
    bank: Option<Res<SfxBank>>,
    mut commands: Commands,
) {
    let Some(bank) = bank else {
        return;
    };
    for PlaySfx(sfx) in events.read() {
        if let Some(handle) = bank.0.get(sfx) {
            commands.spawn((
                AudioPlayer(handle.clone()),
                PlaybackSettings::DESPAWN.with_volume(Volume::Linear(MASTER_VOLUME)),
            ));
        }
    }
}

/// The one producer wired inside this module itself (so the system is live
/// end-to-end without any HUD changes): every sim `toast` message plays the
/// `Toast` blip. Everything else (hover/confirm/cancel/pause/unpause) is
/// wired up by whichever system emits the corresponding player action -
/// `PlaySfx` is the public surface for that.
fn toast_sfx_system(mut sim_events: EventReader<SimEvent>, mut sfx_events: EventWriter<PlaySfx>) {
    for SimEvent(msg) in sim_events.read() {
        if let FromSimMsg::Json(FromSimJson::Toast(_)) = msg {
            sfx_events.write(PlaySfx(Sfx::Toast));
        }
    }
}

pub struct MfAudioPlugin;

impl Plugin for MfAudioPlugin {
    fn build(&self, app: &mut App) {
        app.add_audio_source::<ChipSfx>()
            .add_event::<PlaySfx>()
            .add_systems(Startup, build_sfx_bank_system)
            .add_systems(Update, (play_sfx_system, toast_sfx_system));
    }
}

// ---------------------------------------------------------------------
// Tests: pure synth-core correctness, no Bevy app needed.
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Tail window fraction checked for "no click on cutoff": the last 5%
    /// of samples must have decayed to near-silence.
    const TAIL_FRACTION: f32 = 0.05;
    const TAIL_AMPLITUDE_MAX: f32 = 0.05;

    fn assert_well_formed(name: &str, spec: &ChipSpec) {
        let samples = render(spec);

        assert!(!samples.is_empty(), "{name}: render produced no samples");

        for (i, s) in samples.iter().enumerate() {
            assert!(s.is_finite(), "{name}: sample {i} is not finite ({s})");
            assert!(
                (-1.0..=1.0).contains(s),
                "{name}: sample {i} out of range ({s})"
            );
        }

        let expected_len = (spec.duration_s * SAMPLE_RATE as f32).round() as usize;
        let diff = (samples.len() as isize - expected_len as isize).unsigned_abs();
        let tolerance = ((expected_len as f32) * 0.01).ceil() as usize;
        assert!(
            diff <= tolerance.max(1),
            "{name}: length {} not within 1% of expected {expected_len}",
            samples.len()
        );

        let tail_len = ((samples.len() as f32) * TAIL_FRACTION).ceil() as usize;
        let tail_start = samples.len().saturating_sub(tail_len.max(1));
        for (offset, s) in samples[tail_start..].iter().enumerate() {
            assert!(
                s.abs() < TAIL_AMPLITUDE_MAX,
                "{name}: tail sample {} amplitude {} did not decay below {TAIL_AMPLITUDE_MAX}",
                tail_start + offset,
                s.abs()
            );
        }
    }

    #[test]
    fn every_sfx_spec_is_well_formed() {
        for sfx in ALL_SFX {
            assert_well_formed(&format!("{sfx:?}"), &spec_for(sfx));
        }
    }

    #[test]
    fn noise_render_is_deterministic() {
        let spec = ChipSpec {
            wave: ChipWave::Noise,
            start_hz: 200.0,
            end_hz: 200.0,
            duration_s: 0.1,
            attack_s: 0.005,
            decay_s: 0.02,
            sustain_level: 0.5,
            release_s: 0.03,
            gain: 0.2,
        };
        let a = render(&spec);
        let b = render(&spec);
        assert_eq!(a, b, "same ChipSpec must render identical samples");
        assert!(!a.is_empty());
        for s in &a {
            assert!(s.is_finite());
            assert!((-1.0..=1.0).contains(s));
        }
    }

    #[test]
    fn square_and_triangle_are_distinct_waveforms() {
        // Sanity check that the two tonal waveforms actually differ (not a
        // copy-paste bug where Triangle silently reused Square's branch).
        let base = ChipSpec {
            wave: ChipWave::Square,
            start_hz: 440.0,
            end_hz: 440.0,
            duration_s: 0.05,
            attack_s: 0.0,
            decay_s: 0.0,
            sustain_level: 1.0,
            release_s: 0.01,
            gain: 1.0,
        };
        let square = render(&base);
        let triangle = render(&ChipSpec {
            wave: ChipWave::Triangle,
            ..base
        });
        assert_ne!(square, triangle);
    }

    #[test]
    fn zero_duration_spec_does_not_panic() {
        // Defensive: a degenerate spec (e.g. a future bug upstream) must not
        // divide by zero or panic; it should just render nothing/near-nothing.
        let spec = ChipSpec {
            wave: ChipWave::Square,
            start_hz: 440.0,
            end_hz: 440.0,
            duration_s: 0.0,
            attack_s: 0.0,
            decay_s: 0.0,
            sustain_level: 0.0,
            release_s: 0.0,
            gain: 0.1,
        };
        let samples = render(&spec);
        assert!(samples.len() <= 1);
    }
}
