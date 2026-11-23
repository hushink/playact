//! Snapshot and restore of behavior tree node state.
//!
//! A snapshot contains only node-internal state and structural information; it
//! does not include the world, blackboard values, closure captures,
//! `AsyncAction` tasks, `SubtreeSlot` dynamic subtrees, or external resources.
//! Restore into the same tree structure; each node owns its custom state format.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::node::{BoxedNode, Node, NodeId};

/// Current structural snapshot format version.
pub const SNAPSHOT_VERSION: u32 = 1;

/// Current ID-keyed snapshot format version.
pub const NODE_ID_SNAPSHOT_VERSION: u32 = 1;

/// A state snapshot of a behavior tree.
///
/// The bytes in `root` and `services` are defined by each node's
/// `Node::save_state`; `None` means that node (or a key node in its subtree) does
/// not support snapshots. A snapshot contains no world, blackboard, or closure
/// capture state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeSnapshot {
    /// Snapshot format version, normally equal to [`SNAPSHOT_VERSION`].
    pub version: u32,
    /// Root node state.
    pub root: Option<Vec<u8>>,
    /// Service node states, in service registration order.
    pub services: Vec<Option<Vec<u8>>>,
}

/// A snapshot keyed by stable [`NodeId`].
///
/// Unlike [`TreeSnapshot`], this format is independent of child order: every
/// stateful node with an ID stores its own state in `nodes`. Nodes without IDs
/// or without [`Node::save_own_state`] support are not included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeIdSnapshot {
    /// Snapshot format version, normally [`NODE_ID_SNAPSHOT_VERSION`].
    pub version: u32,
    /// Own state for each stateful node that has an ID.
    pub nodes: BTreeMap<NodeId, Vec<u8>>,
}

/// Errors produced while capturing or restoring an ID-keyed snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum NodeIdSnapshotError {
    /// The snapshot version is not supported.
    UnsupportedVersion {
        /// Version found in the snapshot.
        found: u32,
        /// Currently supported version.
        supported: u32,
    },
    /// The tree contains the same [`NodeId`] more than once.
    DuplicateId(String),
    /// The snapshot contains an ID that does not exist in the tree.
    UnknownId(String),
    /// A node rejected its own state.
    InvalidState {
        /// Node identity.
        id: String,
    },
}

impl fmt::Display for NodeIdSnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion { found, supported } => write!(
                f,
                "unsupported ID snapshot version {found}; supported is {supported}"
            ),
            Self::DuplicateId(id) => write!(f, "duplicate node id `{id}`"),
            Self::UnknownId(id) => write!(f, "ID snapshot contains unknown node `{id}`"),
            Self::InvalidState { id } => write!(f, "node `{id}` rejected its own snapshot state"),
        }
    }
}

impl core::error::Error for NodeIdSnapshotError {}

/// Errors found while restoring a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RestoreError {
    /// The snapshot version is not supported.
    UnsupportedVersion {
        /// Version found in the snapshot.
        found: u32,
        /// Currently supported version.
        supported: u32,
    },
    /// The number of services in the snapshot does not match the current tree.
    ServiceCountMismatch {
        /// Number of services in the snapshot.
        snapshot: usize,
        /// Number of services in the current tree.
        tree: usize,
    },
    /// A node rejected the state (usually meaning the tree structure changed or
    /// the state bytes are corrupt).
    InvalidState {
        /// Node name, or `service[i] name`.
        node: String,
    },
}

impl fmt::Display for RestoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion { found, supported } => {
                write!(
                    f,
                    "unsupported snapshot version {found}; supported is {supported}"
                )
            }
            Self::ServiceCountMismatch { snapshot, tree } => write!(
                f,
                "snapshot has {snapshot} services but the tree has {tree}"
            ),
            Self::InvalidState { node } => write!(f, "node `{node}` rejected snapshot state"),
        }
    }
}

impl core::error::Error for RestoreError {}

pub(crate) fn capture<W>(root: &BoxedNode<W>, services: &[BoxedNode<W>]) -> TreeSnapshot {
    TreeSnapshot {
        version: SNAPSHOT_VERSION,
        root: root.save_state(),
        services: services
            .iter()
            .map(|service| service.save_state())
            .collect(),
    }
}

pub(crate) fn apply<W>(
    root: &mut BoxedNode<W>,
    services: &mut [BoxedNode<W>],
    snapshot: &TreeSnapshot,
) -> Result<(), RestoreError> {
    if snapshot.version != SNAPSHOT_VERSION {
        return Err(RestoreError::UnsupportedVersion {
            found: snapshot.version,
            supported: SNAPSHOT_VERSION,
        });
    }
    if snapshot.services.len() != services.len() {
        return Err(RestoreError::ServiceCountMismatch {
            snapshot: snapshot.services.len(),
            tree: services.len(),
        });
    }

    root.reset();
    for service in services.iter_mut() {
        service.reset();
    }

    if let Some(state) = &snapshot.root
        && !root.load_state(state)
    {
        return Err(RestoreError::InvalidState {
            node: String::from(root.name()),
        });
    }

    for (index, (service, state)) in services.iter_mut().zip(&snapshot.services).enumerate() {
        if let Some(state) = state
            && !service.load_state(state)
        {
            return Err(RestoreError::InvalidState {
                node: alloc::format!("service[{index}] {}", service.name()),
            });
        }
    }

    Ok(())
}

pub(crate) fn save_children<W>(children: &[BoxedNode<W>]) -> Vec<Option<Vec<u8>>> {
    children.iter().map(|child| child.save_state()).collect()
}

pub(crate) fn load_children<W>(children: &mut [BoxedNode<W>], states: &[Option<Vec<u8>>]) -> bool {
    if children.len() != states.len() {
        return false;
    }

    for (child, state) in children.iter_mut().zip(states) {
        if let Some(state) = state
            && !child.load_state(state)
        {
            return false;
        }
    }

    true
}

pub(crate) fn capture_by_id<W>(
    root: &BoxedNode<W>,
    services: &[BoxedNode<W>],
) -> Result<NodeIdSnapshot, NodeIdSnapshotError> {
    let mut nodes = BTreeMap::new();
    let mut seen = BTreeSet::new();

    capture_node_by_id(root.as_ref(), &mut nodes, &mut seen)?;
    for service in services {
        capture_node_by_id(service.as_ref(), &mut nodes, &mut seen)?;
    }

    Ok(NodeIdSnapshot {
        version: NODE_ID_SNAPSHOT_VERSION,
        nodes,
    })
}

fn capture_node_by_id<W>(
    node: &(dyn Node<W> + Send),
    nodes: &mut BTreeMap<NodeId, Vec<u8>>,
    seen: &mut BTreeSet<NodeId>,
) -> Result<(), NodeIdSnapshotError> {
    if let Some(id) = node.id() {
        if !seen.insert(id.clone()) {
            return Err(NodeIdSnapshotError::DuplicateId(String::from(id.as_str())));
        }
        if let Some(state) = node.save_own_state() {
            nodes.insert(id.clone(), state);
        }
    }

    for child in node.children() {
        capture_node_by_id(child.as_ref(), nodes, seen)?;
    }

    Ok(())
}

pub(crate) fn apply_by_id<W>(
    root: &mut BoxedNode<W>,
    services: &mut [BoxedNode<W>],
    snapshot: &NodeIdSnapshot,
) -> Result<(), NodeIdSnapshotError> {
    apply_by_id_with_policy(root, services, snapshot, true).map(|_| ())
}

pub(crate) fn apply_by_id_matching<W>(
    root: &mut BoxedNode<W>,
    services: &mut [BoxedNode<W>],
    snapshot: &NodeIdSnapshot,
) -> Result<usize, NodeIdSnapshotError> {
    apply_by_id_with_policy(root, services, snapshot, false)
}

fn apply_by_id_with_policy<W>(
    root: &mut BoxedNode<W>,
    services: &mut [BoxedNode<W>],
    snapshot: &NodeIdSnapshot,
    deny_unknown: bool,
) -> Result<usize, NodeIdSnapshotError> {
    if snapshot.version != NODE_ID_SNAPSHOT_VERSION {
        return Err(NodeIdSnapshotError::UnsupportedVersion {
            found: snapshot.version,
            supported: NODE_ID_SNAPSHOT_VERSION,
        });
    }

    root.reset();
    for service in services.iter_mut() {
        service.reset();
    }

    let mut seen = BTreeSet::new();
    let mut restored = 0;
    restore_node_by_id(root.as_mut(), snapshot, &mut seen, &mut restored)?;
    for service in services.iter_mut() {
        restore_node_by_id(service.as_mut(), snapshot, &mut seen, &mut restored)?;
    }

    if deny_unknown {
        for id in snapshot.nodes.keys() {
            if !seen.contains(id) {
                return Err(NodeIdSnapshotError::UnknownId(String::from(id.as_str())));
            }
        }
    }

    Ok(restored)
}

fn restore_node_by_id<W>(
    node: &mut (dyn Node<W> + Send),
    snapshot: &NodeIdSnapshot,
    seen: &mut BTreeSet<NodeId>,
    restored: &mut usize,
) -> Result<(), NodeIdSnapshotError> {
    if let Some(id) = node.id().cloned() {
        if !seen.insert(id.clone()) {
            return Err(NodeIdSnapshotError::DuplicateId(String::from(id.as_str())));
        }
        if let Some(state) = snapshot.nodes.get(&id) {
            if !node.load_own_state(state) {
                return Err(NodeIdSnapshotError::InvalidState {
                    id: String::from(id.as_str()),
                });
            }
            *restored += 1;
        }
    }

    for child in node.children_mut() {
        restore_node_by_id(child.as_mut(), snapshot, seen, restored)?;
    }

    Ok(())
}

/// Compact binary encoding/decoding of node state.
pub(crate) mod state {
    use alloc::vec::Vec;
    use core::time::Duration;

    /// Per-`Vec` length limit, preventing corrupt or malicious bytes from
    /// triggering huge allocations.
    const MAX_ITEMS: usize = 1 << 20;

    pub(crate) fn encode<T: Codec>(value: &T) -> Vec<u8> {
        let mut out = Vec::new();
        value.encode(&mut out);
        out
    }

    pub(crate) fn decode<T: Codec>(bytes: &[u8]) -> Option<T> {
        let mut reader = Reader { bytes, pos: 0 };
        let value = T::decode(&mut reader)?;
        if reader.pos == bytes.len() {
            Some(value)
        } else {
            None
        }
    }

    pub(crate) trait Codec: Sized {
        fn encode(&self, out: &mut Vec<u8>);

        fn decode(reader: &mut Reader<'_>) -> Option<Self>;
    }

    pub(crate) struct Reader<'a> {
        bytes: &'a [u8],
        pos: usize,
    }

    impl<'a> Reader<'a> {
        fn take(&mut self, len: usize) -> Option<&'a [u8]> {
            let end = self.pos.checked_add(len)?;
            let bytes = self.bytes.get(self.pos..end)?;
            self.pos = end;
            Some(bytes)
        }

        fn u8(&mut self) -> Option<u8> {
            Some(*self.take(1)?.first()?)
        }

        fn u32(&mut self) -> Option<u32> {
            Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
        }

        fn u64(&mut self) -> Option<u64> {
            Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
        }
    }

    impl Codec for u8 {
        fn encode(&self, out: &mut Vec<u8>) {
            out.push(*self);
        }

        fn decode(reader: &mut Reader<'_>) -> Option<Self> {
            reader.u8()
        }
    }

    impl Codec for u32 {
        fn encode(&self, out: &mut Vec<u8>) {
            out.extend_from_slice(&self.to_le_bytes());
        }

        fn decode(reader: &mut Reader<'_>) -> Option<Self> {
            reader.u32()
        }
    }

    impl Codec for u64 {
        fn encode(&self, out: &mut Vec<u8>) {
            out.extend_from_slice(&self.to_le_bytes());
        }

        fn decode(reader: &mut Reader<'_>) -> Option<Self> {
            reader.u64()
        }
    }

    impl Codec for usize {
        fn encode(&self, out: &mut Vec<u8>) {
            out.extend_from_slice(&(*self as u64).to_le_bytes());
        }

        fn decode(reader: &mut Reader<'_>) -> Option<Self> {
            usize::try_from(reader.u64()?).ok()
        }
    }

    impl Codec for bool {
        fn encode(&self, out: &mut Vec<u8>) {
            out.push(u8::from(*self));
        }

        fn decode(reader: &mut Reader<'_>) -> Option<Self> {
            match reader.u8()? {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            }
        }
    }

    impl Codec for Duration {
        fn encode(&self, out: &mut Vec<u8>) {
            self.as_secs().encode(out);
            self.subsec_nanos().encode(out);
        }

        fn decode(reader: &mut Reader<'_>) -> Option<Self> {
            let secs = u64::decode(reader)?;
            let nanos = u32::decode(reader)?;
            Duration::from_secs(secs).checked_add(Duration::from_nanos(u64::from(nanos)))
        }
    }

    impl<T: Codec> Codec for Option<T> {
        fn encode(&self, out: &mut Vec<u8>) {
            match self {
                Some(value) => {
                    out.push(1);
                    value.encode(out);
                }
                None => out.push(0),
            }
        }

        fn decode(reader: &mut Reader<'_>) -> Option<Self> {
            match reader.u8()? {
                0 => Some(None),
                1 => Some(Some(T::decode(reader)?)),
                _ => None,
            }
        }
    }

    impl<T: Codec> Codec for Vec<T> {
        fn encode(&self, out: &mut Vec<u8>) {
            (self.len() as u64).encode(out);
            for value in self {
                value.encode(out);
            }
        }

        fn decode(reader: &mut Reader<'_>) -> Option<Self> {
            let len = usize::try_from(reader.u64()?).ok()?;
            if len > MAX_ITEMS {
                return None;
            }
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(T::decode(reader)?);
            }
            Some(values)
        }
    }

    impl<A: Codec, B: Codec> Codec for (A, B) {
        fn encode(&self, out: &mut Vec<u8>) {
            self.0.encode(out);
            self.1.encode(out);
        }

        fn decode(reader: &mut Reader<'_>) -> Option<Self> {
            Some((A::decode(reader)?, B::decode(reader)?))
        }
    }

    impl<A: Codec, B: Codec, C: Codec> Codec for (A, B, C) {
        fn encode(&self, out: &mut Vec<u8>) {
            self.0.encode(out);
            self.1.encode(out);
            self.2.encode(out);
        }

        fn decode(reader: &mut Reader<'_>) -> Option<Self> {
            Some((A::decode(reader)?, B::decode(reader)?, C::decode(reader)?))
        }
    }

    impl<A: Codec, B: Codec, C: Codec, D: Codec> Codec for (A, B, C, D) {
        fn encode(&self, out: &mut Vec<u8>) {
            self.0.encode(out);
            self.1.encode(out);
            self.2.encode(out);
            self.3.encode(out);
        }

        fn decode(reader: &mut Reader<'_>) -> Option<Self> {
            Some((
                A::decode(reader)?,
                B::decode(reader)?,
                C::decode(reader)?,
                D::decode(reader)?,
            ))
        }
    }

    impl<A: Codec, B: Codec, C: Codec, D: Codec, E: Codec> Codec for (A, B, C, D, E) {
        fn encode(&self, out: &mut Vec<u8>) {
            self.0.encode(out);
            self.1.encode(out);
            self.2.encode(out);
            self.3.encode(out);
            self.4.encode(out);
        }

        fn decode(reader: &mut Reader<'_>) -> Option<Self> {
            Some((
                A::decode(reader)?,
                B::decode(reader)?,
                C::decode(reader)?,
                D::decode(reader)?,
                E::decode(reader)?,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use crate::builder::BehaviorTreeBuilder;
    use crate::leaf::{Action, Wait};
    use crate::status::Status;
    use crate::time::Time;

    use super::*;

    #[test]
    fn restores_sequence_progress() {
        fn build(first_calls: Arc<AtomicU32>) -> crate::BehaviorTree<()> {
            BehaviorTreeBuilder::<()>::new("snap")
                .sequence(|b| {
                    b.action("first", move |_| {
                        first_calls.fetch_add(1, Ordering::Relaxed);
                        Status::Success
                    })
                    .action("second", |_| Status::Running)
                })
                .build()
                .expect("tree")
        }

        let first_calls = Arc::new(AtomicU32::new(0));
        let mut original = build(Arc::clone(&first_calls));

        let mut world = ();
        assert_eq!(original.tick(&mut world, Time::ZERO), Status::Running);
        assert_eq!(first_calls.load(Ordering::Relaxed), 1);

        let snapshot = original.snapshot();
        let mut restored = build(Arc::clone(&first_calls));
        restored.restore(&snapshot).expect("restore");

        let mut world = ();
        assert_eq!(restored.tick(&mut world, Time::ZERO), Status::Running);
        assert_eq!(first_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn restores_wait_start() {
        let mut original =
            crate::BehaviorTree::new(Box::new(Wait::<()>::new(Duration::from_millis(100))));

        let mut world = ();
        assert_eq!(
            original.tick(&mut world, Time::from_millis(0, 0)),
            Status::Running
        );
        let snapshot = original.snapshot();

        let mut restored =
            crate::BehaviorTree::new(Box::new(Wait::<()>::new(Duration::from_millis(100))));
        restored.restore(&snapshot).expect("restore");

        let mut world = ();
        assert_eq!(
            restored.tick(&mut world, Time::from_millis(100, 100)),
            Status::Success
        );
    }

    #[test]
    fn id_snapshot_restores_reordered_wait_state() {
        let mut original = BehaviorTreeBuilder::<()>::new("original")
            .selector(|b| {
                b.wait(Duration::from_millis(100))
                    .id("a")
                    .wait(Duration::from_millis(50))
                    .id("b")
            })
            .build()
            .expect("tree");
        let mut world = ();
        assert_eq!(
            original.tick(&mut world, Time::from_millis(0, 0)),
            Status::Running
        );

        let snapshot = original.snapshot_by_id().expect("snapshot");
        assert_eq!(snapshot.nodes.len(), 2);
        assert!(snapshot.nodes.contains_key(&crate::NodeId::new("a")));
        assert!(snapshot.nodes.contains_key(&crate::NodeId::new("b")));

        let mut reordered = BehaviorTreeBuilder::<()>::new("reordered")
            .selector(|b| {
                b.wait(Duration::from_millis(50))
                    .id("b")
                    .wait(Duration::from_millis(100))
                    .id("a")
            })
            .build()
            .expect("tree");
        reordered.restore_by_id(&snapshot).expect("restore");

        // b must start at 50 ms; if a's state had been applied structurally,
        // b would have started at 0 ms and already succeeded.
        let mut world = ();
        assert_eq!(
            reordered.tick(&mut world, Time::from_millis(50, 50)),
            Status::Running
        );
    }

    #[test]
    fn id_snapshot_restores_sequence_progress() {
        fn build(calls: Arc<AtomicU32>) -> crate::BehaviorTree<()> {
            BehaviorTreeBuilder::<()>::new("sequence")
                .sequence(|b| {
                    b.action("first", move |_| {
                        calls.fetch_add(1, Ordering::Relaxed);
                        Status::Success
                    })
                    .id("first")
                    .action("second", |_| Status::Running)
                    .id("second")
                })
                .id("sequence")
                .build()
                .expect("tree")
        }

        let calls = Arc::new(AtomicU32::new(0));
        let mut original = build(Arc::clone(&calls));
        let mut world = ();
        assert_eq!(original.tick(&mut world, Time::ZERO), Status::Running);
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        let snapshot = original.snapshot_by_id().expect("snapshot");
        let mut restored = build(Arc::clone(&calls));
        restored.restore_by_id(&snapshot).expect("restore");
        assert_eq!(restored.tick(&mut world, Time::ZERO), Status::Running);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn id_snapshot_rejects_unknown_duplicate_and_invalid_state() {
        let mut original = BehaviorTreeBuilder::<()>::new("original")
            .wait(Duration::from_millis(10))
            .id("a")
            .build()
            .expect("tree");
        let snapshot = original.snapshot_by_id().expect("snapshot");

        let mut target = BehaviorTreeBuilder::<()>::new("target")
            .wait(Duration::from_millis(10))
            .id("b")
            .build()
            .expect("tree");
        assert!(matches!(
            target.restore_by_id(&snapshot),
            Err(NodeIdSnapshotError::UnknownId(id)) if id == "a"
        ));

        let duplicate = crate::BehaviorTree::new(Box::new(crate::composite::Selector::new(vec![
            Box::new(crate::node::IdentifiedNode::new(
                "same",
                Box::new(Wait::<()>::new(Duration::from_millis(1))),
            )),
            Box::new(crate::node::IdentifiedNode::new(
                "same",
                Box::new(Wait::<()>::new(Duration::from_millis(1))),
            )),
        ])));
        assert!(matches!(
            duplicate.snapshot_by_id(),
            Err(NodeIdSnapshotError::DuplicateId(id)) if id == "same"
        ));

        let mut corrupt = snapshot.clone();
        corrupt.nodes.insert(crate::NodeId::new("a"), vec![0xff]);
        assert!(matches!(
            original.restore_by_id(&corrupt),
            Err(NodeIdSnapshotError::InvalidState { id }) if id == "a"
        ));
    }

    #[test]
    fn id_snapshot_matching_ignores_unknown_ids() {
        let mut original = BehaviorTreeBuilder::<()>::new("original")
            .wait(Duration::from_millis(100))
            .id("a")
            .build()
            .expect("tree");
        let mut world = ();
        assert_eq!(
            original.tick(&mut world, Time::from_millis(20, 20)),
            Status::Running
        );
        let snapshot = original.snapshot_by_id().expect("snapshot");

        let mut renamed = BehaviorTreeBuilder::<()>::new("renamed")
            .wait(Duration::from_millis(100))
            .id("b")
            .build()
            .expect("tree");
        assert!(matches!(
            renamed.restore_by_id(&snapshot),
            Err(NodeIdSnapshotError::UnknownId(id)) if id == "a"
        ));
        assert_eq!(
            renamed.restore_by_id_matching(&snapshot).expect("matching"),
            0
        );

        let mut matching = BehaviorTreeBuilder::<()>::new("matching")
            .wait(Duration::from_millis(100))
            .id("a")
            .build()
            .expect("tree");
        assert_eq!(
            matching
                .restore_by_id_matching(&snapshot)
                .expect("matching"),
            1
        );
        assert_eq!(
            matching.tick(&mut world, Time::from_millis(100, 80)),
            Status::Running
        );
        assert_eq!(
            matching.tick(&mut world, Time::from_millis(120, 20)),
            Status::Success
        );
    }

    #[test]
    fn id_snapshot_restores_decorator_state() {
        fn build() -> crate::BehaviorTree<()> {
            BehaviorTreeBuilder::<()>::new("cooldown")
                .cooldown(Duration::from_millis(100), |b| {
                    b.action("hit", |_| Status::Success)
                })
                .id("cooldown")
                .build()
                .expect("tree")
        }

        let mut original = build();
        let mut world = ();
        assert_eq!(
            original.tick(&mut world, Time::from_millis(0, 0)),
            Status::Success
        );
        let snapshot = original.snapshot_by_id().expect("snapshot");

        let mut restored = build();
        restored.restore_by_id(&snapshot).expect("restore");
        assert_eq!(
            restored.tick(&mut world, Time::from_millis(50, 50)),
            Status::Failure
        );
        assert_eq!(
            restored.tick(&mut world, Time::from_millis(150, 100)),
            Status::Success
        );
    }

    #[test]
    fn restores_limiter_quota() {
        fn build() -> crate::BehaviorTree<()> {
            BehaviorTreeBuilder::<()>::new("limit")
                .limiter(NonZeroUsize::new(1).expect("non-zero"), |b| {
                    b.action("one", |_| Status::Success)
                })
                .build()
                .expect("tree")
        }

        let mut original = build();
        let mut world = ();
        assert_eq!(original.tick(&mut world, Time::ZERO), Status::Success);
        let snapshot = original.snapshot();

        let mut restored = build();
        restored.restore(&snapshot).expect("restore");
        assert_eq!(restored.tick(&mut world, Time::ZERO), Status::Failure);
    }

    #[test]
    fn restores_cooldown_timestamp() {
        fn build() -> crate::BehaviorTree<()> {
            BehaviorTreeBuilder::<()>::new("cooldown")
                .cooldown(Duration::from_millis(100), |b| {
                    b.action("hit", |_| Status::Success)
                })
                .build()
                .expect("tree")
        }

        let mut original = build();
        let mut world = ();
        assert_eq!(
            original.tick(&mut world, Time::from_millis(0, 0)),
            Status::Success
        );
        let snapshot = original.snapshot();

        let mut restored = build();
        restored.restore(&snapshot).expect("restore");

        assert_eq!(
            restored.tick(&mut world, Time::from_millis(50, 50)),
            Status::Failure
        );
        assert_eq!(
            restored.tick(&mut world, Time::from_millis(150, 100)),
            Status::Success
        );
    }

    #[test]
    fn rejects_unsupported_version_and_service_mismatch() {
        let mut tree = BehaviorTreeBuilder::<()>::new("version")
            .action("ok", |_| Status::Success)
            .build()
            .expect("tree");

        let mut bad = tree.snapshot();
        bad.version = 999;
        assert!(matches!(
            tree.restore(&bad),
            Err(RestoreError::UnsupportedVersion { .. })
        ));

        let snapshot = tree.snapshot();
        tree.add_service(Box::new(Action::<()>::new("service", |_| Status::Running)));
        assert!(matches!(
            tree.restore(&snapshot),
            Err(RestoreError::ServiceCountMismatch { .. })
        ));
    }

    #[test]
    fn rejects_different_tree_shape() {
        let mut left = BehaviorTreeBuilder::<()>::new("left")
            .sequence(|b| {
                b.action("a", |_| Status::Success)
                    .action("b", |_| Status::Running)
            })
            .build()
            .expect("left");

        let mut world = ();
        assert_eq!(left.tick(&mut world, Time::ZERO), Status::Running);
        let snapshot = left.snapshot();

        let mut right = BehaviorTreeBuilder::<()>::new("right")
            .sequence(|b| b.action("only", |_| Status::Success))
            .build()
            .expect("right");

        assert!(matches!(
            right.restore(&snapshot),
            Err(RestoreError::InvalidState { .. })
        ));
    }
}
