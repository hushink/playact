//! 公共 API 集成测试：保命打断、冷却、射程切换与 abort 清理。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use playact::prelude::*;

struct World {
    hp: f64,
    in_range: bool,
}

static COMMANDS: Key<CommandBuffer<&'static str>> = playact::key!("commands");

fn push(ctx: &mut Context<'_, World>, command: &'static str) {
    ctx.blackboard()
        .get_or_insert_with(&COMMANDS, CommandBuffer::new)
        .push(command);
}

fn drain(tree: &mut BehaviorTree<World>) -> Vec<&'static str> {
    tree.blackboard_mut()
        .get_mut(&COMMANDS)
        .map(|buffer| buffer.drain().collect())
        .unwrap_or_default()
}

fn build_tree() -> (BehaviorTree<World>, Arc<AtomicU32>) {
    let aborts = Arc::new(AtomicU32::new(0));
    let chase_aborts = Arc::clone(&aborts);

    let mut tree = BehaviorTreeBuilder::<World>::new("rules")
        .reactive_selector(|b| {
            b.sequence(|b| {
                b.condition("低血量", |ctx| ctx.world_ref().hp < 0.3)
                    .action("喝药", |ctx| {
                        push(ctx, "heal");
                        Status::Success
                    })
            })
            .sequence(|b| {
                b.condition("在射程", |ctx| ctx.world_ref().in_range)
                    .cooldown(Duration::from_millis(500), |b| {
                        b.action("普攻", |ctx| {
                            push(ctx, "attack");
                            Status::Success
                        })
                    })
            })
            .raw(Box::new(
                Action::<World>::new("追击", |ctx| {
                    push(ctx, "move");
                    Status::Running
                })
                .with_abort(move |_| {
                    chase_aborts.fetch_add(1, Ordering::Relaxed);
                }),
            ))
        })
        .build()
        .expect("tree");

    tree.blackboard_mut().set(&COMMANDS, CommandBuffer::new());

    (tree, aborts)
}

#[test]
fn low_hp_heals_and_aborts_chase() {
    let (mut tree, aborts) = build_tree();
    let mut world = World {
        hp: 1.0,
        in_range: false,
    };

    assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Running);
    assert_eq!(drain(&mut tree), vec!["move"]);
    assert_eq!(aborts.load(Ordering::Relaxed), 0);

    world.hp = 0.2;
    assert_eq!(
        tree.tick(&mut world, Time::from_millis(250, 250)),
        Status::Success
    );
    assert_eq!(drain(&mut tree), vec!["heal"]);
    assert_eq!(aborts.load(Ordering::Relaxed), 1);
}

#[test]
fn cooldown_blocks_repeated_attack() {
    let (mut tree, _) = build_tree();
    let mut world = World {
        hp: 1.0,
        in_range: true,
    };

    assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    assert_eq!(drain(&mut tree), vec!["attack"]);

    assert_eq!(
        tree.tick(&mut world, Time::from_millis(100, 100)),
        Status::Running
    );
    assert!(!drain(&mut tree).contains(&"attack"));

    assert_eq!(
        tree.tick(&mut world, Time::from_millis(600, 500)),
        Status::Success
    );
    assert_eq!(drain(&mut tree), vec!["attack"]);
}

#[test]
fn entering_range_switches_from_chase_to_attack() {
    let (mut tree, aborts) = build_tree();
    let mut world = World {
        hp: 1.0,
        in_range: false,
    };

    assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Running);
    assert_eq!(drain(&mut tree), vec!["move"]);

    world.in_range = true;
    assert_eq!(
        tree.tick(&mut world, Time::from_millis(250, 250)),
        Status::Success
    );
    assert_eq!(drain(&mut tree), vec!["attack"]);
    assert_eq!(aborts.load(Ordering::Relaxed), 1);
}
