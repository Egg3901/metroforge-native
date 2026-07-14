//! Deterministic PRNG. Faithful port of `sim/src/core/rng.ts`.
//!
//! Algorithm: xoshiro128** with splitmix32 seeding. The TypeScript original
//! does all of its arithmetic on 32-bit unsigned integers (every `>>> 0` in
//! the source is a wrapping-to-u32 coercion, and `Math.imul` is a wrapping
//! 32-bit multiply). Because this is pure integer math with no floating point,
//! the Rust port reproduces the JavaScript output BIT-FOR-BIT. That gives us
//! free RNG parity across the TS -> Rust rebaseline: see the unit tests below,
//! which assert the exact captured sequence for seed 12345.
//!
//! The 4-word integer state is part of the save format, so a native
//! reimplementation can resume a stream mid-game bit-for-bit.

/// The 4-word xoshiro128 state. Mirrors the TS `RngState` tuple.
pub type RngState = [u32; 4];

/// splitmix32 step generator. Mirrors the closure returned by `splitmix32` in
/// rng.ts. Rust wrapping ops map 1:1 onto the TS `>>> 0` / `Math.imul` idioms.
struct SplitMix32 {
    s: u32,
}

impl SplitMix32 {
    fn new(seed: u32) -> Self {
        Self { s: seed }
    }

    fn next(&mut self) -> u32 {
        self.s = self.s.wrapping_add(0x9e37_79b9);
        let mut z = self.s;
        z = (z ^ (z >> 16)).wrapping_mul(0x21f0_aaad);
        z = (z ^ (z >> 15)).wrapping_mul(0x735a_2d97);
        z ^ (z >> 15)
    }
}

/// 32-bit rotate-left. Mirrors `rotl` in rng.ts.
#[inline]
fn rotl(x: u32, k: u32) -> u32 {
    x.rotate_left(k)
}

/// Deterministic PRNG. Port of the TS `Rng` class; same public surface.
#[derive(Clone, Debug)]
pub struct Rng {
    s: RngState,
}

impl Rng {
    /// Construct from a numeric seed (splitmix32-expanded to 4 words).
    /// Mirrors `new Rng(seed: number)`.
    pub fn from_seed(seed: u32) -> Self {
        let mut mix = SplitMix32::new(seed);
        let mut s = [mix.next(), mix.next(), mix.next(), mix.next()];
        // avoid the all-zero degenerate state
        if (s[0] | s[1] | s[2] | s[3]) == 0 {
            s[0] = 1;
        }
        Self { s }
    }

    /// Construct from a saved 4-word state. Mirrors `new Rng(state: RngState)`.
    pub fn from_state(state: RngState) -> Self {
        Self { s: state }
    }

    /// Snapshot the current state (for saves). Mirrors `state()`.
    pub fn state(&self) -> RngState {
        self.s
    }

    /// Next uint32. Mirrors `nextUint()`.
    pub fn next_uint(&mut self) -> u32 {
        let [s0, s1, s2, s3] = self.s;
        let result = rotl(s1.wrapping_mul(5), 7).wrapping_mul(9);
        let t = s1 << 9;
        let mut n2 = s2 ^ s0;
        let mut n3 = s3 ^ s1;
        let n1 = s1 ^ n2;
        let n0 = s0 ^ n3;
        n2 ^= t;
        n3 = rotl(n3, 11);
        self.s = [n0, n1, n2, n3];
        result
    }

    /// Uniform float in [0, 1). 24-bit mantissa. Mirrors `next()`.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_uint() >> 8) as f64 / 16_777_216.0
    }

    /// Uniform float in [min, max). Mirrors `range(min, max)`.
    pub fn range(&mut self, min: f64, max: f64) -> f64 {
        min + (max - min) * self.next_f64()
    }

    /// Uniform integer in [min, max] inclusive. Mirrors `int(min, max)`.
    pub fn int(&mut self, min: i64, max: i64) -> i64 {
        min + (self.next_f64() * (max - min + 1) as f64).floor() as i64
    }

    /// Bernoulli trial. Mirrors `chance(p)`.
    pub fn chance(&mut self, p: f64) -> bool {
        self.next_f64() < p
    }

    /// Pick a random element. Mirrors `pick(arr)`. Returns `None` if empty
    /// (the TS version throws; Rust prefers an explicit `Option`).
    pub fn pick<'a, T>(&mut self, arr: &'a [T]) -> Option<&'a T> {
        if arr.is_empty() {
            return None;
        }
        Some(&arr[self.int(0, arr.len() as i64 - 1) as usize])
    }

    /// Weighted index pick; weights need not sum to 1. Mirrors `weighted()`.
    pub fn weighted(&mut self, weights: &[f64]) -> usize {
        let total: f64 = weights.iter().sum();
        if total <= 0.0 {
            return 0;
        }
        let mut r = self.next_f64() * total;
        for (i, &w) in weights.iter().enumerate() {
            r -= w;
            if r < 0.0 {
                return i;
            }
        }
        weights.len() - 1
    }

    /// Derive an independent child stream. Mirrors `fork(tag)`.
    pub fn fork(&mut self, tag: u32) -> Rng {
        Rng::from_seed(self.next_uint() ^ tag.wrapping_mul(0x9e37_79b9))
    }

    /// Fisher-Yates shuffle in place. Mirrors `shuffle(arr)`.
    pub fn shuffle<T>(&mut self, arr: &mut [T]) {
        if arr.is_empty() {
            return;
        }
        for i in (1..arr.len()).rev() {
            let j = self.int(0, i as i64) as usize;
            arr.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference sequences captured from the TS original for seed 12345:
    //   cd sim && bun run rngtest.ts  (importing src/core/rng.ts)
    // These are the source of truth for RNG parity across the rebaseline.

    #[test]
    fn next_uint_matches_ts_reference() {
        let mut r = Rng::from_seed(12345);
        let expected: [u32; 10] = [
            1093274547, 203003357, 3741353573, 3803725158, 4178738660, 810247443, 1347789520,
            4037788777, 3729597786, 3845877672,
        ];
        for &e in &expected {
            assert_eq!(r.next_uint(), e);
        }
    }

    #[test]
    fn next_f64_matches_ts_reference() {
        let mut r = Rng::from_seed(12345);
        let expected: [f64; 10] = [
            0.25454777479171753,
            0.04726535081863403,
            0.8711017370223999,
            0.8856237530708313,
            0.9729383587837219,
            0.18865042924880981,
            0.3138066530227661,
            0.9401209354400635,
            0.8683646321296692,
            0.8954381346702576,
        ];
        for &e in &expected {
            assert_eq!(r.next_f64(), e);
        }
    }

    #[test]
    fn initial_state_matches_ts_reference() {
        let r = Rng::from_seed(12345);
        assert_eq!(r.state(), [3283241497, 613117429, 2940958500, 516375437]);
    }

    #[test]
    fn state_after_one_draw_matches_ts_reference() {
        let mut r = Rng::from_seed(12345);
        r.next_uint();
        assert_eq!(r.state(), [4194198625, 1215451336, 2049103677, 1634976210]);
    }

    #[test]
    fn int_matches_ts_reference() {
        let mut r = Rng::from_seed(12345);
        let expected: [i64; 10] = [2, 1, 6, 6, 6, 2, 2, 6, 6, 6];
        for &e in &expected {
            assert_eq!(r.int(1, 6), e);
        }
    }

    #[test]
    fn save_restore_roundtrip() {
        let mut r = Rng::from_seed(999);
        r.next_uint();
        r.next_uint();
        let saved = r.state();
        let a = r.next_uint();
        let mut restored = Rng::from_state(saved);
        assert_eq!(restored.next_uint(), a);
    }

    #[test]
    fn all_zero_state_is_avoided() {
        // Seed selection here is not important; just assert the invariant holds
        // for a spread of seeds.
        for seed in 0..64u32 {
            let s = Rng::from_seed(seed).state();
            assert_ne!(s, [0, 0, 0, 0]);
        }
    }
}
