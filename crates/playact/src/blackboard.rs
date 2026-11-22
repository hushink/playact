//! Type-safe blackboard.
//!
//! The blackboard stores values by [`Key<T>`]; the public API exposes neither
//! string lookup nor `Any`. Keys with the same name but different types are
//! different slots, and `set`/`get` type mismatches are caught at compile time
//! by `Key<T>`.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use core::any::{Any, TypeId};
use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;

/// A type-safe blackboard key.
///
/// ```ignore
/// static TARGET: playact::Key<u32> = playact::key!("target");
/// ```
#[must_use]
pub struct Key<T: Send + 'static> {
    name: &'static str,
    _ty: PhantomData<fn() -> T>,
}

impl<T: Send + 'static> Key<T> {
    /// Construct from a `'static` string. Pair it with the `key!` macro to get
    /// namespacing.
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            _ty: PhantomData,
        }
    }

    /// The key's static name.
    pub const fn name(self) -> &'static str {
        self.name
    }
}

impl<T: Send + 'static> Clone for Key<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Send + 'static> Copy for Key<T> {}

impl<T: Send + 'static> fmt::Debug for Key<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Key")
            .field("name", &self.name)
            .field("type", &core::any::type_name::<T>())
            .finish()
    }
}

impl<T: Send + 'static> PartialEq for Key<T> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl<T: Send + 'static> Eq for Key<T> {}

impl<T: Send + 'static> Hash for Key<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

/// Generate a [`Key`] prefixed with the current module path.
///
/// ```
/// use playact::Key;
///
/// static TARGET: Key<u32> = playact::key!("target");
/// assert!(TARGET.name().ends_with("::target"));
/// ```
#[macro_export]
macro_rules! key {
    ($name:literal) => {
        $crate::Key::new(concat!(module_path!(), "::", $name))
    };
}

type SlotKey = (&'static str, TypeId);

/// Type-safe shared data slots.
///
/// A blackboard lives as long as one [`crate::BehaviorTree`] and keeps its data
/// across ticks; the same blackboard lets different nodes pass targets, caches,
/// commands, and so on.
#[derive(Default)]
pub struct Blackboard {
    slots: BTreeMap<SlotKey, Box<dyn Any + Send>>,
}

impl Blackboard {
    /// Create an empty blackboard.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Preallocate slot capacity.
    ///
    /// The core implementation uses a `BTreeMap`; the capacity argument is kept
    /// only for API compatibility and is ignored. Blackboards usually hold few
    /// slots, so no preallocation is done.
    #[must_use]
    pub fn with_capacity(_capacity: usize) -> Self {
        Self::new()
    }

    /// Write a value, returning the old value if any.
    pub fn set<T: Send + 'static>(&mut self, key: &Key<T>, value: T) -> Option<T> {
        self.slots
            .insert((key.name, TypeId::of::<T>()), Box::new(value))
            .and_then(|old| old.downcast::<T>().ok())
            .map(|old| *old)
    }

    /// Read a value.
    #[must_use]
    pub fn get<T: Send + 'static>(&self, key: &Key<T>) -> Option<&T> {
        self.slots.get(&(key.name, TypeId::of::<T>())).map(|slot| {
            slot.downcast_ref::<T>()
                .expect("playact blackboard slot type invariant violated")
        })
    }

    /// Read a value mutably.
    #[must_use]
    pub fn get_mut<T: Send + 'static>(&mut self, key: &Key<T>) -> Option<&mut T> {
        self.slots
            .get_mut(&(key.name, TypeId::of::<T>()))
            .map(|slot| {
                slot.downcast_mut::<T>()
                    .expect("playact blackboard slot type invariant violated")
            })
    }

    /// Read a value or insert one.
    pub fn get_or_insert_with<T: Send + 'static>(
        &mut self,
        key: &Key<T>,
        f: impl FnOnce() -> T,
    ) -> &mut T {
        self.slots
            .entry((key.name, TypeId::of::<T>()))
            .or_insert_with(|| Box::new(f()))
            .downcast_mut::<T>()
            .expect("playact blackboard slot type invariant violated")
    }

    /// Whether the key exists.
    #[must_use]
    pub fn contains<T: Send + 'static>(&self, key: &Key<T>) -> bool {
        self.slots.contains_key(&(key.name, TypeId::of::<T>()))
    }

    /// Remove and return a value.
    pub fn remove<T: Send + 'static>(&mut self, key: &Key<T>) -> Option<T> {
        self.slots
            .remove(&(key.name, TypeId::of::<T>()))
            .and_then(|slot| slot.downcast::<T>().ok())
            .map(|value| *value)
    }

    /// Remove the given key, discarding any old value.
    pub fn clear_key<T: Send + 'static>(&mut self, key: &Key<T>) {
        self.slots.remove(&(key.name, TypeId::of::<T>()));
    }

    /// Clear all slots.
    pub fn clear(&mut self) {
        self.slots.clear();
    }

    /// Number of slots.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether the blackboard is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

impl fmt::Debug for Blackboard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Blackboard")
            .field("len", &self.slots.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static A: Key<u32> = Key::new("test.a");
    static A_ALIAS: Key<u32> = Key::new("test.a");
    static B: Key<u64> = Key::new("test.a");

    #[test]
    fn typed_roundtrip_and_aliases() {
        let mut bb = Blackboard::new();
        assert!(!bb.contains(&A));
        assert_eq!(bb.set(&A, 7), None);

        assert_eq!(bb.get(&A), Some(&7));
        assert_eq!(bb.get(&A_ALIAS), Some(&7));
        assert_eq!(bb.get(&B), None);
        assert!(bb.contains(&A));
        assert!(!bb.contains(&B));

        assert_eq!(bb.set(&A, 9), Some(7));
        assert_eq!(bb.get(&A), Some(&9));
    }

    #[test]
    fn different_types_use_different_slots() {
        let mut bb = Blackboard::new();
        bb.set(&A, 1_u32);
        bb.set(&B, 2_u64);

        assert_eq!(bb.get(&A), Some(&1));
        assert_eq!(bb.get(&B), Some(&2));
        assert_eq!(bb.len(), 2);
    }

    #[test]
    fn get_or_insert_and_remove() {
        let mut bb = Blackboard::new();
        assert_eq!(*bb.get_or_insert_with(&A, || 5), 5);
        assert_eq!(*bb.get_or_insert_with(&A, || 99), 5);
        assert_eq!(bb.remove(&A), Some(5));
        assert_eq!(bb.remove(&A), None);
    }

    #[test]
    fn clear_and_clear_key() {
        let mut bb = Blackboard::new();
        bb.set(&A, 1_u32);
        bb.set(&B, 2_u64);

        bb.clear_key(&A);
        assert_eq!(bb.get(&A), None);
        assert_eq!(bb.get(&B), Some(&2));

        bb.clear();
        assert!(bb.is_empty());
    }

    #[test]
    fn board_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Blackboard>();
    }
}
