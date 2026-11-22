//! 闭包作用域 + typestate 构建器。
//!
//! 根节点必须恰好一个子节点；装饰节点闭包必须恰好产生一个子节点。
//! 这些约束都由类型系统保证，不需要 `.end()`，也不会在运行时 panic。

use core::marker::PhantomData;
use core::time::Duration;

use crate::composite::{
    Parallel, ReactiveSelector, ReactiveSequence, Selector, Sequence, Threshold,
};
use crate::context::Context;
use crate::decorator::{
    AlwaysFail, AlwaysSucceed, Cooldown, Inverter, Limiter, Repeat, Retry, Timeout, UntilFailure,
    UntilSuccess,
};
use crate::error::BuildError;
use crate::leaf::{Action, Condition, Wait};
use crate::node::BoxedNode;
use crate::status::Status;
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

    /// 追加一个已经构造好的自定义节点/子树。
    pub fn raw(self, node: BoxedNode<W>) -> BehaviorTreeBuilder<W, S::Next> {
        self.push_node(node)
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

    /// 打开一个并行节点。
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

    use crate::time::Time;

    use super::*;

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
    fn empty_composite_is_allowed() {
        let mut tree = BehaviorTreeBuilder::<()>::new("empty")
            .sequence(|b| b)
            .build()
            .expect("empty sequence is valid");

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }
}
