//! 行为树节点状态快照与恢复。
//!
//! 快照只包含节点内部状态和结构信息，不包含 world、黑板值、闭包捕获状态、
//! `AsyncAction` 任务、`SubtreeSlot` 动态子树或外部资源。恢复时应使用相同
//! 的树结构；节点自定义状态格式由节点自己负责。

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::node::BoxedNode;

/// 当前快照格式版本。
pub const SNAPSHOT_VERSION: u32 = 1;

/// 一棵行为树的状态快照。
///
/// `root` 与 `services` 中的字节由各节点的 `Node::save_state` 定义；`None`
/// 表示该节点（或其子树中的关键节点）不支持状态快照。快照不包含 world、
/// 黑板或闭包捕获状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeSnapshot {
    /// 快照格式版本，通常等于 [`SNAPSHOT_VERSION`]。
    pub version: u32,
    /// 根节点状态。
    pub root: Option<Vec<u8>>,
    /// 服务节点状态，顺序与服务注册顺序一致。
    pub services: Vec<Option<Vec<u8>>>,
}

/// 恢复快照时发现的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RestoreError {
    /// 快照版本不受支持。
    UnsupportedVersion {
        /// 快照中的版本。
        found: u32,
        /// 当前支持的版本。
        supported: u32,
    },
    /// 快照中的服务数量与当前树不一致。
    ServiceCountMismatch {
        /// 快照中的服务数量。
        snapshot: usize,
        /// 当前树的服务数量。
        tree: usize,
    },
    /// 节点拒绝了状态（通常意味着树结构变化或状态字节损坏）。
    InvalidState {
        /// 节点名或 `service[i] 名字`。
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

/// 节点状态的紧凑二进制编解码。
pub(crate) mod state {
    use alloc::vec::Vec;
    use core::time::Duration;

    /// 单个 `Vec` 长度上限，避免损坏/恶意字节触发超大分配。
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
