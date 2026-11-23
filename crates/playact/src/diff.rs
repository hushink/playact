//! Structural diffs between behavior trees.
//!
//! A diff compares nodes by stable [`NodeId`] and reports their structural paths
//! as [`NodePath`] values. Only nodes with IDs participate; ID-less nodes are
//! still traversed so child paths stay accurate.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::node::{Kind, NodeId};
use crate::tree::{BehaviorTree, NodePath, TreeDescription};

/// A node referenced by a [`TreeDiff`].
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Structural changes between two behavior trees.
///
/// `added` and `removed` are sorted by [`NodeId`]; `moved` is also sorted by ID.
/// Duplicate IDs are rejected by builder and configuration builds, so a
/// well-formed tree contributes at most one entry per ID. A name or kind change
/// at the same ID and path is not reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeDiff {
    /// IDs present in the new tree but not the old one.
    pub added: Vec<DiffNode>,
    /// IDs present in the old tree but not the new one.
    pub removed: Vec<DiffNode>,
    /// IDs present in both trees whose structural path changed.
    pub moved: Vec<MovedNode>,
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

        Self {
            added,
            removed,
            moved,
        }
    }

    /// Whether the structures are identical for every node with an ID.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.moved.is_empty()
    }
}

impl fmt::Display for TreeDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("no structural changes");
        }
        writeln!(
            f,
            "added {}, removed {}, moved {}",
            self.added.len(),
            self.removed.len(),
            self.moved.len()
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

    use crate::leaf::Action;
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
        assert!(text.contains("added 1, removed 1, moved 1"));
        assert!(text.contains("+ c c at root.children[1]"));
        assert!(text.contains("- a a at root.children[0]"));
        assert!(text.contains("~ b b: root.children[1] -> root.children[0]"));
    }
}
