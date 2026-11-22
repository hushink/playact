//! 核心构建错误。

use core::fmt;
use core::num::NonZeroUsize;

/// 构建行为树时发现的配置错误。
///
/// typestate 构建器已经用类型系统拦住了“根节点为空”“装饰器子节点数量
/// 不对”等问题；这里只处理仍需运行期校验的语义错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// `Parallel` 的成功阈值大于子节点数量，永远不可能满足。
    InvalidSuccessThreshold {
        /// 配置的 `AtLeast` 值。
        at_least: NonZeroUsize,
        /// 实际子节点数量。
        children: usize,
    },
    /// `Parallel` 的失败阈值大于子节点数量，永远不可能满足。
    InvalidFailureThreshold {
        /// 配置的 `AtLeast` 值。
        at_least: NonZeroUsize,
        /// 实际子节点数量。
        children: usize,
    },
    /// 内部构建状态不一致。正常使用不应出现。
    InvalidTree {
        /// 错误说明。
        message: &'static str,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSuccessThreshold { at_least, children } => write!(
                f,
                "parallel success threshold AtLeast({at_least}) exceeds child count {children}"
            ),
            Self::InvalidFailureThreshold { at_least, children } => write!(
                f,
                "parallel failure threshold AtLeast({at_least}) exceeds child count {children}"
            ),
            Self::InvalidTree { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for BuildError {}
