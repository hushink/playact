//! Deterministic driver for tests and simulations.
//!
//! [`TestDriver`] owns a tree and a world, advances a virtual clock, and offers
//! small helpers for stepping a tree until a terminal status. It does not sleep
//! or read the system clock, so a test can run in microseconds and reproduce
//! exact `Time` values.

use std::time::Duration;

use crate::blackboard::Blackboard;
use crate::status::Status;
use crate::time::Time;
use crate::tree::BehaviorTree;

/// A tree plus world and virtual time, driven one tick at a time.
pub struct TestDriver<W> {
    tree: BehaviorTree<W>,
    world: W,
    now: Duration,
    step: Duration,
}

impl<W> TestDriver<W> {
    /// Creates a driver with a 16 ms default step and time starting at zero.
    #[must_use]
    pub fn new(tree: BehaviorTree<W>, world: W) -> Self {
        Self {
            tree,
            world,
            now: Duration::ZERO,
            step: Duration::from_millis(16),
        }
    }

    /// Sets the default per-tick step used by [`TestDriver::tick`].
    #[must_use]
    pub fn with_step(mut self, step: Duration) -> Self {
        self.step = step;
        self
    }

    /// The default per-tick step.
    #[must_use]
    pub fn step(&self) -> Duration {
        self.step
    }

    /// Changes the default per-tick step.
    pub fn set_step(&mut self, step: Duration) {
        self.step = step;
    }

    /// The virtual time of the next tick.
    #[must_use]
    pub fn now(&self) -> Duration {
        self.now
    }

    /// The full [`Time`] value that the next tick will receive.
    #[must_use]
    pub fn time(&self) -> Time {
        Time::new(self.now, self.step)
    }

    /// Advances virtual time without ticking.
    pub fn advance(&mut self, delta: Duration) {
        self.now = self.now.saturating_add(delta);
    }

    /// Ticks at the current virtual time, then advances by the configured step.
    pub fn tick(&mut self) -> Status {
        let time = self.time();
        let status = self.tree.tick(&mut self.world, time);
        self.now = self.now.saturating_add(self.step);
        status
    }

    /// Ticks with an explicit [`Time`] value, then advances by the configured
    /// step from that time.
    pub fn tick_at(&mut self, time: Time) -> Status {
        let status = self.tree.tick(&mut self.world, time);
        self.now = time.now.saturating_add(self.step);
        status
    }

    /// Ticks `count` times and returns the statuses in order.
    #[must_use]
    pub fn tick_n(&mut self, count: usize) -> Vec<Status> {
        (0..count).map(|_| self.tick()).collect()
    }

    /// Ticks until `Success` or `Failure`.
    ///
    /// Returns the number of ticks (starting at 1) and the terminal status, or
    /// `None` if `max_ticks` is exhausted while the tree is still `Running`.
    pub fn run_until_terminal(&mut self, max_ticks: usize) -> Option<(usize, Status)> {
        for tick in 1..=max_ticks {
            let status = self.tick();
            if !status.is_running() {
                return Some((tick, status));
            }
        }
        None
    }

    /// Ticks until `target` is returned.
    ///
    /// Returns the tick number (starting at 1), or `None` if `max_ticks` is
    /// exhausted first.
    pub fn run_until_status(&mut self, target: Status, max_ticks: usize) -> Option<usize> {
        (1..=max_ticks).find(|_| self.tick() == target)
    }

    /// The underlying tree.
    #[must_use]
    pub fn tree(&self) -> &BehaviorTree<W> {
        &self.tree
    }

    /// Mutable access to the tree, for installing a Tracer or inspecting state.
    pub fn tree_mut(&mut self) -> &mut BehaviorTree<W> {
        &mut self.tree
    }

    /// The world.
    #[must_use]
    pub fn world(&self) -> &W {
        &self.world
    }

    /// Mutable access to the world between ticks.
    pub fn world_mut(&mut self) -> &mut W {
        &mut self.world
    }

    /// The tree's blackboard.
    #[must_use]
    pub fn blackboard(&self) -> &Blackboard {
        self.tree.blackboard()
    }

    /// Mutable access to the tree's blackboard between ticks.
    pub fn blackboard_mut(&mut self) -> &mut Blackboard {
        self.tree.blackboard_mut()
    }

    /// The most recent top-level status.
    #[must_use]
    pub fn last_status(&self) -> Option<Status> {
        self.tree.last_status()
    }

    /// Consumes the driver, returning the tree and world.
    #[must_use]
    pub fn into_parts(self) -> (BehaviorTree<W>, W) {
        (self.tree, self.world)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::BehaviorTreeBuilder;

    fn wait_tree() -> BehaviorTree<()> {
        BehaviorTreeBuilder::<()>::new("wait")
            .wait(Duration::from_millis(32))
            .build()
            .expect("tree")
    }

    #[test]
    fn tick_advances_virtual_time() {
        let mut driver = TestDriver::new(wait_tree(), ()).with_step(Duration::from_millis(16));

        assert_eq!(driver.tick(), Status::Running);
        assert_eq!(driver.now(), Duration::from_millis(16));
        assert_eq!(driver.tick(), Status::Running);
        assert_eq!(driver.now(), Duration::from_millis(32));
        assert_eq!(driver.tick(), Status::Success);
        assert_eq!(driver.now(), Duration::from_millis(48));
        assert_eq!(driver.last_status(), Some(Status::Success));
    }

    #[test]
    fn advance_changes_the_next_tick_time() {
        let mut driver = TestDriver::new(wait_tree(), ()).with_step(Duration::from_millis(16));
        assert_eq!(driver.tick(), Status::Running);
        driver.advance(Duration::from_millis(32));
        assert_eq!(driver.tick(), Status::Success);
    }

    #[test]
    fn run_until_terminal_and_status() {
        let mut driver = TestDriver::new(wait_tree(), ());
        assert_eq!(driver.run_until_terminal(10), Some((3, Status::Success)));

        let mut driver = TestDriver::new(wait_tree(), ());
        assert_eq!(driver.run_until_status(Status::Success, 10), Some(3));
        assert_eq!(driver.run_until_status(Status::Failure, 1), None);
    }

    #[test]
    fn tick_n_and_into_parts() {
        let mut driver = TestDriver::new(wait_tree(), ());
        let statuses = driver.tick_n(2);
        assert_eq!(statuses, vec![Status::Running, Status::Running]);

        let (tree, world) = driver.into_parts();
        assert_eq!(tree.name(), "wait");
        assert_eq!(world, ());
    }
}
