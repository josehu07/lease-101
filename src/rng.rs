//! Deterministic, dependency-free PRNG (`xoshiro256**`). Seedable so every
//! simulation run is reproducible; avoids pulling `rand`/`getrandom` into wasm.

/// A small, fast, seedable pseudo-random generator.
#[derive(Debug, Clone)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    /// Seed the generator. Any seed is valid; the seed is expanded via
    /// SplitMix64 so even `0` produces a well-distributed state.
    pub fn new(seed: u64) -> Self {
        let mut sm = seed;
        let mut next = || {
            sm = sm.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = sm;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        Self {
            s: [next(), next(), next(), next()],
        }
    }

    /// Next raw 64-bit value.
    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform `f64` in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        // Top 53 bits give a uniform double in [0, 1).
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform integer in `[lo, hi]` inclusive. Returns `lo` if `hi <= lo`.
    pub fn next_range(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        let span = (hi - lo + 1) as u64;
        lo + (self.next_u64() % span) as i64
    }

    /// Bernoulli trial: `true` with probability `p` (clamped to `[0, 1]`).
    pub fn chance(&mut self, p: f64) -> bool {
        self.next_f64() < p.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reproducible_from_seed() {
        let a: Vec<u64> = {
            let mut r = Rng::new(42);
            (0..16).map(|_| r.next_u64()).collect()
        };
        let b: Vec<u64> = {
            let mut r = Rng::new(42);
            (0..16).map(|_| r.next_u64()).collect()
        };
        assert_eq!(a, b);
    }

    #[test]
    fn different_seeds_differ() {
        let mut r1 = Rng::new(1);
        let mut r2 = Rng::new(2);
        assert_ne!(r1.next_u64(), r2.next_u64());
    }

    #[test]
    fn f64_in_unit_interval() {
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            let x = r.next_f64();
            assert!((0.0..1.0).contains(&x));
        }
    }

    #[test]
    fn range_within_bounds() {
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            let x = r.next_range(-5, 5);
            assert!((-5..=5).contains(&x));
        }
        assert_eq!(r.next_range(3, 3), 3);
        assert_eq!(r.next_range(9, 2), 9);
    }
}
