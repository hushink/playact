//! 组合节点：只编排 `Status` 流转，不碰业务。

use core::num::NonZeroUsize;

use crate::context::Context;
use crate::error::BuildError;
use crate::node::{BoxedNode, Kind, Node};
use crate::status::Status;
use crate::trace::AbortReason;

/// `Parallel` 的成功/失败阈值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Threshold {
    /// 所有子节点都满足。
    All,
    /// 任一子节点满足。
    Any,
    /// 至少 `n` 个子节点满足。
    AtLeast(NonZeroUsize),
}

impl Threshold {
    /// 在 `total` 个子节点中，已有 `count` 个满足时是否达到阈值。
    #[must_use]
    pub fn is_satisfied(self, count: usize, total: usize) -> bool {
        match self {
            Self::All => count == total,
            Self::Any => count > 0,
            Self::AtLeast(n) => count >= n.get(),
        }
    }
}

// ==================== 记忆式顺序 ====================

/// 记忆式顺序节点。
///
/// 记住当前 Running 的子节点，不重跑已经 Success 的步骤。适合连招、
/// 多步交互等“一旦开始就做完”的流程。
pub struct Sequence<W> {
    name: String,
    children: Vec<BoxedNode<W>>,
    current: usize,
    running: bool,
}

impl<W> Sequence<W> {
    /// 用子节点构造，默认名为 `Sequence`。
    #[must_use]
    pub fn new(children: Vec<BoxedNode<W>>) -> Self {
        Self {
            name: String::from("Sequence"),
            children,
            current: 0,
            running: false,
        }
    }

    /// 覆盖显示名。
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for Sequence<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        while self.current < self.children.len() {
            let status = ctx.tick_child(self.current, &mut self.children[self.current]);
            match status {
                Status::Success => {
                    self.current += 1;
                    self.running = false;
                }
                Status::Failure => {
                    self.current = 0;
                    self.running = false;
                    return Status::Failure;
                }
                Status::Running => {
                    self.running = true;
                    return Status::Running;
                }
            }
        }
        self.current = 0;
        self.running = false;
        Status::Success
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        if self.running && self.current < self.children.len() {
            ctx.abort_child(
                self.current,
                &mut self.children[self.current],
                AbortReason::Preempted,
            );
        }
        self.current = 0;
        self.running = false;
    }

    fn reset(&mut self) {
        self.current = 0;
        self.running = false;
        for child in &mut self.children {
            child.reset();
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Sequence
    }

    fn children(&self) -> &[BoxedNode<W>] {
        &self.children
    }
}

// ==================== 反应式顺序 ====================

/// 反应式顺序节点。
///
/// 每 tick 从第一个子节点重新评估；如果之前的 Running 子节点被新的
/// Running/失败结果取代，会先 abort 旧子节点。适合“条件随时可能变化”
/// 的流程；连招请用 [`Sequence`]。
pub struct ReactiveSequence<W> {
    name: String,
    children: Vec<BoxedNode<W>>,
    last_running: Option<usize>,
}

impl<W> ReactiveSequence<W> {
    /// 用子节点构造，默认名为 `ReactiveSequence`。
    #[must_use]
    pub fn new(children: Vec<BoxedNode<W>>) -> Self {
        Self {
            name: String::from("ReactiveSequence"),
            children,
            last_running: None,
        }
    }

    /// 覆盖显示名。
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    fn abort_index(&mut self, ctx: &mut Context<'_, W>, index: usize) {
        if index < self.children.len() {
            ctx.abort_child(index, &mut self.children[index], AbortReason::Preempted);
        }
    }
}

impl<W> Node<W> for ReactiveSequence<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        let mut previous = self.last_running.take();

        for index in 0..self.children.len() {
            if let Some(old) = previous
                && index > old
            {
                // 防御性处理：旧 Running 子节点已被越过，说明状态不一致。
                self.abort_index(ctx, old);
                previous = None;
            }

            let status = ctx.tick_child(index, &mut self.children[index]);
            match status {
                Status::Success => {
                    if previous == Some(index) {
                        previous = None;
                    }
                }
                Status::Failure => {
                    if let Some(old) = previous
                        && old != index
                    {
                        self.abort_index(ctx, old);
                    }
                    self.last_running = None;
                    return Status::Failure;
                }
                Status::Running => {
                    if let Some(old) = previous
                        && old != index
                    {
                        self.abort_index(ctx, old);
                    }
                    self.last_running = Some(index);
                    return Status::Running;
                }
            }
        }

        if let Some(old) = previous {
            // 正常不变量下 previous 已在上面的分支消费；保留兜底，防止未来
            // 改动漏掉 abort。
            self.abort_index(ctx, old);
        }
        self.last_running = None;
        Status::Success
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        if let Some(index) = self.last_running.take() {
            self.abort_index(ctx, index);
        }
    }

    fn reset(&mut self) {
        self.last_running = None;
        for child in &mut self.children {
            child.reset();
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::ReactiveSequence
    }

    fn children(&self) -> &[BoxedNode<W>] {
        &self.children
    }
}

// ==================== 记忆式选择 ====================

/// 记忆式选择节点。
///
/// 从左到右尝试；一旦某个分支进入 Running，后续 tick 优先继续它，
/// 只有它失败才回退到下一个分支。适合“Plan A 执行中不要切到 Plan B”
/// 的长流程；安全打断请用 [`ReactiveSelector`]。
pub struct Selector<W> {
    name: String,
    children: Vec<BoxedNode<W>>,
    current: usize,
    running: bool,
}

impl<W> Selector<W> {
    /// 用子节点构造，默认名为 `Selector`。
    #[must_use]
    pub fn new(children: Vec<BoxedNode<W>>) -> Self {
        Self {
            name: String::from("Selector"),
            children,
            current: 0,
            running: false,
        }
    }

    /// 覆盖显示名。
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for Selector<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        while self.current < self.children.len() {
            let status = ctx.tick_child(self.current, &mut self.children[self.current]);
            match status {
                Status::Failure => {
                    self.current += 1;
                    self.running = false;
                }
                Status::Success => {
                    self.current = 0;
                    self.running = false;
                    return Status::Success;
                }
                Status::Running => {
                    self.running = true;
                    return Status::Running;
                }
            }
        }
        self.current = 0;
        self.running = false;
        Status::Failure
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        if self.running && self.current < self.children.len() {
            ctx.abort_child(
                self.current,
                &mut self.children[self.current],
                AbortReason::Preempted,
            );
        }
        self.current = 0;
        self.running = false;
    }

    fn reset(&mut self) {
        self.current = 0;
        self.running = false;
        for child in &mut self.children {
            child.reset();
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Selector
    }

    fn children(&self) -> &[BoxedNode<W>] {
        &self.children
    }
}

// ==================== 反应式选择 ====================

/// 反应式选择节点。
///
/// 每 tick 从高优先级到低优先级重新评估；如果更高优先级分支开始
/// Running 或 Success，会 abort 之前低优先级的 Running 分支。这是
/// “低血量随时打断追击”的实现方式。
pub struct ReactiveSelector<W> {
    name: String,
    children: Vec<BoxedNode<W>>,
    last_running: Option<usize>,
}

impl<W> ReactiveSelector<W> {
    /// 用子节点构造，默认名为 `ReactiveSelector`。
    #[must_use]
    pub fn new(children: Vec<BoxedNode<W>>) -> Self {
        Self {
            name: String::from("ReactiveSelector"),
            children,
            last_running: None,
        }
    }

    /// 覆盖显示名。
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    fn abort_index(&mut self, ctx: &mut Context<'_, W>, index: usize) {
        if index < self.children.len() {
            ctx.abort_child(index, &mut self.children[index], AbortReason::Preempted);
        }
    }
}

impl<W> Node<W> for ReactiveSelector<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        let mut previous = self.last_running.take();

        for index in 0..self.children.len() {
            if let Some(old) = previous
                && index > old
            {
                // 防御性处理：正常不变量下 old 已在 index == old 时被消费。
                self.abort_index(ctx, old);
                previous = None;
            }

            let status = ctx.tick_child(index, &mut self.children[index]);
            match status {
                Status::Success => {
                    if let Some(old) = previous
                        && old != index
                    {
                        self.abort_index(ctx, old);
                    }
                    self.last_running = None;
                    return Status::Success;
                }
                Status::Failure => {
                    if previous == Some(index) {
                        previous = None;
                    }
                }
                Status::Running => {
                    if let Some(old) = previous
                        && old != index
                    {
                        self.abort_index(ctx, old);
                    }
                    self.last_running = Some(index);
                    return Status::Running;
                }
            }
        }

        if let Some(old) = previous {
            // 正常不变量下 previous 已在上面的分支消费；保留兜底，防止未来
            // 改动漏掉 abort。
            self.abort_index(ctx, old);
        }
        self.last_running = None;
        Status::Failure
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        if let Some(index) = self.last_running.take() {
            self.abort_index(ctx, index);
        }
    }

    fn reset(&mut self) {
        self.last_running = None;
        for child in &mut self.children {
            child.reset();
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::ReactiveSelector
    }

    fn children(&self) -> &[BoxedNode<W>] {
        &self.children
    }
}

// ==================== 随机选择 ====================

/// 带记忆的随机选择节点。
///
/// 需要选择新分支时，用确定性 PRNG 打乱子节点顺序；某个分支进入
/// `Running` 后，后续 tick 优先继续它，不会因为重新随机而丢弃。分支
/// 失败后才会在同一 tick 从剩余分支中随机尝试（跳过刚失败的分支）。
///
/// 相同 `seed` 与相同 tick 序列产生相同顺序；`reset()` 恢复初始 seed。
pub struct RandomSelector<W> {
    name: String,
    children: Vec<BoxedNode<W>>,
    seed: u64,
    rng: u64,
    order: Vec<usize>,
    current: Option<usize>,
}

impl<W> RandomSelector<W> {
    /// 用子节点和确定性 seed 构造。`seed = 0` 会被替换为固定非零值。
    #[must_use]
    pub fn new(children: Vec<BoxedNode<W>>, seed: u64) -> Self {
        let seed = if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        };
        let order = (0..children.len()).collect();

        Self {
            name: String::from("RandomSelector"),
            children,
            seed,
            rng: seed,
            order,
            current: None,
        }
    }

    /// 覆盖显示名。
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.rng;
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        self.rng = value;
        value.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn shuffle(&mut self) {
        for index in (1..self.order.len()).rev() {
            let swap_with = (self.next_u64() % (index as u64 + 1)) as usize;
            self.order.swap(index, swap_with);
        }
    }

    fn abort_index(&mut self, ctx: &mut Context<'_, W>, index: usize) {
        if index < self.children.len() {
            ctx.abort_child(index, &mut self.children[index], AbortReason::Preempted);
        }
    }
}

impl<W> Node<W> for RandomSelector<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        if self.children.is_empty() {
            return Status::Failure;
        }

        let mut skip = None;

        if let Some(index) = self.current {
            match ctx.tick_child(index, &mut self.children[index]) {
                Status::Success => {
                    self.current = None;
                    return Status::Success;
                }
                Status::Failure => {
                    skip = Some(index);
                    self.current = None;
                }
                Status::Running => return Status::Running,
            }
        }

        self.shuffle();

        for position in 0..self.order.len() {
            let index = self.order[position];
            if skip == Some(index) {
                continue;
            }

            match ctx.tick_child(index, &mut self.children[index]) {
                Status::Failure => continue,
                Status::Running => {
                    self.current = Some(index);
                    return Status::Running;
                }
                Status::Success => {
                    self.current = None;
                    return Status::Success;
                }
            }
        }

        self.current = None;
        Status::Failure
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        if let Some(index) = self.current.take() {
            self.abort_index(ctx, index);
        }
    }

    fn reset(&mut self) {
        self.current = None;
        self.rng = self.seed;
        for (index, slot) in self.order.iter_mut().enumerate() {
            *slot = index;
        }
        for child in &mut self.children {
            child.reset();
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::RandomSelector
    }

    fn children(&self) -> &[BoxedNode<W>] {
        &self.children
    }
}

// ==================== 并行 ====================

fn validate_parallel_thresholds(
    success: Threshold,
    failure: Threshold,
    children: usize,
) -> Result<(), BuildError> {
    if let Threshold::AtLeast(at_least) = success
        && at_least.get() > children
    {
        return Err(BuildError::InvalidSuccessThreshold { at_least, children });
    }
    if let Threshold::AtLeast(at_least) = failure
        && at_least.get() > children
    {
        return Err(BuildError::InvalidFailureThreshold { at_least, children });
    }
    Ok(())
}

/// 并行节点。
///
/// 同一 tick 先 tick 全部子节点，再按阈值汇总。失败优先：只要失败阈值
/// 达成，整体返回 Failure 并 abort 剩余 Running 子节点；否则检查成功
/// 阈值。空 `Parallel` 直接 Success。
///
/// 阈值按**单个 tick 的汇总**计算，不跨 tick 累计终态：两个子节点如果
/// 分别在不同 tick 返回 Success，`Threshold::All` 不会因为它们都成功过
/// 而成功；`Threshold::Any` 每次成功都会 abort 其余仍在 Running 的兄弟。
/// 需要“各并行任务分别完成一次后整体成功”的累积语义，请用
/// [`MemParallel`]。
///
/// 通过 [`Parallel::new`] 构造时允许“不可能满足”的阈值；需要构建期校验
/// 时使用 [`Parallel::try_new`]。
pub struct Parallel<W> {
    name: String,
    children: Vec<BoxedNode<W>>,
    success: Threshold,
    failure: Threshold,
    running: Vec<bool>,
}

impl<W> Parallel<W> {
    /// 构造并行节点，不做阈值校验。
    #[must_use]
    pub fn new(children: Vec<BoxedNode<W>>, success: Threshold, failure: Threshold) -> Self {
        let running = vec![false; children.len()];
        Self {
            name: String::from("Parallel"),
            children,
            success,
            failure,
            running,
        }
    }

    /// 构造并校验 `AtLeast(n)` 阈值不超过子节点数量。
    pub fn try_new(
        children: Vec<BoxedNode<W>>,
        success: Threshold,
        failure: Threshold,
    ) -> Result<Self, BuildError> {
        let node = Self::new(children, success, failure);
        node.validate()?;
        Ok(node)
    }

    /// 覆盖显示名。
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// 校验阈值。
    pub fn validate(&self) -> Result<(), BuildError> {
        validate_parallel_thresholds(self.success, self.failure, self.children.len())
    }

    fn abort_running(&mut self, ctx: &mut Context<'_, W>, reason: AbortReason) {
        for index in 0..self.children.len() {
            if self.running[index] {
                ctx.abort_child(index, &mut self.children[index], reason);
                self.running[index] = false;
            }
        }
    }
}

impl<W> Node<W> for Parallel<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        if self.children.is_empty() {
            return Status::Success;
        }

        let mut successes = 0usize;
        let mut failures = 0usize;

        for index in 0..self.children.len() {
            let status = ctx.tick_child(index, &mut self.children[index]);
            self.running[index] = status.is_running();
            match status {
                Status::Success => successes += 1,
                Status::Failure => failures += 1,
                Status::Running => {}
            }
        }

        let total = self.children.len();

        if self.failure.is_satisfied(failures, total) {
            self.abort_running(ctx, AbortReason::ParallelDone);
            return Status::Failure;
        }
        if self.success.is_satisfied(successes, total) {
            self.abort_running(ctx, AbortReason::ParallelDone);
            return Status::Success;
        }
        if self.running.iter().all(|running| !running) {
            return Status::Failure;
        }

        Status::Running
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        self.abort_running(ctx, AbortReason::Preempted);
    }

    fn reset(&mut self) {
        self.running.fill(false);
        for child in &mut self.children {
            child.reset();
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Parallel
    }

    fn children(&self) -> &[BoxedNode<W>] {
        &self.children
    }
}

/// 跨 tick 记住终态的并行节点。
///
/// 一次激活内，某个子节点返回 Success/Failure 后不再被 tick；终态计数会
/// 持续累计，直到成功或失败阈值满足。结论确定后 abort 剩余 Running 子节点，
/// 并把内部状态复位，下一次激活从头开始。
///
/// 与 [`Parallel`] 的区别：`Parallel` 只看单个 tick 的汇总；`MemParallel`
/// 记住“本激活内已经完成/失败”的子节点。空 `MemParallel` 直接 Success。
pub struct MemParallel<W> {
    name: String,
    children: Vec<BoxedNode<W>>,
    success: Threshold,
    failure: Threshold,
    states: Vec<MemChildState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemChildState {
    Pending,
    Running,
    Success,
    Failure,
}

impl<W> MemParallel<W> {
    /// 构造并行节点，不做阈值校验。
    #[must_use]
    pub fn new(children: Vec<BoxedNode<W>>, success: Threshold, failure: Threshold) -> Self {
        let states = vec![MemChildState::Pending; children.len()];
        Self {
            name: String::from("MemParallel"),
            children,
            success,
            failure,
            states,
        }
    }

    /// 构造并校验 `AtLeast(n)` 阈值不超过子节点数量。
    pub fn try_new(
        children: Vec<BoxedNode<W>>,
        success: Threshold,
        failure: Threshold,
    ) -> Result<Self, BuildError> {
        let node = Self::new(children, success, failure);
        node.validate()?;
        Ok(node)
    }

    /// 覆盖显示名。
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// 校验阈值。
    pub fn validate(&self) -> Result<(), BuildError> {
        validate_parallel_thresholds(self.success, self.failure, self.children.len())
    }

    fn abort_running(&mut self, ctx: &mut Context<'_, W>, reason: AbortReason) {
        for index in 0..self.children.len() {
            if self.states[index] == MemChildState::Running {
                ctx.abort_child(index, &mut self.children[index], reason);
            }
        }
    }

    fn reset_states(&mut self) {
        self.states.fill(MemChildState::Pending);
    }
}

impl<W> Node<W> for MemParallel<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        if self.children.is_empty() {
            return Status::Success;
        }

        let mut successes = 0usize;
        let mut failures = 0usize;
        let mut running = false;

        for index in 0..self.children.len() {
            match self.states[index] {
                MemChildState::Success => {
                    successes += 1;
                    continue;
                }
                MemChildState::Failure => {
                    failures += 1;
                    continue;
                }
                MemChildState::Pending | MemChildState::Running => {}
            }

            match ctx.tick_child(index, &mut self.children[index]) {
                Status::Success => {
                    self.states[index] = MemChildState::Success;
                    successes += 1;
                }
                Status::Failure => {
                    self.states[index] = MemChildState::Failure;
                    failures += 1;
                }
                Status::Running => {
                    self.states[index] = MemChildState::Running;
                    running = true;
                }
            }
        }

        let total = self.children.len();

        if self.failure.is_satisfied(failures, total) {
            self.abort_running(ctx, AbortReason::ParallelDone);
            self.reset_states();
            return Status::Failure;
        }
        if self.success.is_satisfied(successes, total) {
            self.abort_running(ctx, AbortReason::ParallelDone);
            self.reset_states();
            return Status::Success;
        }
        if !running {
            self.reset_states();
            return Status::Failure;
        }

        Status::Running
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        self.abort_running(ctx, AbortReason::Preempted);
        self.reset_states();
    }

    fn reset(&mut self) {
        self.reset_states();
        for child in &mut self.children {
            child.reset();
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::MemParallel
    }

    fn children(&self) -> &[BoxedNode<W>] {
        &self.children
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use crate::blackboard::Blackboard;
    use crate::decorator::Timeout;
    use crate::leaf::{Action, Condition};
    use crate::node::BoxedNode;
    use crate::time::Time;

    use super::*;

    fn ctx<'a>(world: &'a mut (), bb: &'a mut Blackboard) -> Context<'a, ()> {
        Context::new(world, bb, Time::from_millis(0, 100))
    }

    #[test]
    fn sequence_is_memory_based() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let calls_in_action = std::sync::Arc::clone(&calls);

        let mut tree = Sequence::new(vec![
            Box::new(Action::new("first", move |_| {
                calls_in_action.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Status::Success
            })),
            Box::new(Action::new("second", |_| Status::Running)),
        ]);

        let mut world = ();
        let mut bb = Blackboard::new();
        let mut c = ctx(&mut world, &mut bb);

        assert_eq!(tree.tick(&mut c), Status::Running);
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(tree.tick(&mut c), Status::Running);
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn reactive_sequence_rechecks_first_child() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let calls_in_action = std::sync::Arc::clone(&calls);

        let mut tree = ReactiveSequence::new(vec![
            Box::new(Action::new("first", move |_| {
                calls_in_action.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Status::Success
            })),
            Box::new(Action::new("second", |_| Status::Running)),
        ]);

        let mut world = ();
        let mut bb = Blackboard::new();
        let mut c = ctx(&mut world, &mut bb);

        assert_eq!(tree.tick(&mut c), Status::Running);
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(tree.tick(&mut c), Status::Running);
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[test]
    fn selector_memory_and_empty() {
        let mut empty = Selector::<()>::new(Vec::new());
        let mut world = ();
        let mut bb = Blackboard::new();
        let mut c = ctx(&mut world, &mut bb);
        assert_eq!(empty.tick(&mut c), Status::Failure);

        let mut tree = Selector::new(vec![
            Box::new(Action::new("a", |_| Status::Failure)),
            Box::new(Action::new("b", |_| Status::Success)),
        ]);
        assert_eq!(tree.tick(&mut c), Status::Success);
    }

    #[test]
    fn parallel_thresholds_and_validation() {
        let mut tree = Parallel::try_new(
            vec![
                Box::new(Action::new("a", |_| Status::Success)),
                Box::new(Action::new("b", |_| Status::Running)),
                Box::new(Action::new("c", |_| Status::Failure)),
            ],
            Threshold::AtLeast(NonZeroUsize::new(1).unwrap()),
            Threshold::All,
        )
        .unwrap();

        let mut world = ();
        let mut bb = Blackboard::new();
        let mut c = ctx(&mut world, &mut bb);
        assert_eq!(tree.tick(&mut c), Status::Success);

        let err = Parallel::<()>::try_new(
            vec![Box::new(Condition::new("never", |_| false))],
            Threshold::AtLeast(NonZeroUsize::new(2).unwrap()),
            Threshold::Any,
        )
        .err()
        .expect("threshold must be rejected");
        assert!(matches!(
            err,
            BuildError::InvalidSuccessThreshold { children: 1, .. }
        ));
    }

    #[test]
    fn empty_parallel_succeeds() {
        let mut tree = Parallel::<()>::new(Vec::new(), Threshold::All, Threshold::Any);
        let mut world = ();
        let mut bb = Blackboard::new();
        let mut c = ctx(&mut world, &mut bb);
        assert_eq!(tree.tick(&mut c), Status::Success);
    }

    #[test]
    fn reactive_sequence_aborts_old_running_child() {
        let aborts = Arc::new(AtomicU32::new(0));
        let aborts_cb = Arc::clone(&aborts);

        let low = Action::<bool>::new("low", |_| Status::Running).with_abort(
            move |_: &mut Context<'_, bool>| {
                aborts_cb.fetch_add(1, Ordering::Relaxed);
            },
        );

        let mut tree = ReactiveSequence::new(vec![
            Box::new(Condition::new("gate", |ctx: &Context<'_, bool>| {
                *ctx.world_ref()
            })) as BoxedNode<bool>,
            Box::new(low) as BoxedNode<bool>,
        ]);

        let mut world = true;
        let mut bb = Blackboard::new();

        let mut c = Context::new(&mut world, &mut bb, Time::ZERO);
        assert_eq!(tree.tick(&mut c), Status::Running);
        assert_eq!(aborts.load(Ordering::Relaxed), 0);

        *c.world() = false;
        assert_eq!(tree.tick(&mut c), Status::Failure);
        assert_eq!(aborts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn reactive_selector_aborts_preempted_branch() {
        let aborts = Arc::new(AtomicU32::new(0));
        let aborts_cb = Arc::clone(&aborts);

        let low = Action::<bool>::new("low", |_| Status::Running).with_abort(
            move |_: &mut Context<'_, bool>| {
                aborts_cb.fetch_add(1, Ordering::Relaxed);
            },
        );

        let high = Sequence::new(vec![
            Box::new(Condition::new("high_gate", |ctx: &Context<'_, bool>| {
                *ctx.world_ref()
            })) as BoxedNode<bool>,
            Box::new(Action::<bool>::new("high_action", |_| Status::Success)) as BoxedNode<bool>,
        ]);

        let mut tree = ReactiveSelector::new(vec![
            Box::new(high) as BoxedNode<bool>,
            Box::new(low) as BoxedNode<bool>,
        ]);

        let mut world = false;
        let mut bb = Blackboard::new();

        let mut c = Context::new(&mut world, &mut bb, Time::ZERO);
        assert_eq!(tree.tick(&mut c), Status::Running);
        assert_eq!(aborts.load(Ordering::Relaxed), 0);

        *c.world() = true;
        assert_eq!(tree.tick(&mut c), Status::Success);
        assert_eq!(aborts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn mem_parallel_accumulates_successes_across_ticks() {
        let a_calls = Arc::new(AtomicU32::new(0));
        let a_calls_in_action = Arc::clone(&a_calls);
        let b_calls = Arc::new(AtomicU32::new(0));
        let b_calls_in_action = Arc::clone(&b_calls);

        let a = Action::<()>::new("a", move |_| {
            a_calls_in_action.fetch_add(1, Ordering::Relaxed);
            Status::Success
        });
        let b = Action::<()>::new("b", move |_| {
            let attempt = b_calls_in_action.fetch_add(1, Ordering::Relaxed);
            if attempt == 0 {
                Status::Running
            } else {
                Status::Success
            }
        });

        let mut tree = MemParallel::new(
            vec![Box::new(a) as BoxedNode<()>, Box::new(b) as BoxedNode<()>],
            Threshold::All,
            Threshold::Any,
        );

        let mut world = ();
        let mut bb = Blackboard::new();
        let mut c = Context::new(&mut world, &mut bb, Time::ZERO);

        assert_eq!(tree.tick(&mut c), Status::Running);
        assert_eq!(a_calls.load(Ordering::Relaxed), 1);
        assert_eq!(b_calls.load(Ordering::Relaxed), 1);

        // A 已成功，不应被重复 tick；B 成功累计到 2/2。
        assert_eq!(tree.tick(&mut c), Status::Success);
        assert_eq!(a_calls.load(Ordering::Relaxed), 1);
        assert_eq!(b_calls.load(Ordering::Relaxed), 2);

        // 下一次激活重新开始。
        assert_eq!(tree.tick(&mut c), Status::Success);
        assert_eq!(a_calls.load(Ordering::Relaxed), 2);
        assert_eq!(b_calls.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn mem_parallel_keeps_failure_and_aborts_running() {
        let aborts = Arc::new(AtomicU32::new(0));
        let aborts_cb = Arc::clone(&aborts);

        let failing = Action::<()>::new("fail", |_| Status::Failure);
        let running = Action::<()>::new("running", |_| Status::Running).with_abort(
            move |_: &mut Context<'_, ()>| {
                aborts_cb.fetch_add(1, Ordering::Relaxed);
            },
        );

        let mut tree = MemParallel::new(
            vec![
                Box::new(failing) as BoxedNode<()>,
                Box::new(running) as BoxedNode<()>,
            ],
            Threshold::All,
            Threshold::Any,
        );

        let mut world = ();
        let mut bb = Blackboard::new();
        let mut c = Context::new(&mut world, &mut bb, Time::ZERO);

        assert_eq!(tree.tick(&mut c), Status::Failure);
        assert_eq!(aborts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn empty_mem_parallel_succeeds() {
        let mut tree = MemParallel::<()>::new(Vec::new(), Threshold::All, Threshold::Any);
        let mut world = ();
        let mut bb = Blackboard::new();
        let mut c = Context::new(&mut world, &mut bb, Time::ZERO);
        assert_eq!(tree.tick(&mut c), Status::Success);
    }

    #[test]
    fn random_selector_empty_fails() {
        let mut tree = RandomSelector::<()>::new(Vec::new(), 42);
        let mut world = ();
        let mut bb = Blackboard::new();
        let mut c = Context::new(&mut world, &mut bb, Time::ZERO);
        assert_eq!(tree.tick(&mut c), Status::Failure);
    }

    #[test]
    fn random_selector_aborts_current_branch() {
        let aborts = Arc::new(AtomicU32::new(0));
        let aborts_cb = Arc::clone(&aborts);

        let child = Action::<()>::new("running", |_| Status::Running).with_abort(
            move |_: &mut Context<'_, ()>| {
                aborts_cb.fetch_add(1, Ordering::Relaxed);
            },
        );

        let mut tree = RandomSelector::new(vec![Box::new(child) as BoxedNode<()>], 7);

        let mut world = ();
        let mut bb = Blackboard::new();
        let mut c = Context::new(&mut world, &mut bb, Time::ZERO);

        assert_eq!(tree.tick(&mut c), Status::Running);
        tree.abort(&mut c);
        assert_eq!(aborts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn random_selector_keeps_running_branch() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_in_action = Arc::clone(&calls);

        let child = Action::<()>::new("child", move |_| {
            let attempt = calls_in_action.fetch_add(1, Ordering::Relaxed);
            if attempt == 0 {
                Status::Running
            } else {
                Status::Success
            }
        });

        let mut tree = RandomSelector::new(vec![Box::new(child) as BoxedNode<()>], 7);

        let mut world = ();
        let mut bb = Blackboard::new();
        let mut c = Context::new(&mut world, &mut bb, Time::ZERO);

        assert_eq!(tree.tick(&mut c), Status::Running);
        assert_eq!(tree.tick(&mut c), Status::Success);
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    fn random_tree(seed: u64, log: Arc<Mutex<Vec<usize>>>) -> RandomSelector<()> {
        let mut children: Vec<BoxedNode<()>> = Vec::new();
        for index in 0..5 {
            let log = Arc::clone(&log);
            children.push(Box::new(Action::<()>::new("choice", move |_| {
                log.lock().expect("log lock").push(index);
                Status::Failure
            })));
        }
        RandomSelector::new(children, seed)
    }

    #[test]
    fn random_selector_is_deterministic_and_resettable() {
        let log_a = Arc::new(Mutex::new(Vec::new()));
        let log_b = Arc::new(Mutex::new(Vec::new()));

        let mut tree_a = random_tree(42, Arc::clone(&log_a));
        let mut tree_b = random_tree(42, Arc::clone(&log_b));

        let mut world = ();
        let mut bb = Blackboard::new();
        let mut c = Context::new(&mut world, &mut bb, Time::ZERO);
        assert_eq!(tree_a.tick(&mut c), Status::Failure);
        assert_eq!(tree_b.tick(&mut c), Status::Failure);

        let first_order = log_a.lock().expect("log lock").clone();
        assert_eq!(&first_order, &*log_b.lock().expect("log lock"));
        assert_eq!(first_order.len(), 5);

        log_a.lock().expect("log lock").clear();
        tree_a.reset();
        assert_eq!(tree_a.tick(&mut c), Status::Failure);
        assert_eq!(&first_order, &*log_a.lock().expect("log lock"));
    }

    #[test]
    fn parallel_failure_takes_priority_over_success() {
        let mut tree = Parallel::new(
            vec![
                Box::new(Action::<()>::new("ok", |_| Status::Success)) as BoxedNode<()>,
                Box::new(Action::<()>::new("fail", |_| Status::Failure)) as BoxedNode<()>,
            ],
            Threshold::Any,
            Threshold::Any,
        );

        let mut world = ();
        let mut bb = Blackboard::new();
        let mut c = Context::new(&mut world, &mut bb, Time::ZERO);

        assert_eq!(tree.tick(&mut c), Status::Failure);
    }

    #[test]
    fn reactive_selector_keeps_same_running_branch_without_abort() {
        let aborts = Arc::new(AtomicU32::new(0));
        let aborts_cb = Arc::clone(&aborts);

        let low = Action::<bool>::new("low", |_| Status::Running).with_abort(
            move |_: &mut Context<'_, bool>| {
                aborts_cb.fetch_add(1, Ordering::Relaxed);
            },
        );

        let mut tree = ReactiveSelector::new(vec![
            Box::new(Condition::new("high_gate", |ctx: &Context<'_, bool>| {
                *ctx.world_ref()
            })) as BoxedNode<bool>,
            Box::new(low) as BoxedNode<bool>,
        ]);

        let mut world = false;
        let mut bb = Blackboard::new();
        let mut c = Context::new(&mut world, &mut bb, Time::ZERO);

        assert_eq!(tree.tick(&mut c), Status::Running);
        assert_eq!(tree.tick(&mut c), Status::Running);
        assert_eq!(aborts.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn parallel_aborts_remaining_running_children() {
        let aborts = Arc::new(AtomicU32::new(0));
        let aborts_cb = Arc::clone(&aborts);

        let running = Action::<()>::new("running", |_| Status::Running).with_abort(
            move |_: &mut Context<'_, ()>| {
                aborts_cb.fetch_add(1, Ordering::Relaxed);
            },
        );

        let mut tree = Parallel::new(
            vec![
                Box::new(Action::<()>::new("done", |_| Status::Success)) as BoxedNode<()>,
                Box::new(running) as BoxedNode<()>,
            ],
            Threshold::Any,
            Threshold::Any,
        );

        let mut world = ();
        let mut bb = Blackboard::new();
        let mut c = Context::new(&mut world, &mut bb, Time::ZERO);

        assert_eq!(tree.tick(&mut c), Status::Success);
        assert_eq!(aborts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn timeout_aborts_running_child() {
        let aborts = Arc::new(AtomicU32::new(0));
        let aborts_cb = Arc::clone(&aborts);

        let child = Action::<()>::new("work", |_| Status::Running).with_abort(
            move |_: &mut Context<'_, ()>| {
                aborts_cb.fetch_add(1, Ordering::Relaxed);
            },
        );

        let mut tree = Timeout::new(Duration::from_millis(100), Box::new(child));

        let mut world = ();
        let mut bb = Blackboard::new();

        let mut c = Context::new(&mut world, &mut bb, Time::ZERO);
        assert_eq!(tree.tick(&mut c), Status::Running);
        assert_eq!(aborts.load(Ordering::Relaxed), 0);

        let mut c = Context::new(&mut world, &mut bb, Time::from_millis(150, 150));
        assert_eq!(tree.tick(&mut c), Status::Failure);
        assert_eq!(aborts.load(Ordering::Relaxed), 1);
    }
}
