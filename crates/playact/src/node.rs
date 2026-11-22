//! The node contract and node type tags.

use alloc::boxed::Box;
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
    /// Closure action.
    Action,
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
            Self::Action => "action",
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

    /// Direct children; empty by default. Used by `describe()` and custom
    /// tooling.
    fn children(&self) -> &[BoxedNode<W>] {
        &[]
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
