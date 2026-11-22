//! 装饰节点：包装一个子节点，改变其状态语义。

use alloc::string::String;
use core::num::NonZeroUsize;
use core::time::Duration;

use crate::context::Context;
use crate::node::{BoxedNode, Kind, Node};
use crate::status::Status;
use crate::trace::AbortReason;

fn single_child<W>(child: &BoxedNode<W>) -> &[BoxedNode<W>] {
    core::slice::from_ref(child)
}

// ==================== 结果翻转 ====================

/// 反转 Success / Failure，Running 透传。
pub struct Inverter<W> {
    name: String,
    child: BoxedNode<W>,
}

impl<W> Inverter<W> {
    /// 构造。
    #[must_use]
    pub fn new(child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("Inverter"),
            child,
        }
    }

    /// 覆盖显示名。
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
}

// ==================== 恒定结果 ====================

/// 无论子节点成功还是失败，终态都返回 Success；Running 透传。
pub struct AlwaysSucceed<W> {
    name: String,
    child: BoxedNode<W>,
}

impl<W> AlwaysSucceed<W> {
    /// 构造。
    #[must_use]
    pub fn new(child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("AlwaysSucceed"),
            child,
        }
    }

    /// 覆盖显示名。
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
}

/// 无论子节点成功还是失败，终态都返回 Failure；Running 透传。
pub struct AlwaysFail<W> {
    name: String,
    child: BoxedNode<W>,
}

impl<W> AlwaysFail<W> {
    /// 构造。
    #[must_use]
    pub fn new(child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("AlwaysFail"),
            child,
        }
    }

    /// 覆盖显示名。
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
}

// ==================== 重复 ====================

/// `Repeat` 单 tick 最多推进的迭代次数。
///
/// 超过后返回 `Running`，下一帧继续。这样即使配置里写了极大的 `times`，
/// 也不会让一个逻辑帧卡死或在一帧内堆积海量命令。
pub const MAX_REPEAT_STEPS_PER_TICK: usize = 1024;

/// 连续成功执行子节点 `times` 次。
///
/// 每成功一次显式 reset 子节点再开始下一轮；单 tick 最多执行
/// [`MAX_REPEAT_STEPS_PER_TICK`] 次，之后返回 `Running`，下一帧继续。
pub struct Repeat<W> {
    name: String,
    child: BoxedNode<W>,
    times: NonZeroUsize,
    done: usize,
}

impl<W> Repeat<W> {
    /// 构造。
    #[must_use]
    pub fn new(times: NonZeroUsize, child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("Repeat"),
            child,
            times,
            done: 0,
        }
    }

    /// 覆盖显示名。
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
}

// ==================== 重试 ====================

/// 子节点失败后重试，最多 `max_attempts` 次。
///
/// 每个 tick 最多尝试一次：失败但未到上限时返回 Running，让出一帧；
/// 避免同一逻辑帧内连续刷副作用。
pub struct Retry<W> {
    name: String,
    child: BoxedNode<W>,
    max_attempts: NonZeroUsize,
    attempts: usize,
    child_running: bool,
}

impl<W> Retry<W> {
    /// 构造。
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

    /// 覆盖显示名。
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
}

// ==================== 直到成功 / 失败 ====================

/// 反复 tick 子节点直到 Success；Failure/Running 都返回 Running。
///
/// 每 tick 只尝试一次，不会在同一帧内自旋。建议用 Timeout 包住无上限
/// 的 UntilSuccess。
pub struct UntilSuccess<W> {
    name: String,
    child: BoxedNode<W>,
    child_running: bool,
}

impl<W> UntilSuccess<W> {
    /// 构造。
    #[must_use]
    pub fn new(child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("UntilSuccess"),
            child,
            child_running: false,
        }
    }

    /// 覆盖显示名。
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
}

/// 反复 tick 子节点直到 Failure；此时整体返回 Success。
pub struct UntilFailure<W> {
    name: String,
    child: BoxedNode<W>,
    child_running: bool,
}

impl<W> UntilFailure<W> {
    /// 构造。
    #[must_use]
    pub fn new(child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("UntilFailure"),
            child,
            child_running: false,
        }
    }

    /// 覆盖显示名。
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
}

// ==================== 冷却 ====================

/// 子节点成功后进入冷却；冷却期间直接 Failure，不 tick 子节点。
pub struct Cooldown<W> {
    name: String,
    child: BoxedNode<W>,
    duration: Duration,
    last_success: Option<Duration>,
}

impl<W> Cooldown<W> {
    /// 构造。
    #[must_use]
    pub fn new(duration: Duration, child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("Cooldown"),
            child,
            duration,
            last_success: None,
        }
    }

    /// 覆盖显示名。
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
}

// ==================== 超时 ====================

/// 子节点 Running 超过 `duration` 后 abort 子节点并返回 Failure。
///
/// `duration` 为 0 时首次 tick 直接 Failure，不会 tick 子节点，也不会对
/// 尚未运行过的子节点调用 `reset`。
pub struct Timeout<W> {
    name: String,
    child: BoxedNode<W>,
    duration: Duration,
    start: Option<Duration>,
    child_running: bool,
}

impl<W> Timeout<W> {
    /// 构造。
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

    /// 覆盖显示名。
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
}

// ==================== 次数限制 ====================

/// 子节点最多产生 `max` 次终态结果；之后直接 Failure。
///
/// `Failure` 也计入次数。计数跨 tick 保留，但全局 `reset()` 或引擎放弃
/// Running 子树时的 `abort→reset` 会清零；也就是说 `Limiter` 是“每次激活”
/// 的配额。需要跨抢占保留的配额，请把计数放进黑板 typed slot。
pub struct Limiter<W> {
    name: String,
    child: BoxedNode<W>,
    max: NonZeroUsize,
    used: usize,
}

impl<W> Limiter<W> {
    /// 构造。
    #[must_use]
    pub fn new(max: NonZeroUsize, child: BoxedNode<W>) -> Self {
        Self {
            name: String::from("Limiter"),
            child,
            max,
            used: 0,
        }
    }

    /// 覆盖显示名。
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
}

/// 时间窗口配额装饰节点。
///
/// 在一个 `window` 内最多允许子节点产生 `max` 个终态结果；`Running` 不消耗
/// 配额。配额耗尽时直接返回 `Failure`，不 tick 子节点。窗口按
/// [`Context::time`] 的 `now` 滑动，`window = 0` 等价于不限流。
///
/// 与 [`Limiter`] 的区别：`Limiter` 是“每次激活”的固定次数；`Throttle` 按
/// 时间窗口恢复配额，适合限制按键、技能或网络请求频率。
pub struct Throttle<W> {
    name: String,
    child: BoxedNode<W>,
    window: Duration,
    max: NonZeroUsize,
    window_start: Option<Duration>,
    used: usize,
}

impl<W> Throttle<W> {
    /// 构造。
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

    /// 覆盖显示名。
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

        // 窗口从首次 tick 的 now=0 开始，now=100 时恢复配额。
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
