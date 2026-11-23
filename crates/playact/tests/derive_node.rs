#![cfg(feature = "macros")]

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use playact::{BehaviorTreeBuilder, BoxedNode, Context, NodeId, Status, Time};

#[derive(playact::BtNode)]
#[bt(world = (), name = "counter")]
struct Counter {
    #[bt(id)]
    id: NodeId,
    #[bt(children)]
    children: Vec<BoxedNode<()>>,
}

impl Counter {
    fn tick(&mut self, _ctx: &mut Context<'_, ()>) -> Status {
        Status::Success
    }
}

#[derive(playact::BtNode)]
#[bt(world = (), abort, reset)]
struct Abortable {
    aborts: Arc<AtomicU32>,
    resets: Arc<AtomicU32>,
}

impl Abortable {
    fn tick(&mut self, _ctx: &mut Context<'_, ()>) -> Status {
        Status::Running
    }

    fn abort(&mut self, _ctx: &mut Context<'_, ()>) {
        self.aborts.fetch_add(1, Ordering::Relaxed);
    }

    fn reset(&mut self) {
        self.resets.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(playact::BtNode)]
#[bt(world = W, name = "generic")]
struct Generic<W: Send> {
    #[bt(id)]
    id: NodeId,
    marker: PhantomData<W>,
}

impl<W: Send> Generic<W> {
    fn tick(&mut self, _ctx: &mut Context<'_, W>) -> Status {
        let _ = self.marker;
        Status::Success
    }
}

fn counter() -> Counter {
    Counter {
        id: NodeId::new("counter"),
        children: Vec::new(),
    }
}

#[test]
fn derive_generates_the_node_contract() {
    let mut tree = BehaviorTreeBuilder::<()>::new("derive")
        .raw(Box::new(counter()))
        .build()
        .expect("tree");
    let mut world = ();
    assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);

    let description = tree.describe();
    assert_eq!(description.name, "counter");
    assert_eq!(description.id.as_ref().map(NodeId::as_str), Some("counter"));
    assert!(description.children.is_empty());
}

#[test]
fn derive_delegates_abort_and_reset() {
    let aborts = Arc::new(AtomicU32::new(0));
    let resets = Arc::new(AtomicU32::new(0));
    let mut tree = BehaviorTreeBuilder::<()>::new("derive")
        .raw(Box::new(Abortable {
            aborts: Arc::clone(&aborts),
            resets: Arc::clone(&resets),
        }))
        .build()
        .expect("tree");

    let mut world = ();
    assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Running);
    tree.abort(&mut world, Time::ZERO);
    assert_eq!(aborts.load(Ordering::Relaxed), 1);
    assert_eq!(resets.load(Ordering::Relaxed), 1);
}

#[derive(playact::BtNode)]
#[bt(world = (), own_state, state)]
struct Stateful {
    value: u8,
}

impl Stateful {
    fn tick(&mut self, _ctx: &mut Context<'_, ()>) -> Status {
        Status::Success
    }

    fn save_own_state(&self) -> Option<Vec<u8>> {
        Some(vec![self.value])
    }

    fn load_own_state(&mut self, state: &[u8]) -> bool {
        self.value = state[0];
        true
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        Some(vec![self.value])
    }

    fn load_state(&mut self, state: &[u8]) -> bool {
        self.value = state[0];
        true
    }
}

#[test]
fn derive_delegates_own_and_full_state() {
    let mut node = Stateful { value: 3 };
    assert_eq!(
        <Stateful as playact::Node<()>>::save_own_state(&node),
        Some(vec![3])
    );
    assert_eq!(
        <Stateful as playact::Node<()>>::save_state(&node),
        Some(vec![3])
    );
    assert!(<Stateful as playact::Node<()>>::load_own_state(
        &mut node,
        &[7]
    ));
    assert_eq!(node.value, 7);
    assert!(<Stateful as playact::Node<()>>::load_state(&mut node, &[9]));
    assert_eq!(node.value, 9);
}

#[test]
fn derive_supports_a_generic_world() {
    let mut tree = BehaviorTreeBuilder::<bool>::new("derive")
        .raw(Box::new(Generic {
            id: NodeId::new("generic"),
            marker: PhantomData,
        }))
        .build()
        .expect("tree");
    let mut world = false;
    assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
}
