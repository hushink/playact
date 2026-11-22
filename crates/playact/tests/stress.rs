//! 确定性随机压力测试。
//!
//! 随机生成组合、装饰、叶子与嵌套树，覆盖 tick / abort / reset / describe /
//! Send 等路径。固定 seed 完全可复现，不需要 nightly 或 cargo-fuzz；它不能
//! 替代覆盖率引导的 fuzzer，但可以低成本守住“任意结构不 panic”的底线。

use std::num::NonZeroUsize;
use std::time::Duration;

use playact::{
    Action, AlwaysFail, AlwaysSucceed, BehaviorTree, BoxedNode, Condition, Context, Cooldown,
    Guard, IfThenElse, Inverter, Key, Limiter, MemParallel, Parallel, Race, RandomSelector,
    ReactiveSelector, ReactiveSequence, Repeat, Retry, Selector, Sequence, SetValue, Status,
    Threshold, Throttle, Time, Timeout, UntilFailure, UntilSuccess, UtilityChild, UtilitySelector,
    Wait, WeightedChild, WeightedRandomSelector,
};

static COUNTER: Key<u64> = playact::key!("stress_counter");

#[derive(Default)]
struct World {
    hp: i64,
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        self.0 = value;
        value.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: usize) -> usize {
        debug_assert!(bound > 0);
        (self.next() % bound as u64) as usize
    }
}

fn random_leaf(rng: &mut Rng) -> BoxedNode<World> {
    match rng.below(4) {
        0 => {
            let mut state = rng.next() | 1;
            Box::new(Action::<World>::new("random_action", move |ctx| {
                state ^= state >> 12;
                state ^= state << 25;
                state ^= state >> 27;

                let hp = ctx.world_ref().hp;
                ctx.world().hp = hp.wrapping_add((state & 0x7) as i64);

                let seen = ctx.blackboard_ref().get(&COUNTER).copied().unwrap_or(0);
                ctx.blackboard().set(&COUNTER, seen.wrapping_add(1));

                match state % 3 {
                    0 => Status::Success,
                    1 => Status::Failure,
                    _ => Status::Running,
                }
            }))
        }
        1 => {
            let threshold = rng.next() % 16;
            Box::new(Condition::<World>::new("random_condition", move |ctx| {
                ctx.blackboard_ref().get(&COUNTER).copied().unwrap_or(0) % 16 == threshold
            }))
        }
        2 => Box::new(Wait::<World>::new(Duration::from_millis(
            rng.below(32) as u64
        ))),
        _ => Box::new(SetValue::<World, u64>::new(
            "set_counter",
            &COUNTER,
            rng.next(),
        )),
    }
}

fn random_nonzero(rng: &mut Rng) -> NonZeroUsize {
    NonZeroUsize::new(1 + rng.below(4)).expect("1..=4 is non-zero")
}

fn random_threshold(rng: &mut Rng, total: usize) -> Threshold {
    if total == 0 {
        return Threshold::Any;
    }
    match rng.below(3) {
        0 => Threshold::All,
        1 => Threshold::Any,
        _ => Threshold::AtLeast(NonZeroUsize::new(1 + rng.below(total)).expect("non-zero")),
    }
}

fn random_children(rng: &mut Rng, depth: usize, budget: &mut usize) -> Vec<BoxedNode<World>> {
    let count = rng.below(5);
    let mut children = Vec::with_capacity(count);
    for _ in 0..count {
        children.push(random_node(rng, depth - 1, budget));
    }
    children
}

fn random_decorator(rng: &mut Rng, depth: usize, budget: &mut usize) -> BoxedNode<World> {
    let child = random_node(rng, depth - 1, budget);
    match rng.below(12) {
        0 => Box::new(Inverter::new(child)),
        1 => Box::new(AlwaysSucceed::new(child)),
        2 => Box::new(AlwaysFail::new(child)),
        3 => Box::new(Repeat::new(random_nonzero(rng), child)),
        4 => Box::new(Retry::new(random_nonzero(rng), child)),
        5 => Box::new(UntilSuccess::new(child)),
        6 => Box::new(UntilFailure::new(child)),
        7 => Box::new(Cooldown::new(
            Duration::from_millis(rng.below(64) as u64),
            child,
        )),
        8 => Box::new(Timeout::new(
            Duration::from_millis(rng.below(64) as u64),
            child,
        )),
        9 => Box::new(Throttle::new(
            Duration::from_millis(rng.below(64) as u64),
            random_nonzero(rng),
            child,
        )),
        10 => {
            let threshold = rng.below(17) as i64 - 8;
            Box::new(Guard::new(
                move |ctx: &Context<'_, World>| ctx.world_ref().hp > threshold,
                child,
            ))
        }
        _ => Box::new(Limiter::new(random_nonzero(rng), child)),
    }
}

fn random_node(rng: &mut Rng, depth: usize, budget: &mut usize) -> BoxedNode<World> {
    *budget = budget.saturating_sub(1);
    if depth == 0 || *budget == 0 {
        return random_leaf(rng);
    }

    match rng.below(16) {
        0..=2 => random_leaf(rng),
        3 => Box::new(Sequence::new(random_children(rng, depth, budget))),
        4 => Box::new(Selector::new(random_children(rng, depth, budget))),
        5 => Box::new(ReactiveSequence::new(random_children(rng, depth, budget))),
        6 => Box::new(ReactiveSelector::new(random_children(rng, depth, budget))),
        7 => {
            let children = random_children(rng, depth, budget);
            let total = children.len();
            Box::new(Parallel::new(
                children,
                random_threshold(rng, total),
                random_threshold(rng, total),
            ))
        }
        8 => {
            let children = random_children(rng, depth, budget);
            let total = children.len();
            Box::new(MemParallel::new(
                children,
                random_threshold(rng, total),
                random_threshold(rng, total),
            ))
        }
        9 => {
            let children = random_children(rng, depth, budget);
            Box::new(RandomSelector::new(children, rng.next()))
        }
        10 => {
            let count = rng.below(5);
            let mut children = Vec::with_capacity(count);
            for _ in 0..count {
                let weight = rng.below(6) as u32;
                children.push(WeightedChild::new(
                    weight,
                    random_node(rng, depth - 1, budget),
                ));
            }
            Box::new(WeightedRandomSelector::new(children, rng.next()))
        }
        11 => Box::new(Race::new(random_children(rng, depth, budget))),
        12 => {
            let count = rng.below(5);
            let mut children = Vec::with_capacity(count);
            for _ in 0..count {
                let offset = (rng.below(17) as f32) - 8.0;
                children.push(UtilityChild::new(
                    move |ctx: &Context<'_, World>| ctx.world_ref().hp as f32 + offset,
                    random_node(rng, depth - 1, budget),
                ));
            }
            Box::new(UtilitySelector::new(children))
        }
        13 => {
            let threshold = rng.below(17) as i64 - 8;
            let then_branch = random_node(rng, depth - 1, budget);
            let else_branch = random_node(rng, depth - 1, budget);
            Box::new(IfThenElse::new(
                move |ctx: &Context<'_, World>| ctx.world_ref().hp > threshold,
                then_branch,
                else_branch,
            ))
        }
        14 => Box::new(BehaviorTree::new(random_node(rng, depth - 1, budget))),
        _ => random_decorator(rng, depth, budget),
    }
}

fn random_tree(seed: u64, depth: usize, budget: usize) -> BehaviorTree<World> {
    let mut rng = Rng::new(seed);
    let mut budget = budget;
    let root = random_node(&mut rng, depth, &mut budget);
    let mut tree = BehaviorTree::new(root);

    for _ in 0..rng.below(3) {
        tree.add_service(random_node(&mut rng, 2, &mut budget));
    }

    tree
}

#[test]
fn random_trees_survive_tick_abort_reset() {
    for seed in 0..64_u64 {
        let mut tree = random_tree(seed, 4, 48);
        let mut world = World::default();

        let description = tree.describe();
        assert!(!description.to_string().is_empty());
        assert!(description.to_dot().starts_with("digraph playact"));
        assert_eq!(description, tree.describe());

        for frame in 0..200_u64 {
            let time = Time::from_millis(frame * 16, 16);
            let _ = tree.tick(&mut world, time);
            if frame % 13 == 0 {
                tree.abort(&mut world, time);
            }
            if frame % 31 == 0 {
                tree.reset();
            }
        }

        // abort / reset 幂等，之后仍能继续 tick。
        let time = Time::from_millis(200 * 16, 16);
        tree.abort(&mut world, time);
        tree.abort(&mut world, time);
        tree.reset();
        tree.reset();
        let _ = tree.tick(&mut world, time);
        assert_eq!(description, tree.describe());
    }
}

#[test]
fn same_seed_produces_same_behavior() {
    fn run(seed: u64) -> (String, Vec<Status>, i64) {
        let mut tree = random_tree(seed, 4, 48);
        let mut world = World::default();

        let mut statuses = Vec::with_capacity(80);
        for frame in 0..80_u64 {
            statuses.push(tree.tick(&mut world, Time::from_millis(frame * 16, 16)));
        }
        (tree.describe().to_string(), statuses, world.hp)
    }

    assert_eq!(run(0xA11CE), run(0xA11CE));
}

#[test]
fn random_tree_can_tick_on_another_thread() {
    let tree = random_tree(0xC0FFEE, 5, 64);

    let handle = std::thread::spawn(move || {
        let mut tree = tree;
        let mut world = World::default();
        for frame in 0..100_u64 {
            let _ = tree.tick(&mut world, Time::from_millis(frame * 16, 16));
        }
        world.hp
    });

    let _ = handle.join().expect("stress thread");
}
