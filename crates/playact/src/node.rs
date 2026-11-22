//! 节点契约与节点类型标签。

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use crate::context::Context;
use crate::status::Status;

/// 节点类型标签。
///
/// 内建节点使用固定变体，享有编译期穷尽性；自定义节点使用
/// [`Kind::Custom`] 携带一个 `'static` 的可读名字。
///
/// `Kind` 只是类型标签，不承载参数或运行状态。配置层的 `SpecNode`
/// 使用自己的 `type` 标签，运行期不会通过字符串查表。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Kind {
    /// 记忆式顺序节点。
    Sequence,
    /// 反应式顺序节点。
    ReactiveSequence,
    /// 记忆式选择节点。
    Selector,
    /// 反应式选择节点。
    ReactiveSelector,
    /// 带记忆的随机选择节点。
    RandomSelector,
    /// 带权重、确定性 seed 的随机选择节点。
    WeightedRandomSelector,
    /// 按动态评分选择子节点的效用选择节点。
    UtilitySelector,
    /// 条件成立才执行子节点。
    Guard,
    /// 按条件选择 then/else 分支。
    IfThenElse,
    /// 并行节点。
    Parallel,
    /// 跨 tick 记住终态的并行节点。
    MemParallel,
    /// 第一个终态获胜的并行节点。
    Race,
    /// 结果反转。
    Inverter,
    /// 始终成功。
    AlwaysSucceed,
    /// 始终失败。
    AlwaysFail,
    /// 重复执行。
    Repeat,
    /// 失败重试。
    Retry,
    /// 直到成功。
    UntilSuccess,
    /// 直到失败。
    UntilFailure,
    /// 成功冷却。
    Cooldown,
    /// 运行超时。
    Timeout,
    /// 次数限制。
    Limiter,
    /// 时间窗口配额。
    Throttle,
    /// 等待时间。
    Wait,
    /// 闭包动作。
    Action,
    /// 异步/远程动作。
    AsyncAction,
    /// 闭包条件。
    Condition,
    /// 写入黑板固定值。
    SetValue,
    /// 按黑板 key 动态切换的子树槽。
    SubtreeSlot,
    /// 整棵行为树作为节点。
    Tree,
    /// 自定义节点类型。
    Custom(&'static str),
}

impl Kind {
    /// 可读的 snake_case 名称；自定义 kind 返回其静态字符串。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sequence => "sequence",
            Self::ReactiveSequence => "reactive_sequence",
            Self::Selector => "selector",
            Self::ReactiveSelector => "reactive_selector",
            Self::RandomSelector => "random_selector",
            Self::WeightedRandomSelector => "weighted_random_selector",
            Self::UtilitySelector => "utility_selector",
            Self::Guard => "guard",
            Self::IfThenElse => "if_then_else",
            Self::Parallel => "parallel",
            Self::MemParallel => "mem_parallel",
            Self::Race => "race",
            Self::Inverter => "inverter",
            Self::AlwaysSucceed => "always_succeed",
            Self::AlwaysFail => "always_fail",
            Self::Repeat => "repeat",
            Self::Retry => "retry",
            Self::UntilSuccess => "until_success",
            Self::UntilFailure => "until_failure",
            Self::Cooldown => "cooldown",
            Self::Timeout => "timeout",
            Self::Limiter => "limiter",
            Self::Throttle => "throttle",
            Self::Wait => "wait",
            Self::Action => "action",
            Self::AsyncAction => "async_action",
            Self::Condition => "condition",
            Self::SetValue => "set_value",
            Self::SubtreeSlot => "subtree_slot",
            Self::Tree => "tree",
            Self::Custom(name) => name,
        }
    }

    /// 是否为自定义类型标签。
    #[must_use]
    pub const fn is_custom(self) -> bool {
        matches!(self, Self::Custom(_))
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 所有节点的统一契约。
///
/// `W` 是使用方提供的世界/客户端类型；引擎不关心它的具体内容。
///
/// # 生命周期契约
///
/// - [`Node::tick`] 每个逻辑帧调用一次。
/// - [`Node::abort`] 在一个仍在 `Running` 的子树被放弃时调用，用于释放按键、
///   取消技能等外部副作用；默认空实现。
/// - [`Node::reset`] 清除内部记忆，用于角色复活、战斗重开等全局复位；默认空实现。
/// - 引擎放弃 Running 子树时会先调用 `abort`，再调用 `reset`；两者都应幂等。
/// - 节点返回 `Success` / `Failure` 时必须自行恢复到可重新开始的状态。
pub trait Node<W>: Send {
    /// 推进一次逻辑帧。
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status;

    /// 放弃当前 Running 的子树时调用，负责外部副作用清理。
    fn abort(&mut self, _ctx: &mut Context<'_, W>) {}

    /// 清除内部记忆。全局复位或引擎在 abort 后调用。
    fn reset(&mut self) {}

    /// 调试显示名。
    fn name(&self) -> &str {
        "node"
    }

    /// 节点类型标签。
    fn kind(&self) -> Kind {
        Kind::Custom("node")
    }

    /// 直接子节点；默认空。用于 `describe()` 与自定义工具遍历。
    fn children(&self) -> &[BoxedNode<W>] {
        &[]
    }

    /// 导出节点内部状态；默认不支持，返回 `None`。
    ///
    /// 组合/装饰节点应把子树状态一起编码（内建节点已实现）；叶子可以返回
    /// `None`。状态字节的格式由节点自己定义，引擎只负责保存/恢复。
    fn save_state(&self) -> Option<Vec<u8>> {
        None
    }

    /// 载入 [`Node::save_state`] 产生的状态；返回 `false` 表示拒绝。
    fn load_state(&mut self, _state: &[u8]) -> bool {
        false
    }
}

/// 树中孩子的统一载体。
///
/// 异构树必须动态派发；`Send` 让树可以被移到线程池或工作线程。
pub type BoxedNode<W> = Box<dyn Node<W> + Send>;
