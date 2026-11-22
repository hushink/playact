//! 节点级运行统计。
//!
//! [`StatsTracer`] 通过共享句柄收集每个节点路径的 tick 次数、状态分布、
//! abort 次数与 wall-clock 耗时。安装到树上后，可随时从另一份 clone 读取
//! 快照，不需要访问 `BehaviorTree` 内部。
//!
//! 统计按事件路径区分节点；一个 `StatsTracer` 只应用于一棵树，否则不同树
//! 的相同路径会被合并。

use alloc::string::String;
use alloc::vec::Vec;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::node::Kind;
use crate::status::Status;
use crate::trace::{TraceEvent, Tracer};

/// 单个节点的累计统计。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeStats {
    /// 最近一次事件中的节点名。
    pub name: String,
    /// 节点类型。
    pub kind: Kind,
    /// 完成的 tick 次数。
    pub ticks: u64,
    /// 返回 `Success` 的次数。
    pub successes: u64,
    /// 返回 `Failure` 的次数。
    pub failures: u64,
    /// 返回 `Running` 的次数。
    pub running: u64,
    /// 被 abort 的次数。
    pub aborts: u64,
    /// 所有 tick 的累计 wall-clock 耗时。
    pub total_time: Duration,
    /// 单次 tick 的最长 wall-clock 耗时。
    pub max_time: Duration,
}

impl NodeStats {
    fn new(name: &str, kind: Kind) -> Self {
        Self {
            name: String::from(name),
            kind,
            ticks: 0,
            successes: 0,
            failures: 0,
            running: 0,
            aborts: 0,
            total_time: Duration::ZERO,
            max_time: Duration::ZERO,
        }
    }

    /// 平均 tick 耗时；尚未完成 tick 时返回零。
    #[must_use]
    pub fn average_time(&self) -> Duration {
        if self.ticks == 0 {
            Duration::ZERO
        } else {
            // Duration 只支持除以 u32；超过 u32::MAX 次的极端情况用上限近似，
            // 避免截断后除以 0。
            self.total_time / u32::try_from(self.ticks).unwrap_or(u32::MAX)
        }
    }

    fn record(&mut self, status: Status, elapsed: Duration) {
        self.ticks += 1;
        match status {
            Status::Success => self.successes += 1,
            Status::Failure => self.failures += 1,
            Status::Running => self.running += 1,
        }
        self.total_time += elapsed;
        self.max_time = self.max_time.max(elapsed);
    }
}

#[derive(Default)]
struct Inner {
    nodes: BTreeMap<Vec<usize>, NodeStats>,
    active: BTreeMap<Vec<usize>, Instant>,
}

/// 收集节点运行统计的 Tracer。
///
/// ```ignore
/// let stats = StatsTracer::new();
/// tree.set_tracer(Some(Box::new(stats.clone())));
/// tree.tick(&mut world, time);
/// for (path, node) in stats.nodes() {
///     println!("{path:?} {} ticks={} max={:?}", node.name, node.ticks, node.max_time);
/// }
/// ```
#[derive(Clone, Default)]
pub struct StatsTracer {
    inner: Arc<Mutex<Inner>>,
}

impl StatsTracer {
    /// 创建空统计器。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 所有节点的快照，按路径排序。
    #[must_use]
    pub fn nodes(&self) -> Vec<(Vec<usize>, NodeStats)> {
        self.lock()
            .nodes
            .iter()
            .map(|(path, stats)| (path.clone(), stats.clone()))
            .collect()
    }

    /// 指定路径的统计快照。
    #[must_use]
    pub fn node(&self, path: &[usize]) -> Option<NodeStats> {
        self.lock().nodes.get(path).cloned()
    }

    /// 已统计的节点数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().nodes.len()
    }

    /// 是否还没有统计到任何节点。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().nodes.is_empty()
    }

    /// 清空统计和进行中的计时。
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.nodes.clear();
        inner.active.clear();
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn entry<'a>(
        inner: &'a mut Inner,
        name: &str,
        kind: Kind,
        path: &[usize],
    ) -> &'a mut NodeStats {
        let stats = inner
            .nodes
            .entry(path.to_vec())
            .or_insert_with(|| NodeStats::new(name, kind));
        stats.name = String::from(name);
        stats.kind = kind;
        stats
    }
}

impl core::fmt::Debug for StatsTracer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StatsTracer")
            .field("nodes", &self.len())
            .finish()
    }
}

impl Tracer for StatsTracer {
    fn event(&mut self, event: TraceEvent<'_>) {
        let mut inner = self.lock();
        match event {
            TraceEvent::Enter {
                name, kind, path, ..
            } => {
                let path = path.to_vec();
                Self::entry(&mut inner, name, kind, &path);
                inner.active.insert(path, Instant::now());
            }
            TraceEvent::Exit {
                name,
                kind,
                path,
                status,
                ..
            } => {
                let path = path.to_vec();
                let elapsed = inner
                    .active
                    .remove(&path)
                    .map_or(Duration::ZERO, |start| start.elapsed());
                Self::entry(&mut inner, name, kind, &path).record(status, elapsed);
            }
            TraceEvent::Abort {
                name, kind, path, ..
            } => {
                let path = path.to_vec();
                inner.active.remove(&path);
                Self::entry(&mut inner, name, kind, &path).aborts += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::builder::BehaviorTreeBuilder;
    use crate::context::Context;
    use crate::time::Time;

    use super::*;

    #[test]
    fn collects_status_and_abort_counts() {
        let stats = StatsTracer::new();
        let mut tree = BehaviorTreeBuilder::<bool>::new("stats")
            .reactive_selector(|b| {
                b.condition("preempt", |ctx: &Context<'_, bool>| *ctx.world_ref())
                    .action("hold", |_| Status::Running)
            })
            .build()
            .expect("tree");
        tree.set_tracer(Some(Box::new(stats.clone())));

        let mut world = false;
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Running);
        world = true;
        assert_eq!(
            tree.tick(&mut world, Time::from_millis(100, 100)),
            Status::Success
        );

        let root = stats.node(&[]).expect("root stats");
        assert_eq!(root.kind, Kind::ReactiveSelector);
        assert_eq!(root.ticks, 2);
        assert_eq!(root.successes, 1);
        assert_eq!(root.running, 1);

        let preempt = stats.node(&[0]).expect("condition stats");
        assert_eq!(preempt.ticks, 2);
        assert_eq!(preempt.failures, 1);
        assert_eq!(preempt.successes, 1);

        let hold = stats.node(&[1]).expect("action stats");
        assert_eq!(hold.ticks, 1);
        assert_eq!(hold.running, 1);
        assert_eq!(hold.aborts, 1);
        assert!(hold.average_time() <= hold.max_time);

        assert_eq!(stats.len(), 3);
        assert!(!stats.is_empty());

        stats.clear();
        assert!(stats.is_empty());
        assert!(stats.node(&[]).is_none());
    }

    #[test]
    fn collects_nested_paths() {
        let stats = StatsTracer::new();
        let mut tree = BehaviorTreeBuilder::<()>::new("nested")
            .sequence(|b| {
                b.sequence(|b| b.action("inner", |_| Status::Success))
                    .action("outer", |_| Status::Success)
            })
            .build()
            .expect("tree");
        tree.set_tracer(Some(Box::new(stats.clone())));

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);

        assert!(stats.node(&[0, 0]).is_some());
        assert_eq!(stats.node(&[0, 0]).expect("inner").ticks, 1);
        assert_eq!(stats.node(&[1]).expect("outer").ticks, 1);
    }

    #[test]
    fn unused_action_is_not_counted() {
        let stats = StatsTracer::new();
        let mut tree = BehaviorTreeBuilder::<()>::new("unused")
            .sequence(|b| {
                b.condition("stop", |_| false)
                    .action("never", |_| Status::Success)
            })
            .build()
            .expect("tree");
        tree.set_tracer(Some(Box::new(stats.clone())));

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Failure);
        assert!(stats.node(&[1]).is_none());
    }

    #[test]
    fn stats_tracer_is_cloneable_and_debuggable() {
        let stats = StatsTracer::new();
        let cloned = stats.clone();
        assert!(format!("{cloned:?}").contains("StatsTracer"));
    }
}
