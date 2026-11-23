//! Per-node runtime statistics.
//!
//! [`StatsTracer`] uses a shared handle to collect tick counts, status
//! distribution, abort counts, and wall-clock time per node path. Once installed
//! on a tree, a snapshot can be read at any time from another clone without
//! accessing `BehaviorTree` internals.
//!
//! Statistics distinguish nodes by event path; use one `StatsTracer` per tree,
//! otherwise identical paths from different trees get merged.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::node::{Kind, NodeId};
use crate::status::Status;
use crate::trace::{TraceEvent, Tracer};

/// Accumulated statistics for a single node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeStats {
    /// Node name from the most recent event.
    pub name: String,
    /// Stable identity, when assigned.
    pub id: Option<NodeId>,
    /// Node type.
    pub kind: Kind,
    /// Number of completed ticks.
    pub ticks: u64,
    /// Number of `Success` returns.
    pub successes: u64,
    /// Number of `Failure` returns.
    pub failures: u64,
    /// Number of `Running` returns.
    pub running: u64,
    /// Number of aborts.
    pub aborts: u64,
    /// Number of reported errors.
    pub errors: u64,
    /// Status returned by the most recent completed tick.
    pub last_status: Option<Status>,
    /// Total wall-clock time across all ticks.
    pub total_time: Duration,
    /// Longest wall-clock time of a single tick.
    pub max_time: Duration,
}

impl NodeStats {
    fn new(name: &str, kind: Kind) -> Self {
        Self {
            name: String::from(name),
            id: None,
            kind,
            ticks: 0,
            successes: 0,
            failures: 0,
            running: 0,
            aborts: 0,
            errors: 0,
            last_status: None,
            total_time: Duration::ZERO,
            max_time: Duration::ZERO,
        }
    }

    /// Average tick duration; zero until a tick has completed.
    #[must_use]
    pub fn average_time(&self) -> Duration {
        if self.ticks == 0 {
            Duration::ZERO
        } else {
            // Duration only supports division by u32; in the extreme case of more
            // than u32::MAX ticks, saturate at the limit to avoid dividing by zero.
            self.total_time / u32::try_from(self.ticks).unwrap_or(u32::MAX)
        }
    }

    fn record(&mut self, status: Status, elapsed: Duration) {
        self.ticks += 1;
        self.last_status = Some(status);
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

/// A Tracer that collects per-node runtime statistics.
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
    /// Create an empty statistics collector.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of all nodes, sorted by path.
    #[must_use]
    pub fn nodes(&self) -> Vec<(Vec<usize>, NodeStats)> {
        self.lock()
            .nodes
            .iter()
            .map(|(path, stats)| (path.clone(), stats.clone()))
            .collect()
    }

    /// Snapshot of the statistics for the given path.
    #[must_use]
    pub fn node(&self, path: &[usize]) -> Option<NodeStats> {
        self.lock().nodes.get(path).cloned()
    }

    /// Snapshot of the statistics for the given stable identity.
    #[must_use]
    pub fn node_by_id(&self, id: &NodeId) -> Option<NodeStats> {
        self.lock()
            .nodes
            .values()
            .find(|stats| stats.id.as_ref() == Some(id))
            .cloned()
    }

    /// Number of nodes tracked so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().nodes.len()
    }

    /// Whether no node has been tracked yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().nodes.is_empty()
    }

    /// Renders current statistics in Prometheus text exposition format.
    ///
    /// Each node is labelled by child path, display name, kind, and stable ID.
    /// The output is suitable for a textfile collector or a scrape endpoint.
    #[must_use]
    pub fn render_prometheus(&self) -> String {
        let nodes = self.nodes();
        let mut out = String::new();

        write_metric_header(
            &mut out,
            "playact_node_ticks",
            "counter",
            "Completed ticks.",
        );
        for (path, node) in &nodes {
            write_node_metric(&mut out, "playact_node_ticks", path, node, node.ticks);
        }

        write_metric_header(
            &mut out,
            "playact_node_successes_total",
            "counter",
            "Successful completions.",
        );
        for (path, node) in &nodes {
            write_node_metric(
                &mut out,
                "playact_node_successes_total",
                path,
                node,
                node.successes,
            );
        }

        write_metric_header(
            &mut out,
            "playact_node_failures_total",
            "counter",
            "Failed completions.",
        );
        for (path, node) in &nodes {
            write_node_metric(
                &mut out,
                "playact_node_failures_total",
                path,
                node,
                node.failures,
            );
        }

        write_metric_header(
            &mut out,
            "playact_node_running_total",
            "counter",
            "Running results.",
        );
        for (path, node) in &nodes {
            write_node_metric(
                &mut out,
                "playact_node_running_total",
                path,
                node,
                node.running,
            );
        }

        write_metric_header(
            &mut out,
            "playact_node_aborts_total",
            "counter",
            "Aborted Running subtrees.",
        );
        for (path, node) in &nodes {
            write_node_metric(
                &mut out,
                "playact_node_aborts_total",
                path,
                node,
                node.aborts,
            );
        }

        write_metric_header(
            &mut out,
            "playact_node_errors_total",
            "counter",
            "Reported node errors.",
        );
        for (path, node) in &nodes {
            write_node_metric(
                &mut out,
                "playact_node_errors_total",
                path,
                node,
                node.errors,
            );
        }

        write_metric_header(
            &mut out,
            "playact_node_total_time_seconds",
            "gauge",
            "Cumulative wall-clock time spent in the node.",
        );
        for (path, node) in &nodes {
            let _ = writeln!(
                out,
                "playact_node_total_time_seconds{{{}}} {}",
                node_labels(path, node),
                node.total_time.as_secs_f64()
            );
        }

        write_metric_header(
            &mut out,
            "playact_node_max_time_seconds",
            "gauge",
            "Longest observed node tick.",
        );
        for (path, node) in &nodes {
            let _ = writeln!(
                out,
                "playact_node_max_time_seconds{{{}}} {}",
                node_labels(path, node),
                node.max_time.as_secs_f64()
            );
        }

        write_metric_header(
            &mut out,
            "playact_node_last_status",
            "gauge",
            "Last completed tick status, encoded as a labelled info metric.",
        );
        for (path, node) in &nodes {
            if let Some(status) = node.last_status {
                let _ = writeln!(
                    out,
                    "playact_node_last_status{{{},status=\"{}\"}} 1",
                    node_labels(path, node),
                    status_label(status)
                );
            }
        }

        out
    }

    /// Clear the statistics and any in-progress timings.
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
        id: Option<&NodeId>,
        name: &str,
        kind: Kind,
        path: &[usize],
    ) -> &'a mut NodeStats {
        let stats = inner
            .nodes
            .entry(path.to_vec())
            .or_insert_with(|| NodeStats::new(name, kind));
        stats.id = id.cloned();
        stats.name = String::from(name);
        stats.kind = kind;
        stats
    }
}

fn write_metric_header(out: &mut String, name: &str, kind: &str, help: &str) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
}

fn write_node_metric(out: &mut String, name: &str, path: &[usize], node: &NodeStats, value: u64) {
    let _ = writeln!(out, "{name}{{{}}} {value}", node_labels(path, node));
}

fn node_labels(path: &[usize], node: &NodeStats) -> String {
    let path = path
        .iter()
        .map(|index| index.to_string())
        .collect::<Vec<_>>()
        .join("/");
    let id = node.id.as_ref().map_or("", NodeId::as_str);
    format!(
        "path=\"{}\",name=\"{}\",kind=\"{}\",id=\"{}\"",
        escape_label(&path),
        escape_label(&node.name),
        node.kind.as_str(),
        escape_label(id)
    )
}

fn escape_label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

fn status_label(status: Status) -> &'static str {
    match status {
        Status::Success => "success",
        Status::Failure => "failure",
        Status::Running => "running",
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
                id,
                name,
                kind,
                path,
                ..
            } => {
                let path = path.to_vec();
                Self::entry(&mut inner, id, name, kind, &path);
                inner.active.insert(path, Instant::now());
            }
            TraceEvent::Exit {
                id,
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
                Self::entry(&mut inner, id, name, kind, &path).record(status, elapsed);
            }
            TraceEvent::Abort {
                id,
                name,
                kind,
                path,
                ..
            } => {
                let path = path.to_vec();
                inner.active.remove(&path);
                Self::entry(&mut inner, id, name, kind, &path).aborts += 1;
            }
            TraceEvent::Error {
                id,
                name,
                kind,
                path,
                ..
            } => {
                let path = path.to_vec();
                Self::entry(&mut inner, id, name, kind, &path).errors += 1;
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
        assert_eq!(root.last_status, Some(Status::Success));

        let preempt = stats.node(&[0]).expect("condition stats");
        assert_eq!(preempt.ticks, 2);
        assert_eq!(preempt.failures, 1);
        assert_eq!(preempt.successes, 1);

        let hold = stats.node(&[1]).expect("action stats");
        assert_eq!(hold.ticks, 1);
        assert_eq!(hold.running, 1);
        assert_eq!(hold.last_status, Some(Status::Running));
        assert_eq!(hold.aborts, 1);
        assert!(hold.average_time() <= hold.max_time);

        assert_eq!(stats.len(), 3);
        assert!(!stats.is_empty());

        stats.clear();
        assert!(stats.is_empty());
        assert!(stats.node(&[]).is_none());
    }

    #[test]
    fn counts_reported_errors() {
        let stats = StatsTracer::new();
        let mut tree = BehaviorTreeBuilder::<()>::new("errors")
            .try_action("fail", |_| Err::<Status, &str>("boom"))
            .build()
            .expect("tree");
        tree.set_tracer(Some(Box::new(stats.clone())));

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Failure);

        let root = stats.node(&[]).expect("root stats");
        assert_eq!(root.errors, 1);
        assert_eq!(root.failures, 1);
        assert_eq!(root.last_status, Some(Status::Failure));
    }

    #[test]
    fn stats_are_queryable_by_stable_id() {
        let stats = StatsTracer::new();
        let mut tree = BehaviorTreeBuilder::<()>::new("ids")
            .sequence(|b| b.action("work", |_| Status::Success).id("worker"))
            .build()
            .expect("tree");
        tree.set_tracer(Some(Box::new(stats.clone())));

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);

        let worker = stats
            .node_by_id(&NodeId::new("worker"))
            .expect("worker stats");
        assert_eq!(worker.name, "work");
        assert_eq!(worker.last_status, Some(Status::Success));
    }

    #[test]
    fn renders_prometheus_metrics() {
        let stats = StatsTracer::new();
        let mut tree = BehaviorTreeBuilder::<()>::new("metrics")
            .action("work", |_| Status::Success)
            .id("worker")
            .build()
            .expect("tree");
        tree.set_tracer(Some(Box::new(stats.clone())));

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);

        let text = stats.render_prometheus();
        assert!(text.contains("# TYPE playact_node_ticks counter"));
        assert!(text.contains(
            "playact_node_ticks{path=\"\",name=\"work\",kind=\"action\",id=\"worker\"} 1"
        ));
        assert!(text.contains(
            "playact_node_last_status{path=\"\",name=\"work\",kind=\"action\",id=\"worker\",status=\"success\"} 1"
        ));
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
