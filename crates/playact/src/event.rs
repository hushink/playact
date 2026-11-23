//! Type-safe event queues.
//!
//! Events are ordinary values stored in a per-key FIFO queue on the
//! [`crate::Blackboard`]. [`EventKey`] hides the underlying queue type while
//! preserving compile-time key/type checking; no public `Any` or string lookup
//! is involved.

use alloc::collections::VecDeque;
use core::fmt;

use crate::blackboard::Key;
use crate::context::Context;

/// A FIFO queue of events of one type.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventQueue<T> {
    events: VecDeque<T>,
}

impl<T> EventQueue<T> {
    /// Creates an empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: VecDeque::new(),
        }
    }

    /// Appends an event.
    pub fn push(&mut self, event: T) {
        self.events.push_back(event);
    }

    /// Removes and returns the oldest event.
    pub fn pop(&mut self) -> Option<T> {
        self.events.pop_front()
    }

    /// Returns the oldest event without removing it.
    #[must_use]
    pub fn peek(&self) -> Option<&T> {
        self.events.front()
    }

    /// Number of queued events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Drops all queued events.
    pub fn clear(&mut self) {
        self.events.clear();
    }
}

/// A typed key for an [`EventQueue`].
///
/// Create one with [`EventKey::new`] or the [`event_key!`](crate::event_key)
/// macro and store it in a `static`. Producers call [`EventKey::push`]; wait
/// nodes consume events in FIFO order.
pub struct EventKey<T: Send + 'static> {
    key: Key<EventQueue<T>>,
}

impl<T: Send + 'static> EventKey<T> {
    /// Creates an event key from a `'static` name.
    pub const fn new(name: &'static str) -> Self {
        Self {
            key: Key::new(name),
        }
    }

    /// The key's static name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.key.name()
    }

    /// Appends an event to the queue, creating the queue on first use.
    pub fn push<W>(&self, ctx: &mut Context<'_, W>, event: T) {
        ctx.blackboard()
            .get_or_insert_with(&self.key, EventQueue::new)
            .push(event);
    }

    /// Removes the oldest event, if any.
    pub fn pop<W>(&self, ctx: &mut Context<'_, W>) -> Option<T> {
        ctx.blackboard().get_mut(&self.key)?.pop()
    }

    /// Number of queued events.
    #[must_use]
    pub fn len<W>(&self, ctx: &Context<'_, W>) -> usize {
        ctx.blackboard_ref()
            .get(&self.key)
            .map_or(0, EventQueue::len)
    }

    /// Whether the queue is empty.
    #[must_use]
    pub fn is_empty<W>(&self, ctx: &Context<'_, W>) -> bool {
        self.len(ctx) == 0
    }

    /// Drops all queued events.
    pub fn clear<W>(&self, ctx: &mut Context<'_, W>) {
        if let Some(queue) = ctx.blackboard().get_mut(&self.key) {
            queue.clear();
        }
    }
}

impl<T: Send + 'static> Clone for EventKey<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Send + 'static> Copy for EventKey<T> {}

impl<T: Send + 'static> fmt::Debug for EventKey<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventKey")
            .field("name", &self.name())
            .field("type", &core::any::type_name::<T>())
            .finish()
    }
}

/// Generate an [`EventKey`] prefixed with the current module path.
///
/// ```
/// use playact::EventKey;
///
/// static EVENTS: EventKey<u32> = playact::event_key!("events");
/// assert!(EVENTS.name().ends_with("::events"));
/// ```
#[macro_export]
macro_rules! event_key {
    ($name:literal) => {
        $crate::EventKey::new(concat!(module_path!(), "::", $name))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_is_fifo_and_clearable() {
        let mut queue = EventQueue::new();
        queue.push(1);
        queue.push(2);
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.peek(), Some(&1));
        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), None);
        assert!(queue.is_empty());

        queue.push(3);
        queue.clear();
        assert!(queue.is_empty());
    }

    #[test]
    fn event_key_is_copy_and_namespaced() {
        static EVENTS: EventKey<u32> = event_key!("events");
        let copied = EVENTS;
        assert_eq!(copied.name(), EVENTS.name());
        assert!(EVENTS.name().ends_with("::events"));
    }
}
