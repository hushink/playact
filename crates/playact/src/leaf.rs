//! Leaf nodes: the execution nodes that do the actual work.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;
use core::time::Duration;

use crate::blackboard::Key;
use crate::context::Context;
use crate::event::EventKey;
use crate::node::{Kind, Node};
use crate::snapshot::state;
use crate::status::Status;

type TickFn<W> = Box<dyn for<'a> FnMut(&mut Context<'a, W>) -> Status + Send>;
type TryTickFn<W, E> = Box<dyn for<'a> FnMut(&mut Context<'a, W>) -> Result<Status, E> + Send>;
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

    fn save_own_state(&self) -> Option<Vec<u8>> {
        Some(state::encode(&self.start))
    }

    fn load_own_state(&mut self, bytes: &[u8]) -> bool {
        let Some(start) = state::decode::<Option<Duration>>(bytes) else {
            return false;
        };
        self.start = start;
        true
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

/// Wait until a predicate becomes true.
///
/// Returns `Running` while the predicate is false and `Success` once it becomes
/// true. Unlike [`Condition`], a false predicate is not a failure.
pub struct WaitUntil<W> {
    name: String,
    predicate: PredicateFn<W>,
}

impl<W> WaitUntil<W> {
    /// Creates the node from a name and predicate.
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

impl<W> Node<W> for WaitUntil<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        if (self.predicate)(ctx) {
            Status::Success
        } else {
            Status::Running
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::WaitUntil
    }
}

/// Wait for the next event of a typed event queue.
///
/// The event is consumed in FIFO order. Use [`crate::EventKey::push`] from an
/// action or service to publish events.
pub struct WaitForEvent<W, T: Send + 'static> {
    name: String,
    key: EventKey<T>,
    _marker: PhantomData<fn() -> W>,
}

impl<W, T: Send + 'static> WaitForEvent<W, T> {
    /// Creates the node for the given event key.
    #[must_use]
    pub fn new(key: EventKey<T>) -> Self {
        Self {
            name: String::from("WaitForEvent"),
            key,
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

impl<W, T: Send + 'static> Node<W> for WaitForEvent<W, T> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        if self.key.pop(ctx).is_some() {
            Status::Success
        } else {
            Status::Running
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::WaitForEvent
    }
}

/// An action whose failure carries a user-defined error.
///
/// On `Err`, the error is reported through the installed [`crate::Tracer`] and
/// the node returns `Failure`. This keeps the core [`Node`] contract unchanged
/// while making network, file, and automation errors observable.
pub struct TryAction<W, E> {
    name: String,
    tick: TryTickFn<W, E>,
}

impl<W, E> TryAction<W, E>
where
    E: fmt::Display + Send + 'static,
{
    /// Creates the node from a name and fallible tick closure.
    pub fn new<F>(name: impl Into<String>, tick: F) -> Self
    where
        F: for<'a> FnMut(&mut Context<'a, W>) -> Result<Status, E> + Send + 'static,
    {
        Self {
            name: name.into(),
            tick: Box::new(tick),
        }
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W, E> Node<W> for TryAction<W, E>
where
    E: fmt::Display + Send + 'static,
{
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        match (self.tick)(ctx) {
            Ok(status) => status,
            Err(error) => {
                ctx.report_error(&self.name, Kind::TryAction, &error);
                Status::Failure
            }
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::TryAction
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
    use crate::trace::{TraceEvent, Tracer};

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
    fn wait_until_turns_success_when_predicate_holds() {
        let mut node = WaitUntil::<bool>::new("ready", |ctx| *ctx.world_ref());
        let mut world = false;
        let mut bb = Blackboard::new();
        let mut ctx = Context::new(&mut world, &mut bb, Time::ZERO);
        assert_eq!(node.tick(&mut ctx), Status::Running);

        *ctx.world() = true;
        assert_eq!(node.tick(&mut ctx), Status::Success);
        assert_eq!(node.kind(), Kind::WaitUntil);
    }

    #[test]
    fn wait_for_event_consumes_typed_events_in_fifo_order() {
        static EVENTS: EventKey<u32> = EventKey::new("test.events");

        let mut producer = Action::<()>::new("emit", |ctx| {
            EVENTS.push(ctx, 7);
            EVENTS.push(ctx, 8);
            Status::Success
        });
        let mut waiter = WaitForEvent::<(), u32>::new(EVENTS);
        let mut world = ();
        let mut bb = Blackboard::new();
        let mut ctx = Context::new(&mut world, &mut bb, Time::ZERO);

        assert_eq!(producer.tick(&mut ctx), Status::Success);
        assert_eq!(EVENTS.len(&ctx), 2);
        assert_eq!(waiter.tick(&mut ctx), Status::Success);
        assert_eq!(waiter.tick(&mut ctx), Status::Success);
        assert_eq!(waiter.tick(&mut ctx), Status::Running);
        assert_eq!(waiter.kind(), Kind::WaitForEvent);
    }

    #[derive(Clone, Default)]
    struct ErrorSink(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

    impl Tracer for ErrorSink {
        fn event(&mut self, event: TraceEvent<'_>) {
            if let TraceEvent::Error { message, .. } = event {
                self.0
                    .lock()
                    .expect("error sink lock")
                    .push(String::from(message));
            }
        }
    }

    #[test]
    fn try_action_reports_the_error_and_fails() {
        let sink = ErrorSink::default();
        let mut tracer = sink.clone();
        let mut node = TryAction::<(), &'static str>::new("fallible", |_| Err("boom"));
        let mut world = ();
        let mut bb = Blackboard::new();
        let mut ctx = Context::with_tracer(&mut world, &mut bb, Time::ZERO, Some(&mut tracer));

        assert_eq!(node.tick(&mut ctx), Status::Failure);
        assert_eq!(node.kind(), Kind::TryAction);
        assert_eq!(
            sink.0
                .lock()
                .expect("error sink lock")
                .first()
                .map(String::as_str),
            Some("boom")
        );
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
