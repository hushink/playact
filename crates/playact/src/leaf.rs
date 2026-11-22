//! 叶子节点：真正干活的执行节点。

use alloc::boxed::Box;
use alloc::string::String;
use core::time::Duration;

use crate::context::Context;
use crate::node::{Kind, Node};
use crate::status::Status;

type TickFn<W> = Box<dyn for<'a> FnMut(&mut Context<'a, W>) -> Status + Send>;
type AbortFn<W> = Box<dyn for<'a> FnMut(&mut Context<'a, W>) + Send>;
type ResetFn = Box<dyn FnMut() + Send>;
type PredicateFn<W> = Box<dyn for<'a> Fn(&Context<'a, W>) -> bool + Send>;

/// 闭包动作节点。
///
/// 简单动作直接返回 `Success` / `Failure`；跨 tick 动作返回 `Running`
/// 并在闭包内部维护状态。
///
/// 需要外部副作用清理时用 [`Action::with_abort`]；需要重置闭包内部状态
/// 时用 [`Action::with_reset`]。二者都会在对应生命周期由引擎调用。
pub struct Action<W> {
    name: String,
    tick: TickFn<W>,
    abort: Option<AbortFn<W>>,
    reset: Option<ResetFn>,
}

impl<W> Action<W> {
    /// 用名字和 tick 闭包构造。
    pub fn new<F>(name: impl Into<String>, tick: F) -> Self
    where
        F: for<'a> FnMut(&mut Context<'a, W>) -> Status + Send + 'static,
    {
        Self {
            name: name.into(),
            tick: Box::new(tick),
            abort: None,
            reset: None,
        }
    }

    /// 覆盖显示名。
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// 注册被抢占时的外部清理回调。
    #[must_use]
    pub fn with_abort<F>(mut self, abort: F) -> Self
    where
        F: for<'a> FnMut(&mut Context<'a, W>) + Send + 'static,
    {
        self.abort = Some(Box::new(abort));
        self
    }

    /// 注册内部状态复位回调。
    #[must_use]
    pub fn with_reset<F>(mut self, reset: F) -> Self
    where
        F: FnMut() + Send + 'static,
    {
        self.reset = Some(Box::new(reset));
        self
    }
}

impl<W> Node<W> for Action<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        (self.tick)(ctx)
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        if let Some(abort) = &mut self.abort {
            abort(ctx);
        }
    }

    fn reset(&mut self) {
        if let Some(reset) = &mut self.reset {
            reset();
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Action
    }
}

/// 只读条件节点；`true` → Success，`false` → Failure。
pub struct Condition<W> {
    name: String,
    predicate: PredicateFn<W>,
}

impl<W> Condition<W> {
    /// 用名字和判断闭包构造。
    pub fn new<F>(name: impl Into<String>, predicate: F) -> Self
    where
        F: for<'a> Fn(&Context<'a, W>) -> bool + Send + 'static,
    {
        Self {
            name: name.into(),
            predicate: Box::new(predicate),
        }
    }

    /// 覆盖显示名。
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for Condition<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        Status::from_bool((self.predicate)(ctx))
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Condition
    }
}

/// 等待节点：在 `duration` 内返回 Running，到点后 Success。
pub struct Wait<W> {
    name: String,
    duration: Duration,
    start: Option<Duration>,
    _marker: core::marker::PhantomData<fn() -> W>,
}

impl<W> Wait<W> {
    /// 构造。
    #[must_use]
    pub fn new(duration: Duration) -> Self {
        Self {
            name: String::from("Wait"),
            duration,
            start: None,
            _marker: core::marker::PhantomData,
        }
    }

    /// 覆盖显示名。
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for Wait<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        let now = ctx.time().now;
        let start = *self.start.get_or_insert(now);

        if now.saturating_sub(start) >= self.duration {
            self.start = None;
            Status::Success
        } else {
            Status::Running
        }
    }

    fn abort(&mut self, _ctx: &mut Context<'_, W>) {
        self.start = None;
    }

    fn reset(&mut self) {
        self.start = None;
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Wait
    }
}

#[cfg(test)]
mod tests {
    use crate::blackboard::Blackboard;
    use crate::time::Time;

    use super::*;

    #[test]
    fn condition_maps_bool() {
        let mut node = Condition::<()>::new("always", |_| true);
        let mut world = ();
        let mut bb = Blackboard::new();
        let mut ctx = Context::new(&mut world, &mut bb, Time::ZERO);
        assert_eq!(node.tick(&mut ctx), Status::Success);

        let mut node = Condition::<()>::new("never", |_| false);
        assert_eq!(node.tick(&mut ctx), Status::Failure);
    }

    #[test]
    fn wait_uses_virtual_time() {
        let mut node = Wait::<()>::new(Duration::from_millis(100));
        let mut world = ();
        let mut bb = Blackboard::new();

        let mut ctx = Context::new(&mut world, &mut bb, Time::from_millis(0, 0));
        assert_eq!(node.tick(&mut ctx), Status::Running);

        let mut ctx = Context::new(&mut world, &mut bb, Time::from_millis(50, 50));
        assert_eq!(node.tick(&mut ctx), Status::Running);

        let mut ctx = Context::new(&mut world, &mut bb, Time::from_millis(100, 50));
        assert_eq!(node.tick(&mut ctx), Status::Success);
    }

    #[test]
    fn action_abort_and_reset_callbacks() {
        let aborted = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let reset = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

        let aborted_in_cb = std::sync::Arc::clone(&aborted);
        let reset_in_cb = std::sync::Arc::clone(&reset);

        let mut node = Action::<()>::new("a", |_| Status::Running)
            .with_abort(move |_| {
                aborted_in_cb.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            })
            .with_reset(move || {
                reset_in_cb.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            });

        let mut world = ();
        let mut bb = Blackboard::new();
        let mut ctx = Context::new(&mut world, &mut bb, Time::ZERO);

        node.abort(&mut ctx);
        node.reset();
        assert_eq!(aborted.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(reset.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
}
