//! Structural diffs between behavior trees.
//!
//! A diff compares nodes by stable [`NodeId`] and reports their structural paths
//! as [`NodePath`] values. Only nodes with IDs participate; ID-less nodes are
//! still traversed so child paths stay accurate.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::node::{Kind, NodeId};
use crate::tree::{BehaviorTree, NodePath, TreeDescription};

/// A node referenced by a [`TreeDiff`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "diff-serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DiffNode {
    /// Stable identity.
    pub id: NodeId,
    /// Structural path at the time of capture.
    pub path: NodePath,
    /// Display name.
    pub name: String,
    /// Node type.
    pub kind: Kind,
}

/// A node whose structural path changed between two trees.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "diff-serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MovedNode {
    /// Stable identity.
    pub id: NodeId,
    /// Display name from the new tree.
    pub name: String,
    /// Node type from the new tree.
    pub kind: Kind,
    /// Path in the old tree.
    pub from: NodePath,
    /// Path in the new tree.
    pub to: NodePath,
}

/// A node whose name or kind changed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "diff-serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RenamedNode {
    /// Stable identity.
    pub id: NodeId,
    /// Structural path in the new tree.
    pub path: NodePath,
    /// Display name in the old tree.
    pub old_name: String,
    /// Display name in the new tree.
    pub new_name: String,
    /// Node type in the old tree.
    pub old_kind: Kind,
    /// Node type in the new tree.
    pub new_kind: Kind,
}

/// Structural changes between two behavior trees.
///
/// `added` and `removed` are sorted by [`NodeId`]; `moved` and `renamed` are
/// also sorted by ID. Duplicate IDs are rejected by builder and configuration
/// builds, so a well-formed tree contributes at most one entry per ID.
///
/// A node can appear in both `moved` and `renamed` when its path and its name or
/// kind changed in the same reload. `moved` reports the path transition and
/// `renamed` reports the old/new name and kind at the new path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "diff-serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TreeDiff {
    /// IDs present in the new tree but not the old one.
    pub added: Vec<DiffNode>,
    /// IDs present in the old tree but not the new one.
    pub removed: Vec<DiffNode>,
    /// IDs present in both trees whose structural path changed.
    pub moved: Vec<MovedNode>,
    /// IDs present in both trees whose name or kind changed.
    pub renamed: Vec<RenamedNode>,
}

impl TreeDiff {
    /// Compares the root and service structures of two trees.
    ///
    /// IDs are compared globally across the main tree and services, so moving a
    /// node between a service and the main tree is reported as a move.
    #[must_use]
    pub fn between<W>(old: &BehaviorTree<W>, new: &BehaviorTree<W>) -> Self {
        let old = collect(old);
        let new = collect(new);

        let added = new
            .iter()
            .filter(|(id, _)| !old.contains_key(*id))
            .map(|(_, node)| node.clone())
            .collect();
        let removed = old
            .iter()
            .filter(|(id, _)| !new.contains_key(*id))
            .map(|(_, node)| node.clone())
            .collect();
        let moved = new
            .iter()
            .filter_map(|(id, node)| {
                old.get(id)
                    .filter(|previous| previous.path != node.path)
                    .map(|previous| MovedNode {
                        id: id.clone(),
                        name: node.name.clone(),
                        kind: node.kind,
                        from: previous.path.clone(),
                        to: node.path.clone(),
                    })
            })
            .collect();
        let renamed = new
            .iter()
            .filter_map(|(id, node)| {
                old.get(id)
                    .filter(|previous| previous.name != node.name || previous.kind != node.kind)
                    .map(|previous| RenamedNode {
                        id: id.clone(),
                        path: node.path.clone(),
                        old_name: previous.name.clone(),
                        new_name: node.name.clone(),
                        old_kind: previous.kind,
                        new_kind: node.kind,
                    })
            })
            .collect();

        Self {
            added,
            removed,
            moved,
            renamed,
        }
    }

    /// Whether the structures are identical for every node with an ID.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.moved.is_empty()
            && self.renamed.is_empty()
    }

    /// Serializes the diff as compact JSON.
    ///
    /// Requires the `diff-serde` feature.
    #[cfg(feature = "diff-serde")]
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("TreeDiff serialization cannot fail")
    }

    /// Parses a diff from JSON produced by [`TreeDiff::to_json`].
    ///
    /// Requires the `diff-serde` feature. Custom node kinds cannot be restored
    /// because [`Kind::Custom`] borrows a `'static`
    /// name; such a payload returns an error.
    #[cfg(feature = "diff-serde")]
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

/// A diff stored in a [`DiffHistory`] with the revision assigned when it was
/// pushed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "diff-serde", derive(serde::Serialize))]
pub struct DiffEntry<'a> {
    /// Monotonic revision, starting at 1 and never reused.
    pub revision: u64,
    /// Caller-supplied wall-clock time in Unix milliseconds, when available.
    #[cfg_attr(feature = "diff-serde", serde(skip_serializing_if = "Option::is_none"))]
    pub timestamp: Option<u64>,
    /// The structural diff.
    pub diff: &'a TreeDiff,
}

/// A bounded history of structural diffs, ordered oldest to newest.
///
/// `limit = 0` disables storage. The default keeps only the most recent diff.
/// Every stored diff gets a monotonic revision so external logs can identify a
/// reload precisely; revisions are never reused, even after entries are evicted
/// by the limit, [`pop_latest`](DiffHistory::pop_latest), or
/// [`clear`](DiffHistory::clear). Entries may also carry a caller-supplied
/// Unix-millisecond timestamp via [`push_at`](DiffHistory::push_at), or the
/// current system time via [`push_now`](DiffHistory::push_now).
#[derive(Debug, Clone)]
pub struct DiffHistory {
    limit: usize,
    next_revision: u64,
    diffs: VecDeque<StoredDiff>,
}

#[derive(Debug, Clone)]
struct StoredDiff {
    revision: u64,
    timestamp: Option<u64>,
    diff: TreeDiff,
}

impl DiffHistory {
    /// Creates an empty history with the given limit.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            next_revision: 1,
            diffs: VecDeque::new(),
        }
    }

    /// The configured limit.
    #[must_use]
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Changes the limit and drops the oldest entries when necessary.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
        while self.diffs.len() > limit {
            self.diffs.pop_front();
        }
    }

    /// Number of stored diffs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.diffs.len()
    }

    /// Whether no diff is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.diffs.is_empty()
    }

    /// The most recent diff.
    #[must_use]
    pub fn latest(&self) -> Option<&TreeDiff> {
        self.diffs.back().map(|stored| &stored.diff)
    }

    /// The most recent diff with its revision.
    #[must_use]
    pub fn latest_entry(&self) -> Option<DiffEntry<'_>> {
        self.diffs.back().map(|stored| DiffEntry {
            revision: stored.revision,
            timestamp: stored.timestamp,
            diff: &stored.diff,
        })
    }

    /// The largest stored revision.
    #[must_use]
    pub fn latest_revision(&self) -> Option<u64> {
        self.diffs.back().map(|stored| stored.revision)
    }

    /// The smallest retained revision.
    ///
    /// A polling client whose cursor is below this value has missed entries
    /// that the limit already evicted.
    #[must_use]
    pub fn oldest_revision(&self) -> Option<u64> {
        self.diffs.iter().map(|stored| stored.revision).min()
    }

    /// Iterates over diffs from oldest to newest.
    pub fn iter(&self) -> impl Iterator<Item = &TreeDiff> {
        self.diffs.iter().map(|stored| &stored.diff)
    }

    /// Iterates over diffs and revisions from oldest to newest.
    pub fn entries(&self) -> impl Iterator<Item = DiffEntry<'_>> {
        self.diffs.iter().map(|stored| DiffEntry {
            revision: stored.revision,
            timestamp: stored.timestamp,
            diff: &stored.diff,
        })
    }

    /// Iterates over entries newer than `revision`, oldest first.
    ///
    /// Revisions are compared numerically, so `entries_since(0)` returns every
    /// stored entry and `entries_since(latest_revision())` returns none.
    pub fn entries_since(&self, revision: u64) -> impl Iterator<Item = DiffEntry<'_>> {
        self.entries()
            .filter(move |entry| entry.revision > revision)
    }

    /// Appends a diff without a timestamp, dropping the oldest entry when the
    /// limit is reached.
    ///
    /// Returns the assigned revision, or `None` when the limit is `0` and
    /// nothing is stored.
    pub fn push(&mut self, diff: TreeDiff) -> Option<u64> {
        self.push_entry(diff, None)
    }

    /// Appends a diff with a caller-supplied Unix-millisecond timestamp.
    ///
    /// Returns the assigned revision, or `None` when the limit is `0` and
    /// nothing is stored.
    pub fn push_at(&mut self, diff: TreeDiff, unix_ms: u64) -> Option<u64> {
        self.push_entry(diff, Some(unix_ms))
    }

    /// Appends a diff stamped with the current system time in Unix milliseconds.
    ///
    /// Requires the `std` feature. Returns the assigned revision, or `None` when
    /// the limit is `0` and nothing is stored.
    #[cfg(feature = "std")]
    pub fn push_now(&mut self, diff: TreeDiff) -> Option<u64> {
        self.push_at(diff, unix_millis_now())
    }

    fn push_entry(&mut self, diff: TreeDiff, timestamp: Option<u64>) -> Option<u64> {
        if self.limit == 0 {
            return None;
        }
        if self.diffs.len() == self.limit {
            self.diffs.pop_front();
        }
        let revision = self.next_revision;
        self.next_revision += 1;
        self.diffs.push_back(StoredDiff {
            revision,
            timestamp,
            diff,
        });
        Some(revision)
    }

    /// Removes and returns the most recent diff.
    pub fn pop_latest(&mut self) -> Option<TreeDiff> {
        self.diffs.pop_back().map(|stored| stored.diff)
    }

    /// Drops all stored diffs.
    pub fn clear(&mut self) {
        self.diffs.clear();
    }

    /// Serializes the history as a JSON array of `{"revision", "diff"}` entries,
    /// oldest first; entries with a timestamp also include `"timestamp"`.
    ///
    /// Requires the `diff-serde` feature.
    #[cfg(feature = "diff-serde")]
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("DiffHistory serialization cannot fail")
    }

    /// Serializes only entries newer than `revision`, oldest first.
    ///
    /// Requires the `diff-serde` feature.
    #[cfg(feature = "diff-serde")]
    #[must_use]
    pub fn to_json_since(&self, revision: u64) -> String {
        let entries: Vec<DiffEntry<'_>> = self.entries_since(revision).collect();
        serde_json::to_string(&entries).expect("DiffEntry serialization cannot fail")
    }

    /// Parses a history from JSON produced by [`DiffHistory::to_json`].
    ///
    /// Requires the `diff-serde` feature. Stored revisions are preserved; the
    /// next push continues after the largest revision in the payload. The
    /// restored limit is the number of entries, or 1 for an empty history; call
    /// [`set_limit`](DiffHistory::set_limit) to choose a different bound.
    #[cfg(feature = "diff-serde")]
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        #[derive(serde::Deserialize)]
        struct JsonEntry {
            revision: u64,
            #[serde(default)]
            timestamp: Option<u64>,
            diff: TreeDiff,
        }

        let entries: Vec<JsonEntry> = serde_json::from_str(json)?;
        let mut history = Self::new(entries.len().max(1));
        for entry in entries {
            history.next_revision = history.next_revision.max(entry.revision.saturating_add(1));
            history.diffs.push_back(StoredDiff {
                revision: entry.revision,
                timestamp: entry.timestamp,
                diff: entry.diff,
            });
        }
        Ok(history)
    }
}

#[cfg(feature = "std")]
fn unix_millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

impl Default for DiffHistory {
    fn default() -> Self {
        Self::new(1)
    }
}

#[cfg(feature = "diff-serde")]
impl serde::Serialize for DiffHistory {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_seq(self.entries())
    }
}

impl fmt::Display for TreeDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("no structural changes");
        }
        writeln!(
            f,
            "added {}, removed {}, moved {}, renamed {}",
            self.added.len(),
            self.removed.len(),
            self.moved.len(),
            self.renamed.len()
        )?;
        for node in &self.added {
            writeln!(f, "+ {} {} at {}", node.id, node.name, node.path)?;
        }
        for node in &self.removed {
            writeln!(f, "- {} {} at {}", node.id, node.name, node.path)?;
        }
        for node in &self.moved {
            writeln!(
                f,
                "~ {} {}: {} -> {}",
                node.id, node.name, node.from, node.to
            )?;
        }
        for node in &self.renamed {
            writeln!(
                f,
                "* {} at {}: {} ({}) -> {} ({})",
                node.id, node.path, node.old_name, node.old_kind, node.new_name, node.new_kind
            )?;
        }
        Ok(())
    }
}

fn collect<W>(tree: &BehaviorTree<W>) -> BTreeMap<NodeId, DiffNode> {
    let mut nodes = BTreeMap::new();
    collect_description(&mut nodes, &tree.describe(), NodePath::root());
    for (index, service) in tree.describe_services().iter().enumerate() {
        collect_description(&mut nodes, service, NodePath::service(index));
    }
    nodes
}

fn collect_description(
    nodes: &mut BTreeMap<NodeId, DiffNode>,
    node: &TreeDescription,
    path: NodePath,
) {
    if let Some(id) = &node.id {
        nodes.entry(id.clone()).or_insert_with(|| DiffNode {
            id: id.clone(),
            path: path.clone(),
            name: node.name.clone(),
            kind: node.kind,
        });
    }
    for (index, child) in node.children.iter().enumerate() {
        collect_description(nodes, child, path.with_child(index));
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use crate::leaf::{Action, Condition};
    use crate::node::{BoxedNode, IdentifiedNode};
    use crate::status::Status;

    use super::*;

    fn leaf(name: &'static str, id: &'static str) -> BoxedNode<()> {
        Box::new(IdentifiedNode::new(
            id,
            Box::new(Action::<()>::new(name, |_| Status::Success)),
        ))
    }

    fn tree(children: Vec<BoxedNode<()>>) -> BehaviorTree<()> {
        BehaviorTree::new(Box::new(crate::composite::Sequence::new(children)))
    }

    #[test]
    fn reports_added_removed_and_moved_nodes() {
        let old = tree(vec![leaf("a", "a"), leaf("b", "b")]);
        let new = tree(vec![leaf("b", "b"), leaf("c", "c")]);

        let diff = TreeDiff::between(&old, &new);
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.added[0].id, NodeId::new("c"));
        assert_eq!(diff.added[0].path, NodePath::root().with_child(1));
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.removed[0].id, NodeId::new("a"));
        assert_eq!(diff.removed[0].path, NodePath::root().with_child(0));
        assert_eq!(diff.moved.len(), 1);
        assert_eq!(diff.moved[0].id, NodeId::new("b"));
        assert_eq!(diff.moved[0].from, NodePath::root().with_child(1));
        assert_eq!(diff.moved[0].to, NodePath::root().with_child(0));
        assert!(diff.renamed.is_empty());
    }

    #[test]
    fn reports_renamed_nodes() {
        let old = tree(vec![leaf("old", "same")]);
        let new = tree(vec![Box::new(IdentifiedNode::new(
            "same",
            Box::new(Condition::<()>::new("new", |_| true)),
        ))]);

        let diff = TreeDiff::between(&old, &new);
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert!(diff.moved.is_empty());
        assert_eq!(diff.renamed.len(), 1);

        let renamed = &diff.renamed[0];
        assert_eq!(renamed.id, NodeId::new("same"));
        assert_eq!(renamed.path, NodePath::root().with_child(0));
        assert_eq!(renamed.old_name, "old");
        assert_eq!(renamed.new_name, "new");
        assert_eq!(renamed.old_kind, Kind::Action);
        assert_eq!(renamed.new_kind, Kind::Condition);

        let text = diff.to_string();
        assert!(text.contains("renamed 1"));
        assert!(text.contains("* same at root.children[0]: old (action) -> new (condition)"));
    }

    #[test]
    fn reports_moved_node_that_also_renamed() {
        let old = tree(vec![leaf("old", "same"), leaf("other", "other")]);
        let new = tree(vec![
            leaf("other", "other"),
            Box::new(IdentifiedNode::new(
                "same",
                Box::new(Condition::<()>::new("new", |_| true)),
            )),
        ]);

        let diff = TreeDiff::between(&old, &new);
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert_eq!(diff.moved.len(), 2);
        assert_eq!(diff.moved[0].id.as_str(), "other");
        assert_eq!(diff.moved[0].from, NodePath::root().with_child(1));
        assert_eq!(diff.moved[0].to, NodePath::root().with_child(0));
        assert_eq!(diff.moved[1].id.as_str(), "same");
        assert_eq!(diff.moved[1].from, NodePath::root().with_child(0));
        assert_eq!(diff.moved[1].to, NodePath::root().with_child(1));

        assert_eq!(diff.renamed.len(), 1);
        let renamed = &diff.renamed[0];
        assert_eq!(renamed.id.as_str(), "same");
        assert_eq!(renamed.path, NodePath::root().with_child(1));
        assert_eq!(renamed.old_name, "old");
        assert_eq!(renamed.new_name, "new");
        assert_eq!(renamed.old_kind, Kind::Action);
        assert_eq!(renamed.new_kind, Kind::Condition);

        let text = diff.to_string();
        assert!(text.contains("added 0, removed 0, moved 2, renamed 1"));
        assert!(text.contains("~ same new: root.children[0] -> root.children[1]"));
        assert!(text.contains("* same at root.children[1]: old (action) -> new (condition)"));
    }

    #[test]
    fn compares_main_tree_and_services_together() {
        let old = tree(vec![leaf("a", "a")]);
        let mut new = tree(vec![leaf("a", "a")]);
        new.add_service(Box::new(IdentifiedNode::new(
            "service",
            Box::new(Action::<()>::new("service", |_| Status::Success)),
        )));

        let diff = TreeDiff::between(&old, &new);
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.added[0].id, NodeId::new("service"));
        assert_eq!(diff.added[0].path, NodePath::service(0));
        assert!(diff.removed.is_empty());
        assert!(diff.moved.is_empty());
        assert!(diff.renamed.is_empty());
    }

    #[test]
    fn history_is_bounded_and_ordered() {
        let empty = tree(Vec::new());
        let one = tree(vec![leaf("a", "a")]);
        let two = tree(vec![leaf("a", "a"), leaf("b", "b")]);
        let three = tree(vec![leaf("a", "a"), leaf("b", "b"), leaf("c", "c")]);

        let mut history = DiffHistory::new(2);
        assert_eq!(history.push(TreeDiff::between(&empty, &one)), Some(1));
        assert_eq!(history.push(TreeDiff::between(&one, &two)), Some(2));
        assert_eq!(history.push(TreeDiff::between(&two, &three)), Some(3));

        assert_eq!(history.limit(), 2);
        assert_eq!(history.len(), 2);
        assert_eq!(history.latest().expect("latest").added[0].id.as_str(), "c");
        assert_eq!(
            history.latest_entry().expect("latest entry"),
            DiffEntry {
                revision: 3,
                timestamp: None,
                diff: history.latest().expect("latest"),
            }
        );
        let ids: Vec<&str> = history
            .iter()
            .flat_map(|diff| diff.added.iter().map(|node| node.id.as_str()))
            .collect();
        assert_eq!(ids, vec!["b", "c"]);
        let revisions: Vec<u64> = history.entries().map(|entry| entry.revision).collect();
        assert_eq!(revisions, vec![2, 3]);
        assert_eq!(history.oldest_revision(), Some(2));

        let latest = history.pop_latest().expect("popped");
        assert_eq!(latest.added[0].id.as_str(), "c");
        assert_eq!(
            history.latest().expect("previous").added[0].id.as_str(),
            "b"
        );
        assert_eq!(history.latest_entry().expect("previous").revision, 2);
        assert_eq!(history.oldest_revision(), Some(2));

        history.set_limit(0);
        assert!(history.is_empty());
        assert_eq!(history.oldest_revision(), None);
        assert_eq!(history.push(TreeDiff::default()), None);
        assert!(history.is_empty());

        history.set_limit(1);
        assert_eq!(history.push(TreeDiff::default()), Some(4));
        assert_eq!(history.len(), 1);
        assert_eq!(history.oldest_revision(), Some(4));
        history.clear();
        assert!(history.is_empty());
        // Revisions stay monotonic across a clear.
        assert_eq!(history.push(TreeDiff::default()), Some(5));
        assert_eq!(history.oldest_revision(), Some(5));
    }

    #[test]
    fn identical_trees_have_empty_diff() {
        let tree = tree(vec![leaf("a", "a")]);
        let diff = TreeDiff::between(&tree, &tree);
        assert!(diff.is_empty());
        assert_eq!(diff.to_string(), "no structural changes");
    }

    #[test]
    fn display_lists_changes() {
        let old = tree(vec![leaf("a", "a"), leaf("b", "b")]);
        let new = tree(vec![leaf("b", "b"), leaf("c", "c")]);
        let text = TreeDiff::between(&old, &new).to_string();
        assert!(text.contains("added 1, removed 1, moved 1, renamed 0"));
        assert!(text.contains("+ c c at root.children[1]"));
        assert!(text.contains("- a a at root.children[0]"));
        assert!(text.contains("~ b b: root.children[1] -> root.children[0]"));
    }

    #[cfg(feature = "diff-serde")]
    #[test]
    fn serializes_history_to_json() {
        let empty = tree(Vec::new());
        let one = tree(vec![leaf("a", "a")]);
        let two = tree(vec![leaf("a", "a"), leaf("b", "b")]);
        let three = tree(vec![leaf("a", "a"), leaf("b", "b"), leaf("c", "c")]);

        let mut history = DiffHistory::new(2);
        history.push(TreeDiff::between(&empty, &one));
        history.push(TreeDiff::between(&one, &two));
        history.push_at(TreeDiff::between(&two, &three), 1_700_000_000_000);

        let value: serde_json::Value = serde_json::from_str(&history.to_json()).expect("json");
        assert_eq!(value.as_array().expect("array").len(), 2);
        assert_eq!(value[0]["revision"], 2);
        assert_eq!(value[0]["diff"]["added"][0]["id"], "b");
        assert!(value[0].get("timestamp").is_none());
        assert_eq!(value[1]["revision"], 3);
        assert_eq!(value[1]["timestamp"], 1_700_000_000_000_u64);
        assert_eq!(value[1]["diff"]["added"][0]["id"], "c");
    }

    #[cfg(feature = "diff-serde")]
    #[test]
    fn serializes_to_json() {
        let old = tree(vec![leaf("a", "a"), leaf("b", "b")]);
        let new = tree(vec![leaf("b", "b"), leaf("c", "c")]);

        let value: serde_json::Value =
            serde_json::from_str(&TreeDiff::between(&old, &new).to_json()).expect("json");
        assert_eq!(value["added"][0]["id"], "c");
        assert_eq!(value["added"][0]["path"], "root.children[1]");
        assert_eq!(value["added"][0]["kind"], "action");
        assert_eq!(value["removed"][0]["id"], "a");
        assert_eq!(value["moved"][0]["from"], "root.children[1]");
        assert_eq!(value["moved"][0]["to"], "root.children[0]");
        assert!(value["renamed"].as_array().expect("renamed").is_empty());

        let old_renamed = tree(vec![leaf("old", "same")]);
        let new_renamed = tree(vec![Box::new(IdentifiedNode::new(
            "same",
            Box::new(Condition::<()>::new("new", |_| true)),
        ))]);
        let value: serde_json::Value =
            serde_json::from_str(&TreeDiff::between(&old_renamed, &new_renamed).to_json())
                .expect("json");
        assert_eq!(value["renamed"][0]["path"], "root.children[0]");
        assert_eq!(value["renamed"][0]["old_name"], "old");
        assert_eq!(value["renamed"][0]["new_name"], "new");
        assert_eq!(value["renamed"][0]["old_kind"], "action");
        assert_eq!(value["renamed"][0]["new_kind"], "condition");
    }

    #[test]
    fn entries_since_filters_newer_revisions() {
        let empty = tree(Vec::new());
        let one = tree(vec![leaf("a", "a")]);
        let two = tree(vec![leaf("a", "a"), leaf("b", "b")]);
        let three = tree(vec![leaf("a", "a"), leaf("b", "b"), leaf("c", "c")]);

        let mut history = DiffHistory::new(4);
        history.push(TreeDiff::between(&empty, &one));
        history.push(TreeDiff::between(&one, &two));
        history.push(TreeDiff::between(&two, &three));

        assert_eq!(history.latest_revision(), Some(3));
        let revisions: Vec<u64> = history
            .entries_since(1)
            .map(|entry| entry.revision)
            .collect();
        assert_eq!(revisions, vec![2, 3]);
        assert_eq!(history.entries_since(0).count(), 3);
        assert!(history.entries_since(3).next().is_none());
    }

    #[cfg(feature = "diff-serde")]
    #[test]
    fn serializes_history_since_revision() {
        let empty = tree(Vec::new());
        let one = tree(vec![leaf("a", "a")]);
        let two = tree(vec![leaf("a", "a"), leaf("b", "b")]);
        let three = tree(vec![leaf("a", "a"), leaf("b", "b"), leaf("c", "c")]);

        let mut history = DiffHistory::new(4);
        history.push(TreeDiff::between(&empty, &one));
        history.push(TreeDiff::between(&one, &two));
        history.push(TreeDiff::between(&two, &three));

        let value: serde_json::Value =
            serde_json::from_str(&history.to_json_since(1)).expect("json");
        let entries = value.as_array().expect("array");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["revision"], 2);
        assert_eq!(entries[1]["revision"], 3);

        let value: serde_json::Value =
            serde_json::from_str(&history.to_json_since(3)).expect("json");
        assert!(value.as_array().expect("array").is_empty());
    }

    #[cfg(feature = "diff-serde")]
    #[test]
    fn roundtrips_diff_json() {
        let old = tree(vec![leaf("a", "a"), leaf("b", "b")]);
        let new = tree(vec![leaf("b", "b"), leaf("c", "c")]);
        let diff = TreeDiff::between(&old, &new);

        let parsed = TreeDiff::from_json(&diff.to_json()).expect("parse diff");
        assert_eq!(parsed, diff);
    }

    #[cfg(feature = "diff-serde")]
    #[test]
    fn roundtrips_history_json_and_continues_revisions() {
        let empty = tree(Vec::new());
        let one = tree(vec![leaf("a", "a")]);
        let two = tree(vec![leaf("a", "a"), leaf("b", "b")]);
        let three = tree(vec![leaf("a", "a"), leaf("b", "b"), leaf("c", "c")]);

        let mut history = DiffHistory::new(4);
        history.push(TreeDiff::between(&empty, &one));
        history.push_at(TreeDiff::between(&one, &two), 1_600_000_000_000);
        history.push(TreeDiff::between(&two, &three));

        let mut parsed = DiffHistory::from_json(&history.to_json()).expect("parse history");
        let expected: Vec<_> = history.entries().collect();
        let actual: Vec<_> = parsed.entries().collect();
        assert_eq!(actual, expected);
        assert_eq!(actual[1].timestamp, Some(1_600_000_000_000));
        assert_eq!(parsed.oldest_revision(), Some(1));
        assert_eq!(parsed.limit(), 3);
        assert_eq!(parsed.push(TreeDiff::default()), Some(4));
    }

    #[cfg(feature = "std")]
    #[test]
    fn push_now_records_a_system_timestamp() {
        let empty = tree(Vec::new());
        let one = tree(vec![leaf("a", "a")]);
        let mut history = DiffHistory::new(1);
        assert_eq!(history.push_now(TreeDiff::between(&empty, &one)), Some(1));
        assert!(history.latest_entry().expect("entry").timestamp.is_some());
    }
}
