//! A command buffer produced by the decision layer and consumed by the execution
//! layer.
//!
//! This is a lightweight, business-agnostic utility: `C` is defined by the user
//! and the engine never interprets it. The recommended setup is to put
//! `CommandBuffer<C>` in the world or in a typed blackboard slot, have action
//! nodes only `push`, and let the driver `drain` it outside the tick and hand
//! the commands to the real executor.

use alloc::vec::Vec;

/// A simple command buffer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandBuffer<C> {
    commands: Vec<C>,
}

impl<C> CommandBuffer<C> {
    /// Create an empty buffer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    /// Preallocate capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            commands: Vec::with_capacity(capacity),
        }
    }

    /// Append a command.
    pub fn push(&mut self, command: C) {
        self.commands.push(command);
    }

    /// Append a batch of commands.
    pub fn extend<I>(&mut self, commands: I)
    where
        I: IntoIterator<Item = C>,
    {
        self.commands.extend(commands);
    }

    /// Take all commands, keeping the buffer's capacity.
    pub fn drain(&mut self) -> alloc::vec::Drain<'_, C> {
        self.commands.drain(..)
    }

    /// Clear the commands.
    pub fn clear(&mut self) {
        self.commands.clear();
    }

    /// Number of commands.
    #[must_use]
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Read-only iteration.
    pub fn iter(&self) -> core::slice::Iter<'_, C> {
        self.commands.iter()
    }

    /// Read-only slice.
    #[must_use]
    pub fn as_slice(&self) -> &[C] {
        &self.commands
    }

    /// Take the underlying `Vec`.
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
