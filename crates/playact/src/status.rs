//! Node status: the only control-flow communication protocol in the behavior tree.

use core::fmt;

/// The result of a single tick of a node.
///
/// `Success` / `Failure` / `Running` is the only protocol between control nodes
/// and execution nodes: composites orchestrate control flow solely from it and
/// do not care about the concrete business logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "trace-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "snake_case")
)]
pub enum Status {
    /// This execution completed successfully.
    Success,
    /// This execution completed with failure.
    Failure,
    /// Not finished; continue on the next tick. The node should keep whatever
    /// internal state it needs.
    Running,
}

impl Status {
    /// Convert a boolean into a status: `true` → `Success`, `false` → `Failure`.
    #[must_use]
    pub const fn from_bool(ok: bool) -> Self {
        if ok { Self::Success } else { Self::Failure }
    }

    /// Whether this is a success.
    #[must_use]
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }

    /// Whether this is a failure.
    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(self, Self::Failure)
    }

    /// Whether this is still running.
    #[must_use]
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    /// Whether this is a terminal state (success or failure).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !self.is_running()
    }
}

impl From<bool> for Status {
    fn from(value: bool) -> Self {
        Self::from_bool(value)
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Success => "Success",
            Self::Failure => "Failure",
            Self::Running => "Running",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bool_conversion() {
        assert_eq!(Status::from_bool(true), Status::Success);
        assert_eq!(Status::from_bool(false), Status::Failure);
        assert_eq!(Status::from(true), Status::Success);
    }

    #[test]
    fn predicates() {
        assert!(Status::Success.is_success());
        assert!(Status::Success.is_terminal());
        assert!(Status::Failure.is_failure());
        assert!(Status::Running.is_running());
        assert!(!Status::Running.is_terminal());
    }
}
