//! 运行期可观测性：结构化 trace 事件。

use core::fmt;

use crate::node::Kind;
use crate::status::Status;
use crate::time::Time;

/// 放弃一个 Running 子树的原因，只用于 Tracer 事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AbortReason {
    /// 被更高优先级的兄弟分支抢占。
    Preempted,
    /// Timeout 超时。
    Timeout,
    /// Parallel 已得出结论，停止剩余子节点。
    ParallelDone,
    /// 整棵树被外部 abort。
    TreeAbort,
}

impl fmt::Display for AbortReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Preempted => "preempted",
            Self::Timeout => "timeout",
            Self::ParallelDone => "parallel_done",
            Self::TreeAbort => "tree_abort",
        })
    }
}

/// 一条节点级 trace 事件。
///
/// `path` 是从根到当前节点的子索引路径：根为 `[]`，第一个孩子为 `[0]`，
/// 第二个孩子的第三个孩子为 `[1, 2]`。事件不包含 wall-clock 耗时，保证
/// 虚拟时间下的确定性。
#[derive(Debug, Clone, Copy)]
pub enum TraceEvent<'a> {
    /// 节点开始 tick。
    Enter {
        /// 节点名。
        name: &'a str,
        /// 节点类型。
        kind: Kind,
        /// 子索引路径。
        path: &'a [usize],
        /// 当前时间。
        time: Time,
    },
    /// 节点结束 tick。
    Exit {
        /// 节点名。
        name: &'a str,
        /// 节点类型。
        kind: Kind,
        /// 子索引路径。
        path: &'a [usize],
        /// 返回状态。
        status: Status,
        /// 当前时间。
        time: Time,
    },
    /// 一个 Running 子树被放弃。
    Abort {
        /// 节点名。
        name: &'a str,
        /// 节点类型。
        kind: Kind,
        /// 子索引路径。
        path: &'a [usize],
        /// 放弃原因。
        reason: AbortReason,
        /// 当前时间。
        time: Time,
    },
}

/// 可选的节点事件观察者。
///
/// 默认不安装 Tracer 时引擎只做一次 `Option` 判断，不产生事件、不分配。
/// Tracer 要求 `Send` 以便行为树随线程移动；不要求 `Sync`。
pub trait Tracer: Send {
    /// 接收一条事件。实现应尽量短小，避免阻塞 tick。
    fn event(&mut self, event: TraceEvent<'_>);
}

/// 一个把事件转成 `tracing` 日志的简单适配器。
#[cfg(feature = "tracing")]
#[derive(Debug, Default, Clone, Copy)]
pub struct TracingTracer;

#[cfg(feature = "tracing")]
impl Tracer for TracingTracer {
    fn event(&mut self, event: TraceEvent<'_>) {
        match event {
            TraceEvent::Enter {
                name,
                kind,
                path,
                time,
            } => tracing::debug!(
                target: "playact",
                node = name,
                kind = kind.as_str(),
                ?path,
                ?time,
                "enter"
            ),
            TraceEvent::Exit {
                name,
                kind,
                path,
                status,
                time,
            } => tracing::debug!(
                target: "playact",
                node = name,
                kind = kind.as_str(),
                ?path,
                %status,
                ?time,
                "exit"
            ),
            TraceEvent::Abort {
                name,
                kind,
                path,
                reason,
                time,
            } => tracing::debug!(
                target: "playact",
                node = name,
                kind = kind.as_str(),
                ?path,
                %reason,
                ?time,
                "abort"
            ),
        }
    }
}
