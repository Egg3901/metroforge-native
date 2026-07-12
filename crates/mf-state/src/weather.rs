//! User-facing weather-effects toggle plus the sim-driven weather render
//! state (v0.7).
//!
//! [`WeatherEffects`] is the Settings checkbox that lets players turn soft
//! cloud cards / ground shadows off even on Medium/High.
//!
//! [`WeatherRender`] is the *sim-authored* weather (`UiState::weather_state`
//! etc.) smoothed into continuous render weights. Art direction (BINDING,
//! `art-direction.md`): Mirror's Edge white city, transit is the only color;
//! weather reads as fog + light grading + precip particles, never a coloured
//! wash. Following the `SubwayView` convention, this module owns only the
//! *state* + the pure [`WeatherRender::step`] integrator; the driving system
//! (which has frame `Time` + `LatestUi`) lives in `mf-render`'s
//! `weather_render.rs`. Kept `bevy_time`-free so `mf-state` stays a light
//! dependency.

use bevy_ecs::prelude::*;
use mf_protocol::{Season, WeatherEvent, WeatherState};

/// Whether the player wants atmospheric weather (soft cloud cards + scrolling
/// ground shadows) drawn when the active quality tier supports it.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeatherEffects {
    /// When `true` and the quality tier allows atmosphere, draw weather fog/clouds.
    pub enabled: bool,
}

impl Default for WeatherEffects {
    fn default() -> Self {
        // On by default when the tier allows it; Potato/Low still skip the
        // effect via `QualityKnobs::atmosphere_enabled`.
        WeatherEffects { enabled: true }
    }
}

/// Full weight-transition time (state change -> new look fully settled).
/// Effects ease exponentially so a sim state flip never pops (~95% settled
/// by this horizon: the exponential time-constant is a third of it).
pub const WEATHER_TRANSITION_SECS: f32 = 30.0;
/// Exponential time-constant for the weight ease (`WEATHER_TRANSITION_SECS/3`).
const WEATHER_TAU_SECS: f32 = WEATHER_TRANSITION_SECS / 3.0;
/// Seconds of heavy snow to reach full ground accumulation.
const SNOW_ACCUM_SECS: f32 = 55.0;
/// Seconds for full accumulation to melt away once snow stops.
const SNOW_MELT_SECS: f32 = 85.0;
/// A lightning flash decays to nothing over this many seconds (visual only;
/// the *trigger* is deterministic from the sim tick, see [`flash_gap_ticks`]).
const FLASH_DECAY_SECS: f32 = 0.22;
/// Sim ticks per second the sidecar emits `UiState` at (2 Hz). Used only to
/// map the "2-8s" storm-flash cadence into tick space.
const TICKS_PER_SEC: u64 = 2;

/// Continuous 0..1 effect weights derived from a discrete [`WeatherState`].
/// `WeatherRender` eases toward these so transitions never pop.
#[derive(Debug, Clone, Copy, PartialEq)]
struct WeatherWeights {
    rain: f32,
    snow: f32,
    overcast: f32,
    fog: f32,
    storm: f32,
}

impl WeatherWeights {
    const CLEAR: WeatherWeights = WeatherWeights {
        rain: 0.0,
        snow: 0.0,
        overcast: 0.0,
        fog: 0.0,
        storm: 0.0,
    };

    /// Target weights for a sim state at the given precip `intensity` (0..1).
    fn for_state(state: Option<WeatherState>, intensity: f32) -> WeatherWeights {
        let i = intensity.clamp(0.0, 1.0);
        match state {
            None | Some(WeatherState::Clear) => WeatherWeights::CLEAR,
            Some(WeatherState::Overcast) => WeatherWeights {
                overcast: 0.9,
                fog: 0.05,
                ..WeatherWeights::CLEAR
            },
            Some(WeatherState::Rain) => WeatherWeights {
                rain: i.max(0.35),
                overcast: 0.6,
                fog: 0.15,
                ..WeatherWeights::CLEAR
            },
            Some(WeatherState::Fog) => WeatherWeights {
                overcast: 0.4,
                fog: 1.0,
                ..WeatherWeights::CLEAR
            },
            Some(WeatherState::Snow) => WeatherWeights {
                snow: i.max(0.4),
                overcast: 0.7,
                fog: 0.25,
                ..WeatherWeights::CLEAR
            },
            Some(WeatherState::Storm) => WeatherWeights {
                rain: i.max(0.85),
                snow: 0.0,
                overcast: 1.0,
                fog: 0.2,
                storm: 1.0,
            },
        }
    }
}

/// Sim-authored weather, smoothed into render-ready continuous weights.
///
/// The discrete inputs (`state`/`season`/`event`/`intensity`) snap to the
/// latest `UiState`; the *weights* ([`Self::rain`] .. [`Self::storm`]),
/// [`Self::snow_depth`], and [`Self::lightning`] ease every frame via
/// [`Self::step`] so nothing pops. All weights are gated further downstream
/// by quality tier + [`WeatherEffects`].
#[derive(Resource, Debug, Clone)]
pub struct WeatherRender {
    /// Latest discrete sky state (snaps immediately; weights lag).
    pub state: Option<WeatherState>,
    /// Latest season (drives the HUD tooltip only).
    pub season: Option<Season>,
    /// Latest headline event (blizzard / heat wave), when active.
    pub event: Option<WeatherEvent>,
    /// Raw target precip intensity (0..1) from the sim; unsmoothed.
    pub intensity: f32,
    /// Eased rain weight (0..1) — streak count / wet-road sheen.
    pub rain: f32,
    /// Eased snow weight (0..1) — flake count.
    pub snow: f32,
    /// Eased overcast weight (0..1) — sun dim + shadow flatten + cloud thicken.
    pub overcast: f32,
    /// Eased fog weight (0..1) — mist density / fog-tier distance shrink.
    pub fog: f32,
    /// Eased storm weight (0..1) — extra wind + lightning enable.
    pub storm: f32,
    /// Ground snow accumulation (0..1): rises under snow, decays after.
    pub snow_depth: f32,
    /// Current lightning-flash luminance (0..1); brief decaying pulse.
    pub lightning: f32,
    /// Next sim tick a storm flash fires on (deterministic; replay-stable).
    next_flash_tick: u64,
    /// Highest tick seen, so `step` only advances the flash schedule forward.
    seen_tick: u64,
}

impl Default for WeatherRender {
    fn default() -> Self {
        WeatherRender {
            state: None,
            season: None,
            event: None,
            intensity: 0.0,
            rain: 0.0,
            snow: 0.0,
            overcast: 0.0,
            fog: 0.0,
            storm: 0.0,
            snow_depth: 0.0,
            lightning: 0.0,
            next_flash_tick: 0,
            seen_tick: 0,
        }
    }
}

/// Deterministic 2..8s storm-flash gap in ticks, hashed from `tick` so a
/// replay of the same tick stream reproduces the exact same lightning.
fn flash_gap_ticks(tick: u64) -> u64 {
    // 2s..8s at 2 Hz -> 4..16 ticks.
    let mut n = tick.wrapping_mul(0x9E3779B97F4A7C15);
    n ^= n >> 29;
    let min = 2 * TICKS_PER_SEC;
    let span = 6 * TICKS_PER_SEC + 1; // inclusive 8s
    min + n % span
}

impl WeatherRender {
    /// Whether any weight is non-trivial (used to skip render work / mark the
    /// resource unchanged at a fully-clear steady state).
    pub fn is_active(&self) -> bool {
        self.rain > 1e-3
            || self.snow > 1e-3
            || self.overcast > 1e-3
            || self.fog > 1e-3
            || self.storm > 1e-3
            || self.snow_depth > 1e-3
            || self.lightning > 1e-3
    }

    /// Snap the discrete sim inputs (called when a new `UiState` arrives or an
    /// `MF_FORCE_WEATHER` override is set). Does not move the eased weights.
    pub fn set_inputs(
        &mut self,
        state: Option<WeatherState>,
        intensity: Option<f32>,
        season: Option<Season>,
        event: Option<WeatherEvent>,
    ) {
        self.state = state;
        self.intensity = intensity.unwrap_or(0.0).clamp(0.0, 1.0);
        self.season = season;
        self.event = event;
    }

    /// Ease every weight toward the current state's targets by `dt` seconds,
    /// advance snow accumulation, and fire deterministic storm flashes for
    /// `tick`. Pure and frame-rate independent (exponential ease); returns the
    /// pre-step snapshot so the caller can skip marking the resource changed
    /// at steady state.
    pub fn step(&mut self, dt_secs: f32, tick: u64) {
        let dt = dt_secs.clamp(0.0, 0.25);
        let target = WeatherWeights::for_state(self.state, self.intensity);
        // Exponential ease: `a = 1 - e^(-dt/tau)`.
        let a = 1.0 - (-dt / WEATHER_TAU_SECS).exp();
        self.rain += (target.rain - self.rain) * a;
        self.snow += (target.snow - self.snow) * a;
        self.overcast += (target.overcast - self.overcast) * a;
        self.fog += (target.fog - self.fog) * a;
        self.storm += (target.storm - self.storm) * a;

        // Snow accumulates while it is actually snowing, melts afterwards.
        if self.snow > 0.15 {
            self.snow_depth += self.snow * dt / SNOW_ACCUM_SECS;
        } else {
            self.snow_depth -= dt / SNOW_MELT_SECS;
        }
        self.snow_depth = self.snow_depth.clamp(0.0, 1.0);

        // Lightning: decay the visible pulse, then fire deterministically.
        self.lightning = (self.lightning - dt / FLASH_DECAY_SECS).max(0.0);
        if tick > self.seen_tick {
            // Keep the flash schedule armed just ahead of the tick stream so a
            // storm that just started flashes within a few seconds.
            if self.next_flash_tick <= self.seen_tick {
                self.next_flash_tick = tick + flash_gap_ticks(tick);
            }
            if self.storm > 0.5 && tick >= self.next_flash_tick {
                self.lightning = 1.0;
                self.next_flash_tick = tick + flash_gap_ticks(tick);
            }
            self.seen_tick = tick;
        }
    }
}

/// Parse an `MF_FORCE_WEATHER` dev override, e.g. `"rain"`, `"snow:0.9"`,
/// `"storm"`. Returns the forced state and optional intensity. Dev-only:
/// documented in `docs/BUILDING.md`; lets CI / photo mode pin a state the sim
/// might rarely roll. Unknown values yield `None` (override ignored).
pub fn parse_forced_weather(raw: &str) -> Option<(WeatherState, Option<f32>)> {
    let raw = raw.trim();
    let (name, intensity) = match raw.split_once(':') {
        Some((n, i)) => (n.trim(), i.trim().parse::<f32>().ok()),
        None => (raw, None),
    };
    let state = match name.to_ascii_lowercase().as_str() {
        "clear" => WeatherState::Clear,
        "overcast" => WeatherState::Overcast,
        "rain" => WeatherState::Rain,
        "fog" => WeatherState::Fog,
        "snow" => WeatherState::Snow,
        "storm" => WeatherState::Storm,
        _ => return None,
    };
    Some((state, intensity.map(|i| i.clamp(0.0, 1.0))))
}

#[cfg(test)]
mod weather_render_tests {
    use super::*;

    #[test]
    fn weights_ease_toward_target_without_overshoot() {
        let mut w = WeatherRender::default();
        w.set_inputs(Some(WeatherState::Rain), Some(1.0), None, None);
        // ~40s of settling (well past the 30s transition horizon).
        for _ in 0..60 * 40 {
            w.step(1.0 / 60.0, 0);
        }
        assert!(w.rain > 0.9, "rain settled: {}", w.rain);
        assert!(w.rain <= 1.0 + 1e-4);
        assert!(w.overcast > 0.5);
    }

    #[test]
    fn clearing_decays_weights_back_to_zero() {
        let mut w = WeatherRender::default();
        w.set_inputs(Some(WeatherState::Storm), Some(1.0), None, None);
        for _ in 0..60 * 40 {
            w.step(1.0 / 60.0, 0);
        }
        w.set_inputs(Some(WeatherState::Clear), None, None, None);
        for _ in 0..60 * 60 {
            w.step(1.0 / 60.0, 0);
        }
        assert!(
            w.rain < 1e-2 && w.storm < 1e-2,
            "rain={} storm={}",
            w.rain,
            w.storm
        );
    }

    #[test]
    fn snow_accumulates_then_melts() {
        let mut w = WeatherRender::default();
        w.set_inputs(Some(WeatherState::Snow), Some(1.0), None, None);
        for _ in 0..60 * 90 {
            w.step(1.0 / 60.0, 0);
        }
        assert!(w.snow_depth > 0.8, "depth={}", w.snow_depth);
        w.set_inputs(Some(WeatherState::Clear), None, None, None);
        for _ in 0..60 * 200 {
            w.step(1.0 / 60.0, 0);
        }
        assert!(w.snow_depth < 1e-2, "melt depth={}", w.snow_depth);
    }

    #[test]
    fn storm_flashes_are_deterministic_from_tick() {
        let run = || {
            let mut w = WeatherRender::default();
            w.set_inputs(Some(WeatherState::Storm), Some(1.0), None, None);
            // Settle storm weight first.
            for _ in 0..60 * 40 {
                w.step(1.0 / 60.0, 1);
            }
            let mut flashes = Vec::new();
            for tick in 2..200u64 {
                // ~30 frames per tick at 60fps / 2Hz; a flash fires on the
                // first frame of the tick and decays within it, so watch the
                // peak luminance across the tick rather than the end value.
                let mut peak = 0.0f32;
                for _ in 0..30 {
                    w.step(1.0 / 60.0, tick);
                    peak = peak.max(w.lightning);
                }
                if peak > 0.9 {
                    flashes.push(tick);
                }
            }
            flashes
        };
        let a = run();
        let b = run();
        assert_eq!(a, b, "flash schedule must be replay-stable");
        assert!(!a.is_empty(), "storm should flash at least once");
    }

    #[test]
    fn flash_gap_stays_in_two_to_eight_seconds() {
        for tick in 0..1000u64 {
            let g = flash_gap_ticks(tick);
            assert!((4..=16).contains(&g), "gap {g} ticks out of 2-8s band");
        }
    }

    #[test]
    fn parse_forced_weather_reads_name_and_intensity() {
        assert_eq!(
            parse_forced_weather("rain"),
            Some((WeatherState::Rain, None))
        );
        assert_eq!(
            parse_forced_weather("snow:0.9"),
            Some((WeatherState::Snow, Some(0.9)))
        );
        assert_eq!(
            parse_forced_weather("STORM"),
            Some((WeatherState::Storm, None))
        );
        assert!(parse_forced_weather("banana").is_none());
    }
}
