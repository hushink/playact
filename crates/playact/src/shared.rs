//! A blackboard that can be shared across behavior trees and threads.
//!
//! [`SharedBlackboard`] is a thin wrapper around `Arc<Mutex<Blackboard>>`. A call
//! to [`SharedBlackboard::tick`] holds the lock for the entire tree tick, so:
//! - multiple trees can share typed-key data;
//! - do not lock the same `SharedBlackboard` again inside actions/conditions (it
//!   deadlocks); use `ctx.blackboard()` / `ctx.blackboard_ref()` instead;
//! - ticks from multiple threads are serialized by the blackboard lock.

use std::sync::{Arc, Mutex, MutexGuard, TryLockError};

use crate::blackboard::Blackboard;
use crate::status::Status;
use crate::time::Time;
use crate::tree::BehaviorTree;

/// A cloneable blackboard that can be shared across threads.
///
/// # Example
///
/// ```
/// use playact::prelude::*;
///
/// static HP: Key<u32> = playact::key!("hp");
///
/// let shared = SharedBlackboard::new();
///
/// let mut writer = BehaviorTree::new(Box::new(Action::<()>::new("write", |ctx| {
///     ctx.blackboard().set(&HP, 10);
///     Status::Success
/// })));
/// let mut reader = BehaviorTree::new(Box::new(Condition::<()>::new("read", |ctx| {
///     ctx.blackboard_ref().get(&HP) == Some(&10)
/// })));
///
/// let mut world = ();
/// assert_eq!(shared.tick(&mut writer, &mut world, Time::ZERO), Status::Success);
/// assert_eq!(shared.tick(&mut reader, &mut world, Time::ZERO), Status::Success);
/// ```
#[derive(Clone, Default)]
pub struct SharedBlackboard {
    board: Arc<Mutex<Blackboard>>,
}

impl SharedBlackboard {
    /// Create an empty shared blackboard.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a shared blackboard from an existing blackboard.
    #[must_use]
    pub fn from_board(board: Blackboard) -> Self {
        Self {
            board: Arc::new(Mutex::new(board)),
        }
    }

    /// Lock the blackboard. If the lock is poisoned, the inner data is recovered
    /// and used.
    pub fn lock(&self) -> MutexGuard<'_, Blackboard> {
        self.board
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Try to lock the blackboard; returns `None` when the lock is already held.
    ///
    /// As with [`SharedBlackboard::lock`], a poisoned lock recovers the inner
    /// data and keeps using it.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, Blackboard>> {
        match self.board.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::Poisoned(poison)) => Some(poison.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    /// Run a closure with read-only access while holding the lock.
    pub fn with<R>(&self, f: impl FnOnce(&Blackboard) -> R) -> R {
        let board = self.lock();
        f(&board)
    }

    /// Run a closure with mutable access while holding the lock.
    pub fn with_mut<R>(&self, f: impl FnOnce(&mut Blackboard) -> R) -> R {
        let mut board = self.lock();
        f(&mut board)
    }

    /// Tick a tree with the shared blackboard.
    ///
    /// The lock covers the whole tick; the tree's own blackboard is not used for
    /// this tick.
    pub fn tick<W>(&self, tree: &mut BehaviorTree<W>, world: &mut W, time: Time) -> Status {
        let mut board = self.lock();
        tree.tick_with_blackboard(world, time, &mut board)
    }

    /// Abort a tree with the shared blackboard.
    pub fn abort<W>(&self, tree: &mut BehaviorTree<W>, world: &mut W, time: Time) {
        let mut board = self.lock();
        tree.abort_with_blackboard(world, time, &mut board);
    }

    /// Recover the blackboard if this is the only remaining reference.
    #[must_use]
    pub fn into_inner(self) -> Option<Blackboard> {
        Arc::try_unwrap(self.board).ok().map(|mutex| {
            mutex
                .into_inner()
                .unwrap_or_else(|poison| poison.into_inner())
        })
    }
}

impl core::fmt::Debug for SharedBlackboard {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SharedBlackboard").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::blackboard::Key;
    use crate::leaf::{Action, Condition};

    use super::*;

    static SHARED_VALUE: Key<u32> = crate::key!("shared_value");

    #[test]
    fn trees_share_typed_blackboard_data() {
        let shared = SharedBlackboard::new();

        let mut writer = BehaviorTree::new(Box::new(Action::<()>::new("write", |ctx| {
            ctx.blackboard().set(&SHARED_VALUE, 42);
            Status::Success
        })));
        let mut reader = BehaviorTree::new(Box::new(Condition::<()>::new("read", |ctx| {
            ctx.blackboard_ref().get(&SHARED_VALUE).copied() == Some(42)
        })));

        let mut world = ();
        assert_eq!(
            shared.tick(&mut writer, &mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(
            shared.tick(&mut reader, &mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(
            shared.with(|board| board.get(&SHARED_VALUE).copied()),
            Some(42)
        );

        // The tree's own blackboard was not used.
        assert!(writer.blackboard().is_empty());
        assert!(reader.blackboard().is_empty());
    }

    #[test]
    fn shared_blackboard_is_cloneable_across_threads() {
        let shared = SharedBlackboard::new();
        let cloned = shared.clone();
        let writes = Arc::new(AtomicU32::new(0));
        let writes_in_thread = Arc::clone(&writes);

        let handle = std::thread::spawn(move || {
            cloned.with_mut(|board| {
                board.set(&SHARED_VALUE, 7);
            });
            writes_in_thread.fetch_add(1, Ordering::Relaxed);
        });

        handle.join().expect("thread");
        assert_eq!(writes.load(Ordering::Relaxed), 1);
        assert_eq!(
            shared.with(|board| board.get(&SHARED_VALUE).copied()),
            Some(7)
        );
    }

    #[test]
    fn try_lock_reports_contention() {
        let shared = SharedBlackboard::new();
        let guard = shared.lock();
        assert!(shared.try_lock().is_none());
        drop(guard);
        assert!(shared.try_lock().is_some());
    }

    #[test]
    fn into_inner_when_unique() {
        let shared = SharedBlackboard::new();
        shared.with_mut(|board| board.set(&SHARED_VALUE, 9));
        let board = shared.into_inner().expect("unique board");
        assert_eq!(board.get(&SHARED_VALUE), Some(&9));
    }
}
