//! Decorator nodes: they wrap a single child and change its status semantics.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::num::NonZeroUsize;
use core::time::Duration;

use crate::context::Context;
use crate::node::{BoxedNode, Kind, Node};
use crate::snapshot::state;
use crate::status::Status;
use crate::trace::AbortReason;

fn single_child<W>(child: &BoxedNode<W>) -> &[BoxedNode<W>] {
    core::slice::from_ref(child)
}

fn load_child_state<W>(child: &mut BoxedNode<W>, state: Option<Vec<u8>>) -> bool {
    match state {
        Some(state) => child.load_state(&state),
        None => true,
    }
}

// ==================== Result inversion ====================

/// Inverts Success / Failure; Running passes through.
pub struct Inverter<W> {
    name: String,
    child: BoxedNode<W>,
}

impl<W> Inverter<W> {
    /// Creates the node.
    #[must_use]
    pub fn new(child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("Inverter"),
            child,
        }
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for Inverter<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        match ctx.tick_child(0, &mut self.child) {
            Status::Success => Status::Failure,
            Status::Failure => Status::Success,
            Status::Running => Status::Running,
        }
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        ctx.abort_child(0, &mut self.child, AbortReason::Preempted);
    }

    fn reset(&mut self) {
        self.child.reset();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Inverter
    }

    fn children(&self) -> &[BoxedNode<W>] {
        single_child(&self.child)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        self.child.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> bool {
        self.child.load_state(state)
    }
}

// ==================== Constant results ====================

/// Returns Success for either terminal status of the child; Running passes through.
pub struct AlwaysSucceed<W> {
    name: String,
    child: BoxedNode<W>,
}

impl<W> AlwaysSucceed<W> {
    /// Creates the node.
    #[must_use]
    pub fn new(child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("AlwaysSucceed"),
            child,
        }
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for AlwaysSucceed<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        match ctx.tick_child(0, &mut self.child) {
            Status::Running => Status::Running,
            Status::Success | Status::Failure => Status::Success,
        }
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        ctx.abort_child(0, &mut self.child, AbortReason::Preempted);
    }

    fn reset(&mut self) {
        self.child.reset();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::AlwaysSucceed
    }

    fn children(&self) -> &[BoxedNode<W>] {
        single_child(&self.child)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        self.child.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> bool {
        self.child.load_state(state)
    }
}

/// Returns Failure for either terminal status of the child; Running passes through.
pub struct AlwaysFail<W> {
    name: String,
    child: BoxedNode<W>,
}

impl<W> AlwaysFail<W> {
    /// Creates the node.
    #[must_use]
    pub fn new(child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("AlwaysFail"),
            child,
        }
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for AlwaysFail<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        match ctx.tick_child(0, &mut self.child) {
            Status::Running => Status::Running,
            Status::Success | Status::Failure => Status::Failure,
        }
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        ctx.abort_child(0, &mut self.child, AbortReason::Preempted);
    }

    fn reset(&mut self) {
        self.child.reset();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::AlwaysFail
    }

    fn children(&self) -> &[BoxedNode<W>] {
        single_child(&self.child)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        self.child.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> bool {
        self.child.load_state(state)
    }
}

// ==================== Repeat ====================

/// Maximum number of iterations `Repeat` advances within a single tick.
///
/// Once exceeded it returns `Running` and continues on the next frame. This
/// keeps one logic frame from stalling or piling up a huge number of commands
/// even if the configuration specifies a very large `times`.
pub const MAX_REPEAT_STEPS_PER_TICK: usize = 1024;

/// Runs the child to Success `times` times in a row.
///
/// After each success the child is explicitly reset before the next round; at
/// most [`MAX_REPEAT_STEPS_PER_TICK`] iterations happen per tick, after which
/// `Running` is returned and the next frame continues.
pub struct Repeat<W> {
    name: String,
    child: BoxedNode<W>,
    times: NonZeroUsize,
    done: usize,
}

impl<W> Repeat<W> {
    /// Creates the node.
    #[must_use]
    pub fn new(times: NonZeroUsize, child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("Repeat"),
            child,
            times,
            done: 0,
        }
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for Repeat<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        let mut steps = 0usize;

        while self.done < self.times.get() {
            if steps >= MAX_REPEAT_STEPS_PER_TICK {
                return Status::Running;
            }

            match ctx.tick_child(0, &mut self.child) {
                Status::Success => {
                    self.done += 1;
                    steps += 1;
                    if self.done >= self.times.get() {
                        self.done = 0;
                        return Status::Success;
                    }
                    self.child.reset();
                }
                Status::Failure => {
                    self.done = 0;
                    return Status::Failure;
                }
                Status::Running => return Status::Running,
            }
        }
        self.done = 0;
        Status::Success
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        self.done = 0;
        ctx.abort_child(0, &mut self.child, AbortReason::Preempted);
    }

    fn reset(&mut self) {
        self.done = 0;
        self.child.reset();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Repeat
    }

    fn children(&self) -> &[BoxedNode<W>] {
        single_child(&self.child)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        Some(state::encode(&(self.done, self.child.save_state())))
    }

    fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some((done, child)) = state::decode::<(usize, Option<Vec<u8>>)>(bytes) else {
            return false;
        };
        if !load_child_state(&mut self.child, child) {
            return false;
        }
        self.done = done;
        true
    }
}

// ==================== Retry ====================

/// Retries the child after a failure, up to `max_attempts` times.
///
/// At most one attempt per tick: while the failure count has not reached the
/// limit it returns Running, yielding a frame; this avoids firing side effects
/// repeatedly within the same logic frame.
pub struct Retry<W> {
    name: String,
    child: BoxedNode<W>,
    max_attempts: NonZeroUsize,
    attempts: usize,
    child_running: bool,
}

impl<W> Retry<W> {
    /// Creates the node.
    #[must_use]
    pub fn new(max_attempts: NonZeroUsize, child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("Retry"),
            child,
            max_attempts,
            attempts: 0,
            child_running: false,
        }
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for Retry<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        if self.attempts >= self.max_attempts.get() {
            self.attempts = 0;
            self.child_running = false;
            return Status::Failure;
        }

        let status = ctx.tick_child(0, &mut self.child);
        self.child_running = status.is_running();

        match status {
            Status::Success => {
                self.attempts = 0;
                self.child_running = false;
                Status::Success
            }
            Status::Running => Status::Running,
            Status::Failure => {
                self.child_running = false;
                self.attempts += 1;
                if self.attempts >= self.max_attempts.get() {
                    self.attempts = 0;
                    Status::Failure
                } else {
                    Status::Running
                }
            }
        }
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        self.attempts = 0;
        if self.child_running {
            ctx.abort_child(0, &mut self.child, AbortReason::Preempted);
        }
        self.child_running = false;
    }

    fn reset(&mut self) {
        self.attempts = 0;
        self.child_running = false;
        self.child.reset();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Retry
    }

    fn children(&self) -> &[BoxedNode<W>] {
        single_child(&self.child)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        Some(state::encode(&(
            self.attempts,
            self.child_running,
            self.child.save_state(),
        )))
    }

    fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some((attempts, child_running, child)) =
            state::decode::<(usize, bool, Option<Vec<u8>>)>(bytes)
        else {
            return false;
        };
        if attempts > self.max_attempts.get() || !load_child_state(&mut self.child, child) {
            return false;
        }
        self.attempts = attempts;
        self.child_running = child_running;
        true
    }
}

// ==================== Until success / failure ====================

/// Ticks the child repeatedly until Success; both Failure and Running report
/// Running.
///
/// Only one attempt per tick, so it does not spin within a single frame. Wrap
/// an unbounded `UntilSuccess` in a `Timeout`.
pub struct UntilSuccess<W> {
    name: String,
    child: BoxedNode<W>,
    child_running: bool,
}

impl<W> UntilSuccess<W> {
    /// Creates the node.
    #[must_use]
    pub fn new(child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("UntilSuccess"),
            child,
            child_running: false,
        }
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for UntilSuccess<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        let status = ctx.tick_child(0, &mut self.child);
        self.child_running = status.is_running();
        match status {
            Status::Success => {
                self.child_running = false;
                Status::Success
            }
            Status::Failure | Status::Running => Status::Running,
        }
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        if self.child_running {
            ctx.abort_child(0, &mut self.child, AbortReason::Preempted);
        }
        self.child_running = false;
    }

    fn reset(&mut self) {
        self.child_running = false;
        self.child.reset();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::UntilSuccess
    }

    fn children(&self) -> &[BoxedNode<W>] {
        single_child(&self.child)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        Some(state::encode(&(
            self.child_running,
            self.child.save_state(),
        )))
    }

    fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some((child_running, child)) = state::decode::<(bool, Option<Vec<u8>>)>(bytes) else {
            return false;
        };
        if !load_child_state(&mut self.child, child) {
            return false;
        }
        self.child_running = child_running;
        true
    }
}

/// Ticks the child repeatedly until Failure, at which point the node returns Success.
pub struct UntilFailure<W> {
    name: String,
    child: BoxedNode<W>,
    child_running: bool,
}

impl<W> UntilFailure<W> {
    /// Creates the node.
    #[must_use]
    pub fn new(child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("UntilFailure"),
            child,
            child_running: false,
        }
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for UntilFailure<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        let status = ctx.tick_child(0, &mut self.child);
        self.child_running = status.is_running();
        match status {
            Status::Failure => {
                self.child_running = false;
                Status::Success
            }
            Status::Success | Status::Running => Status::Running,
        }
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        if self.child_running {
            ctx.abort_child(0, &mut self.child, AbortReason::Preempted);
        }
        self.child_running = false;
    }

    fn reset(&mut self) {
        self.child_running = false;
        self.child.reset();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::UntilFailure
    }

    fn children(&self) -> &[BoxedNode<W>] {
        single_child(&self.child)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        Some(state::encode(&(
            self.child_running,
            self.child.save_state(),
        )))
    }

    fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some((child_running, child)) = state::decode::<(bool, Option<Vec<u8>>)>(bytes) else {
            return false;
        };
        if !load_child_state(&mut self.child, child) {
            return false;
        }
        self.child_running = child_running;
        true
    }
}

// ==================== Cooldown ====================

/// Enters cooldown after the child succeeds; during cooldown it returns Failure without ticking the child.
pub struct Cooldown<W> {
    name: String,
    child: BoxedNode<W>,
    duration: Duration,
    last_success: Option<Duration>,
}

impl<W> Cooldown<W> {
    /// Creates the node.
    #[must_use]
    pub fn new(duration: Duration, child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("Cooldown"),
            child,
            duration,
            last_success: None,
        }
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for Cooldown<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        let now = ctx.time().now;
        if let Some(last) = self.last_success
            && now.saturating_sub(last) < self.duration
        {
            return Status::Failure;
        }

        let status = ctx.tick_child(0, &mut self.child);
        if status.is_success() {
            self.last_success = Some(now);
        }
        status
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        ctx.abort_child(0, &mut self.child, AbortReason::Preempted);
    }

    fn reset(&mut self) {
        self.last_success = None;
        self.child.reset();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Cooldown
    }

    fn children(&self) -> &[BoxedNode<W>] {
        single_child(&self.child)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        Some(state::encode(&(self.last_success, self.child.save_state())))
    }

    fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some((last_success, child)) =
            state::decode::<(Option<Duration>, Option<Vec<u8>>)>(bytes)
        else {
            return false;
        };
        if !load_child_state(&mut self.child, child) {
            return false;
        }
        self.last_success = last_success;
        true
    }
}

// ==================== Timeout ====================

/// Aborts the child and returns Failure once it has been Running longer than
/// `duration`.
///
/// With `duration` of 0 the first tick returns Failure immediately without
/// ticking the child, and `reset` is not called on a child that has never run.
pub struct Timeout<W> {
    name: String,
    child: BoxedNode<W>,
    duration: Duration,
    start: Option<Duration>,
    child_running: bool,
}

impl<W> Timeout<W> {
    /// Creates the node.
    #[must_use]
    pub fn new(duration: Duration, child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("Timeout"),
            child,
            duration,
            start: None,
            child_running: false,
        }
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for Timeout<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        let now = ctx.time().now;
        let start = *self.start.get_or_insert(now);

        if now.saturating_sub(start) >= self.duration {
            self.start = None;
            if self.child_running {
                ctx.abort_child(0, &mut self.child, AbortReason::Timeout);
            }
            self.child_running = false;
            return Status::Failure;
        }

        let status = ctx.tick_child(0, &mut self.child);
        self.child_running = status.is_running();

        match status {
            Status::Running => Status::Running,
            other => {
                self.start = None;
                self.child_running = false;
                other
            }
        }
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        self.start = None;
        if self.child_running {
            ctx.abort_child(0, &mut self.child, AbortReason::Preempted);
        }
        self.child_running = false;
    }

    fn reset(&mut self) {
        self.start = None;
        self.child_running = false;
        self.child.reset();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Timeout
    }

    fn children(&self) -> &[BoxedNode<W>] {
        single_child(&self.child)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        Some(state::encode(&(
            self.start,
            self.child_running,
            self.child.save_state(),
        )))
    }

    fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some((start, child_running, child)) =
            state::decode::<(Option<Duration>, bool, Option<Vec<u8>>)>(bytes)
        else {
            return false;
        };
        if !load_child_state(&mut self.child, child) {
            return false;
        }
        self.start = start;
        self.child_running = child_running;
        true
    }
}

// ==================== Usage limits ====================

/// Lets the child produce at most `max` terminal results; afterwards it returns
/// Failure directly.
///
/// `Failure` also counts. The count persists across ticks, but a global
/// `reset()` or the `abort→reset` performed when the engine abandons a Running
/// subtree clears it; in other words, `Limiter` is a per-activation quota. For a
/// quota that survives preemption, keep the count in a typed blackboard slot.
pub struct Limiter<W> {
    name: String,
    child: BoxedNode<W>,
    max: NonZeroUsize,
    used: usize,
}

impl<W> Limiter<W> {
    /// Creates the node.
    #[must_use]
    pub fn new(max: NonZeroUsize, child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("Limiter"),
            child,
            max,
            used: 0,
        }
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for Limiter<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        if self.used >= self.max.get() {
            return Status::Failure;
        }

        let status = ctx.tick_child(0, &mut self.child);
        if status.is_terminal() {
            self.used += 1;
        }
        status
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        ctx.abort_child(0, &mut self.child, AbortReason::Preempted);
    }

    fn reset(&mut self) {
        self.used = 0;
        self.child.reset();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Limiter
    }

    fn children(&self) -> &[BoxedNode<W>] {
        single_child(&self.child)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        Some(state::encode(&(self.used, self.child.save_state())))
    }

    fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some((used, child)) = state::decode::<(usize, Option<Vec<u8>>)>(bytes) else {
            return false;
        };
        if used > self.max.get() || !load_child_state(&mut self.child, child) {
            return false;
        }
        self.used = used;
        true
    }
}

type GuardFn<W> = Box<dyn for<'a> Fn(&Context<'a, W>) -> bool + Send>;

/// Conditional guard: ticks the child while the condition is true, otherwise
/// returns `Failure` directly.
///
/// The condition is re-evaluated every tick; when it turns false while the
/// child was `Running` on the previous tick, the child is `abort → reset`
/// first. Suited to reactive guards such as "keep chasing/casting only while
/// the target is still valid".
///
/// This is equivalent to the common `ReactiveSequence(condition, child)` shape,
/// but without the explicit condition leaf, and its semantics are more direct
/// in `describe()`/Tracer.
pub struct Guard<W> {
    name: String,
    condition: GuardFn<W>,
    child: BoxedNode<W>,
    running: bool,
}

impl<W> Guard<W> {
    /// Creates the node from a condition closure and a child.
    #[must_use]
    pub fn new<F>(condition: F, child: BoxedNode<W>) -> Self
    where
        F: for<'a> Fn(&Context<'a, W>) -> bool + Send + 'static,
    {
        Self {
            name: String::from("Guard"),
            condition: Box::new(condition),
            child,
            running: false,
        }
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for Guard<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        if !(self.condition)(ctx) {
            if self.running {
                ctx.abort_child(0, &mut self.child, AbortReason::Preempted);
                self.running = false;
            }
            return Status::Failure;
        }

        let status = ctx.tick_child(0, &mut self.child);
        self.running = status.is_running();
        status
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        if self.running {
            ctx.abort_child(0, &mut self.child, AbortReason::Preempted);
            self.running = false;
        }
    }

    fn reset(&mut self) {
        self.running = false;
        self.child.reset();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Guard
    }

    fn children(&self) -> &[BoxedNode<W>] {
        single_child(&self.child)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        Some(state::encode(&(self.running, self.child.save_state())))
    }

    fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some((running, child)) = state::decode::<(bool, Option<Vec<u8>>)>(bytes) else {
            return false;
        };
        if !load_child_state(&mut self.child, child) {
            return false;
        }
        self.running = running;
        true
    }
}

/// Time-window quota decorator.
///
/// Allows the child at most `max` terminal results within one `window`;
/// `Running` does not consume quota. When the quota is exhausted it returns
/// `Failure` directly without ticking the child. The window slides according to
/// `now` from [`Context::time`]; `window = 0` is equivalent to no throttling.
///
/// Compared with [`Limiter`]: `Limiter` caps a fixed count per activation;
/// `Throttle` restores quota by time window, making it suitable for limiting
/// the rate of key presses, abilities, or network requests.
pub struct Throttle<W> {
    name: String,
    child: BoxedNode<W>,
    window: Duration,
    max: NonZeroUsize,
    window_start: Option<Duration>,
    used: usize,
}

impl<W> Throttle<W> {
    /// Creates the node.
    #[must_use]
    pub fn new(window: Duration, max: NonZeroUsize, child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("Throttle"),
            child,
            window,
            max,
            window_start: None,
            used: 0,
        }
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for Throttle<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        let now = ctx.time().now;
        let expired = self
            .window_start
            .is_none_or(|start| now.saturating_sub(start) >= self.window);

        if expired {
            self.window_start = Some(now);
            self.used = 0;
        }

        if self.used >= self.max.get() {
            return Status::Failure;
        }

        let status = ctx.tick_child(0, &mut self.child);
        if status.is_terminal() {
            self.used += 1;
        }
        status
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        ctx.abort_child(0, &mut self.child, AbortReason::Preempted);
        self.window_start = None;
        self.used = 0;
    }

    fn reset(&mut self) {
        self.window_start = None;
        self.used = 0;
        self.child.reset();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Throttle
    }

    fn children(&self) -> &[BoxedNode<W>] {
        single_child(&self.child)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        Some(state::encode(&(
            self.window_start,
            self.used,
            self.child.save_state(),
        )))
    }

    fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some((window_start, used, child)) =
            state::decode::<(Option<Duration>, usize, Option<Vec<u8>>)>(bytes)
        else {
            return false;
        };
        if used > self.max.get() || !load_child_state(&mut self.child, child) {
            return false;
        }
        self.window_start = window_start;
        self.used = used;
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::blackboard::Blackboard;
    use crate::leaf::Action;
    use crate::time::Time;

    use super::*;

    fn context<'a>(world: &'a mut (), board: &'a mut Blackboard) -> Context<'a, ()> {
        Context::new(world, board, Time::ZERO)
    }

    #[test]
    fn repeat_runs_child_n_times() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_in_action = Arc::clone(&calls);

        let child = Action::<()>::new("count", move |_| {
            calls_in_action.fetch_add(1, Ordering::Relaxed);
            Status::Success
        });

        let mut node = Repeat::new(NonZeroUsize::new(3).unwrap(), Box::new(child));

        let mut world = ();
        let mut board = Blackboard::new();
        let mut ctx = context(&mut world, &mut board);

        assert_eq!(node.tick(&mut ctx), Status::Success);
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn repeat_is_bounded_per_tick() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_in_action = Arc::clone(&calls);
        let times = MAX_REPEAT_STEPS_PER_TICK + 7;

        let child = Action::<()>::new("count", move |_| {
            calls_in_action.fetch_add(1, Ordering::Relaxed);
            Status::Success
        });

        let mut node = Repeat::new(NonZeroUsize::new(times).unwrap(), Box::new(child));

        let mut world = ();
        let mut board = Blackboard::new();
        let mut ctx = context(&mut world, &mut board);

        assert_eq!(node.tick(&mut ctx), Status::Running);
        assert_eq!(
            calls.load(Ordering::Relaxed),
            MAX_REPEAT_STEPS_PER_TICK as u32
        );

        assert_eq!(node.tick(&mut ctx), Status::Success);
        assert_eq!(calls.load(Ordering::Relaxed), times as u32);
    }

    #[test]
    fn retry_attempts_once_per_tick() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_in_action = Arc::clone(&calls);

        let child = Action::<()>::new("fail", move |_| {
            calls_in_action.fetch_add(1, Ordering::Relaxed);
            Status::Failure
        });

        let mut node = Retry::new(NonZeroUsize::new(2).unwrap(), Box::new(child));

        let mut world = ();
        let mut board = Blackboard::new();
        let mut ctx = context(&mut world, &mut board);

        assert_eq!(node.tick(&mut ctx), Status::Running);
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        assert_eq!(node.tick(&mut ctx), Status::Failure);
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn until_success_retries_next_tick() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_in_action = Arc::clone(&calls);

        let child = Action::<()>::new("eventually", move |_| {
            let attempt = calls_in_action.fetch_add(1, Ordering::Relaxed);
            if attempt == 0 {
                Status::Failure
            } else {
                Status::Success
            }
        });

        let mut node = UntilSuccess::new(Box::new(child));

        let mut world = ();
        let mut board = Blackboard::new();
        let mut ctx = context(&mut world, &mut board);

        assert_eq!(node.tick(&mut ctx), Status::Running);
        assert_eq!(node.tick(&mut ctx), Status::Success);
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn limiter_stops_after_max_terminal_results() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_in_action = Arc::clone(&calls);

        let child = Action::<()>::new("once", move |_| {
            calls_in_action.fetch_add(1, Ordering::Relaxed);
            Status::Success
        });

        let mut node = Limiter::new(NonZeroUsize::new(1).unwrap(), Box::new(child));

        let mut world = ();
        let mut board = Blackboard::new();
        let mut ctx = context(&mut world, &mut board);

        assert_eq!(node.tick(&mut ctx), Status::Success);
        assert_eq!(node.tick(&mut ctx), Status::Failure);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn until_failure_succeeds_when_child_fails() {
        let child = Action::<()>::new("fail", |_| Status::Failure);
        let mut node = UntilFailure::new(Box::new(child));

        let mut world = ();
        let mut board = Blackboard::new();
        let mut ctx = context(&mut world, &mut board);

        assert_eq!(node.tick(&mut ctx), Status::Success);
    }

    #[test]
    fn zero_timeout_does_not_reset_untouched_child() {
        let resets = Arc::new(AtomicU32::new(0));
        let resets_in_cb = Arc::clone(&resets);

        let child = Action::<()>::new("work", |_| Status::Running).with_reset(move || {
            resets_in_cb.fetch_add(1, Ordering::Relaxed);
        });

        let mut node = Timeout::new(Duration::ZERO, Box::new(child));

        let mut world = ();
        let mut board = Blackboard::new();
        let mut ctx = context(&mut world, &mut board);

        assert_eq!(node.tick(&mut ctx), Status::Failure);
        assert_eq!(resets.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cooldown_starts_only_after_success() {
        let child = Action::<()>::new("fail", |_| Status::Failure);
        let mut node = Cooldown::new(Duration::from_millis(100), Box::new(child));

        let mut world = ();
        let mut board = Blackboard::new();
        let mut ctx = context(&mut world, &mut board);
        assert_eq!(node.tick(&mut ctx), Status::Failure);

        let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(50, 50));
        assert_eq!(node.tick(&mut ctx), Status::Failure);
    }

    #[test]
    fn guard_blocks_child_when_condition_is_false() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_in_action = Arc::clone(&calls);

        let child = Action::<bool>::new("work", move |_| {
            calls_in_action.fetch_add(1, Ordering::Relaxed);
            Status::Success
        });
        let mut guard = Guard::new(|ctx: &Context<'_, bool>| *ctx.world_ref(), Box::new(child));

        let mut world = false;
        let mut board = Blackboard::new();
        let mut ctx = Context::new(&mut world, &mut board, Time::ZERO);
        assert_eq!(guard.tick(&mut ctx), Status::Failure);
        assert_eq!(calls.load(Ordering::Relaxed), 0);

        world = true;
        let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(16, 16));
        assert_eq!(guard.tick(&mut ctx), Status::Success);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn guard_aborts_child_when_condition_turns_false() {
        let aborts = Arc::new(AtomicU32::new(0));
        let aborts_cb = Arc::clone(&aborts);

        let child = Action::<bool>::new("hold", |_| Status::Running).with_abort(
            move |_: &mut Context<'_, bool>| {
                aborts_cb.fetch_add(1, Ordering::Relaxed);
            },
        );
        let mut guard = Guard::new(|ctx: &Context<'_, bool>| *ctx.world_ref(), Box::new(child));

        let mut world = true;
        let mut board = Blackboard::new();
        let mut ctx = Context::new(&mut world, &mut board, Time::ZERO);
        assert_eq!(guard.tick(&mut ctx), Status::Running);

        world = false;
        let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(16, 16));
        assert_eq!(guard.tick(&mut ctx), Status::Failure);
        assert_eq!(aborts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn throttle_limits_terminal_results_per_window() {
        let mut node = Throttle::new(
            Duration::from_millis(100),
            NonZeroUsize::new(2).expect("non-zero"),
            Box::new(Action::<()>::new("ok", |_| Status::Success)),
        );

        let mut world = ();
        let mut board = Blackboard::new();

        let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(0, 50));
        assert_eq!(node.tick(&mut ctx), Status::Success);

        let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(50, 50));
        assert_eq!(node.tick(&mut ctx), Status::Success);

        let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(99, 49));
        assert_eq!(node.tick(&mut ctx), Status::Failure);

        // The window starts at now=0 on the first tick; quota is restored at now=100.
        let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(100, 1));
        assert_eq!(node.tick(&mut ctx), Status::Success);
    }

    #[test]
    fn throttle_does_not_count_running_ticks() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_in_action = Arc::clone(&calls);

        let child = Action::<()>::new("eventually", move |_| {
            if calls_in_action.fetch_add(1, Ordering::Relaxed) < 2 {
                Status::Running
            } else {
                Status::Success
            }
        });
        let mut node = Throttle::new(
            Duration::from_secs(1),
            NonZeroUsize::new(1).expect("non-zero"),
            Box::new(child),
        );

        let mut world = ();
        let mut board = Blackboard::new();

        let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(0, 10));
        assert_eq!(node.tick(&mut ctx), Status::Running);
        let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(10, 10));
        assert_eq!(node.tick(&mut ctx), Status::Running);
        let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(20, 10));
        assert_eq!(node.tick(&mut ctx), Status::Success);
        let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(30, 10));
        assert_eq!(node.tick(&mut ctx), Status::Failure);

        node.reset();
        let mut ctx = Context::new(&mut world, &mut board, Time::from_millis(40, 10));
        assert_eq!(node.tick(&mut ctx), Status::Success);
        assert_eq!(calls.load(Ordering::Relaxed), 4);
    }
}
