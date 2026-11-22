//! 决策层产出、执行层消费的命令缓冲区。
//!
//! 这是一个业务无关的轻量工具：`C` 由使用方定义，引擎不解释它。
//! 推荐把 `CommandBuffer<C>` 放进世界或黑板的 typed slot，动作节点只
//! `push`，驱动方在 tick 外 `drain` 后交给真正的执行器。

use alloc::vec::Vec;

/// 一个简单的命令缓冲区。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandBuffer<C> {
    commands: Vec<C>,
}

impl<C> CommandBuffer<C> {
    /// 创建空缓冲区。
    #[must_use]
    pub const fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    /// 预分配容量。
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            commands: Vec::with_capacity(capacity),
        }
    }

    /// 追加一条命令。
    pub fn push(&mut self, command: C) {
        self.commands.push(command);
    }

    /// 追加一批命令。
    pub fn extend<I>(&mut self, commands: I)
    where
        I: IntoIterator<Item = C>,
    {
        self.commands.extend(commands);
    }

    /// 取走所有命令，保留缓冲区容量。
    pub fn drain(&mut self) -> alloc::vec::Drain<'_, C> {
        self.commands.drain(..)
    }

    /// 清空命令。
    pub fn clear(&mut self) {
        self.commands.clear();
    }

    /// 命令数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// 只读迭代。
    pub fn iter(&self) -> core::slice::Iter<'_, C> {
        self.commands.iter()
    }

    /// 只读切片。
    #[must_use]
    pub fn as_slice(&self) -> &[C] {
        &self.commands
    }

    /// 取出底层 `Vec`。
    #[must_use]
    pub fn into_vec(self) -> Vec<C> {
        self.commands
    }
}

impl<C> FromIterator<C> for CommandBuffer<C> {
    fn from_iter<T: IntoIterator<Item = C>>(iter: T) -> Self {
        Self {
            commands: iter.into_iter().collect(),
        }
    }
}

impl<C> IntoIterator for CommandBuffer<C> {
    type Item = C;
    type IntoIter = alloc::vec::IntoIter<C>;

    fn into_iter(self) -> Self::IntoIter {
        self.commands.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_drain_and_clear() {
        let mut buffer = CommandBuffer::new();
        assert!(buffer.is_empty());

        buffer.push(1);
        buffer.extend([2, 3]);
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.as_slice(), &[1, 2, 3]);

        let drained: Vec<_> = buffer.drain().collect();
        assert_eq!(drained, vec![1, 2, 3]);
        assert!(buffer.is_empty());

        buffer.push(4);
        buffer.clear();
        assert!(buffer.is_empty());
    }
}
