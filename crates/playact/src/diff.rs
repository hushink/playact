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
#[cfg_attr(feature = "diff-serde", derive(serde::Serialize))]
pub struct DiffNode {
    /// Stable identity.
    pub id: NodeId,
    /// Structural path at the time of capture.
    pub path: NodePath,
    /// Display name.
    pub name: String,
    /// Node type.
    #[cfg_attr(feature = "diff-serde", serde(serialize_with = "serialize_kind"))]
    pub kind: Kind,
}

/// A node whose structural path changed between two trees.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "diff-serde", derive(serde::Serialize))]
pub struct MovedNode {
    /// Stable identity.
    pub id: NodeId,
    /// Display name from the new tree.
    pub name: String,
    /// Node type from the new tree.
    #[cfg_attr(feature = "diff-serde", serde(serialize_with = "serialize_kind"))]
    pub kind: Kind,
    /// Path in the old tree.
    pub from: NodePath,
    /// Path in the new tree.
    pub to: NodePath,
}

/// A node whose name or kind changed without moving.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "diff-serde", derive(serde::Serialize))]
pub struct RenamedNode {
    /// Stable identity.
    pub id: NodeId,
    /// Unchanged structural path.
    pub path: NodePath,
    /// Display name in the old tree.
    pub old_name: String,
    /// Display name in the new tree.
    pub new_name: String,
    /// Node type in the old tree.
    #[cfg_attr(feature = "diff-serde", serde(serialize_with = "serialize_kind"))]
    pub old_kind: Kind,
    /// Node type in the new tree.
    #[cfg_attr(feature = "diff-serde", serde(serialize_with = "serialize_kind"))]
    pub new_kind: Kind,
}

/// Structural changes between two behavior trees.
///
/// `added` and `removed` are sorted by [`NodeId`]; `moved` and `renamed` are
/// also sorted by ID. Duplicate IDs are rejected by builder and configuration
/// builds, so a well-formed tree contributes at most one entry per ID.
///
/// A path change is reported as `moved` even when the name or kind changed too;
/// `renamed` covers nodes that stayed at the same path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "diff-serde", derive(serde::Serialize))]
pub struct TreeDiff {
    /// IDs present in the new tree but not the old one.
    pub added: Vec<DiffNode>,
    /// IDs present in the old tree but not the new one.
    pub removed: Vec<DiffNode>,
    /// IDs present in both trees whose structural path changed.
    pub moved: Vec<MovedNode>,
    /// IDs present at the same path whose name or kind changed.
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
                    .filter(|previous| {
                        previous.path == node.path
                            && (previous.name != node.name || previous.kind != node.kind)
                    })
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
}

#[cfg(feature = "diff-serde")]
fn serialize_kind<S>(kind: &Kind, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(kind.as_str())
}

/// A bounded history of structural diffs, ordered oldest to newest.
///
/// `limit = 0` disables storage. The default keeps only the most recent diff.
#[derive(Debug, Clone)]
pub struct DiffHistory {
    limit: usize,
    diffs: VecDeque<TreeDiff>,
}

impl DiffHistory {
    /// Creates an empty history with the given limit.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
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
        self.diffs.back()
    }

    /// Iterates from oldest to newest.
    pub fn iter(&self) -> impl Iterator<Item = &TreeDiff> {
        self.diffs.iter()
    }

    /// Appends a diff, dropping the oldest entry when the limit is reached.
    pub fn push(&mut self, diff: TreeDiff) {
        if self.limit == 0 {
            return;
        }
        if self.diffs.len() == self.limit {
            self.diffs.pop_front();
        }
        self.diffs.push_back(diff);
    }

    /// Removes and returns the most recent diff.
    pub fn pop_latest(&mut self) -> Option<TreeDiff> {
        self.diffs.pop_back()
    }

    /// Drops all stored diffs.
    pub fn clear(&mut self) {
        self.diffs.clear();
    }

    /// Serializes the history as a JSON array, oldest first.
    ///
    /// Requires the `diff-serde` feature.
    #[cfg(feature = "diff-serde")]
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("DiffHistory serialization cannot fail")
    }
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
        serializer.collect_seq(self.diffs.iter())
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
        history.push(TreeDiff::between(&empty, &one));
        history.push(TreeDiff::between(&one, &two));
        history.push(TreeDiff::between(&two, &three));

        assert_eq!(history.limit(), 2);
        assert_eq!(history.len(), 2);
        assert_eq!(history.latest().expect("latest").added[0].id.as_str(), "c");
        let ids: Vec<&str> = history
            .iter()
            .flat_map(|diff| diff.added.iter().map(|node| node.id.as_str()))
            .collect();
        assert_eq!(ids, vec!["b", "c"]);

        let latest = history.pop_latest().expect("popped");
        assert_eq!(latest.added[0].id.as_str(), "c");
        assert_eq!(
            history.latest().expect("previous").added[0].id.as_str(),
            "b"
        );

        history.set_limit(0);
        assert!(history.is_empty());
        history.push(TreeDiff::default());
        assert!(history.is_empty());

        history.set_limit(1);
        history.push(TreeDiff::default());
        assert_eq!(history.len(), 1);
        history.clear();
        assert!(history.is_empty());
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
        history.push(TreeDiff::between(&two, &three));

        let value: serde_json::Value = serde_json::from_str(&history.to_json()).expect("json");
        assert_eq!(value.as_array().expect("array").len(), 2);
        assert_eq!(value[0]["added"][0]["id"], "b");
        assert_eq!(value[1]["added"][0]["id"], "c");
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
}
