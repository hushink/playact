//! Scoped blackboard execution for subtrees.
//!
//! [`ScopedSubtree`] ticks its child with a private [`Blackboard`]. Selected
//! inputs are copied in when the subtree activates; selected outputs are copied
//! back when it reaches a terminal status. Keys created inside the subtree do
//! not leak into the parent blackboard.

use alloc::string::String;
use alloc::vec::Vec;

use crate::blackboard::{Blackboard, Key};
use crate::context::Context;
use crate::node::{BoxedNode, Kind, Node};
use crate::status::Status;
use crate::trace::AbortReason;

/// A copyable key declaration for a [`ScopedSubtree`].
///
/// Only `Key<T>` values with `T: Clone` can participate in a scope.
pub trait ScopedKeySpec: Send + Sync {
    /// The key's static name.
    fn name(&self) -> &'static str;
    /// Copies the value from one blackboard to another, if present.
    fn copy(&self, from: &Blackboard, to: &mut Blackboard);
    /// Removes the key from a blackboard.
    fn remove(&self, board: &mut Blackboard);
}

impl<T: Clone + Send + 'static> ScopedKeySpec for Key<T> {
    fn name(&self) -> &'static str {
        Key::name(*self)
    }

    fn copy(&self, from: &Blackboard, to: &mut Blackboard) {
        if let Some(value) = from.get(self) {
            to.set(self, value.clone());
        }
    }

    fn remove(&self, board: &mut Blackboard) {
        board.clear_key(self);
    }
}

/// Executes a child with a private blackboard.
///
/// On activation, the declared inputs are copied from the parent into a fresh
/// scoped board, and declared outputs are removed from the parent. When the
/// child reaches a terminal status, declared outputs are copied back. Aborting
/// the node aborts the child on the scoped board and clears the parent outputs.
///
/// Inputs are snapshotted at activation: later changes to the parent blackboard
/// are not seen by the child until the next activation. Keys that the child
/// creates but does not declare as outputs never reach the parent.
///
/// # Tracer and snapshots
///
/// The scoped child runs without a tracer, so `Enter`/`Exit` events from inside
/// the subtree are not emitted; the `ScopedSubtree` node itself still appears in
/// the parent trace. The scoped board is runtime-only: snapshots restore the
/// child node state, but declared inputs are copied again on the next tick after
/// a restore.
pub struct ScopedSubtree<W> {
    name: String,
    inputs: Vec<&'static dyn ScopedKeySpec>,
    outputs: Vec<&'static dyn ScopedKeySpec>,
    child: BoxedNode<W>,
    board: Blackboard,
    active: bool,
}

impl<W> ScopedSubtree<W> {
    /// Creates a scoped subtree around a child.
    #[must_use]
    pub fn new(name: impl Into<String>, child: BoxedNode<W>) -> Self {
        Self {
            name: name.into(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            child,
            board: Blackboard::new(),
            active: false,
        }
    }

    /// Copies this key into the scoped board on activation.
    #[must_use]
    pub fn input(mut self, key: &'static dyn ScopedKeySpec) -> Self {
        self.inputs.push(key);
        self
    }

    /// Copies this key back to the parent when the child terminates.
    #[must_use]
    pub fn output(mut self, key: &'static dyn ScopedKeySpec) -> Self {
        self.outputs.push(key);
        self
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    fn activate(&mut self, parent: &mut Blackboard) {
        self.board.clear();
        for key in &self.inputs {
            key.copy(parent, &mut self.board);
        }
        for key in &self.outputs {
            key.remove(parent);
        }
        self.active = true;
    }

    fn copy_outputs(&self, parent: &mut Blackboard) {
        for key in &self.outputs {
            key.copy(&self.board, parent);
        }
    }

    fn clear_outputs(parent: &mut Blackboard, outputs: &[&'static dyn ScopedKeySpec]) {
        for key in outputs {
            key.remove(parent);
        }
    }
}

impl<W> Node<W> for ScopedSubtree<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        if !self.active {
            self.activate(ctx.blackboard());
        }

        let status = {
            let mut scoped = ctx.scoped_context(&mut self.board);
            scoped.tick_child(0, &mut self.child)
        };

        if !status.is_running() {
            self.copy_outputs(ctx.blackboard());
            self.active = false;
        }
        status
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        if self.active {
            {
                let mut scoped = ctx.scoped_context(&mut self.board);
                scoped.abort_child(0, &mut self.child, AbortReason::Preempted);
            }
            self.active = false;
        }
        Self::clear_outputs(ctx.blackboard(), &self.outputs);
    }

    fn reset(&mut self) {
        self.active = false;
        self.board.clear();
        self.child.reset();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::ScopedSubtree
    }

    fn children(&self) -> &[BoxedNode<W>] {
        core::slice::from_ref(&self.child)
    }

    fn children_mut(&mut self) -> &mut [BoxedNode<W>] {
        core::slice::from_mut(&mut self.child)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        self.child.save_state()
    }

    fn load_state(&mut self, bytes: &[u8]) -> bool {
        if !self.child.load_state(bytes) {
            return false;
        }
        self.active = false;
        self.board.clear();
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::blackboard::Key;
    use crate::builder::BehaviorTreeBuilder;
    use crate::leaf::Action;
    use crate::time::Time;

    use super::*;

    static INPUT: Key<u32> = Key::new("test.scope.input");
    static OUTPUT: Key<u32> = Key::new("test.scope.output");
    static PRIVATE: Key<u32> = Key::new("test.scope.private");

    #[test]
    fn inputs_and_outputs_cross_the_boundary() {
        let mut tree = BehaviorTreeBuilder::<()>::new("scope")
            .raw(Box::new(
                ScopedSubtree::new(
                    "scoped",
                    Box::new(Action::new("child", |ctx| {
                        let input = ctx.blackboard_ref().get(&INPUT).copied().unwrap_or(0);
                        ctx.blackboard().set(&OUTPUT, input + 1);
                        ctx.blackboard().set(&PRIVATE, 99_u32);
                        Status::Success
                    })),
                )
                .input(&INPUT)
                .output(&OUTPUT),
            ))
            .build()
            .expect("tree");

        tree.blackboard_mut().set(&INPUT, 5_u32);
        tree.blackboard_mut().set(&OUTPUT, 99_u32);
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);

        assert_eq!(tree.blackboard().get(&INPUT), Some(&5));
        assert_eq!(tree.blackboard().get(&OUTPUT), Some(&6));
        assert_eq!(tree.blackboard().get(&PRIVATE), None);
    }

    #[test]
    fn restore_reactivates_inputs_on_the_next_tick() {
        let mut tree = BehaviorTreeBuilder::<()>::new("scope")
            .scoped("skill", [&INPUT], [&OUTPUT], |b| {
                b.limiter(core::num::NonZeroUsize::new(10).unwrap(), |b| {
                    b.action("copy", |ctx| {
                        match ctx.blackboard_ref().get(&INPUT).copied() {
                            Some(value) => {
                                ctx.blackboard().set(&OUTPUT, value);
                                Status::Success
                            }
                            None => Status::Failure,
                        }
                    })
                })
            })
            .build()
            .expect("tree");

        tree.blackboard_mut().set(&INPUT, 1_u32);
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        assert_eq!(tree.blackboard().get(&OUTPUT), Some(&1));

        let snapshot = tree.snapshot();
        tree.blackboard_mut().set(&INPUT, 2_u32);
        tree.restore(&snapshot).expect("restore");
        assert_eq!(
            tree.tick(&mut world, Time::from_millis(10, 10)),
            Status::Success
        );
        assert_eq!(tree.blackboard().get(&OUTPUT), Some(&2));
    }

    #[test]
    fn abort_clears_parent_outputs_and_aborts_child() {
        let aborts = Arc::new(AtomicU32::new(0));
        let aborts_in_child = Arc::clone(&aborts);
        let mut tree = BehaviorTreeBuilder::<()>::new("scope")
            .raw(Box::new(
                ScopedSubtree::new(
                    "scoped",
                    Box::new(
                        Action::new("child", |ctx| {
                            ctx.blackboard().set(&OUTPUT, 7_u32);
                            Status::Running
                        })
                        .with_abort(move |_| {
                            aborts_in_child.fetch_add(1, Ordering::Relaxed);
                        }),
                    ),
                )
                .output(&OUTPUT),
            ))
            .build()
            .expect("tree");

        tree.blackboard_mut().set(&OUTPUT, 99_u32);
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Running);
        assert_eq!(tree.blackboard().get(&OUTPUT), None);

        tree.abort(&mut world, Time::ZERO);
        assert_eq!(aborts.load(Ordering::Relaxed), 1);
        assert_eq!(tree.blackboard().get(&OUTPUT), None);
    }
}
