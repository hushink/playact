//! Leaf nodes: the execution nodes that do the actual work.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::marker::PhantomData;
use core::time::Duration;

use crate::blackboard::Key;
use crate::context::Context;
use crate::node::{Kind, Node};
use crate::snapshot::state;
use crate::status::Status;

type TickFn<W> = Box<dyn for<'a> FnMut(&mut Context<'a, W>) -> Status + Send>;
type AbortFn<W> = Box<dyn for<'a> FnMut(&mut Context<'a, W>) + Send>;
type ResetFn = Box<dyn FnMut() + Send>;
type PredicateFn<W> = Box<dyn for<'a> Fn(&Context<'a, W>) -> bool + Send>;

/// Closure-based action node.
///
/// Simple actions return `Success` / `Failure` directly; actions spanning
/// multiple ticks return `Running` and keep their state inside the closure.
///
/// Use [`Action::with_abort`] when external side effects need cleanup, and
/// [`Action::with_reset`] when the closure's internal state needs resetting.
/// The engine calls both at the corresponding lifecycle points.
pub struct Action<W> {
    name: String,
    tick: TickFn<W>,
    abort: Option<AbortFn<W>>,
    reset: Option<ResetFn>,
}

impl<W> Action<W> {
    /// Creates the node from a name and a tick closure.
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

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Registers an external cleanup callback invoked on preemption.
    #[must_use]
    pub fn with_abort<F>(mut self, abort: F) -> Self
    where
        F: for<'a> FnMut(&mut Context<'a, W>) + Send + 'static,
    {
        self.abort = Some(Box::new(abort));
        self
    }

    /// Registers a callback that resets internal state.
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

/// Read-only condition node; `true` → Success, `false` → Failure.
pub struct Condition<W> {
    name: String,
    predicate: PredicateFn<W>,
}

impl<W> Condition<W> {
    /// Creates the node from a name and a predicate closure.
    pub fn new<F>(name: impl Into<String>, predicate: F) -> Self
    where
        F: for<'a> Fn(&Context<'a, W>) -> bool + Send + 'static,
    {
        Self {
            name: name.into(),
            predicate: Box::new(predicate),
        }
    }

    /// Overrides the display name.
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

/// Wait node: returns Running for `duration`, then Success once it elapses.
pub struct Wait<W> {
    name: String,
    duration: Duration,
    start: Option<Duration>,
    _marker: core::marker::PhantomData<fn() -> W>,
}

impl<W> Wait<W> {
    /// Creates the node.
    #[must_use]
    pub fn new(duration: Duration) -> Self {
        Self {
            name: String::from("Wait"),
            duration,
            start: None,
            _marker: core::marker::PhantomData,
        }
    }

    /// Overrides the display name.
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

    fn save_state(&self) -> Option<Vec<u8>> {
        Some(state::encode(&self.start))
    }

    fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some(start) = state::decode::<Option<Duration>>(bytes) else {
            return false;
        };
        self.start = start;
        true
    }
}

/// Leaf node that writes a fixed value into the typed blackboard.
///
/// It writes on every tick and returns `Success`, so it also keeps the value
/// up to date inside loops such as `Repeat`; `T` must be `Clone`. `reset` does
/// not clear values already written to the blackboard.
pub struct SetValue<W, T: Send + 'static> {
    name: String,
    key: Key<T>,
    value: T,
    _marker: PhantomData<fn() -> W>,
}

impl<W, T: Send + 'static> SetValue<W, T> {
    /// Creates the node from a key and a fixed value.
    #[must_use]
    pub fn new(name: impl Into<String>, key: &Key<T>, value: T) -> Self {
        Self {
            name: name.into(),
            key: *key,
            value,
            _marker: PhantomData,
        }
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W, T: Clone + Send + 'static> Node<W> for SetValue<W, T> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        ctx.blackboard().set(&self.key, self.value.clone());
        Status::Success
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::SetValue
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
    fn set_value_writes_blackboard_each_tick() {
        static VALUE: Key<u32> = Key::new("test.set_value");

        let mut node = SetValue::<(), u32>::new("set", &VALUE, 7_u32);
        let mut world = ();
        let mut bb = Blackboard::new();

        {
            let mut ctx = Context::new(&mut world, &mut bb, Time::ZERO);
            assert_eq!(node.tick(&mut ctx), Status::Success);
        }
        assert_eq!(bb.get(&VALUE), Some(&7));

        bb.set(&VALUE, 9);
        {
            let mut ctx = Context::new(&mut world, &mut bb, Time::from_millis(16, 16));
            assert_eq!(node.tick(&mut ctx), Status::Success);
        }
        assert_eq!(bb.get(&VALUE), Some(&7));
        assert_eq!(node.kind(), Kind::SetValue);
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
