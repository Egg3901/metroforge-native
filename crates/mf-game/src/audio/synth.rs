//! Pure PCM synthesizers for MetroForge UI SFX and city ambience.
//!
//! No Bevy types here — every public function returns a mono `Vec<f32>` in
//! approximately `[-1, 1]` at [`SAMPLE_RATE`], so unit tests can assert on
//! buffers without spinning up an `App`. Soft-clipping at the end of each
//! renderer keeps peaks below hard clipping even if a gain is mistuned.

/// Bevy/rodio-friendly sample rate used for every buffer in this module.
pub const SAMPLE_RATE: u32 = 44_100;

/// Soft clip so an over-aggressive gain can never produce a sample outside
/// `[-1, 1]` (and thus never an audible click at the DAC). `tanh` is
/// ~identity near zero and only bends near the rails.
fn soft_clip(x: f32) -> f32 {
    x.tanh()
}

fn sample_count(duration_s: f32) -> usize {
    ((duration_s * SAMPLE_RATE as f32).round().max(1.0)) as usize
}

/// Deterministic LFSR noise sample in `[-1, 1]`. Same seed → same sequence.
fn lfsr_step(lfsr: &mut u16) -> f32 {
    let bit = (*lfsr ^ (*lfsr >> 2)) & 1;
    *lfsr = (*lfsr >> 1) | (bit << 15);
    (*lfsr as f32 / 32_767.5) - 1.0
}

/// One-pole low-pass: `y += alpha * (x - y)`.
#[derive(Clone, Copy)]
struct OnePoleLp {
    y: f32,
    alpha: f32,
}

impl OnePoleLp {
    fn new(cutoff_hz: f32) -> Self {
        let sr = SAMPLE_RATE as f32;
        let alpha = (2.0 * std::f32::consts::PI * cutoff_hz / sr).clamp(0.0, 1.0);
        Self { y: 0.0, alpha }
    }

    fn process(&mut self, x: f32) -> f32 {
        self.y += self.alpha * (x - self.y);
        self.y
    }
}

/// ADSR-ish envelope value at time `t` for a note of length `duration_s`.
fn adsr(
    t: f32,
    duration_s: f32,
    attack_s: f32,
    decay_s: f32,
    sustain_level: f32,
    release_s: f32,
) -> f32 {
    let total = attack_s + decay_s + release_s;
    let scale = if total > duration_s && total > 0.0 {
        duration_s / total
    } else {
        1.0
    };
    let attack_s = attack_s * scale;
    let decay_s = decay_s * scale;
    let release_s = release_s * scale;
    let decay_end = attack_s + decay_s;
    let sustain_s = (duration_s - attack_s - decay_s - release_s).max(0.0);
    let sustain_end = decay_end + sustain_s;

    if t < attack_s {
        if attack_s > 0.0 {
            t / attack_s
        } else {
            1.0
        }
    } else if t < decay_end {
        if decay_s > 0.0 {
            let f = (t - attack_s) / decay_s;
            1.0 + (sustain_level - 1.0) * f
        } else {
            sustain_level
        }
    } else if t < sustain_end {
        sustain_level
    } else if release_s > 0.0 {
        let f = ((t - sustain_end) / release_s).min(1.0);
        sustain_level * (1.0 - f)
    } else {
        0.0
    }
}

fn render_sine_note(
    start_hz: f32,
    end_hz: f32,
    duration_s: f32,
    attack_s: f32,
    decay_s: f32,
    sustain_level: f32,
    release_s: f32,
    gain: f32,
) -> Vec<f32> {
    let n = sample_count(duration_s);
    let sr = SAMPLE_RATE as f32;
    let mut out = Vec::with_capacity(n);
    let mut phase = 0.0_f32;
    for i in 0..n {
        let t = i as f32 / sr;
        let env = adsr(t, duration_s, attack_s, decay_s, sustain_level, release_s);
        let frac = if duration_s > 0.0 {
            (t / duration_s).min(1.0)
        } else {
            0.0
        };
        let freq = start_hz + (end_hz - start_hz) * frac;
        phase = (phase + freq / sr).fract();
        let sample = (phase * std::f32::consts::TAU).sin();
        out.push(soft_clip(sample * env * gain));
    }
    out
}

fn render_square_note(
    start_hz: f32,
    end_hz: f32,
    duration_s: f32,
    attack_s: f32,
    decay_s: f32,
    sustain_level: f32,
    release_s: f32,
    gain: f32,
) -> Vec<f32> {
    let n = sample_count(duration_s);
    let sr = SAMPLE_RATE as f32;
    let mut out = Vec::with_capacity(n);
    let mut phase = 0.0_f32;
    for i in 0..n {
        let t = i as f32 / sr;
        let env = adsr(t, duration_s, attack_s, decay_s, sustain_level, release_s);
        let frac = if duration_s > 0.0 {
            (t / duration_s).min(1.0)
        } else {
            0.0
        };
        let freq = start_hz + (end_hz - start_hz) * frac;
        phase = (phase + freq / sr).fract();
        let sample = if phase < 0.5 { 1.0 } else { -1.0 };
        out.push(soft_clip(sample * env * gain));
    }
    out
}

fn render_triangle_note(
    start_hz: f32,
    end_hz: f32,
    duration_s: f32,
    attack_s: f32,
    decay_s: f32,
    sustain_level: f32,
    release_s: f32,
    gain: f32,
) -> Vec<f32> {
    let n = sample_count(duration_s);
    let sr = SAMPLE_RATE as f32;
    let mut out = Vec::with_capacity(n);
    let mut phase = 0.0_f32;
    for i in 0..n {
        let t = i as f32 / sr;
        let env = adsr(t, duration_s, attack_s, decay_s, sustain_level, release_s);
        let frac = if duration_s > 0.0 {
            (t / duration_s).min(1.0)
        } else {
            0.0
        };
        let freq = start_hz + (end_hz - start_hz) * frac;
        phase = (phase + freq / sr).fract();
        let sample = 4.0 * (phase - 0.5).abs() - 1.0;
        out.push(soft_clip(sample * env * gain));
    }
    out
}

// ---------------------------------------------------------------------
// Public palette renderers
// ---------------------------------------------------------------------

/// Short filtered tick for UI hover / click feedback: a tiny noise burst
/// through a one-pole high-pass so it reads as a dry "tick", not a thud.
pub fn render_ui_click() -> Vec<f32> {
    let duration_s = 0.040;
    let n = sample_count(duration_s);
    let sr = SAMPLE_RATE as f32;
    let mut lfsr: u16 = 0xBEEF;
    let mut lp = OnePoleLp::new(2_800.0);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr;
        let env = adsr(t, duration_s, 0.001, 0.008, 0.25, 0.020);
        let noise = lfsr_step(&mut lfsr);
        // High-pass ≈ noise − low-pass(noise).
        let tick = noise - lp.process(noise);
        out.push(soft_clip(tick * env * 0.22));
    }
    out
}

/// Placement thunk: low sine body + short noise burst, shared fast envelope.
pub fn render_placement_thunk() -> Vec<f32> {
    let duration_s = 0.120;
    let n = sample_count(duration_s);
    let sr = SAMPLE_RATE as f32;
    let mut lfsr: u16 = 0xACE1;
    let mut phase = 0.0_f32;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr;
        let env = adsr(t, duration_s, 0.003, 0.025, 0.35, 0.055);
        // Slight downward pitch glide on the body.
        let freq = 95.0 + (70.0 - 95.0) * (t / duration_s).min(1.0);
        phase = (phase + freq / sr).fract();
        let sine = (phase * std::f32::consts::TAU).sin();
        // Noise only in the first ~35 ms so the tail is a clean sine decay.
        let noise_env = (1.0 - (t / 0.035).min(1.0)).powi(2);
        let noise = lfsr_step(&mut lfsr) * noise_env * 0.55;
        out.push(soft_clip((sine * 0.85 + noise) * env * 0.28));
    }
    out
}

/// Low square buzz for errors / invalid actions.
pub fn render_error_buzz() -> Vec<f32> {
    render_square_note(110.0, 90.0, 0.140, 0.005, 0.020, 0.6, 0.050, 0.20)
}

/// Two-note goal-complete chime (rising major third: C5 → E5).
pub fn render_goal_chime() -> Vec<f32> {
    let note_a = render_sine_note(523.25, 523.25, 0.140, 0.008, 0.030, 0.45, 0.070, 0.18);
    let gap = sample_count(0.030);
    let note_b = render_sine_note(659.25, 659.25, 0.180, 0.008, 0.040, 0.40, 0.090, 0.16);
    let mut out = Vec::with_capacity(note_a.len() + gap + note_b.len());
    out.extend_from_slice(&note_a);
    out.extend(std::iter::repeat(0.0).take(gap));
    out.extend_from_slice(&note_b);
    out
}

/// Bright toast pop — short triangle ping.
pub fn render_toast_pop() -> Vec<f32> {
    render_triangle_note(1320.0, 1320.0, 0.110, 0.003, 0.018, 0.35, 0.040, 0.14)
}

/// Quiet confirm: rising fourth square sweep (kept for existing Confirm SFX).
pub fn render_confirm() -> Vec<f32> {
    render_square_note(659.25, 880.0, 0.090, 0.005, 0.020, 0.5, 0.030, 0.18)
}

/// Cancel: falling fourth, darker than confirm.
pub fn render_cancel() -> Vec<f32> {
    render_square_note(440.0, 329.63, 0.090, 0.005, 0.020, 0.5, 0.030, 0.15)
}

/// Pause settle: triangle octave down.
pub fn render_pause() -> Vec<f32> {
    render_triangle_note(660.0, 220.0, 0.180, 0.010, 0.030, 0.6, 0.060, 0.15)
}

/// Unpause: mirror of pause.
pub fn render_unpause() -> Vec<f32> {
    render_triangle_note(220.0, 660.0, 0.180, 0.010, 0.030, 0.6, 0.060, 0.15)
}

/// Speed scrub tick: shortest/quietest click in the palette.
pub fn render_speed_tick() -> Vec<f32> {
    render_square_note(1760.0, 1760.0, 0.030, 0.001, 0.005, 0.3, 0.012, 0.08)
}

/// Looping city ambience: band-passed noise with a slow amplitude LFO whose
/// period equals the buffer length so the loop point is seamless.
pub fn render_city_ambience() -> Vec<f32> {
    // ~4 s loop; LFO period matches so amp is identical at both ends.
    let duration_s = 4.0;
    let n = sample_count(duration_s);
    let sr = SAMPLE_RATE as f32;
    let mut lfsr: u16 = 0xC0DE;
    // Band-pass ≈ low-pass(high-pass(noise)): rumble floor ~200 Hz, ceiling ~1.2 kHz.
    let mut hp_lp = OnePoleLp::new(200.0);
    let mut bp_lp = OnePoleLp::new(1_200.0);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr;
        let noise = lfsr_step(&mut lfsr);
        let hp = noise - hp_lp.process(noise);
        let bp = bp_lp.process(hp);
        // Slow swell: one full cycle over the buffer (seamless at loop).
        let lfo = 0.55 + 0.45 * (t * std::f32::consts::TAU / duration_s).sin();
        // Very quiet — underlay, not a bed that fights UI ticks.
        out.push(soft_clip(bp * lfo * 0.045));
    }
    // Force exact loop continuity at the endpoints (numerical drift guard).
    if let (Some(first), Some(last)) = (out.first().copied(), out.last_mut()) {
        *last = first;
    }
    out
}

/// Expected duration in seconds for a rendered buffer (for length tests).
pub fn expected_duration_s(samples: &[f32]) -> f32 {
    samples.len() as f32 / SAMPLE_RATE as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEAK_MAX: f32 = 0.99;
    const SILENCE_FLOOR: f32 = 1e-4;

    fn assert_pcm_ok(name: &str, samples: &[f32], expected_duration_s: f32) {
        assert!(
            !samples.is_empty(),
            "{name}: render produced an empty buffer"
        );

        let mut peak = 0.0_f32;
        for (i, &s) in samples.iter().enumerate() {
            assert!(s.is_finite(), "{name}: sample {i} is not finite ({s})");
            assert!(
                (-1.0..=1.0).contains(&s),
                "{name}: sample {i} out of [-1, 1] ({s})"
            );
            peak = peak.max(s.abs());
        }

        assert!(
            peak > SILENCE_FLOOR,
            "{name}: buffer is silent (peak {peak})"
        );
        assert!(
            peak < PEAK_MAX,
            "{name}: peak {peak} reaches clipping threshold {PEAK_MAX}"
        );

        let expected_len = (expected_duration_s * SAMPLE_RATE as f32).round() as usize;
        let diff = (samples.len() as isize - expected_len as isize).unsigned_abs();
        let tolerance = ((expected_len as f32) * 0.02).ceil().max(1.0) as usize;
        assert!(
            diff <= tolerance,
            "{name}: length {} not within 2% of expected {expected_len} ({expected_duration_s}s)",
            samples.len()
        );
    }

    #[test]
    fn ui_click_is_non_silent_correct_length_below_clip() {
        let s = render_ui_click();
        assert_pcm_ok("ui_click", &s, 0.040);
    }

    #[test]
    fn placement_thunk_is_non_silent_correct_length_below_clip() {
        let s = render_placement_thunk();
        assert_pcm_ok("placement_thunk", &s, 0.120);
    }

    #[test]
    fn error_buzz_is_non_silent_correct_length_below_clip() {
        let s = render_error_buzz();
        assert_pcm_ok("error_buzz", &s, 0.140);
    }

    #[test]
    fn goal_chime_is_non_silent_correct_length_below_clip() {
        let s = render_goal_chime();
        // 0.140 + 0.030 gap + 0.180
        assert_pcm_ok("goal_chime", &s, 0.350);
    }

    #[test]
    fn toast_pop_is_non_silent_correct_length_below_clip() {
        let s = render_toast_pop();
        assert_pcm_ok("toast_pop", &s, 0.110);
    }

    #[test]
    fn city_ambience_is_non_silent_correct_length_below_clip() {
        let s = render_city_ambience();
        assert_pcm_ok("city_ambience", &s, 4.0);
        // Loop seam: first and last sample match (LFO period == duration).
        assert_eq!(
            s.first().copied(),
            s.last().copied(),
            "ambience loop seam must match"
        );
    }

    #[test]
    fn remaining_ui_palette_buffers_are_well_formed() {
        assert_pcm_ok("confirm", &render_confirm(), 0.090);
        assert_pcm_ok("cancel", &render_cancel(), 0.090);
        assert_pcm_ok("pause", &render_pause(), 0.180);
        assert_pcm_ok("unpause", &render_unpause(), 0.180);
        assert_pcm_ok("speed_tick", &render_speed_tick(), 0.030);
    }

    #[test]
    fn noise_lfsr_is_deterministic() {
        let a = render_ui_click();
        let b = render_ui_click();
        assert_eq!(a, b);
    }

    #[test]
    fn placement_and_click_are_distinct() {
        assert_ne!(render_ui_click(), render_placement_thunk());
    }
}
