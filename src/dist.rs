//! Sampleable distributions for configuring delays and jitter.

use crate::clock::Time;
use crate::rng::Rng;

/// A distribution over non-negative tick durations. Samples are clamped to
/// `>= 0` so they are always valid delays.
#[derive(Debug, Clone, Copy)]
pub enum Dist {
    /// Always the same value.
    Fixed(Time),
    /// Uniform over `[lo, hi]` inclusive.
    Uniform { lo: Time, hi: Time },
    /// Gaussian with the given mean and standard deviation (in ticks).
    Normal { mean: f64, std: f64 },
}

impl Dist {
    /// Draw one sample, clamped to be non-negative.
    pub fn sample(&self, rng: &mut Rng) -> Time {
        let raw = match *self {
            Dist::Fixed(v) => v,
            Dist::Uniform { lo, hi } => rng.next_range(lo, hi),
            Dist::Normal { mean, std } => (mean + std * standard_normal(rng)).round() as Time,
        };
        raw.max(0)
    }
}

/// One draw from a standard normal via the Box-Muller transform.
fn standard_normal(rng: &mut Rng) -> f64 {
    // Guard u1 away from 0 so ln() is finite.
    let u1 = rng.next_f64().max(f64::MIN_POSITIVE);
    let u2 = rng.next_f64();
    (-2.0 * u1.ln()).sqrt() * (core::f64::consts::TAU * u2).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_is_constant() {
        let mut rng = Rng::new(1);
        let d = Dist::Fixed(50);
        for _ in 0..100 {
            assert_eq!(d.sample(&mut rng), 50);
        }
    }

    #[test]
    fn uniform_within_bounds() {
        let mut rng = Rng::new(1);
        let d = Dist::Uniform { lo: 10, hi: 20 };
        for _ in 0..1000 {
            let v = d.sample(&mut rng);
            assert!((10..=20).contains(&v));
        }
    }

    #[test]
    fn normal_is_non_negative_and_roughly_centered() {
        let mut rng = Rng::new(1);
        let d = Dist::Normal {
            mean: 100.0,
            std: 15.0,
        };
        let n = 10_000;
        let sum: i64 = (0..n).map(|_| d.sample(&mut rng)).sum();
        let avg = sum as f64 / n as f64;
        assert!((avg - 100.0).abs() < 3.0, "avg was {avg}");
    }

    #[test]
    fn negative_samples_clamped() {
        let mut rng = Rng::new(1);
        let d = Dist::Normal {
            mean: 0.0,
            std: 100.0,
        };
        for _ in 0..1000 {
            assert!(d.sample(&mut rng) >= 0);
        }
    }
}
