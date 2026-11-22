//! 节点状态：行为树中唯一的控制流通信协议。

use core::fmt;

/// 节点一次 tick 的执行结果。
///
/// `Success` / `Failure` / `Running` 是控制节点与执行节点之间的唯一协议：
/// 组合节点只据此编排控制流，不关心具体业务。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    /// 本次执行已完成并成功。
    Success,
    /// 本次执行已完成并失败。
    Failure,
    /// 尚未结束，下一次 tick 继续。节点应保留必要的内部状态。
    Running,
}

impl Status {
    /// 将布尔判断转换为状态：`true` → `Success`，`false` → `Failure`。
    #[must_use]
    pub const fn from_bool(ok: bool) -> Self {
        if ok { Self::Success } else { Self::Failure }
    }

    /// 是否为成功。
    #[must_use]
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }

    /// 是否为失败。
    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(self, Self::Failure)
    }

    /// 是否仍在运行。
    #[must_use]
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    /// 是否为终态（成功或失败）。
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !self.is_running()
    }
}

impl From<bool> for Status {
    fn from(value: bool) -> Self {
        Self::from_bool(value)
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Success => "Success",
            Self::Failure => "Failure",
            Self::Running => "Running",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bool_conversion() {
        assert_eq!(Status::from_bool(true), Status::Success);
        assert_eq!(Status::from_bool(false), Status::Failure);
        assert_eq!(Status::from(true), Status::Success);
    }

    #[test]
    fn predicates() {
        assert!(Status::Success.is_success());
        assert!(Status::Success.is_terminal());
        assert!(Status::Failure.is_failure());
        assert!(Status::Running.is_running());
        assert!(!Status::Running.is_terminal());
    }
}
