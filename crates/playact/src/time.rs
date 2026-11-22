//! Tick time.

use core::time::Duration;

/// The time supplied by the driver on every logic frame.
///
/// `now` should be monotonic simulation time or a monotonic system clock; the
/// Cooldown / Timeout / Wait nodes inside the tree compute from it rather than
/// reading the system clock directly, so tests can advance virtual time fully
/// deterministically.
///
/// If the driver resets `now` (restarting combat, zeroing the clock after a
/// reconnect), it must first call [`crate::BehaviorTree::reset`] or
/// [`crate::BehaviorTree::abort`], otherwise nodes based on absolute time will
/// observe time moving backwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Time {
    /// The current point in time.
    pub now: Duration,
    /// The duration since the previous logic frame.
    pub dt: Duration,
}

impl Time {
    /// Zero time.
    pub const ZERO: Self = Self {
        now: Duration::ZERO,
        dt: Duration::ZERO,
    };

    /// Construct a point in time.
    #[must_use]
    pub const fn new(now: Duration, dt: Duration) -> Self {
        Self { now, dt }
    }

    /// Construct from milliseconds, convenient for examples and tests.
    #[must_use]
    pub const fn from_millis(now_ms: u64, dt_ms: u64) -> Self {
        Self {
            now: Duration::from_millis(now_ms),
            dt: Duration::from_millis(dt_ms),
        }
    }
}
