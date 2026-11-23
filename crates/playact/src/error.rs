//! Core build errors.

use core::fmt;
use core::num::NonZeroUsize;

use crate::node::NodeId;
use crate::tree::NodePath;

/// Configuration errors found while building a behavior tree.
///
/// The typestate builder already uses the type system to rule out problems such
/// as an empty root or a decorator with the wrong number of children; this
/// covers only semantic errors that still need runtime validation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BuildError {
    /// The `Parallel` success threshold exceeds the child count and can never
    /// be met.
    InvalidSuccessThreshold {
        /// The configured `AtLeast` value.
        at_least: NonZeroUsize,
        /// The actual child count.
        children: usize,
    },
    /// The `Parallel` failure threshold exceeds the child count and can never
    /// be met.
    InvalidFailureThreshold {
        /// The configured `AtLeast` value.
        at_least: NonZeroUsize,
        /// The actual child count.
        children: usize,
    },
    /// The same stable `NodeId` appears more than once in the root subtree.
    DuplicateNodeId {
        /// Duplicate identity.
        id: NodeId,
        /// Structural path of the second occurrence.
        path: NodePath,
    },
    /// Internal build state is inconsistent. Should not occur in normal use.
    InvalidTree {
        /// Error description.
        message: &'static str,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSuccessThreshold { at_least, children } => write!(
                f,
                "parallel success threshold AtLeast({at_least}) exceeds child count {children}"
            ),
            Self::InvalidFailureThreshold { at_least, children } => write!(
                f,
                "parallel failure threshold AtLeast({at_least}) exceeds child count {children}"
            ),
            Self::DuplicateNodeId { id, path } => {
                write!(f, "duplicate node id `{id}` at {path}")
            }
            Self::InvalidTree { message } => f.write_str(message),
        }
    }
}

impl core::error::Error for BuildError {}
