//! Virtual time and per-node clocks.
//!
//! Global (true) time is the simulation's authoritative timeline. Each node
//! reads time through its own [`Clock`], which may be offset (skew) and tick at
//! a slightly different rate (drift). This is what makes lease safety arguments
//! observable — see `docs/design/algorithm.md`.

/// Global simulation time, in integer ticks. Authoritative across all nodes.
pub type Time = i64;

/// A node-local clock: `local = offset + drift * global`.
///
/// `offset` models arbitrary clock skew (cancels out in lease math). `drift`
/// models rate mismatch (must stay within the lease's `t_delta` budget).
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    /// Constant skew added to local readings, in ticks.
    pub offset: Time,
    /// Tick-rate multiplier; `1.0` means perfectly in step with global time.
    pub drift: f64,
}

impl Clock {
    /// A clock perfectly aligned with global time.
    pub fn perfect() -> Self {
        Self {
            offset: 0,
            drift: 1.0,
        }
    }

    /// Construct with a given skew and drift.
    pub fn new(offset: Time, drift: f64) -> Self {
        Self { offset, drift }
    }

    /// Local reading at a given global time.
    pub fn local(&self, global: Time) -> Time {
        self.offset + (self.drift * global as f64).round() as Time
    }

    /// Global time at which this clock reads `local` — inverse of [`local`].
    ///
    /// [`local`]: Clock::local
    pub fn global_for_local(&self, local: Time) -> Time {
        ((local - self.offset) as f64 / self.drift).round() as Time
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::perfect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perfect_clock_is_identity() {
        let c = Clock::perfect();
        assert_eq!(c.local(1000), 1000);
        assert_eq!(c.global_for_local(1000), 1000);
    }

    #[test]
    fn offset_shifts_reading() {
        let c = Clock::new(500, 1.0);
        assert_eq!(c.local(1000), 1500);
        assert_eq!(c.global_for_local(1500), 1000);
    }

    #[test]
    fn drift_scales_rate() {
        let c = Clock::new(0, 1.1);
        assert_eq!(c.local(1000), 1100);
    }

    #[test]
    fn local_and_inverse_roundtrip() {
        let c = Clock::new(123, 1.05);
        let g = 4242;
        // Round-trip should land within a tick of rounding error.
        assert!((c.global_for_local(c.local(g)) - g).abs() <= 1);
    }
}
