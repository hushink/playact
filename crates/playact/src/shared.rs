//! 可跨行为树/线程共享的黑板。
//!
//! [`SharedBlackboard`] 是 `Arc<Mutex<Blackboard>>` 的薄封装。一次
//! [`SharedBlackboard::tick`] 会在整个树 tick 期间持有锁，因此：
//! - 多个树可以共享 typed key 数据；
//! - 不要在动作/条件内部再次 lock 同一个 `SharedBlackboard`（会死锁），
//!   直接用 `ctx.blackboard()` / `ctx.blackboard_ref()`；
//! - 多线程 tick 会被黑板锁串行化。

use std::sync::{Arc, Mutex, MutexGuard};

use crate::blackboard::Blackboard;
use crate::status::Status;
use crate::time::Time;
use crate::tree::BehaviorTree;

/// 可克隆、可跨线程共享的黑板。
///
/// # 示例
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
    /// 创建空共享黑板。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 用已有黑板创建共享黑板。
    #[must_use]
    pub fn from_board(board: Blackboard) -> Self {
        Self {
            board: Arc::new(Mutex::new(board)),
        }
    }

    /// 锁定黑板。若锁被 poison，会取回内部数据继续使用。
    pub fn lock(&self) -> MutexGuard<'_, Blackboard> {
        self.board
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// 在锁内只读执行闭包。
    pub fn with<R>(&self, f: impl FnOnce(&Blackboard) -> R) -> R {
        let board = self.lock();
        f(&board)
    }

    /// 在锁内可变执行闭包。
    pub fn with_mut<R>(&self, f: impl FnOnce(&mut Blackboard) -> R) -> R {
        let mut board = self.lock();
        f(&mut board)
    }

    /// 用共享黑板 tick 一棵树。
    ///
    /// 锁会覆盖整个 tick；树自带的黑板不参与本次 tick。
    pub fn tick<W>(&self, tree: &mut BehaviorTree<W>, world: &mut W, time: Time) -> Status {
        let mut board = self.lock();
        tree.tick_with_blackboard(world, time, &mut board)
    }

    /// 用共享黑板 abort 一棵树。
    pub fn abort<W>(&self, tree: &mut BehaviorTree<W>, world: &mut W, time: Time) {
        let mut board = self.lock();
        tree.abort_with_blackboard(world, time, &mut board);
    }

    /// 如果当前只剩这一个引用，取回黑板。
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

        // 树自带黑板没有被使用。
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
    fn into_inner_when_unique() {
        let shared = SharedBlackboard::new();
        shared.with_mut(|board| board.set(&SHARED_VALUE, 9));
        let board = shared.into_inner().expect("unique board");
        assert_eq!(board.get(&SHARED_VALUE), Some(&9));
    }
}
