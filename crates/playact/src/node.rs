//! The node contract and node type tags.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::context::Context;
use crate::status::Status;

/// A node type tag.
///
/// Built-in nodes use fixed variants and get compile-time exhaustiveness; custom
/// nodes use [`Kind::Custom`] to carry a readable `'static` name.
///
/// `Kind` is only a type tag; it carries no parameters or runtime state. The
/// config layer's `SpecNode` uses its own `type` tag, and no string lookup
/// happens at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Kind {
    /// Memorizing sequence node.
    Sequence,
    /// Reactive sequence node.
    ReactiveSequence,
    /// Memorizing selector node.
    Selector,
    /// Reactive selector node.
    ReactiveSelector,
    /// Random selector with memory.
    RandomSelector,
    /// Weighted random selector with a deterministic seed.
    WeightedRandomSelector,
    /// Utility selector that picks a child by dynamic score.
    UtilitySelector,
    /// Executes the child only when the condition holds.
    Guard,
    /// Picks the then/else branch by condition.
    IfThenElse,
    /// Parallel node.
    Parallel,
    /// Parallel node that remembers terminal states across ticks.
    MemParallel,
    /// Parallel node where the first terminal state wins.
    Race,
    /// Inverts the result.
    Inverter,
    /// Always succeeds.
    AlwaysSucceed,
    /// Always fails.
    AlwaysFail,
    /// Repeats execution.
    Repeat,
    /// Retries on failure.
    Retry,
    /// Repeats until success.
    UntilSuccess,
    /// Repeats until failure.
    UntilFailure,
    /// Cooldown after success.
    Cooldown,
    /// Execution timeout.
    Timeout,
    /// Usage limit.
    Limiter,
    /// Time-window quota.
    Throttle,
    /// Waits for a duration.
    Wait,
    /// Waits until a predicate holds.
    WaitUntil,
    /// Waits for a typed event.
    WaitForEvent,
    /// Closure action.
    Action,
    /// Fallible closure action.
    TryAction,
    /// Async/remote action.
    AsyncAction,
    /// Closure condition.
    Condition,
    /// Writes a fixed value to the blackboard.
    SetValue,
    /// Subtree slot switched dynamically by a blackboard key.
    SubtreeSlot,
    /// A whole behavior tree as a node.
    Tree,
    /// Custom node type.
    Custom(&'static str),
}

impl Kind {
    /// Readable snake_case name; a custom kind returns its static string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sequence => "sequence",
            Self::ReactiveSequence => "reactive_sequence",
            Self::Selector => "selector",
            Self::ReactiveSelector => "reactive_selector",
            Self::RandomSelector => "random_selector",
            Self::WeightedRandomSelector => "weighted_random_selector",
            Self::UtilitySelector => "utility_selector",
            Self::Guard => "guard",
            Self::IfThenElse => "if_then_else",
            Self::Parallel => "parallel",
            Self::MemParallel => "mem_parallel",
            Self::Race => "race",
            Self::Inverter => "inverter",
            Self::AlwaysSucceed => "always_succeed",
            Self::AlwaysFail => "always_fail",
            Self::Repeat => "repeat",
            Self::Retry => "retry",
            Self::UntilSuccess => "until_success",
            Self::UntilFailure => "until_failure",
            Self::Cooldown => "cooldown",
            Self::Timeout => "timeout",
            Self::Limiter => "limiter",
            Self::Throttle => "throttle",
            Self::Wait => "wait",
            Self::WaitUntil => "wait_until",
            Self::WaitForEvent => "wait_for_event",
            Self::Action => "action",
            Self::TryAction => "try_action",
            Self::AsyncAction => "async_action",
            Self::Condition => "condition",
            Self::SetValue => "set_value",
            Self::SubtreeSlot => "subtree_slot",
            Self::Tree => "tree",
            Self::Custom(name) => name,
        }
    }

    /// Whether this is a custom type tag.
    #[must_use]
    pub const fn is_custom(self) -> bool {
        matches!(self, Self::Custom(_))
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stable, user-assigned node identity.
///
/// Names are display labels and may change; a `NodeId` is intended for
/// snapshots, metrics, debugging tools, and configuration reloads that need to
/// recognize the same logical node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(String);

impl NodeId {
    /// Creates an ID from any string-like value.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the ID as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for NodeId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for NodeId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl AsRef<str> for NodeId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A node wrapper that attaches a stable [`NodeId`] without changing behavior.
///
/// Existing custom nodes can opt into identities by returning
/// [`Node::id`]; this wrapper is the built-in way to add one to any node.
pub struct IdentifiedNode<W> {
    id: NodeId,
    inner: BoxedNode<W>,
}

impl<W> IdentifiedNode<W> {
    /// Wraps a node with an identity.
    #[must_use]
    pub fn new(id: impl Into<NodeId>, inner: BoxedNode<W>) -> Self {
        Self {
            id: id.into(),
            inner,
        }
    }

    /// The attached identity.
    #[must_use]
    pub fn node_id(&self) -> &NodeId {
        &self.id
    }

    /// Unwraps the inner node.
    #[must_use]
    pub fn into_inner(self) -> BoxedNode<W> {
        self.inner
    }
}

impl<W> Node<W> for IdentifiedNode<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        self.inner.tick(ctx)
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        self.inner.abort(ctx);
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }

    fn id(&self) -> Option<&NodeId> {
        Some(&self.id)
    }

    fn children(&self) -> &[BoxedNode<W>] {
        self.inner.children()
    }

    fn children_mut(&mut self) -> &mut [BoxedNode<W>] {
        self.inner.children_mut()
    }

    fn save_own_state(&self) -> Option<Vec<u8>> {
        self.inner.save_own_state()
    }

    fn load_own_state(&mut self, state: &[u8]) -> bool {
        self.inner.load_own_state(state)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        self.inner.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> bool {
        self.inner.load_state(state)
    }
}

/// The unified contract for all nodes.
///
/// `W` is the world/client type supplied by the user; the engine does not care
/// what it contains.
///
/// # Lifecycle contract
///
/// - [`Node::tick`] is called once per logic frame.
/// - [`Node::abort`] is called when a subtree that is still `Running` is
///   abandoned, to release external side effects such as held keys and active
///   skills; empty by default.
/// - [`Node::reset`] clears internal memory for global resets such as character
///   revival or combat restart; empty by default.
/// - When the engine abandons a Running subtree it calls `abort` first, then
///   `reset`; both must be idempotent.
/// - A node returning `Success` / `Failure` must restore itself to a state where
///   it can start again.
pub trait Node<W>: Send {
    /// Advance one logic frame.
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status;

    /// Called when the current Running subtree is abandoned; responsible for
    /// cleaning up external side effects.
    fn abort(&mut self, _ctx: &mut Context<'_, W>) {}

    /// Clear internal memory. Called on global reset or by the engine after
    /// abort.
    fn reset(&mut self) {}

    /// Display name for debugging.
    fn name(&self) -> &str {
        "node"
    }

    /// Node type tag.
    fn kind(&self) -> Kind {
        Kind::Custom("node")
    }

    /// Stable identity assigned by the tree author, if any.
    fn id(&self) -> Option<&NodeId> {
        None
    }

    /// Direct children; empty by default. Used by `describe()` and custom
    /// tooling.
    fn children(&self) -> &[BoxedNode<W>] {
        &[]
    }

    /// Mutable direct children; empty by default.
    ///
    /// Required for ID-keyed snapshot traversal. Custom composites that want
    /// their children restored from [`NodeIdSnapshot`] must implement it.
    ///
    /// [`NodeIdSnapshot`]: crate::NodeIdSnapshot
    fn children_mut(&mut self) -> &mut [BoxedNode<W>] {
        &mut []
    }

    /// Export only this node's own state, excluding child state.
    ///
    /// Used by ID-keyed snapshots. The default is `None`; stateful custom nodes
    /// must implement this together with [`Node::load_own_state`].
    fn save_own_state(&self) -> Option<Vec<u8>> {
        None
    }

    /// Load state produced by [`Node::save_own_state`].
    fn load_own_state(&mut self, _state: &[u8]) -> bool {
        false
    }

    /// Export the node's internal state; unsupported by default, returns `None`.
    ///
    /// Composite/decorator nodes should encode their subtree state as well (the
    /// built-in nodes already do); leaves may return `None`. The node defines the
    /// state byte format itself; the engine only saves and restores it.
    fn save_state(&self) -> Option<Vec<u8>> {
        None
    }

    /// Load state produced by [`Node::save_state`]; returns `false` on rejection.
    fn load_state(&mut self, _state: &[u8]) -> bool {
        false
    }
}

/// The unified carrier for children in a tree.
///
/// Heterogeneous trees require dynamic dispatch; `Send` lets the tree move to a
/// thread pool or worker thread.
pub type BoxedNode<W> = Box<dyn Node<W> + Send>;
