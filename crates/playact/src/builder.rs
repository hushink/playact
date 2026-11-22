//! 闭包作用域 + typestate 构建器。
//!
//! 根节点必须恰好一个子节点；装饰节点闭包必须恰好产生一个子节点。
//! 这些约束都由类型系统保证，不需要 `.end()`，也不会在运行时 panic。

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::marker::PhantomData;
use core::time::Duration;

#[cfg(feature = "std")]
use crate::async_action::{AsyncAction, AsyncTask};
use crate::blackboard::Key;
use crate::composite::{
    IfThenElse, MemParallel, Parallel, Race, RandomSelector, ReactiveSelector, ReactiveSequence,
    Selector, Sequence, Threshold, UtilityChild, UtilitySelector, WeightedChild,
    WeightedRandomSelector,
};
use crate::context::Context;
use crate::decorator::{
    AlwaysFail, AlwaysSucceed, Cooldown, Guard, Inverter, Limiter, Repeat, Retry, Throttle,
    Timeout, UntilFailure, UntilSuccess,
};
use crate::error::BuildError;
use crate::leaf::{Action, Condition, SetValue, Wait};
use crate::node::BoxedNode;
use crate::status::Status;
use crate::subtree::SubtreeFactory;
use crate::tree::BehaviorTree;

mod sealed {
    pub trait Sealed {}
}

/// 构建器状态标记的密封 trait。
#[doc(hidden)]
pub trait BuilderState: sealed::Sealed {
    /// 追加一个节点后的状态。
    type Next: BuilderState;
}

/// 还没有子节点。[`BuilderState`] 标记类型。
#[doc(hidden)]
pub struct Empty;

/// 恰好一个子节点。[`BuilderState`] 标记类型。
#[doc(hidden)]
pub struct One;

/// 两个或更多子节点。[`BuilderState`] 标记类型。
#[doc(hidden)]
pub struct Many;

impl sealed::Sealed for Empty {}
impl sealed::Sealed for One {}
impl sealed::Sealed for Many {}

impl BuilderState for Empty {
    type Next = One;
}

impl BuilderState for One {
    type Next = Many;
}

impl BuilderState for Many {
    type Next = Many;
}

struct Frame<W> {
    children: Vec<BoxedNode<W>>,
    error: Option<BuildError>,
}

impl<W> Frame<W> {
    fn new() -> Self {
        Self {
            children: Vec::new(),
            error: None,
        }
    }
}

/// 行为树构建器。
///
/// 根构建器用 [`BehaviorTreeBuilder::new`] 创建；闭包参数类型是
/// [`BranchBuilder`]。示例：
///
/// ```
/// use std::time::Duration;
/// use playact::{BehaviorTreeBuilder, Status};
///
/// let tree = BehaviorTreeBuilder::<()>::new("demo")
///     .reactive_selector(|b| {
///         b.sequence(|b| {
///             b.condition("ready", |_| true)
///                 .action("work", |_| Status::Success)
///         })
///         .action("idle", |_| Status::Running)
///     })
///     .build()
///     .unwrap();
///
/// assert_eq!(tree.name(), "demo");
/// ```
pub struct BehaviorTreeBuilder<W, S = Empty> {
    tree_name: Option<String>,
    frame: Frame<W>,
    _state: PhantomData<S>,
}

/// 闭包内部的分支构建器别名。
pub type BranchBuilder<W, S = Empty> = BehaviorTreeBuilder<W, S>;

impl<W: 'static> BehaviorTreeBuilder<W, Empty> {
    /// 创建一个根构建器。
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            tree_name: Some(name.into()),
            frame: Frame::new(),
            _state: PhantomData,
        }
    }

    fn branch() -> Self {
        Self {
            tree_name: None,
            frame: Frame::new(),
            _state: PhantomData,
        }
    }
}

impl<W: 'static, S> BehaviorTreeBuilder<W, S>
where
    S: BuilderState,
{
    fn push_node(self, node: BoxedNode<W>) -> BehaviorTreeBuilder<W, S::Next> {
        let Self {
            tree_name,
            mut frame,
            ..
        } = self;
        frame.children.push(node);
        BehaviorTreeBuilder {
            tree_name,
            frame,
            _state: PhantomData,
        }
    }

    fn set_error(mut self, error: Option<BuildError>) -> Self {
        if let Some(error) = error
            && self.frame.error.is_none()
        {
            self.frame.error = Some(error);
        }
        self
    }

    fn into_parts(self) -> (Vec<BoxedNode<W>>, Option<BuildError>) {
        (self.frame.children, self.frame.error)
    }

    fn into_single(mut self) -> (Option<BoxedNode<W>>, Option<BuildError>) {
        let child = self.frame.children.pop();
        let mut error = self.frame.error;
        if !self.frame.children.is_empty() && error.is_none() {
            error = Some(BuildError::InvalidTree {
                message: "decorator requires exactly one child",
            });
        }
        if child.is_none() && error.is_none() {
            error = Some(BuildError::InvalidTree {
                message: "decorator requires one child",
            });
        }
        (child, error)
    }

    // ---------- 叶子 ----------

    /// 追加一个闭包动作。
    pub fn action<F>(self, name: impl Into<String>, f: F) -> BehaviorTreeBuilder<W, S::Next>
    where
        F: for<'a> FnMut(&mut Context<'a, W>) -> Status + Send + 'static,
    {
        self.push_node(Box::new(Action::new(name, f)))
    }

    /// 追加一个异步/远程动作。
    #[cfg(feature = "std")]
    pub fn async_action<F>(
        self,
        name: impl Into<String>,
        start: F,
    ) -> BehaviorTreeBuilder<W, S::Next>
    where
        F: for<'a> FnMut(&mut Context<'a, W>) -> Box<dyn AsyncTask> + Send + 'static,
    {
        self.push_node(Box::new(AsyncAction::new(name, start)))
    }

    /// 追加一个只读条件。
    pub fn condition<F>(self, name: impl Into<String>, f: F) -> BehaviorTreeBuilder<W, S::Next>
    where
        F: for<'a> Fn(&Context<'a, W>) -> bool + Send + 'static,
    {
        self.push_node(Box::new(Condition::new(name, f)))
    }

    /// 追加一个等待叶子。
    pub fn wait(self, duration: Duration) -> BehaviorTreeBuilder<W, S::Next> {
        self.push_node(Box::new(Wait::new(duration)))
    }

    /// 追加一个向黑板写入固定值的叶子；每次 tick 都写入。
    pub fn set_value<T>(
        self,
        name: impl Into<String>,
        key: &Key<T>,
        value: T,
    ) -> BehaviorTreeBuilder<W, S::Next>
    where
        T: Clone + Send + 'static,
    {
        self.push_node(Box::new(SetValue::<W, T>::new(name, key, value)))
    }

    /// 追加一个已经构造好的自定义节点/子树。
    pub fn raw(self, node: BoxedNode<W>) -> BehaviorTreeBuilder<W, S::Next> {
        self.push_node(node)
    }

    /// 通过子树工厂追加一份全新实例化的子树。
    pub fn subtree(self, factory: &SubtreeFactory<W>) -> BehaviorTreeBuilder<W, S::Next> {
        self.push_node(factory.instantiate())
    }

    // ---------- 组合 ----------

    /// 打开一个记忆式顺序节点。
    pub fn sequence<S2>(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, S2>,
    ) -> BehaviorTreeBuilder<W, S::Next>
    where
        S2: BuilderState,
    {
        let branch = f(BehaviorTreeBuilder::branch());
        let (children, error) = branch.into_parts();
        self.push_node(Box::new(Sequence::new(children)))
            .set_error(error)
    }

    /// 打开一个反应式顺序节点。
    pub fn reactive_sequence<S2>(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, S2>,
    ) -> BehaviorTreeBuilder<W, S::Next>
    where
        S2: BuilderState,
    {
        let branch = f(BehaviorTreeBuilder::branch());
        let (children, error) = branch.into_parts();
        self.push_node(Box::new(ReactiveSequence::new(children)))
            .set_error(error)
    }

    /// 打开一个记忆式选择节点。
    pub fn selector<S2>(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, S2>,
    ) -> BehaviorTreeBuilder<W, S::Next>
    where
        S2: BuilderState,
    {
        let branch = f(BehaviorTreeBuilder::branch());
        let (children, error) = branch.into_parts();
        self.push_node(Box::new(Selector::new(children)))
            .set_error(error)
    }

    /// 打开一个反应式选择节点。
    pub fn reactive_selector<S2>(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, S2>,
    ) -> BehaviorTreeBuilder<W, S::Next>
    where
        S2: BuilderState,
    {
        let branch = f(BehaviorTreeBuilder::branch());
        let (children, error) = branch.into_parts();
        self.push_node(Box::new(ReactiveSelector::new(children)))
            .set_error(error)
    }

    /// 打开一个带记忆的随机选择节点。
    pub fn random_selector<S2>(
        self,
        seed: u64,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, S2>,
    ) -> BehaviorTreeBuilder<W, S::Next>
    where
        S2: BuilderState,
    {
        let branch = f(BehaviorTreeBuilder::branch());
        let (children, error) = branch.into_parts();
        self.push_node(Box::new(RandomSelector::new(children, seed)))
            .set_error(error)
    }

    /// 追加一个带权重的随机选择节点。
    ///
    /// builder 闭包无法表达每个子节点的权重，因此这里直接接收构造好的
    /// [`WeightedChild`]；需要闭包作用域时可先构造节点再用 `.raw(...)`。
    pub fn weighted_random_selector(
        self,
        children: Vec<WeightedChild<W>>,
        seed: u64,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.push_node(Box::new(WeightedRandomSelector::new(children, seed)))
    }

    /// 追加一个效用选择节点。
    ///
    /// 评分闭包无法用 builder 闭包表达，因此直接接收构造好的
    /// [`UtilityChild`]；需要 hysteresis 时可先构造节点再 `.raw(...)`。
    pub fn utility_selector(
        self,
        children: Vec<UtilityChild<W>>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.push_node(Box::new(UtilitySelector::new(children)))
    }

    /// 打开一个并行节点（单 tick 汇总）。
    pub fn parallel<S2>(
        self,
        success: Threshold,
        failure: Threshold,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, S2>,
    ) -> BehaviorTreeBuilder<W, S::Next>
    where
        S2: BuilderState,
    {
        let branch = f(BehaviorTreeBuilder::branch());
        let (children, mut error) = branch.into_parts();
        let node = Parallel::new(children, success, failure);
        if error.is_none() {
            error = node.validate().err();
        }
        self.push_node(Box::new(node)).set_error(error)
    }

    /// 打开一个跨 tick 累计终态的并行节点。
    pub fn mem_parallel<S2>(
        self,
        success: Threshold,
        failure: Threshold,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, S2>,
    ) -> BehaviorTreeBuilder<W, S::Next>
    where
        S2: BuilderState,
    {
        let branch = f(BehaviorTreeBuilder::branch());
        let (children, mut error) = branch.into_parts();
        let node = MemParallel::new(children, success, failure);
        if error.is_none() {
            error = node.validate().err();
        }
        self.push_node(Box::new(node)).set_error(error)
    }

    /// 打开一个条件分支节点；两个分支都必须恰好一个子节点。
    pub fn if_then_else<F, T, E>(
        self,
        condition: F,
        then_branch: T,
        else_branch: E,
    ) -> BehaviorTreeBuilder<W, S::Next>
    where
        F: for<'a> Fn(&Context<'a, W>) -> bool + Send + 'static,
        T: FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
        E: FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    {
        let then_branch = then_branch(BehaviorTreeBuilder::branch());
        let (then_child, mut error) = then_branch.into_single();
        let else_branch = else_branch(BehaviorTreeBuilder::branch());
        let (else_child, else_error) = else_branch.into_single();
        if error.is_none() {
            error = else_error;
        }

        let then_child = then_child.unwrap_or_else(|| {
            Box::new(Action::<W>::new("__invalid_then__", |_| Status::Failure)) as BoxedNode<W>
        });
        let else_child = else_child.unwrap_or_else(|| {
            Box::new(Action::<W>::new("__invalid_else__", |_| Status::Failure)) as BoxedNode<W>
        });

        self.push_node(Box::new(IfThenElse::new(condition, then_child, else_child)))
            .set_error(error)
    }

    /// 打开一个竞速节点：第一个返回终态的子节点获胜。
    pub fn race<S2>(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, S2>,
    ) -> BehaviorTreeBuilder<W, S::Next>
    where
        S2: BuilderState,
    {
        let branch = f(BehaviorTreeBuilder::branch());
        let (children, error) = branch.into_parts();
        self.push_node(Box::new(Race::new(children)))
            .set_error(error)
    }

    // ---------- 装饰 ----------

    fn decorator<M>(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
        make: M,
    ) -> BehaviorTreeBuilder<W, S::Next>
    where
        M: FnOnce(BoxedNode<W>) -> BoxedNode<W>,
    {
        let branch = f(BehaviorTreeBuilder::branch());
        let (child, mut error) = branch.into_single();

        let node = match child {
            Some(child) => make(child),
            None => {
                if error.is_none() {
                    error = Some(BuildError::InvalidTree {
                        message: "decorator requires one child",
                    });
                }
                Box::new(Action::<W>::new("__invalid_decorator__", |_| {
                    Status::Failure
                }))
            }
        };

        self.push_node(node).set_error(error)
    }

    /// 反转结果。
    pub fn inverter(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, |child| Box::new(Inverter::new(child)))
    }

    /// 终态恒为成功。
    pub fn always_succeed(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, |child| Box::new(AlwaysSucceed::new(child)))
    }

    /// 终态恒为失败。
    pub fn always_fail(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, |child| Box::new(AlwaysFail::new(child)))
    }

    /// 重复成功执行 `times` 次。
    pub fn repeat(
        self,
        times: core::num::NonZeroUsize,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, move |child| Box::new(Repeat::new(times, child)))
    }

    /// 失败最多重试 `max_attempts` 次，每 tick 一次尝试。
    pub fn retry(
        self,
        max_attempts: core::num::NonZeroUsize,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, move |child| Box::new(Retry::new(max_attempts, child)))
    }

    /// 直到成功。
    pub fn until_success(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, |child| Box::new(UntilSuccess::new(child)))
    }

    /// 直到失败。
    pub fn until_failure(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, |child| Box::new(UntilFailure::new(child)))
    }

    /// 条件守卫；条件为真才 tick 子节点。
    pub fn guard<F>(
        self,
        condition: F,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next>
    where
        F: for<'a> Fn(&Context<'a, W>) -> bool + Send + 'static,
    {
        self.decorator(f, move |child| Box::new(Guard::new(condition, child)))
    }

    /// 成功后冷却。
    pub fn cooldown(
        self,
        duration: Duration,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, move |child| Box::new(Cooldown::new(duration, child)))
    }

    /// 运行超时。
    pub fn timeout(
        self,
        duration: Duration,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, move |child| Box::new(Timeout::new(duration, child)))
    }

    /// 次数限制。
    pub fn limiter(
        self,
        max: core::num::NonZeroUsize,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, move |child| Box::new(Limiter::new(max, child)))
    }

    /// 时间窗口限流。
    pub fn throttle(
        self,
        window: Duration,
        max: core::num::NonZeroUsize,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, move |child| Box::new(Throttle::new(window, max, child)))
    }
}

impl<W: 'static> BehaviorTreeBuilder<W, One> {
    /// 构建行为树。
    pub fn build(mut self) -> Result<BehaviorTree<W>, BuildError> {
        if let Some(error) = self.frame.error {
            return Err(error);
        }

        let root = match self.frame.children.pop() {
            Some(root) => root,
            None => {
                return Err(BuildError::InvalidTree {
                    message: "empty behavior tree",
                });
            }
        };

        if !self.frame.children.is_empty() {
            return Err(BuildError::InvalidTree {
                message: "behavior tree must have exactly one root node",
            });
        }

        let name = self
            .tree_name
            .take()
            .unwrap_or_else(|| String::from("BehaviorTree"));

        Ok(BehaviorTree::new(root).with_name(name))
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroUsize;
    use core::time::Duration;

    use crate::async_action::AsyncTask;
    use crate::leaf::Action;
    use crate::subtree::SubtreeFactory;
    use crate::time::Time;

    use super::*;

    struct ImmediateTask;

    impl AsyncTask for ImmediateTask {
        fn poll(&mut self) -> Status {
            Status::Success
        }

        fn cancel(&mut self) {}
    }

    #[test]
    fn builds_nested_tree() {
        let mut tree = BehaviorTreeBuilder::<()>::new("demo")
            .reactive_selector(|b| {
                b.sequence(|b| {
                    b.condition("ready", |_| true)
                        .action("work", |_| Status::Success)
                })
                .action("idle", |_| Status::Running)
            })
            .build()
            .expect("tree should build");

        let mut world = ();
        assert_eq!(tree.name(), "demo");
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn decorators_accept_single_child() {
        let mut tree = BehaviorTreeBuilder::<()>::new("decorated")
            .cooldown(Duration::from_millis(100), |b| {
                b.action("work", |_| Status::Success)
            })
            .build()
            .expect("tree should build");

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        assert_eq!(
            tree.tick(&mut world, Time::from_millis(50, 50)),
            Status::Failure
        );
        assert_eq!(
            tree.tick(&mut world, Time::from_millis(150, 50)),
            Status::Success
        );
    }

    #[test]
    fn guard_builds() {
        let mut tree = BehaviorTreeBuilder::<bool>::new("guard")
            .guard(
                |ctx: &Context<'_, bool>| *ctx.world_ref(),
                |b| b.action("work", |_| Status::Success),
            )
            .build()
            .expect("tree");

        let mut world = false;
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Failure);
        world = true;
        assert_eq!(
            tree.tick(&mut world, Time::from_millis(16, 16)),
            Status::Success
        );
    }

    #[test]
    fn if_then_else_builds_both_branches() {
        let mut tree = BehaviorTreeBuilder::<bool>::new("branch")
            .if_then_else(
                |ctx: &Context<'_, bool>| *ctx.world_ref(),
                |b| b.action("then", |_| Status::Success),
                |b| b.action("else", |_| Status::Success),
            )
            .build()
            .expect("tree");

        let mut world = true;
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        world = false;
        assert_eq!(
            tree.tick(&mut world, Time::from_millis(16, 16)),
            Status::Success
        );
    }

    #[test]
    fn invalid_parallel_threshold_is_reported_at_build() {
        let result = BehaviorTreeBuilder::<()>::new("bad")
            .parallel(
                Threshold::AtLeast(NonZeroUsize::new(2).unwrap()),
                Threshold::Any,
                |b| b.action("only", |_| Status::Success),
            )
            .build();

        match result {
            Err(BuildError::InvalidSuccessThreshold { at_least, children }) => {
                assert_eq!(at_least.get(), 2);
                assert_eq!(children, 1);
            }
            _ => panic!("expected invalid success threshold"),
        }
    }

    #[test]
    fn mem_parallel_builds() {
        let mut tree = BehaviorTreeBuilder::<()>::new("mem_parallel")
            .mem_parallel(Threshold::All, Threshold::Any, |b| {
                b.action("only", |_| Status::Success)
            })
            .build()
            .expect("mem parallel tree");

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn async_action_builds() {
        let mut tree = BehaviorTreeBuilder::<()>::new("async")
            .async_action("now", |_| Box::new(ImmediateTask) as Box<dyn AsyncTask>)
            .build()
            .expect("async action tree");

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn subtree_factory_builds_fresh_instances() {
        let factory = SubtreeFactory::<()>::new(|| {
            Box::new(Action::<()>::new("leaf", |_| Status::Success)) as BoxedNode<()>
        });

        let mut tree = BehaviorTreeBuilder::<()>::new("subtree")
            .sequence(|b| b.subtree(&factory).subtree(&factory))
            .build()
            .expect("subtree tree");

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn random_selector_builds() {
        let mut tree = BehaviorTreeBuilder::<()>::new("random")
            .random_selector(7, |b| b.action("only", |_| Status::Success))
            .build()
            .expect("random selector tree");

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn empty_composite_is_allowed() {
        let mut tree = BehaviorTreeBuilder::<()>::new("empty")
            .sequence(|b| b)
            .build()
            .expect("empty sequence is valid");

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }
}
