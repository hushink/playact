//! Subtree contracts: required and provided blackboard keys.
//!
//! A contract wraps a subtree, checks that its inputs exist before each
//! activation, and clears its declared outputs when the subtree is aborted or a
//! new activation begins. It is a validation and lifecycle boundary, not a
//! separate blackboard namespace.

use alloc::string::String;
use alloc::vec::Vec;

use crate::blackboard::{Blackboard, Key};
use crate::context::Context;
use crate::node::{BoxedNode, Kind, Node};
use crate::snapshot::state;
use crate::status::Status;
use crate::trace::AbortReason;

/// Type-erased metadata for a blackboard key used by a subtree contract.
pub trait KeySpec: Send + Sync {
    /// The key's static name.
    fn name(&self) -> &'static str;
    /// Whether the key exists on the blackboard.
    fn is_present(&self, board: &Blackboard) -> bool;
    /// Removes the key from the blackboard.
    fn remove(&self, board: &mut Blackboard);
}

impl<T: Send + 'static> KeySpec for Key<T> {
    fn name(&self) -> &'static str {
        Key::name(*self)
    }

    fn is_present(&self, board: &Blackboard) -> bool {
        board.contains(self)
    }

    fn remove(&self, board: &mut Blackboard) {
        board.clear_key(self);
    }
}

/// A subtree boundary with required inputs and provided outputs.
///
/// Required keys are checked before the child runs. If one is missing the node
/// reports an error through the tracer and returns `Failure`. Declared outputs
/// are cleared when a new activation starts, and when the subtree is aborted.
///
/// Outputs are intentionally kept after a successful/failed activation so the
/// parent can read them; they are cleared on the next activation.
pub struct SubtreeContract<W> {
    name: String,
    required: Vec<&'static dyn KeySpec>,
    provided: Vec<&'static dyn KeySpec>,
    child: BoxedNode<W>,
    active: bool,
}

impl<W> SubtreeContract<W> {
    /// Creates a contract around a subtree.
    #[must_use]
    pub fn new(name: impl Into<String>, child: BoxedNode<W>) -> Self {
        Self {
            name: name.into(),
            required: Vec::new(),
            provided: Vec::new(),
            child,
            active: false,
        }
    }

    /// Adds a required input key.
    #[must_use]
    pub fn require(mut self, key: &'static dyn KeySpec) -> Self {
        self.required.push(key);
        self
    }

    /// Adds an output key cleared on activation start and abort.
    #[must_use]
    pub fn provide(mut self, key: &'static dyn KeySpec) -> Self {
        self.provided.push(key);
        self
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    fn clear_outputs(&self, board: &mut Blackboard) {
        for key in &self.provided {
            key.remove(board);
        }
    }

    fn missing_required(&self, board: &Blackboard) -> Option<&'static dyn KeySpec> {
        self.required
            .iter()
            .copied()
            .find(|key| !key.is_present(board))
    }
}

impl<W> Node<W> for SubtreeContract<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        if !self.active {
            self.clear_outputs(ctx.blackboard());
            self.active = true;
        }

        if let Some(key) = self.missing_required(ctx.blackboard_ref()) {
            ctx.report_error(
                &self.name,
                Kind::Contract,
                alloc::format!("missing required blackboard key `{}`", key.name()),
            );
            self.active = false;
            return Status::Failure;
        }

        let status = ctx.tick_child(0, &mut self.child);
        if !status.is_running() {
            self.active = false;
        }
        status
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        if self.active {
            ctx.abort_child(0, &mut self.child, AbortReason::Preempted);
            self.active = false;
        }
        self.clear_outputs(ctx.blackboard());
    }

    fn reset(&mut self) {
        self.active = false;
        self.child.reset();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Contract
    }

    fn children(&self) -> &[BoxedNode<W>] {
        core::slice::from_ref(&self.child)
    }

    fn children_mut(&mut self) -> &mut [BoxedNode<W>] {
        core::slice::from_mut(&mut self.child)
    }

    fn save_own_state(&self) -> Option<Vec<u8>> {
        Some(state::encode(&self.active))
    }

    fn load_own_state(&mut self, bytes: &[u8]) -> bool {
        let Some(active) = state::decode::<bool>(bytes) else {
            return false;
        };
        self.active = active;
        true
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        Some(state::encode(&(self.active, self.child.save_state())))
    }

    fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some((active, child)) = state::decode::<(bool, Option<Vec<u8>>)>(bytes) else {
            return false;
        };
        if let Some(child_state) = child
            && !self.child.load_state(&child_state)
        {
            return false;
        }
        self.active = active;
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

    static INPUT: Key<u32> = Key::new("test.contract.input");
    static OUTPUT: Key<u32> = Key::new("test.contract.output");

    #[test]
    fn missing_required_key_fails_then_recovers() {
        let mut tree = BehaviorTreeBuilder::<()>::new("contract")
            .raw(Box::new(
                SubtreeContract::new(
                    "subtree",
                    Box::new(Action::new("child", |_| Status::Success)),
                )
                .require(&INPUT),
            ))
            .build()
            .expect("tree");

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Failure);

        tree.blackboard_mut().set(&INPUT, 1);
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn outputs_are_cleared_on_abort_and_next_activation() {
        let mut tree = BehaviorTreeBuilder::<()>::new("contract")
            .raw(Box::new(
                SubtreeContract::new(
                    "subtree",
                    Box::new(Action::new("child", |ctx| {
                        ctx.blackboard().set(&OUTPUT, 7_u32);
                        Status::Running
                    })),
                )
                .provide(&OUTPUT),
            ))
            .build()
            .expect("tree");

        let mut world = ();
        tree.blackboard_mut().set(&OUTPUT, 99);
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Running);
        assert_eq!(tree.blackboard().get(&OUTPUT), Some(&7));

        tree.abort(&mut world, Time::ZERO);
        assert_eq!(tree.blackboard().get(&OUTPUT), None);
    }

    #[test]
    fn child_abort_is_propagated() {
        let aborts = Arc::new(AtomicU32::new(0));
        let aborts_in_child = Arc::clone(&aborts);
        let mut tree = BehaviorTreeBuilder::<()>::new("contract")
            .raw(Box::new(
                SubtreeContract::new(
                    "subtree",
                    Box::new(
                        Action::new("child", |_| Status::Running).with_abort(move |_| {
                            aborts_in_child.fetch_add(1, Ordering::Relaxed);
                        }),
                    ),
                )
                .require(&INPUT)
                .provide(&OUTPUT),
            ))
            .build()
            .expect("tree");

        tree.blackboard_mut().set(&INPUT, 1);
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Running);
        tree.abort(&mut world, Time::ZERO);
        assert_eq!(aborts.load(Ordering::Relaxed), 1);
    }
}
