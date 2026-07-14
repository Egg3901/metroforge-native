//! Deterministic state hashing for the Rust sim baseline.
//!
//! This is a NEW baseline (see PORT.md): the Rust sim defines fresh golden
//! stateHashes and does not attempt to match the TypeScript `stateHash`
//! output. The only requirements are that hashing be:
//!   1. deterministic  (same field stream -> same hash, every run),
//!   2. order-sensitive (fields must be fed in a fixed, documented order),
//!   3. platform-stable (no wall-clock, no HashMap iteration order, no float
//!      NaN ambiguity beyond what the caller controls).
//!
//! Algorithm: FNV-1a, 64-bit. Chosen because it is tiny, dependency-free,
//! byte-exact across platforms, and well specified (offset basis
//! 0xcbf29ce484222325, prime 0x100000001b3). It is NOT cryptographic; we only
//! need a stable fingerprint for determinism regression tests, not collision
//! resistance against an adversary.

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Incremental FNV-1a 64-bit hasher. State structs feed their fields in a
/// fixed order via the `write_*` / `Hashable` helpers, then read `finish()`.
#[derive(Clone, Debug)]
pub struct StateHasher {
    state: u64,
}

impl Default for StateHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl StateHasher {
    pub fn new() -> Self {
        Self { state: FNV_OFFSET }
    }

    /// Mix in a single byte.
    #[inline]
    pub fn write_u8(&mut self, b: u8) {
        self.state ^= b as u64;
        self.state = self.state.wrapping_mul(FNV_PRIME);
    }

    /// Mix in a byte slice (little-endian field encodings feed through here).
    #[inline]
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u8(b);
        }
    }

    #[inline]
    pub fn write_u32(&mut self, v: u32) {
        self.write_bytes(&v.to_le_bytes());
    }

    #[inline]
    pub fn write_u64(&mut self, v: u64) {
        self.write_bytes(&v.to_le_bytes());
    }

    #[inline]
    pub fn write_i64(&mut self, v: i64) {
        self.write_bytes(&v.to_le_bytes());
    }

    /// Mix in an f64 by its raw IEEE-754 bits. Callers are responsible for not
    /// feeding uncanonicalized NaNs (the sim produces none in hashed paths).
    #[inline]
    pub fn write_f64(&mut self, v: f64) {
        self.write_u64(v.to_bits());
    }

    /// Feed any `Hashable` value.
    #[inline]
    pub fn write<H: Hashable + ?Sized>(&mut self, value: &H) {
        value.hash_into(self);
    }

    /// Finalize and read the 64-bit fingerprint.
    #[inline]
    pub fn finish(&self) -> u64 {
        self.state
    }
}

/// A value that can feed itself into a [`StateHasher`] in a fixed field order.
/// State structs implement this so the tick loop and tests can fingerprint
/// them without duplicating the field order in multiple places.
pub trait Hashable {
    fn hash_into(&self, h: &mut StateHasher);
}

impl Hashable for u32 {
    fn hash_into(&self, h: &mut StateHasher) {
        h.write_u32(*self);
    }
}

impl Hashable for u64 {
    fn hash_into(&self, h: &mut StateHasher) {
        h.write_u64(*self);
    }
}

impl Hashable for i64 {
    fn hash_into(&self, h: &mut StateHasher) {
        h.write_i64(*self);
    }
}

impl Hashable for f64 {
    fn hash_into(&self, h: &mut StateHasher) {
        h.write_f64(*self);
    }
}

impl<H: Hashable> Hashable for [H] {
    fn hash_into(&self, h: &mut StateHasher) {
        // length-prefix so [a] and [a, 0] cannot collide
        h.write_u64(self.len() as u64);
        for item in self {
            item.hash_into(h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_same_input_same_hash() {
        let mut a = StateHasher::new();
        let mut b = StateHasher::new();
        for v in [1u32, 2, 3] {
            a.write_u32(v);
            b.write_u32(v);
        }
        assert_eq!(a.finish(), b.finish());
    }

    #[test]
    fn order_sensitive() {
        let mut a = StateHasher::new();
        a.write_u32(1);
        a.write_u32(2);
        let mut b = StateHasher::new();
        b.write_u32(2);
        b.write_u32(1);
        assert_ne!(a.finish(), b.finish());
    }

    #[test]
    fn empty_is_offset_basis() {
        assert_eq!(StateHasher::new().finish(), FNV_OFFSET);
    }

    #[test]
    fn known_fnv1a_vector() {
        // FNV-1a 64 of the ASCII bytes "a" is a published constant.
        let mut h = StateHasher::new();
        h.write_u8(b'a');
        assert_eq!(h.finish(), 0xaf63dc4c8601ec8c);
    }

    #[test]
    fn slice_length_prefixed() {
        let mut a = StateHasher::new();
        a.write(&[1u32][..]);
        let mut b = StateHasher::new();
        b.write(&[1u32, 0u32][..]);
        assert_ne!(a.finish(), b.finish());
    }
}
