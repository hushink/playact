//! The tick context.

use alloc::vec::Vec;
use core::fmt;

use crate::blackboard::Blackboard;
use crate::node::{BoxedNode, Kind};
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

/// The context passed to a node on every tick.
///
/// All fields are private: nodes cannot replace the world/blackboard/time, nor
/// bypass path and Tracer semantics. Custom composites should use
/// [`Context::tick_child`] and [`Context::abort_child`] to reuse tracing and
/// abort/reset propagation.
pub struct Context<'a, W> {
    world: &'a mut W,
    blackboard: &'a mut Blackboard,
    time: Time,
    trace: TraceState<'a>,
}

impl<'a, W> Context<'a, W> {
    /// Create a context without a Tracer, mainly for unit tests that tick a node
    /// directly.
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

    /// Mutable access to the world.
    pub fn world(&mut self) -> &mut W {
        self.world
    }

    /// Read-only access to the world.
    #[must_use]
    pub fn world_ref(&self) -> &W {
        self.world
    }

    /// Mutable access to the blackboard.
    pub fn blackboard(&mut self) -> &mut Blackboard {
        self.blackboard
    }

    /// Read-only access to the blackboard.
    #[must_use]
    pub fn blackboard_ref(&self) -> &Blackboard {
        self.blackboard
    }

    /// The current time.
    #[must_use]
    pub fn time(&self) -> Time {
        self.time
    }

    /// Reports a recoverable node error through the installed [`Tracer`].
    ///
    /// The message is formatted only when a tracer is installed. Use this from
    /// fallible actions or custom nodes that return `Failure` but want to
    /// preserve the underlying error for diagnostics.
    pub fn report_error(&mut self, name: &str, kind: Kind, error: impl fmt::Display) {
        if self.trace.tracer.is_none() {
            return;
        }

        let message = alloc::format!("{error}");
        let path = &self.trace.path;
        let time = self.time;
        if let Some(tracer) = self.trace.tracer.as_mut() {
            tracer.event(TraceEvent::Error {
                name,
                kind,
                path,
                message: &message,
                time,
            });
        }
    }

    /// Tick the child at child index `index`, emitting Enter/Exit events
    /// automatically.
    ///
    /// Built-in composite/decorator nodes tick their children through this.
    /// Custom composites should use it too, otherwise that subtree will not
    /// appear in the Tracer path.
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

    /// Abandon a Running child: emit an Abort event, then call `abort` followed
    /// by `reset`.
    ///
    /// Built-in composite/decorator nodes use it for preemption, timeouts, and
    /// when a Parallel finishes.
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

    /// Create a child context for service nodes that emits no Tracer events.
    ///
    /// Services share the world / blackboard / time with the main tree, but
    /// service subtrees do not enter the main tree's event path; callers should
    /// drop the returned context as soon as one service tick ends.
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
