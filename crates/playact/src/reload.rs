//! Audit metadata for the most recent successful reload or refresh.

use crate::diff::TreeDiff;

/// A unified view of the last successful hot reload or remote refresh.
///
/// Obtained from `TreeWatcher::last_reload` or
/// `RemoteTreeLoader::last_reload`. It bundles the diff entry (revision and
/// timestamp) with state-restore metadata so logging and audit code does not
/// need to combine several accessors.
#[derive(Debug, Clone, Copy)]
pub struct ReloadReport<'a> {
    /// Monotonic revision assigned to the diff.
    pub revision: u64,
    /// Caller-supplied Unix-millisecond timestamp, when available.
    pub timestamp: Option<u64>,
    /// Nodes whose own state was restored.
    ///
    /// `None` when the source does not track restoration (for example a
    /// manually constructed history) or before the first successful reload.
    pub restored: Option<usize>,
    /// Restore error, if the new tree rejected snapshot state.
    ///
    /// When set, the new tree was reset and `restored` is `Some(0)`.
    pub restore_error: Option<&'a str>,
    /// Structural changes introduced by the reload.
    pub diff: &'a TreeDiff,
}
