//! A closure-scoped, typestate-based builder.
//!
//! A root must have exactly one child, and a decorator closure must produce
//! exactly one child. The type system enforces these constraints, so no
//! `.end()` calls are needed and nothing panics at runtime.

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
use crate::event::EventKey;
use crate::leaf::{Action, Condition, SetValue, TryAction, Wait, WaitForEvent, WaitUntil};
use crate::node::{BoxedNode, IdentifiedNode, NodeId};
use crate::status::Status;
use crate::subtree::SubtreeFactory;
use crate::tree::BehaviorTree;

mod sealed {
    pub trait Sealed {}
}

/// Sealed trait for builder state markers.
#[doc(hidden)]
pub trait BuilderState: sealed::Sealed {
    /// The state after appending one node.
    type Next: BuilderState;
}

/// No children yet. A [`BuilderState`] marker type.
#[doc(hidden)]
pub struct Empty;

/// Exactly one child. A [`BuilderState`] marker type.
#[doc(hidden)]
pub struct One;

/// Two or more children. A [`BuilderState`] marker type.
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

/// A behavior tree builder.
///
/// Create the root builder with [`BehaviorTreeBuilder::new`]; the closure
/// parameter type is [`BranchBuilder`]. Example:
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

/// Alias for the branch builder used inside closures.
pub type BranchBuilder<W, S = Empty> = BehaviorTreeBuilder<W, S>;

impl<W: 'static> BehaviorTreeBuilder<W, Empty> {
    /// Creates a root builder.
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

    // ---------- Leaves ----------

    /// Appends a closure action.
    pub fn action<F>(self, name: impl Into<String>, f: F) -> BehaviorTreeBuilder<W, S::Next>
    where
        F: for<'a> FnMut(&mut Context<'a, W>) -> Status + Send + 'static,
    {
        self.push_node(Box::new(Action::new(name, f)))
    }

    /// Appends a fallible action.
    ///
    /// On `Err`, the error is reported through the installed tracer and the
    /// node returns `Failure`.
    pub fn try_action<F, E>(self, name: impl Into<String>, f: F) -> BehaviorTreeBuilder<W, S::Next>
    where
        F: for<'a> FnMut(&mut Context<'a, W>) -> Result<Status, E> + Send + 'static,
        E: core::fmt::Display + Send + 'static,
    {
        self.push_node(Box::new(TryAction::new(name, f)))
    }

    /// Appends an async/remote action.
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

    /// Appends a read-only condition.
    pub fn condition<F>(self, name: impl Into<String>, f: F) -> BehaviorTreeBuilder<W, S::Next>
    where
        F: for<'a> Fn(&Context<'a, W>) -> bool + Send + 'static,
    {
        self.push_node(Box::new(Condition::new(name, f)))
    }

    /// Appends a wait leaf.
    pub fn wait(self, duration: Duration) -> BehaviorTreeBuilder<W, S::Next> {
        self.push_node(Box::new(Wait::new(duration)))
    }

    /// Appends a leaf that returns `Running` until the predicate becomes true.
    pub fn wait_until<F>(
        self,
        name: impl Into<String>,
        predicate: F,
    ) -> BehaviorTreeBuilder<W, S::Next>
    where
        F: for<'a> Fn(&Context<'a, W>) -> bool + Send + 'static,
    {
        self.push_node(Box::new(WaitUntil::new(name, predicate)))
    }

    /// Appends a leaf that waits for the next event of a typed event queue.
    pub fn wait_for_event<T>(self, key: EventKey<T>) -> BehaviorTreeBuilder<W, S::Next>
    where
        T: Send + 'static,
    {
        self.push_node(Box::new(WaitForEvent::new(key)))
    }

    /// Appends a leaf that writes a fixed value to the blackboard on every
    /// tick.
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

    /// Appends a pre-built custom node or subtree.
    pub fn raw(self, node: BoxedNode<W>) -> BehaviorTreeBuilder<W, S::Next> {
        self.push_node(node)
    }

    /// Assigns a stable identity to the most recently appended node.
    ///
    /// The identity is metadata only; it does not change tick behavior. It is
    /// preserved by [`crate::TreeDescription`] and is intended for snapshots,
    /// metrics, and tooling that must recognize a node across tree edits.
    ///
    /// ```
    /// use playact::{BehaviorTreeBuilder, Status};
    ///
    /// let tree = BehaviorTreeBuilder::<()>::new("demo")
    ///     .action("work", |_| Status::Success)
    ///     .id("worker")
    ///     .build()
    ///     .unwrap();
    ///
    /// assert_eq!(
    ///     tree.describe().id.as_ref().map(|id| id.as_str()),
    ///     Some("worker")
    /// );
    /// ```
    pub fn id(mut self, id: impl Into<NodeId>) -> Self {
        match self.frame.children.pop() {
            Some(node) => self
                .frame
                .children
                .push(Box::new(IdentifiedNode::new(id, node))),
            None if self.frame.error.is_none() => {
                self.frame.error = Some(BuildError::InvalidTree {
                    message: "id requires a preceding node",
                });
            }
            None => {}
        }
        self
    }

    /// Appends a freshly instantiated subtree from a subtree factory.
    pub fn subtree(self, factory: &SubtreeFactory<W>) -> BehaviorTreeBuilder<W, S::Next> {
        self.push_node(factory.instantiate())
    }

    // ---------- Composites ----------

    /// Opens a memory sequence node.
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

    /// Opens a reactive sequence node.
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

    /// Opens a memory selector node.
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

    /// Opens a reactive selector node.
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

    /// Opens a random selector node with memory.
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

    /// Appends a weighted random selector node.
    ///
    /// A builder closure cannot express per-child weights, so this takes
    /// pre-built [`WeightedChild`] values directly; when closure scoping is
    /// needed, build the node first and attach it with `.raw(...)`.
    pub fn weighted_random_selector(
        self,
        children: Vec<WeightedChild<W>>,
        seed: u64,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.push_node(Box::new(WeightedRandomSelector::new(children, seed)))
    }

    /// Appends a utility selector node.
    ///
    /// Scoring closures cannot be expressed through a builder closure, so this
    /// takes pre-built [`UtilityChild`] values directly; when hysteresis is
    /// needed, build the node first and attach it with `.raw(...)`.
    pub fn utility_selector(
        self,
        children: Vec<UtilityChild<W>>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.push_node(Box::new(UtilitySelector::new(children)))
    }

    /// Opens a parallel node (aggregated within a single tick).
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

    /// Opens a parallel node that accumulates terminal statuses across ticks.
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

    /// Opens an if/then/else branch node; each branch must have exactly one
    /// child.
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

    /// Opens a race node: the first child to return a terminal status wins.
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

    // ---------- Decorators ----------

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

    /// Inverts the result.
    pub fn inverter(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, |child| Box::new(Inverter::new(child)))
    }

    /// Always ends in success.
    pub fn always_succeed(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, |child| Box::new(AlwaysSucceed::new(child)))
    }

    /// Always ends in failure.
    pub fn always_fail(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, |child| Box::new(AlwaysFail::new(child)))
    }

    /// Repeats on success `times` times.
    pub fn repeat(
        self,
        times: core::num::NonZeroUsize,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, move |child| Box::new(Repeat::new(times, child)))
    }

    /// Retries on failure up to `max_attempts` times, one attempt per tick.
    pub fn retry(
        self,
        max_attempts: core::num::NonZeroUsize,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, move |child| Box::new(Retry::new(max_attempts, child)))
    }

    /// Runs until success.
    pub fn until_success(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, |child| Box::new(UntilSuccess::new(child)))
    }

    /// Runs until failure.
    pub fn until_failure(
        self,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, |child| Box::new(UntilFailure::new(child)))
    }

    /// Conditional guard; ticks the child only when the condition holds.
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

    /// Cooldown after success.
    pub fn cooldown(
        self,
        duration: Duration,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, move |child| Box::new(Cooldown::new(duration, child)))
    }

    /// Enforces a timeout on the running child.
    pub fn timeout(
        self,
        duration: Duration,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, move |child| Box::new(Timeout::new(duration, child)))
    }

    /// Limits the number of runs.
    pub fn limiter(
        self,
        max: core::num::NonZeroUsize,
        f: impl FnOnce(BranchBuilder<W, Empty>) -> BranchBuilder<W, One>,
    ) -> BehaviorTreeBuilder<W, S::Next> {
        self.decorator(f, move |child| Box::new(Limiter::new(max, child)))
    }

    /// Rate-limits within a time window.
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
    /// Builds the behavior tree.
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
