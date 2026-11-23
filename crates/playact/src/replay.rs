//! Deterministic trace recording and replay verification.
//!
//! A [`TraceRecorder`] stores owned copies of [`TraceEvent`]s so a test or
//! production run can be compared with a later run. This records the tree's
//! observable behavior, not the world state; replay the same world scenario and
//! use node snapshots / typed blackboard data for stateful inputs.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::node::{Kind, NodeId};
use crate::status::Status;
use crate::time::Time;
use crate::trace::{AbortReason, TraceEvent, Tracer};

/// An owned trace event.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TraceRecord {
    /// The node started ticking.
    Enter {
        /// Stable node identity, when assigned.
        id: Option<NodeId>,
        /// Node name.
        name: String,
        /// Node type.
        kind: Kind,
        /// Child-index path.
        path: Vec<usize>,
        /// Current time.
        time: Time,
    },
    /// The node finished ticking.
    Exit {
        /// Stable node identity, when assigned.
        id: Option<NodeId>,
        /// Node name.
        name: String,
        /// Node type.
        kind: Kind,
        /// Child-index path.
        path: Vec<usize>,
        /// Returned status.
        status: Status,
        /// Current time.
        time: Time,
    },
    /// A Running subtree was abandoned.
    Abort {
        /// Stable node identity, when assigned.
        id: Option<NodeId>,
        /// Node name.
        name: String,
        /// Node type.
        kind: Kind,
        /// Child-index path.
        path: Vec<usize>,
        /// Abort reason.
        reason: AbortReason,
        /// Current time.
        time: Time,
    },
    /// A node reported an error.
    Error {
        /// Stable node identity, when assigned.
        id: Option<NodeId>,
        /// Node name.
        name: String,
        /// Node type.
        kind: Kind,
        /// Child-index path.
        path: Vec<usize>,
        /// Error message.
        message: String,
        /// Current time.
        time: Time,
    },
}

impl TraceRecord {
    fn from_event(event: TraceEvent<'_>) -> Self {
        match event {
            TraceEvent::Enter {
                id,
                name,
                kind,
                path,
                time,
            } => Self::Enter {
                id: id.cloned(),
                name: String::from(name),
                kind,
                path: path.to_vec(),
                time,
            },
            TraceEvent::Exit {
                id,
                name,
                kind,
                path,
                status,
                time,
            } => Self::Exit {
                id: id.cloned(),
                name: String::from(name),
                kind,
                path: path.to_vec(),
                status,
                time,
            },
            TraceEvent::Abort {
                id,
                name,
                kind,
                path,
                reason,
                time,
            } => Self::Abort {
                id: id.cloned(),
                name: String::from(name),
                kind,
                path: path.to_vec(),
                reason,
                time,
            },
            TraceEvent::Error {
                id,
                name,
                kind,
                path,
                message,
                time,
            } => Self::Error {
                id: id.cloned(),
                name: String::from(name),
                kind,
                path: path.to_vec(),
                message: String::from(message),
                time,
            },
        }
    }
}

/// A cloneable recorder that stores owned trace events.
#[derive(Clone, Default)]
pub struct TraceRecorder {
    inner: Arc<Mutex<Vec<TraceRecord>>>,
}

impl TraceRecorder {
    /// Creates an empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a snapshot of recorded events.
    #[must_use]
    pub fn records(&self) -> Vec<TraceRecord> {
        self.lock().clone()
    }

    /// Number of recorded events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no events have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Clears all recorded events.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// Compares recorded events with an expected trace.
    ///
    /// Returns the first differing event, including length mismatches, or
    /// `None` when the traces are identical.
    #[must_use]
    pub fn diff(&self, expected: &[TraceRecord]) -> Option<TraceDiff> {
        let actual = self.lock();
        for (index, actual) in actual.iter().enumerate() {
            match expected.get(index) {
                Some(expected) if expected == actual => {}
                Some(expected) => {
                    return Some(TraceDiff {
                        index,
                        expected: Some(expected.clone()),
                        actual: Some(actual.clone()),
                    });
                }
                None => {
                    return Some(TraceDiff {
                        index,
                        expected: None,
                        actual: Some(actual.clone()),
                    });
                }
            }
        }

        expected.get(actual.len()).map(|expected| TraceDiff {
            index: actual.len(),
            expected: Some(expected.clone()),
            actual: None,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Vec<TraceRecord>> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

impl fmt::Debug for TraceRecorder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TraceRecorder")
            .field("records", &self.len())
            .finish()
    }
}

impl Tracer for TraceRecorder {
    fn event(&mut self, event: TraceEvent<'_>) {
        self.lock().push(TraceRecord::from_event(event));
    }
}

/// The first difference between two traces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceDiff {
    /// Zero-based event index.
    pub index: usize,
    /// Expected event, or `None` when the actual trace is longer.
    pub expected: Option<TraceRecord>,
    /// Actual event, or `None` when the expected trace is longer.
    pub actual: Option<TraceRecord>,
}

#[cfg(test)]
mod tests {
    use crate::builder::BehaviorTreeBuilder;
    use crate::status::Status;
    use crate::time::Time;

    use super::*;

    fn action_tree(name: &str) -> crate::BehaviorTree<()> {
        BehaviorTreeBuilder::<()>::new("record")
            .action(name, |_| Status::Success)
            .build()
            .expect("tree")
    }

    #[test]
    fn records_and_compares_identical_runs() {
        let expected = {
            let mut tree = action_tree("work");
            let recorder = TraceRecorder::new();
            tree.set_tracer(Some(Box::new(recorder.clone())));
            let mut world = ();
            assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
            recorder.records()
        };

        let mut tree = action_tree("work");
        let recorder = TraceRecorder::new();
        tree.set_tracer(Some(Box::new(recorder.clone())));
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        assert_eq!(recorder.diff(&expected), None);
    }

    #[test]
    fn reports_first_divergence() {
        let mut tree = action_tree("work");
        let recorder = TraceRecorder::new();
        tree.set_tracer(Some(Box::new(recorder.clone())));
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);

        let expected = recorder.records();
        let mut different = action_tree("other");
        different.set_tracer(Some(Box::new(recorder.clone())));
        recorder.clear();
        assert_eq!(different.tick(&mut world, Time::ZERO), Status::Success);

        let diff = recorder.diff(&expected).expect("divergence");
        assert_eq!(diff.index, 0);
        assert!(diff.expected.is_some());
        assert!(diff.actual.is_some());
    }

    #[test]
    fn detects_length_mismatches() {
        let mut tree = action_tree("work");
        let recorder = TraceRecorder::new();
        tree.set_tracer(Some(Box::new(recorder.clone())));
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        let expected = recorder.records();

        recorder.clear();
        assert!(matches!(
            recorder.diff(&expected),
            Some(TraceDiff {
                index: 0,
                expected: Some(_),
                actual: None
            })
        ));
    }
}
