//! 类型安全黑板。
//!
//! 黑板按 [`Key<T>`] 存取，公共 API 不暴露字符串查找，也不暴露 `Any`。
//! 同名但不同类型的 key 是不同槽位，`set`/`get` 的类型不匹配在编译期
//! 就被 `Key<T>` 拦住。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use core::any::{Any, TypeId};
use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;

/// 类型安全的黑板键。
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
    /// 用 `'static` 字符串构造。推荐配合 `key!` 宏使用命名空间。
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            _ty: PhantomData,
        }
    }

    /// key 的静态名字。
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

/// 生成带当前模块路径前缀的 [`Key`]。
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

/// 类型安全的共享数据槽。
///
/// 黑板随一棵 [`crate::BehaviorTree`] 存活，跨 tick 保留数据；同一个
/// 黑板可以在不同节点之间传递目标、缓存、命令等。
#[derive(Default)]
pub struct Blackboard {
    slots: BTreeMap<SlotKey, Box<dyn Any + Send>>,
}

impl Blackboard {
    /// 创建空黑板。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 预分配槽位容量。
    ///
    /// 核心实现使用 `BTreeMap`，容量参数仅保留 API 兼容并被忽略；
    /// 黑板通常只有少量槽位，不做预分配。
    #[must_use]
    pub fn with_capacity(_capacity: usize) -> Self {
        Self::new()
    }

    /// 写入一个值，返回旧值（如果有）。
    pub fn set<T: Send + 'static>(&mut self, key: &Key<T>, value: T) -> Option<T> {
        self.slots
            .insert((key.name, TypeId::of::<T>()), Box::new(value))
            .and_then(|old| old.downcast::<T>().ok())
            .map(|old| *old)
    }

    /// 读取一个值。
    #[must_use]
    pub fn get<T: Send + 'static>(&self, key: &Key<T>) -> Option<&T> {
        self.slots.get(&(key.name, TypeId::of::<T>())).map(|slot| {
            slot.downcast_ref::<T>()
                .expect("playact blackboard slot type invariant violated")
        })
    }

    /// 可变读取一个值。
    #[must_use]
    pub fn get_mut<T: Send + 'static>(&mut self, key: &Key<T>) -> Option<&mut T> {
        self.slots
            .get_mut(&(key.name, TypeId::of::<T>()))
            .map(|slot| {
                slot.downcast_mut::<T>()
                    .expect("playact blackboard slot type invariant violated")
            })
    }

    /// 读取或插入一个值。
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

    /// 是否存在该 key。
    #[must_use]
    pub fn contains<T: Send + 'static>(&self, key: &Key<T>) -> bool {
        self.slots.contains_key(&(key.name, TypeId::of::<T>()))
    }

    /// 移除并返回一个值。
    pub fn remove<T: Send + 'static>(&mut self, key: &Key<T>) -> Option<T> {
        self.slots
            .remove(&(key.name, TypeId::of::<T>()))
            .and_then(|slot| slot.downcast::<T>().ok())
            .map(|value| *value)
    }

    /// 移除指定 key，忽略旧值。
    pub fn clear_key<T: Send + 'static>(&mut self, key: &Key<T>) {
        self.slots.remove(&(key.name, TypeId::of::<T>()));
    }

    /// 清空所有槽位。
    pub fn clear(&mut self) {
        self.slots.clear();
    }

    /// 槽位数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// 是否为空。
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
