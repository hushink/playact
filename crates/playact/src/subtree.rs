//! 可重复实例化的子树工厂。

use alloc::sync::Arc;

use crate::node::BoxedNode;

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
}
