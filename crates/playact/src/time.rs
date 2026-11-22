//! tick 时间。

use core::time::Duration;

/// 驱动方每个逻辑帧提供的时间。
///
/// `now` 应当是单调递增的模拟时间或系统单调时钟；行为树内部的
/// Cooldown / Timeout / Wait 都基于它计算，不直接读取系统时钟，
/// 因此测试可以用虚拟时间完全确定性地推进。
///
/// 如果驱动方重置 `now`（战斗重开、重连后把时钟归零），必须先调用
/// [`crate::BehaviorTree::reset`] 或 [`crate::BehaviorTree::abort`]，否则
/// 基于绝对时间的节点会观察到时间回退。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Time {
    /// 当前时间点。
    pub now: Duration,
    /// 距离上一逻辑帧的时长。
    pub dt: Duration,
}

impl Time {
    /// 零时间。
    pub const ZERO: Self = Self {
        now: Duration::ZERO,
        dt: Duration::ZERO,
    };

    /// 构造一个时间点。
    #[must_use]
    pub const fn new(now: Duration, dt: Duration) -> Self {
        Self { now, dt }
    }

    /// 用毫秒构造，方便示例与测试。
    #[must_use]
    pub const fn from_millis(now_ms: u64, dt_ms: u64) -> Self {
        Self {
            now: Duration::from_millis(now_ms),
            dt: Duration::from_millis(dt_ms),
        }
    }
}
