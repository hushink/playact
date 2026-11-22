# playact

**A type-safe, extensible behavior-tree engine for game automation.**

English | [简体中文](README.zh-CN.md)

[![Crates.io](https://img.shields.io/crates/v/playact.svg)](https://crates.io/crates/playact)
[![Docs.rs](https://img.shields.io/docsrs/playact)](https://docs.rs/playact)
[![MSRV](https://img.shields.io/badge/MSRV-1.92-blue)](Cargo.toml)
[![CI](https://github.com/hushink/playact/actions/workflows/ci.yml/badge.svg)](https://github.com/hushink/playact/actions/workflows/ci.yml)

`playact` is a behavior-tree **engine library**, not a game framework. It knows only
`Status`, `Node`, `Context<W>`, `Blackboard`, `Time`, and `Tracer`. Your world,
commands, and configuration parameter types stay on your side of the boundary.

> [!IMPORTANT]
> The engine defines no business types. `W` (world), `C` (command), and `P`
> (config parameters) are supplied by the application. The public API never
> exposes `Any` or string-keyed blackboard lookups.

## Why playact

- **One contract.** Every node implements `tick(&mut Context<W>) -> Status`.
  Control nodes orchestrate `Status`; leaves perform work. Arbitrary nesting,
  replacement, and reuse come for free.
- **Type-safe state.** `Key<T>` makes blackboard mismatches compile errors.
  Configuration parameters are deserialized into concrete types at build time.
- **Compile-time tree shape.** The typestate builder rejects a root with
  multiple children and decorators with the wrong child count at compile time.
- **Predictable ticks.** No sleeping, no async runtime inside `tick`, no hidden
  threads. Long work returns `Running` and resumes next frame.
- **Interruptible.** A preempted `Running` subtree receives `abort` for external
  cleanup, then `reset` for internal state.
- **Batteries included, optionally.** Typed config, hot reload, remote config,
  Tokio tasks, tracing, snapshots, and a proc-macro DSL are opt-in features.
- **Small core.** The default build is pure `std` with zero third-party
  dependencies; the default-features-off core builds with `no_std + alloc`.

## Installation

```toml
[dependencies]
playact = "0.1"
```

MSRV: **Rust 1.92** (edition 2024).

## Quick start

```rust
use playact::prelude::*;

fn main() -> Result<(), BuildError> {
    let mut tree = BehaviorTreeBuilder::<()>::new("demo")
        .reactive_selector(|b| {
            b.sequence(|b| {
                b.condition("ready", |_| true)
                    .action("work", |_| Status::Success)
            })
            .action("idle", |_| Status::Running)
        })
        .build()?;

    let mut world = ();
    assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    Ok(())
}
```

## Core model

### Status

```rust
pub enum Status {
    Success,
    Failure,
    Running,
}
```

`Status` is the only control-flow protocol between nodes. Business errors are
mapped to `Failure` by the action; diagnostics belong in a `Tracer`.

### Node

```rust
pub trait Node<W>: Send {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status;

    fn abort(&mut self, _ctx: &mut Context<'_, W>) {}
    fn reset(&mut self) {}

    fn name(&self) -> &str;
    fn kind(&self) -> Kind;
    fn children(&self) -> &[BoxedNode<W>];

    // optional snapshot hooks
    fn save_state(&self) -> Option<Vec<u8>> { None }
    fn load_state(&mut self, _state: &[u8]) -> bool { false }
}
```

- `abort` releases external effects when a `Running` subtree is abandoned.
- `reset` clears internal memory; both hooks must be idempotent.
- `Kind` is a fixed enum for built-ins plus `Kind::Custom(&'static str)`.
- Nodes are `Send`; trees can move across threads or into worker pools.

### Context and time

```rust
let mut ctx = Context::new(&mut world, &mut blackboard, Time::from_millis(1000, 250));
let _world: &mut World = ctx.world();
let _board: &mut Blackboard = ctx.blackboard();
let _time: Time = ctx.time();
```

`Time { now, dt }` is supplied by the driver. `Cooldown`, `Timeout`, `Wait`, and
`Throttle` use it exclusively, so tests are fully deterministic with virtual time.
`now` must be monotonic; reset the tree when the driver's clock restarts.

### Typed blackboard

```rust
use playact::prelude::*;

static TARGET: Key<usize> = playact::key!("target");

let mut board = Blackboard::new();
board.set(&TARGET, 42_usize);

assert_eq!(board.get(&TARGET), Some(&42));
// board.get::<u64>(&TARGET); // compile error
```

Keys carry their value type. A blackboard value must be `Send + 'static`. There is
no `DynKey`, no runtime string lookup, and no iteration API.

### Decision/execution separation

Actions read the world and write commands; the driver drains commands outside the
tick and hands them to an executor:

```rust
static COMMANDS: Key<CommandBuffer<BattleCommand>> = playact::key!("commands");

// action
ctx.blackboard()
    .get_or_insert_with(&COMMANDS, CommandBuffer::new)
    .push(BattleCommand::Attack);

// driver
let commands: Vec<_> = tree
    .blackboard_mut()
    .get_mut(&COMMANDS)
    .map(|buffer| buffer.drain().collect())
    .unwrap_or_default();
```

The engine never interprets `BattleCommand`.

## Builder

`BehaviorTreeBuilder` uses closure scopes and typestate:

- the root must produce exactly one child;
- a decorator closure must produce exactly one child;
- composites may be empty (empty `Sequence`/`Parallel` succeed, empty `Selector` fails).

There is no `.end()` and no runtime panic from mismatched closing order.

```rust
let tree = BehaviorTreeBuilder::<World>::new("player")
    .reactive_selector(|b| {
        b.sequence(|b| {
            b.condition("low_hp", |ctx| ctx.world_ref().hp < 0.3)
                .action("heal", |_| Status::Success)
        })
        .action("patrol", |_| Status::Running)
    })
    .build()?;
```

## Nodes

### Composites

| Node | Semantics |
|---|---|
| `Sequence` | Memory-based: advance on success, stay on Running, fail the whole sequence on Failure |
| `ReactiveSequence` | Re-evaluates from the first child every tick; aborts an old Running child when preempted |
| `Selector` | Memory-based: try the next child on Failure, stay on Running |
| `ReactiveSelector` | Re-evaluates from the first child every tick; higher-priority Success/Running preempts lower branches |
| `RandomSelector` | Memory-based; uses a deterministic seed to shuffle the remaining branches on activation |
| `WeightedRandomSelector` | Weighted sampling without replacement; zero weight is never selected; Running branches stay sticky |
| `UtilitySelector` | Scores children every tick and picks the highest; optional hysteresis controls switching |
| `Parallel` | Ticks all children, then applies success/failure thresholds; failure priority; single-tick summary |
| `MemParallel` | Remembers terminal children across ticks until a threshold is met |
| `Race` | All children run in parallel; the first terminal result wins and the rest are aborted |
| `IfThenElse` | Runs `then` or `else`; aborts the old branch if it was Running when the condition flips |

Key details:

- `Parallel` thresholds are `All`, `Any`, or `AtLeast(n)`. `AtLeast(n)` is validated
  against the child count at build time. Thresholds apply to one tick, not across ticks.
- `MemParallel` accumulates terminal state within one activation and resets after the
  result is decided. Use it when different children may finish on different ticks.
- `RandomSelector` and `WeightedRandomSelector` are reproducible for the same seed and
  tick sequence; `reset()` restores the initial RNG.
- `UtilitySelector` evaluates read-only score closures; `NaN` is treated as the lowest
  score and ties go to the earliest child.
- `Race` picks the first terminal child in child order if several finish in the same tick.

### Decorators

`Inverter`, `AlwaysSucceed`, `AlwaysFail`, `Repeat`, `Retry`, `UntilSuccess`,
`UntilFailure`, `Cooldown`, `Timeout`, `Limiter`, `Throttle`, `Guard`.

- `Cooldown` starts only after the child succeeds.
- `Timeout` times only Running children; `duration = 0` fails on the first tick without
  ticking the child.
- `Retry`, `UntilSuccess`, and `UntilFailure` make at most one attempt per tick.
- `Repeat` advances at most `MAX_REPEAT_STEPS_PER_TICK` times per tick.
- `Limiter` is a per-activation quota; it resets on global reset and on `abort → reset`.
- `Throttle` limits terminal results per time window; `Running` does not consume quota.
- `Guard` fails immediately when its condition is false and aborts a previously Running
  child. It is the direct form of a reactive `condition → child` sequence.

### Leaves

`Action`, `Condition`, `Wait`, `SetValue`, `AsyncAction`.

```rust
let action = Action::new("hold_key", |_| Status::Running)
    .with_abort(|_ctx| { /* release key */ })
    .with_reset(|| { /* clear internal state */ });
```

`SetValue<T>` writes a typed key every tick and returns `Success`:

```rust
let leaf = SetValue::<World, u32>::new("set_hp", &HP, 100);
// or builder.set_value("set_hp", &HP, 100)
```

## Composition and reuse

### A tree as a node

`BehaviorTree<W>` implements `Node<W>`, so an entire tree can be nested:

```rust
let mut outer = BehaviorTreeBuilder::<World>::new("outer")
    .sequence(|b| b.raw(Box::new(inner_tree)))
    .build()?;
```

The nested tree shares the outer world, time, blackboard, and Tracer. Its own
blackboard and Tracer are ignored. `abort → reset` propagates to its root.

### Reusable subtrees

`SubtreeFactory` is a cloneable blueprint; every `instantiate()` returns fresh node state:

```rust
let goblin = SubtreeFactory::new(|| build_goblin_root());
let tree = BehaviorTreeBuilder::<World>::new("pair")
    .sequence(|b| b.subtree(&goblin).subtree(&goblin))
    .build()?;
```

### Runtime subtree slots

`SubtreeSlot` switches subtrees from a typed blackboard key:

```rust
static PHASE: Key<u32> = playact::key!("phase");

let slot = SubtreeSlot::new("phase_slot", &PHASE, |phase| match phase {
    1 => build_phase_one(),
    _ => build_phase_two(),
});
```

- Same key: the existing subtree keeps its internal state.
- Key changed: the old subtree gets `abort → reset`, then the factory builds a new one.
- Missing key: the slot aborts the current subtree and returns `Failure`.
- `reset()` discards the subtree; the next tick instantiates it again.

### Services

Services run before the root every tick and share world/time/blackboard. Their return
values are ignored:

```rust
let mut tree = BehaviorTreeBuilder::<World>::new("player")
    .selector(|b| { /* ... */ })
    .build()?;
tree.add_service(Box::new(sense_enemies));
```

Services are useful for sensing and cache maintenance. They do not emit Tracer events
and do not affect `last_status`. Tree `abort` cleans Running services and resets all
services; nested trees run their own services.

## Shared blackboard

`SharedBlackboard` is `Arc<Mutex<Blackboard>>` for multiple trees or threads:

```rust
let shared = SharedBlackboard::new();
shared.tick(&mut tree_a, &mut world, time);
shared.tick(&mut tree_b, &mut world, time);

shared.with(|board| board.get(&TARGET).copied());
shared.with_mut(|board| board.set(&TARGET, 7));
```

> [!WARNING]
> `tick` holds the lock for the whole tree tick. Inside nodes, use
> `ctx.blackboard()` / `ctx.blackboard_ref()` — never lock the same
> `SharedBlackboard` again, or the tick will deadlock.

`lock()`, `try_lock()`, `with()`, `with_mut()`, and `into_inner()` are available.
Poisoned locks are recovered.

## Observability

```rust
let stats = StatsTracer::new();
let mut multi = MultiTracer::new();
multi.push(Box::new(stats.clone()));
multi.push(Box::new(MyTracer));
tree.set_tracer(Some(Box::new(multi)));

// tick...

for (path, node) in stats.nodes() {
    println!("{path:?} {} ticks={} max={:?}", node.name, node.ticks, node.max_time);
}

tree.last_status(); // Option<Status>
```

- `Tracer` receives `Enter` / `Exit` / `Abort` events with a child-index path.
- `MultiTracer` fans events out to multiple tracers.
- `StatsTracer` tracks per-path tick counts, status counts, aborts, and wall-clock time
  (requires `std`).
- `last_status()` and `is_running()` expose the last top-level result.
- `describe()` renders an ASCII tree or Graphviz DOT.

`AbortReason` values are `Preempted`, `Timeout`, `ParallelDone`, `RaceDone`, and
`TreeAbort`; they live in trace events, not in `Node::abort`.

## Snapshots

`BehaviorTree::snapshot()` captures built-in node state so long-running automation can
resume after a restart:

```rust
let snapshot = tree.snapshot();
// persist snapshot.version / snapshot.root / snapshot.services

let mut rebuilt = build_same_tree();
rebuilt.restore(&snapshot)?;
```

Covered state includes sequences, selectors, random/weighted/utility selection, parallel
and race progress, decorators, `Wait`, nested trees, and services. The format is
versioned with `SNAPSHOT_VERSION`; malformed data, changed structure, or a service-count
mismatch return `RestoreError`.

Custom nodes can opt in with `Node::save_state` / `Node::load_state`. Composite custom
nodes must encode their children recursively.

> [!WARNING]
> Snapshots do not include the world, blackboard values, closure captures, `AsyncAction`
> tasks, `SubtreeSlot` dynamic subtrees, or Tracer state. Persist those separately.
> Time-based nodes store absolute `Time.now` values, so restore on the same time axis
> or reset and rebuild.

## Configuration

The optional `config` feature builds trees from data while keeping parameters typed:

```rust
use playact::config::{NodeRegistry, TreeSpec};

let mut registry = NodeRegistry::<World>::new();
registry.condition::<HpBelow>("hp_below", |params: HpBelow| {
    move |ctx: &Context<'_, World>| ctx.world_ref().hp_ratio() < params.ratio
})?;
registry.action::<()>("wait", |_params: ()| {
    move |_: &mut Context<'_, World>| Status::Running
})?;

let spec = TreeSpec::from_json_str(JSON)?;
let mut tree = spec.build(&registry)?;
```

- JSON, TOML, YAML, and RON parsers are available behind format features.
- Unknown fields are rejected; errors carry paths such as `root.children[0]`.
- Supported node types include sequences, selectors, random/weighted selection, race,
  parallel, mem-parallel, decorators (including throttle), waits, actions, conditions,
  and custom factories.
- Predicates and scores for `Guard`, `IfThenElse`, and `UtilitySelector` are business
  logic and stay in code or `custom` factories.
- See [`examples/battle_ai.json`](examples/battle_ai.json) for a complete tree.

### Includes

Split large configurations with `include`:

```rust
use playact::config::FsIncludeResolver;

let resolver = FsIncludeResolver::json("ai/")?;
let tree = spec.build_with_resolver(&registry, &resolver)?;
```

```json
{ "root": { "type": "include", "path": "patrol.json", "name": "patrol" } }
```

`FsIncludeResolver` confines paths to its base directory and rejects `..`, absolute
paths, and symlink escapes. Cycles and excessive nesting are detected during build.
A closure `Fn(&str) -> Result<TreeSpec, ConfigError>` also implements `IncludeResolver`
for custom file, HTTP, or in-memory sources.

### Hot reload

```rust
use playact::hot_reload::TreeWatcher;

let mut watcher = TreeWatcher::<World>::json("ai.json", registry)?;
watcher.tree_mut().tick(&mut world, time);

if watcher.poll()? {
    println!("reloaded: {}", watcher.tree().describe());
}
```

A successful rebuild replaces the tree, preserves blackboard data, and resets node
state. Parse/build failures keep the old tree and return an error. Edits are debounced
(200 ms by default). Use `json_with_resolver` / `with_parser_and_resolver` for includes.

The watcher observes the root file only. Changes to included files do not trigger an
event; call `reload_now()` or touch the root file.

### Remote configuration

```rust
use playact::remote_config::RemoteTreeLoader;

let mut loader = RemoteTreeLoader::<World>::json(
    "https://example.com/ai.json",
    registry,
)?
.with_refresh_interval(Duration::from_secs(30));

loader.poll()?; // conditional request; 304 does not rebuild
loader.tree_mut().tick(&mut world, time);
```

`RemoteTreeLoader` uses `reqwest::blocking`, supports ETag / Last-Modified, and keeps
the old tree on refresh errors. Requests happen on the calling thread; use a worker
thread or a custom `Client` timeout in high-latency environments. Includes are
available through `json_with_resolver` / `with_client_and_resolver`.

## Async actions

`AsyncTask` is a runtime-agnostic handle:

```rust
pub trait AsyncTask: Send {
    fn poll(&mut self) -> Status;
    fn cancel(&mut self);
}
```

`AsyncAction` polls once per tick and cancels on preemption. Two adapters ship built in:

```rust
// std thread; cooperative cancellation
let action = AsyncAction::<World>::new("fetch", |_| {
    Box::new(ThreadTask::spawn(|cancel| {
        if cancel.is_cancelled() {
            return Status::Failure;
        }
        // blocking call / remote request
        Status::Success
    })) as Box<dyn AsyncTask>
});
```

```rust
// requires the `tokio` feature
let action = AsyncAction::<World>::new("fetch", |_| {
    Box::new(TokioTask::spawn(async move {
        // let result = client.call().await;
        Status::Success
    })) as Box<dyn AsyncTask>
});
```

- `TokioTask::spawn` uses the current runtime; outside a runtime it returns an
  immediately failing task instead of panicking.
- `TokioTask::spawn_on(&handle, future)` targets an explicit runtime handle.
- Cancel/drop aborts the Tokio task at an await point.
- Business errors are mapped to `Status` by the future/closure.

## Proc-macro DSL

Enable `macros` to write type-safe trees with `bt!`:

```rust
use playact::prelude::*;

let tree = playact::bt!(World, "player", reactive_selector {
    sequence {
        condition("low_hp", |ctx: &Context<'_, World>| ctx.world_ref().hp < 0.3),
        action("heal", |_| Status::Success),
    },
    action("patrol", |_| Status::Running),
})?;
```

The DSL covers sequences, selectors, random selection, parallel, race, decorators
(including guard and throttle), `action`, `condition`, `wait`, `set_value`, and
`raw(expression)`. Weighted and utility selectors use `raw(...)` because their
weights/scores are code expressions. The macro still goes through the typestate
builder, so child-count mistakes are compile errors.

## no_std

Disable default features to build the core with `alloc`:

```toml
[dependencies]
playact = { version = "0.1", default-features = false }
```

Available in the core: nodes, `BehaviorTree` (including services and snapshots),
builder, `Blackboard` (backed by `BTreeMap`), composites, decorators, leaves,
`CommandBuffer`, `SubtreeFactory`, `SubtreeSlot`, `MultiTracer`, and `describe`.

Requires `std`: `SharedBlackboard`, `AsyncAction` / `ThreadTask` / `TokioTask`,
`StatsTracer`, configuration, hot reload, remote config, and `TracingTracer`.

`SubtreeFactory` uses `alloc::sync::Arc`, so no_std targets need atomic pointers.
The `macros` feature is no_std-compatible.

```text
cargo check -p playact --no-default-features --target thumbv7em-none-eabihf --lib
```

## Feature flags

| Feature | Enables |
|---|---|
| default | `std`; pure standard library, zero third-party dependencies |
| `std` | Standard-library integrations; disabling it switches the core to `no_std + alloc` |
| `config` | `TreeSpec`, `NodeRegistry`, JSON parsing, typed parameters, includes (requires `std`) |
| `toml` | TOML parser |
| `yaml` | YAML parser (`serde_yaml_ng`) |
| `ron` | RON parser |
| `tracing` | `TracingTracer` |
| `hot-reload` | `TreeWatcher` with `notify` file watching |
| `remote-config` | `RemoteTreeLoader` with `reqwest::blocking` |
| `tokio` | `TokioTask` adapter |
| `macros` | `playact-macros` `bt!` proc-macro DSL |
| `full` | All of the above |

## Threading model

- `Node<W>: Send` and `BehaviorTree<W>: Send`; `Sync` is not required.
- `W` does not need to be `Send`/`Sync`; the tree borrows `&mut W` only for a tick.
- Blackboard values must be `Send + 'static`.
- `SharedBlackboard` serializes multi-tree ticks with a mutex.
- Hot-reload callbacks only send events from a background thread; rebuild and tick run
  on the caller's thread.
- Remote configuration requests run synchronously on the calling thread.
- Async tasks are polled on the tick thread; worker/remote cancellation is cooperative
  (Tokio tasks are aborted).
- Each AI usually owns its tree and blackboard; build multiple AIs with factories.

## Examples

```text
cargo run -p playact --example battle_ai
cargo run -p playact --example battle_ai_config --features config
```

- `battle_ai`: deterministic 1D duel between a player and three goblins. Shows
  ReactiveSelector interrupts, Cooldown, Timeout, Inverter, Tracer, abort cleanup,
  and CommandBuffer/Executor separation.
- `battle_ai_config`: the same ideas driven by JSON configuration, typed parameters,
  and custom node factories.

## Testing and benchmarks

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo bench -p playact
cargo doc --workspace --all-features --no-deps
cargo check -p playact --no-default-features --target thumbv7em-none-eabihf --lib
```

- Unit tests cover every node, builder, config layer, reload, remote loading, snapshots,
  and the async adapters.
- Public-API integration tests and doctests exercise the crate as a user would.
- `tests/stress.rs` generates deterministic random trees (including services, weighted
  and utility selection, race, guard, and snapshot/restore) and checks tick, abort,
  reset, describe, and Send invariants.
- `benches/engine.rs` is a dependency-free microbenchmark suite for tick paths,
  blackboard access, construction, and describe output.
- CI runs stable and MSRV 1.92 on Linux and Windows, plus an embedded no_std check.
