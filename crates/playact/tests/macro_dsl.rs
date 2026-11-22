#![cfg(feature = "macros")]

//! Integration tests for the `bt!` proc-macro DSL.

use std::num::NonZeroUsize;
use std::time::Duration;

use playact::prelude::*;

struct World {
    hp: f64,
}

#[test]
fn bt_macro_builds_selector() {
    let mut tree = playact::bt!(World, "demo", selector {
        sequence {
            condition("low_hp", |ctx: &Context<'_, World>| ctx.world_ref().hp < 0.3),
            action("heal", |_| Status::Success),
        },
        action("patrol", |_| Status::Running),
    })
    .expect("macro tree");

    let mut world = World { hp: 0.1 };
    assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);

    world.hp = 1.0;
    assert_eq!(
        tree.tick(&mut world, Time::from_millis(250, 250)),
        Status::Running
    );
}

#[test]
fn bt_macro_builds_decorator() {
    let mut tree = playact::bt!(World, "cooldown", cooldown(Duration::from_millis(100)) {
        action("hit", |_| Status::Success)
    })
    .expect("macro tree");

    let mut world = World { hp: 1.0 };
    assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    assert_eq!(
        tree.tick(&mut world, Time::from_millis(50, 50)),
        Status::Failure
    );
    assert_eq!(
        tree.tick(&mut world, Time::from_millis(150, 100)),
        Status::Success
    );
}

#[test]
fn bt_macro_builds_parallel_and_random_selector() {
    let mut parallel = playact::bt!(
        World,
        "parallel",
        parallel(Threshold::All, Threshold::Any) {
            action("a", |_| Status::Success),
            action("b", |_| Status::Success),
        }
    )
    .expect("parallel tree");

    let mut world = World { hp: 1.0 };
    assert_eq!(parallel.tick(&mut world, Time::ZERO), Status::Success);

    let mut random = playact::bt!(World, "random", random_selector(7) {
        action("only", |_| Status::Success)
    })
    .expect("random tree");

    assert_eq!(random.tick(&mut world, Time::ZERO), Status::Success);

    let mut race = playact::bt!(World, "race", race {
        action("a", |_| Status::Success),
        action("b", |_| Status::Running),
    })
    .expect("race tree");

    assert_eq!(race.tick(&mut world, Time::ZERO), Status::Success);
}

#[test]
fn bt_macro_builds_leaf_root_and_raw() {
    let mut leaf =
        playact::bt!(World, "leaf", action("only", |_| Status::Success)).expect("leaf tree");
    let mut world = World { hp: 1.0 };
    assert_eq!(leaf.tick(&mut world, Time::ZERO), Status::Success);

    let raw = playact::bt!(
        World,
        "raw",
        raw(Box::new(Action::<World>::new("custom", |_| {
            Status::Success
        })))
    )
    .expect("raw tree");

    let mut raw = raw;
    assert_eq!(raw.tick(&mut world, Time::ZERO), Status::Success);
}

#[test]
fn bt_macro_builds_guard() {
    let mut tree =
        playact::bt!(World, "guard", guard(|ctx: &Context<'_, World>| ctx.world_ref().hp > 0.0) {
            action("ok", |_| Status::Success)
        })
        .expect("guard tree");

    let mut world = World { hp: 1.0 };
    assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);

    world.hp = 0.0;
    assert_eq!(
        tree.tick(&mut world, Time::from_millis(16, 16)),
        Status::Failure
    );
}

#[test]
fn bt_macro_builds_throttle_and_set_value() {
    static VALUE: Key<u32> = playact::key!("macro_value");

    let mut tree = playact::bt!(World, "macro_extras", sequence {
        set_value("set_value", &VALUE, 7_u32),
        throttle(Duration::from_millis(100), NonZeroUsize::new(1).expect("non-zero")) {
            action("ok", |_| Status::Success)
        },
    })
    .expect("macro tree");

    let mut world = World { hp: 1.0 };
    assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    assert_eq!(tree.blackboard().get(&VALUE), Some(&7));
}
