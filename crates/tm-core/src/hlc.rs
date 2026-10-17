//! Hybrid logical clock (ADR 0006): one sortable integer, `ms << 16 | counter`.

/// How far ahead of wall time a pulled operation may push the local clock, and
/// how far past its server receive time an operation may rank in a conflict.
pub const MAX_DRIFT_MS: u64 = 5 * 60 * 1000;

/// The largest millisecond value: 47 bits keep every `Hlc` within an SQLite
/// (signed 64-bit) integer.
pub const MAX_MS: u64 = (1 << 47) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Hlc(u64);

impl Hlc {
    pub const ZERO: Self = Self(0);
    pub const MAX: Self = Self((MAX_MS << 16) | 0xFFFF);

    /// Saturates at `MAX_MS` rather than wrapping.
    #[must_use]
    pub const fn new(ms: u64, counter: u16) -> Self {
        let ms = if ms > MAX_MS { MAX_MS } else { ms };
        Self((ms << 16) | counter as u64)
    }

    /// Saturates at [`Hlc::MAX`] rather than wrapping.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        if raw > Self::MAX.0 {
            Self::MAX
        } else {
            Self(raw)
        }
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn ms(self) -> u64 {
        self.0 >> 16
    }

    #[must_use]
    pub const fn counter(self) -> u16 {
        (self.0 & 0xFFFF) as u16
    }

    /// The timestamp for a local event at wall time `now_ms`. Never moves
    /// backwards; a full counter rolls into the next millisecond so the
    /// integer stays ordered.
    #[must_use]
    pub const fn tick(self, now_ms: u64) -> Self {
        let now_ms = if now_ms > MAX_MS { MAX_MS } else { now_ms };
        if now_ms > self.ms() {
            Self::new(now_ms, 0)
        } else if self.0 == Self::MAX.0 {
            self
        } else {
            Self(self.0 + 1)
        }
    }

    /// The local clock after seeing `remote`: advanced to it, but never past
    /// `now_ms + MAX_DRIFT_MS` and never backwards.
    #[must_use]
    pub fn observe(self, remote: Self, now_ms: u64) -> Self {
        let cap = Self::new(now_ms.saturating_add(MAX_DRIFT_MS), u16::MAX);
        self.max(remote.min(cap))
    }

    /// The value compared in a concurrent conflict: capped at the server
    /// receive time plus `MAX_DRIFT_MS`.
    #[must_use]
    pub fn capped(self, server_received_ms: u64) -> Self {
        self.min(Self::new(
            server_received_ms.saturating_add(MAX_DRIFT_MS),
            u16::MAX,
        ))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #[test]
        fn tick_never_decreases(raw in any::<u64>(), now in any::<u64>()) {
            let before = Hlc::from_raw(raw);
            let after = before.tick(now);
            prop_assert!(after >= before);
            prop_assert!(after == Hlc::MAX || after > before);
        }

        #[test]
        fn observe_never_decreases(local in any::<u64>(), remote in any::<u64>(), now in any::<u64>()) {
            let local = Hlc::from_raw(local);
            prop_assert!(local.observe(Hlc::from_raw(remote), now) >= local);
        }
    }

    #[test]
    fn out_of_range_values_saturate_instead_of_wrapping() {
        assert_eq!(Hlc::new(MAX_MS + 1, 7), Hlc::new(MAX_MS, 7));
        assert_eq!(Hlc::from_raw(u64::MAX), Hlc::MAX);
        assert!(i64::try_from(Hlc::MAX.raw()).is_ok());
        assert_eq!(Hlc::MAX.tick(u64::MAX), Hlc::MAX);
        assert_eq!(Hlc::new(1, 0).observe(Hlc::MAX, u64::MAX), Hlc::MAX);
        assert_eq!(Hlc::MAX.capped(u64::MAX), Hlc::MAX);
    }

    #[test]
    fn tick_follows_wall_time() {
        assert_eq!(Hlc::ZERO.tick(1_000), Hlc::new(1_000, 0));
    }

    #[test]
    fn tick_never_moves_backwards() {
        let ahead = Hlc::new(5_000, 3);
        assert_eq!(ahead.tick(1_000), Hlc::new(5_000, 4));
    }

    #[test]
    fn full_counter_rolls_into_next_millisecond() {
        let full = Hlc::new(7, u16::MAX);
        let next = full.tick(7);
        assert_eq!(next, Hlc::new(8, 0));
        assert!(next > full);
    }

    #[test]
    fn encoding_sorts_like_the_pair() {
        assert!(Hlc::new(2, 0) > Hlc::new(1, u16::MAX));
        assert!(Hlc::new(1, 2) > Hlc::new(1, 1));
    }

    #[test]
    fn observe_advances_to_a_remote_clock_within_the_drift() {
        let remote = Hlc::new(60_000, 9);
        assert_eq!(Hlc::new(1_000, 0).observe(remote, 1_000), remote);
    }

    #[test]
    fn observe_caps_a_remote_clock_far_ahead() {
        let far = Hlc::new(1_000 + 10 * 3_600_000, 0);
        let seen = Hlc::new(1_000, 0).observe(far, 1_000);
        assert_eq!(seen, Hlc::new(1_000 + MAX_DRIFT_MS, u16::MAX));
    }

    #[test]
    fn observe_never_moves_backwards() {
        let local = Hlc::new(9_000, 1);
        assert_eq!(local.observe(Hlc::new(10, 0), 1_000), local);
    }

    #[test]
    fn capped_limits_a_fast_clock_and_keeps_an_honest_one() {
        let received = 1_000_000;
        let fast = Hlc::new(received + 10 * 3_600_000, 4);
        assert_eq!(fast.capped(received).ms(), received + MAX_DRIFT_MS);
        let honest = Hlc::new(received - 50, 4);
        assert_eq!(honest.capped(received), honest);
    }
}
