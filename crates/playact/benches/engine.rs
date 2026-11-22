//! 无第三方依赖的微基准。
//!
//! 运行：`cargo bench -p playact`
//!
//! 每个样例先预热 100ms，再测量约 300ms。数字适合做相对比较与回归观察，
//! 不替代 profiler。

use std::hint::black_box;
use std::time::{Duration, Instant};

use playact::{
    Action, BehaviorTree, Blackboard, BoxedNode, Condition, Context, Key, MemParallel, Parallel,
    ReactiveSelector, Selector, Sequence, Status, Threshold, Time, WeightedChild,
    WeightedRandomSelector,
};

const BATCH: u64 = 64;

fn bench(name: &str, mut op: impl FnMut()) {
    let warmup = Duration::from_millis(100);
    let measure = Duration::from_millis(300);

    let deadline = Instant::now() + warmup;
    while Instant::now() < deadline {
        for _ in 0..BATCH {
            op();
        }
    }

    let mut iterations = 0_u64;
    let start = Instant::now();
    while start.elapsed() < measure {
        for _ in 0..BATCH {
            op();
        }
        iterations += BATCH;
    }

    let elapsed = start.elapsed();
    let ns_per_op = elapsed.as_secs_f64() * 1e9 / iterations as f64;
    println!(
        "{name:<32} {ns_per_op:>10.1} ns/op {:>12.0} ops/s",
        1e9 / ns_per_op
    );
}

fn success_action(name: &'static str) -> BoxedNode<u64> {
    Box::new(Action::new(name, |ctx| {
        *ctx.world() += 1;
        Status::Success
    }))
}

fn flat_sequence(count: usize) -> BehaviorTree<u64> {
    BehaviorTree::new(Box::new(Sequence::new(
        (0..count).map(|_| success_action("noop")).collect(),
    )))
}

fn main() {
    println!("playact microbenchmarks (batch = {BATCH})");
    println!("{:<32} {:>13} {:>17}", "case", "latency", "throughput");

    let mut sequence = flat_sequence(16);
    let mut world = 0_u64;
    bench("tick/sequence_16", || {
        black_box(sequence.tick(&mut world, Time::ZERO));
    });

    let mut fallback_children: Vec<BoxedNode<u64>> = (0..7)
        .map(|_| Box::new(Condition::new("miss", |_| false)) as BoxedNode<u64>)
        .collect();
    fallback_children.push(Box::new(Condition::new("found", |_| true)));
    let mut selector = BehaviorTree::new(Box::new(Selector::new(fallback_children)));
    let mut selector_world = 0_u64;
    bench("tick/selector_8_fallback", || {
        black_box(selector.tick(&mut selector_world, Time::ZERO));
    });

    let preempt_children: Vec<BoxedNode<bool>> = vec![
        Box::new(Condition::new("preempt", |ctx: &Context<'_, bool>| {
            *ctx.world_ref()
        })),
        Box::new(Action::new("hold", |_| Status::Running)),
    ];
    let mut reactive = BehaviorTree::new(Box::new(ReactiveSelector::new(preempt_children)));
    let mut reactive_world = false;
    bench("tick/reactive_preempt", || {
        reactive_world = !reactive_world;
        black_box(reactive.tick(&mut reactive_world, Time::ZERO));
    });

    let mut parallel = BehaviorTree::new(Box::new(Parallel::new(
        (0..32).map(|_| success_action("noop")).collect(),
        Threshold::All,
        Threshold::Any,
    )));
    let mut parallel_world = 0_u64;
    bench("tick/parallel_32", || {
        black_box(parallel.tick(&mut parallel_world, Time::ZERO));
    });

    let mut mem_parallel = BehaviorTree::new(Box::new(MemParallel::new(
        (0..32).map(|_| success_action("noop")).collect(),
        Threshold::All,
        Threshold::Any,
    )));
    let mut mem_world = 0_u64;
    bench("tick/mem_parallel_32", || {
        black_box(mem_parallel.tick(&mut mem_world, Time::ZERO));
    });

    let mut weighted = BehaviorTree::new(Box::new(WeightedRandomSelector::new(
        (0..16)
            .map(|index| WeightedChild::new(index + 1, success_action("noop")))
            .collect(),
        7,
    )));
    let mut weighted_world = 0_u64;
    bench("tick/weighted_selector_16", || {
        black_box(weighted.tick(&mut weighted_world, Time::ZERO));
    });

    let inner = flat_sequence(8);
    let mut nested = BehaviorTree::new(Box::new(inner));
    let mut nested_world = 0_u64;
    bench("tick/nested_tree_8", || {
        black_box(nested.tick(&mut nested_world, Time::ZERO));
    });

    let mut serviced = flat_sequence(16);
    for _ in 0..4 {
        serviced.add_service(success_action("service"));
    }
    let mut serviced_world = 0_u64;
    bench("tick/services_4", || {
        black_box(serviced.tick(&mut serviced_world, Time::ZERO));
    });

    static BENCH_KEY: Key<u64> = playact::key!("bench_key");
    let mut board = Blackboard::new();
    bench("blackboard/set_get", || {
        let _ = black_box(board.set(&BENCH_KEY, black_box(7_u64)));
        let _ = black_box(board.get(&BENCH_KEY));
    });

    bench("build/tree_16", || {
        let _ = black_box(flat_sequence(16));
    });

    let described = flat_sequence(16);
    bench("describe/text_16", || {
        let _ = black_box(described.describe().to_string());
    });
    bench("describe/dot_16", || {
        let _ = black_box(described.describe().to_dot());
    });
}
