//! tick 上下文。

use alloc::vec::Vec;

use crate::blackboard::Blackboard;
use crate::node::BoxedNode;
use crate::status::Status;
use crate::time::Time;
use crate::trace::{AbortReason, TraceEvent, Tracer};

struct TraceState<'a> {
    tracer: Option<&'a mut (dyn Tracer + Send)>,
    path: Vec<usize>,
}

impl<'a> TraceState<'a> {
    fn new(tracer: Option<&'a mut (dyn Tracer + Send)>) -> Self {
        Self {
            tracer,
            path: Vec::new(),
        }
    }
}

/// 每次 tick 传入节点的上下文。
///
/// 字段全部私有：节点不能替换 world/blackboard/time，也不能绕过路径与
/// Tracer 语义。自定义组合节点应使用 [`Context::tick_child`] 和
/// [`Context::abort_child`]，以复用 tracing 与 abort/reset 传播。
pub struct Context<'a, W> {
    world: &'a mut W,
    blackboard: &'a mut Blackboard,
    time: Time,
    trace: TraceState<'a>,
}

impl<'a, W> Context<'a, W> {
    /// 创建一个不带 Tracer 的上下文，主要用于单测直接 tick 一个节点。
    #[must_use]
    pub fn new(world: &'a mut W, blackboard: &'a mut Blackboard, time: Time) -> Self {
        Self {
            world,
            blackboard,
            time,
            trace: TraceState::new(None),
        }
    }

    pub(crate) fn with_tracer(
        world: &'a mut W,
        blackboard: &'a mut Blackboard,
        time: Time,
        tracer: Option<&'a mut (dyn Tracer + Send)>,
    ) -> Self {
        Self {
            world,
            blackboard,
            time,
            trace: TraceState::new(tracer),
        }
    }

    /// 可变访问世界。
    pub fn world(&mut self) -> &mut W {
        self.world
    }

    /// 只读访问世界。
    #[must_use]
    pub fn world_ref(&self) -> &W {
        self.world
    }

    /// 可变访问黑板。
    pub fn blackboard(&mut self) -> &mut Blackboard {
        self.blackboard
    }

    /// 只读访问黑板。
    #[must_use]
    pub fn blackboard_ref(&self) -> &Blackboard {
        self.blackboard
    }

    /// 当前时间。
    #[must_use]
    pub fn time(&self) -> Time {
        self.time
    }

    /// 以子索引 `index` tick 一个子节点，并自动产生 Enter/Exit 事件。
    ///
    /// 内建组合/装饰节点都通过它 tick 子节点。自定义组合节点也应使用它，
    /// 否则该子树不会出现在 Tracer 路径中。
    pub fn tick_child(&mut self, index: usize, child: &mut BoxedNode<W>) -> Status {
        let kind = child.kind();
        let time = self.time;
        let tracing = self.trace.tracer.is_some();

        if tracing {
            self.trace.path.push(index);
            if let Some(tracer) = self.trace.tracer.as_mut() {
                tracer.event(TraceEvent::Enter {
                    name: child.name(),
                    kind,
                    path: &self.trace.path,
                    time,
                });
            }
        }

        let status = child.tick(self);

        if tracing {
            if let Some(tracer) = self.trace.tracer.as_mut() {
                tracer.event(TraceEvent::Exit {
                    name: child.name(),
                    kind,
                    path: &self.trace.path,
                    status,
                    time,
                });
            }
            self.trace.path.pop();
        }

        status
    }

    /// 放弃一个 Running 子节点：产生 Abort 事件，调用 `abort` 再调用 `reset`。
    ///
    /// 内建组合/装饰节点在抢占、超时、Parallel 结束时使用它。
    pub fn abort_child(&mut self, index: usize, child: &mut BoxedNode<W>, reason: AbortReason) {
        let kind = child.kind();
        let time = self.time;
        let tracing = self.trace.tracer.is_some();

        if tracing {
            self.trace.path.push(index);
            if let Some(tracer) = self.trace.tracer.as_mut() {
                tracer.event(TraceEvent::Abort {
                    name: child.name(),
                    kind,
                    path: &self.trace.path,
                    reason,
                    time,
                });
            }
        }

        child.abort(self);
        child.reset();

        if tracing {
            self.trace.path.pop();
        }
    }

    pub(crate) fn tick_root(&mut self, root: &mut BoxedNode<W>) -> Status {
        let kind = root.kind();
        let time = self.time;

        if let Some(tracer) = self.trace.tracer.as_mut() {
            tracer.event(TraceEvent::Enter {
                name: root.name(),
                kind,
                path: &[],
                time,
            });
        }

        let status = root.tick(self);

        if let Some(tracer) = self.trace.tracer.as_mut() {
            tracer.event(TraceEvent::Exit {
                name: root.name(),
                kind,
                path: &[],
                status,
                time,
            });
        }

        status
    }

    /// 为服务节点创建一个不产生 Tracer 事件的子上下文。
    ///
    /// 服务与主树共享 world / blackboard / time，但服务子树不会进入主树的
    /// 事件路径；调用方应在一次服务 tick 结束后立即丢弃返回的上下文。
    pub(crate) fn service_context(&mut self) -> Context<'_, W> {
        Context {
            world: &mut *self.world,
            blackboard: &mut *self.blackboard,
            time: self.time,
            trace: TraceState::new(None),
        }
    }

    pub(crate) fn abort_root(&mut self, root: &mut BoxedNode<W>, reason: AbortReason) {
        let kind = root.kind();
        let time = self.time;

        if let Some(tracer) = self.trace.tracer.as_mut() {
            tracer.event(TraceEvent::Abort {
                name: root.name(),
                kind,
                path: &[],
                reason,
                time,
            });
        }

        root.abort(self);
        root.reset();
    }
}
