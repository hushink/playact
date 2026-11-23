//! Runtime observability: structured trace events.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use crate::node::{Kind, NodeId};
use crate::status::Status;
use crate::time::Time;

/// Why a Running subtree was abandoned; used only in Tracer events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "trace-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "snake_case")
)]
pub enum AbortReason {
    /// Preempted by a higher-priority sibling branch.
    Preempted,
    /// Timed out by a Timeout node.
    Timeout,
    /// Parallel reached a conclusion and stopped its remaining children.
    ParallelDone,
    /// Race produced a winner and stopped the remaining Running children.
    RaceDone,
    /// The whole tree was aborted externally.
    TreeAbort,
}

impl fmt::Display for AbortReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Preempted => "preempted",
            Self::Timeout => "timeout",
            Self::ParallelDone => "parallel_done",
            Self::RaceDone => "race_done",
            Self::TreeAbort => "tree_abort",
        })
    }
}

/// A node-level trace event.
///
/// `path` is the child-index path from the root to the current node: the root is
/// `[]`, its first child is `[0]`, and the third child of the second child is
/// `[1, 2]`. Each event also carries the node's optional stable [`NodeId`].
/// Events carry no wall-clock durations, preserving determinism under virtual
/// time.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum TraceEvent<'a> {
    /// The node started ticking.
    Enter {
        /// Stable node identity, when assigned.
        id: Option<&'a NodeId>,
        /// Node name.
        name: &'a str,
        /// Node type.
        kind: Kind,
        /// Child-index path.
        path: &'a [usize],
        /// Current time.
        time: Time,
    },
    /// The node finished ticking.
    Exit {
        /// Stable node identity, when assigned.
        id: Option<&'a NodeId>,
        /// Node name.
        name: &'a str,
        /// Node type.
        kind: Kind,
        /// Child-index path.
        path: &'a [usize],
        /// Returned status.
        status: Status,
        /// Current time.
        time: Time,
    },
    /// A Running subtree was abandoned.
    Abort {
        /// Stable node identity, when assigned.
        id: Option<&'a NodeId>,
        /// Node name.
        name: &'a str,
        /// Node type.
        kind: Kind,
        /// Child-index path.
        path: &'a [usize],
        /// Abort reason.
        reason: AbortReason,
        /// Current time.
        time: Time,
    },
    /// A node reported a recoverable error.
    Error {
        /// Stable node identity, when assigned.
        id: Option<&'a NodeId>,
        /// Node name.
        name: &'a str,
        /// Node type.
        kind: Kind,
        /// Child-index path.
        path: &'a [usize],
        /// User-facing error message.
        message: &'a str,
        /// Current time.
        time: Time,
    },
}

/// Optional observer of node events.
///
/// With no Tracer installed, the engine only performs one `Option` check: no
/// events are produced and nothing is allocated. Tracer requires `Send` so the
/// behavior tree can move between threads; it does not require `Sync`.
pub trait Tracer: Send {
    /// Receive one event. Implementations should stay short to avoid blocking
    /// the tick.
    fn event(&mut self, event: TraceEvent<'_>);
}

/// Broadcast events to multiple Tracers.
///
/// Useful for feeding logging, statistics, and visualization at once; events
/// are `Copy` and dispatched in registration order.
#[derive(Default)]
pub struct MultiTracer {
    tracers: Vec<Box<dyn Tracer + Send>>,
}

impl MultiTracer {
    /// Create an empty fan-out.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a Tracer.
    pub fn push(&mut self, tracer: Box<dyn Tracer + Send>) {
        self.tracers.push(tracer);
    }

    /// Number of registered Tracers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tracers.len()
    }

    /// Whether no Tracer is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tracers.is_empty()
    }
}

impl fmt::Debug for MultiTracer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MultiTracer")
            .field("len", &self.tracers.len())
            .finish()
    }
}

impl Tracer for MultiTracer {
    fn event(&mut self, event: TraceEvent<'_>) {
        for tracer in &mut self.tracers {
            tracer.event(event);
        }
    }
}

/// A simple adapter that turns events into `tracing` logs.
#[cfg(feature = "tracing")]
#[derive(Debug, Default, Clone, Copy)]
pub struct TracingTracer;

#[cfg(feature = "tracing")]
impl Tracer for TracingTracer {
    fn event(&mut self, event: TraceEvent<'_>) {
        match event {
            TraceEvent::Enter {
                id,
                name,
                kind,
                path,
                time,
            } => tracing::debug!(
                target: "playact",
                id = ?id,
                node = name,
                kind = kind.as_str(),
                ?path,
                ?time,
                "enter"
            ),
            TraceEvent::Exit {
                id,
                name,
                kind,
                path,
                status,
                time,
            } => tracing::debug!(
                target: "playact",
                id = ?id,
                node = name,
                kind = kind.as_str(),
                ?path,
                %status,
                ?time,
                "exit"
            ),
            TraceEvent::Abort {
                id,
                name,
                kind,
                path,
                reason,
                time,
            } => tracing::debug!(
                target: "playact",
                id = ?id,
                node = name,
                kind = kind.as_str(),
                ?path,
                %reason,
                ?time,
                "abort"
            ),
            TraceEvent::Error {
                id,
                name,
                kind,
                path,
                message,
                time,
            } => tracing::error!(
                target: "playact",
                id = ?id,
                node = name,
                kind = kind.as_str(),
                ?path,
                error = message,
                ?time,
                "node error"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::node::Kind;
    use crate::time::Time;

    use super::*;

    #[derive(Clone, Default)]
    struct Count(Arc<Mutex<usize>>);

    impl Tracer for Count {
        fn event(&mut self, _event: TraceEvent<'_>) {
            *self.0.lock().expect("trace lock") += 1;
        }
    }

    #[test]
    fn multi_tracer_fans_out() {
        let first = Count::default();
        let second = Count::default();
        let mut multi = MultiTracer::new();
        multi.push(Box::new(first.clone()));
        multi.push(Box::new(second.clone()));

        multi.event(TraceEvent::Enter {
            id: None,
            name: "node",
            kind: Kind::Action,
            path: &[],
            time: Time::ZERO,
        });

        assert_eq!(*first.0.lock().expect("trace lock"), 1);
        assert_eq!(*second.0.lock().expect("trace lock"), 1);
        assert_eq!(multi.len(), 2);
        assert!(!multi.is_empty());
    }
}
