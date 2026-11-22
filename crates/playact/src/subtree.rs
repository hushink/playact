//! 可重复实例化的子树工厂。

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;

use crate::blackboard::Key;
use crate::context::Context;
use crate::node::{BoxedNode, Kind, Node};
use crate::status::Status;
use crate::trace::AbortReason;

/// 一个可克隆、可重复实例化的子树工厂。
///
/// 每次 [`SubtreeFactory::instantiate`] 都调用一次工厂，创建一份全新的
/// 节点状态；因此同一个蓝图可以挂到多棵行为树或多个分支，而不会共享
/// Running/计时/黑板写入等内部状态。
///
/// ```ignore
/// let goblin = SubtreeFactory::new(|| build_goblin_root());
/// let root = SubtreeFactory::new(|| build_patrol_root());
///
/// let tree = BehaviorTreeBuilder::<World>::new("pair")
///     .sequence(|b| b.subtree(&goblin).subtree(&root))
///     .build()?;
/// ```
pub struct SubtreeFactory<W: 'static> {
    factory: Arc<dyn Fn() -> BoxedNode<W> + Send + Sync>,
}

impl<W: 'static> SubtreeFactory<W> {
    /// 用工厂闭包构造。
    pub fn new<F>(factory: F) -> Self
    where
        F: Fn() -> BoxedNode<W> + Send + Sync + 'static,
    {
        Self {
            factory: Arc::new(factory),
        }
    }

    /// 创建一份新的子树根节点。
    #[must_use]
    pub fn instantiate(&self) -> BoxedNode<W> {
        (self.factory)()
    }
}

impl<W: 'static> Clone for SubtreeFactory<W> {
    fn clone(&self) -> Self {
        Self {
            factory: Arc::clone(&self.factory),
        }
    }
}

impl<W: 'static> core::fmt::Debug for SubtreeFactory<W> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SubtreeFactory")
    }
}

type SlotFactory<W, K> = Box<dyn Fn(&K) -> BoxedNode<W> + Send>;

/// 根据黑板 typed key 动态切换子树的节点。
///
/// 每个 tick 读取 `Key<K>`：
/// - key 不变：继续 tick 现有子树，保留其内部状态；
/// - key 改变：旧子树 `abort → reset`，再用工厂创建新子树；
/// - key 缺失：放弃现有子树并返回 `Failure`。
///
/// 工厂接收当前 key，可据此返回不同子树，适合 Boss 阶段、任务阶段等
/// 运行时切换。`reset()` 会丢弃当前子树，下一次 tick 重新实例化。
pub struct SubtreeSlot<W, K: Send + 'static> {
    name: String,
    key: Key<K>,
    factory: SlotFactory<W, K>,
    active: Option<(K, BoxedNode<W>)>,
}

impl<W, K> SubtreeSlot<W, K>
where
    K: Clone + PartialEq + Send + 'static,
{
    /// 用显示名、key 和工厂构造。
    pub fn new(
        name: impl Into<String>,
        key: &Key<K>,
        factory: impl Fn(&K) -> BoxedNode<W> + Send + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            key: *key,
            factory: Box::new(factory),
            active: None,
        }
    }

    /// 覆盖显示名。
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    fn drop_active(&mut self, ctx: &mut Context<'_, W>) {
        if let Some((_, mut child)) = self.active.take() {
            ctx.abort_child(0, &mut child, AbortReason::Preempted);
        }
    }
}

impl<W, K> Node<W> for SubtreeSlot<W, K>
where
    K: Clone + PartialEq + Send + 'static,
{
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        let Some(desired) = ctx.blackboard_ref().get(&self.key).cloned() else {
            self.drop_active(ctx);
            return Status::Failure;
        };

        let matches = self
            .active
            .as_ref()
            .is_some_and(|(active, _)| *active == desired);

        if !matches {
            self.drop_active(ctx);
            let child = (self.factory)(&desired);
            self.active = Some((desired, child));
        }

        match self.active.as_mut() {
            Some((_, child)) => ctx.tick_child(0, child),
            None => Status::Failure,
        }
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        self.drop_active(ctx);
    }

    fn reset(&mut self) {
        if let Some((_, mut child)) = self.active.take() {
            child.reset();
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::SubtreeSlot
    }

    fn children(&self) -> &[BoxedNode<W>] {
        match &self.active {
            Some((_, child)) => core::slice::from_ref(child),
            None => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    use crate::blackboard::Blackboard;
    use crate::context::Context;
    use crate::leaf::Action;
    use crate::status::Status;
    use crate::time::Time;

    use super::*;

    #[test]
    fn instances_have_independent_state() {
        let counters: Arc<Mutex<Vec<Arc<AtomicU32>>>> = Arc::new(Mutex::new(Vec::new()));
        let counters_for_factory = Arc::clone(&counters);

        let factory = SubtreeFactory::<()>::new(move || {
            let counter = Arc::new(AtomicU32::new(0));
            counters_for_factory
                .lock()
                .expect("counters lock")
                .push(Arc::clone(&counter));

            Box::new(Action::<()>::new("count", move |_| {
                counter.fetch_add(1, Ordering::Relaxed);
                Status::Success
            })) as BoxedNode<()>
        });

        let mut first = factory.instantiate();
        let mut second = factory.instantiate();

        let mut world_a = ();
        let mut world_b = ();
        let mut board_a = Blackboard::new();
        let mut board_b = Blackboard::new();
        let mut ctx_a = Context::new(&mut world_a, &mut board_a, Time::ZERO);
        let mut ctx_b = Context::new(&mut world_b, &mut board_b, Time::ZERO);

        assert_eq!(first.tick(&mut ctx_a), Status::Success);
        assert_eq!(first.tick(&mut ctx_a), Status::Success);
        assert_eq!(second.tick(&mut ctx_b), Status::Success);

        let counters = counters.lock().expect("counters lock");
        assert_eq!(counters.len(), 2);
        assert_eq!(counters[0].load(Ordering::Relaxed), 2);
        assert_eq!(counters[1].load(Ordering::Relaxed), 1);
    }

    #[test]
    fn clone_can_create_more_instances() {
        let counters: Arc<Mutex<Vec<Arc<AtomicU32>>>> = Arc::new(Mutex::new(Vec::new()));
        let counters_for_factory = Arc::clone(&counters);

        let factory = SubtreeFactory::<()>::new(move || {
            let counter = Arc::new(AtomicU32::new(0));
            counters_for_factory
                .lock()
                .expect("counters lock")
                .push(Arc::clone(&counter));
            Box::new(Action::<()>::new("count", move |_| {
                counter.fetch_add(1, Ordering::Relaxed);
                Status::Success
            })) as BoxedNode<()>
        });

        let cloned = factory.clone();
        let _third = cloned.instantiate();
        assert_eq!(counters.lock().expect("counters lock").len(), 1);
    }

    #[test]
    fn slot_switches_on_key_change() {
        static KEY: Key<u8> = Key::new("test.slot_key");

        let one_calls = Arc::new(AtomicU32::new(0));
        let one_for_factory = Arc::clone(&one_calls);
        let two_calls = Arc::new(AtomicU32::new(0));
        let two_for_factory = Arc::clone(&two_calls);

        let mut slot = SubtreeSlot::new("slot", &KEY, move |key: &u8| -> BoxedNode<()> {
            match *key {
                1 => {
                    let calls = Arc::clone(&one_for_factory);
                    Box::new(Action::new("one", move |_| {
                        calls.fetch_add(1, Ordering::Relaxed);
                        Status::Success
                    }))
                }
                _ => {
                    let calls = Arc::clone(&two_for_factory);
                    Box::new(Action::new("two", move |_| {
                        calls.fetch_add(1, Ordering::Relaxed);
                        Status::Success
                    }))
                }
            }
        });

        let mut world = ();
        let mut board = Blackboard::new();
        board.set(&KEY, 1);

        {
            let mut ctx = Context::new(&mut world, &mut board, Time::ZERO);
            assert_eq!(slot.tick(&mut ctx), Status::Success);
            assert_eq!(slot.tick(&mut ctx), Status::Success);
        }
        assert_eq!(one_calls.load(Ordering::Relaxed), 2);
        assert_eq!(two_calls.load(Ordering::Relaxed), 0);

        board.set(&KEY, 2);
        {
            let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(16, 16));
            assert_eq!(slot.tick(&mut ctx), Status::Success);
        }
        assert_eq!(one_calls.load(Ordering::Relaxed), 2);
        assert_eq!(two_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn slot_aborts_running_child_when_key_changes() {
        static KEY: Key<u8> = Key::new("test.slot_abort_key");

        let aborts = Arc::new(AtomicU32::new(0));
        let aborts_for_factory = Arc::clone(&aborts);

        let mut slot = SubtreeSlot::new("slot", &KEY, move |key: &u8| -> BoxedNode<()> {
            match *key {
                1 => {
                    let aborts = Arc::clone(&aborts_for_factory);
                    Box::new(
                        Action::new("hold", |_| Status::Running).with_abort(move |_| {
                            aborts.fetch_add(1, Ordering::Relaxed);
                        }),
                    )
                }
                _ => Box::new(Action::new("next", |_| Status::Success)),
            }
        });

        let mut world = ();
        let mut board = Blackboard::new();
        board.set(&KEY, 1);

        {
            let mut ctx = Context::new(&mut world, &mut board, Time::ZERO);
            assert_eq!(slot.tick(&mut ctx), Status::Running);
        }
        assert_eq!(aborts.load(Ordering::Relaxed), 0);

        board.set(&KEY, 2);
        {
            let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(16, 16));
            assert_eq!(slot.tick(&mut ctx), Status::Success);
        }
        assert_eq!(aborts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn slot_missing_key_fails_and_recreates_later() {
        static KEY: Key<u8> = Key::new("test.slot_missing_key");

        let mut slot = SubtreeSlot::new("slot", &KEY, |_key: &u8| -> BoxedNode<()> {
            Box::new(Action::new("ok", |_| Status::Success))
        });

        let mut world = ();
        let mut board = Blackboard::new();
        board.set(&KEY, 1);

        {
            let mut ctx = Context::new(&mut world, &mut board, Time::ZERO);
            assert_eq!(slot.tick(&mut ctx), Status::Success);
        }

        board.remove(&KEY);
        {
            let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(16, 16));
            assert_eq!(slot.tick(&mut ctx), Status::Failure);
        }

        board.set(&KEY, 1);
        {
            let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(32, 16));
            assert_eq!(slot.tick(&mut ctx), Status::Success);
        }
    }

    #[test]
    fn slot_keeps_subtree_while_key_is_stable() {
        static KEY: Key<u8> = Key::new("test.slot_stable_key");

        let builds = Arc::new(AtomicU32::new(0));
        let builds_for_factory = Arc::clone(&builds);

        let mut slot = SubtreeSlot::new("slot", &KEY, move |_key: &u8| -> BoxedNode<()> {
            builds_for_factory.fetch_add(1, Ordering::Relaxed);
            Box::new(Action::new("ok", |_| Status::Success))
        });

        let mut world = ();
        let mut board = Blackboard::new();
        board.set(&KEY, 7);

        {
            let mut ctx = Context::new(&mut world, &mut board, Time::ZERO);
            assert_eq!(slot.tick(&mut ctx), Status::Success);
            assert_eq!(slot.tick(&mut ctx), Status::Success);
        }
        assert_eq!(builds.load(Ordering::Relaxed), 1);

        slot.reset();
        {
            let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(16, 16));
            assert_eq!(slot.tick(&mut ctx), Status::Success);
        }
        assert_eq!(builds.load(Ordering::Relaxed), 2);
    }
}
