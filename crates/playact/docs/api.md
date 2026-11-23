# playact API guide

Task-oriented API reference for `playact` 0.1.x. It covers the public types and
methods you normally call, with runnable-shaped examples and the scenarios each
API is designed for.

- [README](../README.md) - project overview and feature flags.
- This guide - API usage, method tables, and scenario recipes.
- [Configuration reference](config.md) - every configuration node, field, and default.
- [docs.rs/playact](https://docs.rs/playact) - per-item rustdoc, including every field and error variant.
- [`examples/`](../examples) - complete runnable programs.

Code snippets assume:

```rust
use playact::prelude::*;
```

They may omit `use std::time::Duration;` and the surrounding function's
error-handling boilerplate. `W` is your world type, for example:

```rust
struct World {
    hp: f32,
    enemy_near: bool,
    phase: u32,
}

impl Default for World {
    fn default() -> Self {
        Self { hp: 100.0, enemy_near: false, phase: 1 }
    }
}
```

> [!IMPORTANT]
> The engine has no `WorldState`, business `Action`, or command enum. All of
> those types stay in your crate. The API is generic over `W` and uses typed
> `Key<T>` and `EventKey<T>` values instead of string lookups.

## Feature flags

Most APIs are always available. Optional features gate the corresponding
modules and methods:

| Feature | What it adds |
|---|---|
| default (`std`) | `std` integration; core is pure `std` with no third-party dependencies |
| `config` | `TreeSpec`, `NodeRegistry`, JSON, includes, validation, and schema |
| `toml` / `yaml` / `ron` | Parsers for those formats |
| `schema` | `TreeSpec::json_schema()` JSON Schema export |
| `hot-reload` | `TreeWatcher` (depends on `notify`) |
| `remote-config` | `RemoteTreeLoader` (depends on `reqwest::blocking`) |
| `inspector` | HTML report and zero-dependency HTTP server |
| `diff-serde` | `TreeDiff` / `DiffHistory` JSON serialization and parsing |
| `blackboard-serde` | `BlackboardCodec` typed blackboard persistence |
| `snapshot-serde` | `TreeSnapshot` / `NodeIdSnapshot` JSON serialization |
| `trace-record` | `TraceRecorder` / `TraceRecord` and trace diffing |
| `trace-serde` | `TraceRecorder` JSON export/import |
| `tracing` | `TracingTracer` integration |
| `tokio` | `TokioTask` async adapter |
| `macros` | `bt!` proc-macro DSL and `#[derive(BtNode)]` |
| `test-util` | `TestDriver` |
| `full` | Everything above |

With `default-features = false`, the core builds on `no_std + alloc`. `std`-only
features (`shared`, `stats`, async actions, config/hot reload/remote config,
inspector) are unavailable there.

## 1. Core contract

### `Status`

Every node communicates only through three values:

| Variant | Meaning | Helper |
|---|---|---|
| `Success` | Work completed successfully | `is_success()` |
| `Failure` | Work failed and should not be resumed | `is_failure()` |
| `Running` | Not finished; tick again | `is_running()` |

```rust
let world = World::default();
let status = Status::from_bool(world.hp > 0.0);
assert_eq!(status.is_terminal(), status.is_success() || status.is_failure());
```

`Status::is_terminal()` is true for `Success` and `Failure`.

### `Node<W>`

The only required method is `tick`. The other methods have defaults and are
called by the engine when needed:

| Method | When it runs | Typical work |
|---|---|---|
| `tick(&mut Context<W>) -> Status` | every logic frame | read world/blackboard, return status |
| `abort(&mut Context<W>)` | a Running subtree is abandoned | release locks, stop sounds, cancel requests |
| `reset()` | full tree reset or after abort | clear timers, indices, cached targets |
| `name() -> &str` | diagnostics | display name |
| `kind() -> Kind` | diagnostics/tools | fixed tag or `Kind::Custom("...")` |
| `id() -> Option<&NodeId>` | snapshots/diffs/metrics | stable identity, usually via `.id(...)` |
| `children()` / `children_mut()` | tooling / ID snapshots | custom composites expose children |
| `save_own_state()` / `load_own_state()` | ID-keyed snapshots | custom stateful leaves |
| `save_state()` / `load_state()` | structural snapshots | custom composites/decorators |

Custom leaf example:

```rust
use playact::{Context, Kind, Node, Status};

struct WaitForTarget;

impl Node<World> for WaitForTarget {
    fn tick(&mut self, ctx: &mut Context<'_, World>) -> Status {
        Status::from_bool(ctx.blackboard_ref().contains(&TARGET))
    }

    fn name(&self) -> &str {
        "wait_for_target"
    }

    fn kind(&self) -> Kind {
        Kind::Condition
    }
}
```

Attach it with `.raw(...)`:

```rust
let mut tree = BehaviorTreeBuilder::<World>::new("guard")
    .sequence(|b| {
        b.raw(Box::new(WaitForTarget))
            .action("strike", |_| Status::Success)
    })
    .build()?;
```

Contract rules:

- A node returning `Success` or `Failure` must be reusable on its next
  activation.
- `abort` and `reset` must be idempotent.
- Long work should return `Running` and resume on the next tick; do not block
  inside `tick` (use `AsyncAction` for blocking or remote work).
- `Node<W>: Send`; `W` itself does not need to be `Send` because the tree only
  borrows `&mut W` during a tick.

### `Context<W>`

`Context` is borrowed for one tick and gives nodes access to the world,
blackboard, time, and child-helpers.

| Method | Purpose |
|---|---|
| `world() -> &mut W` | mutate the world during this tick |
| `world_ref() -> &W` | read the world |
| `blackboard() -> &mut Blackboard` | read/write typed state |
| `blackboard_ref() -> &Blackboard` | read-only typed state |
| `time() -> Time` | current virtual time and frame delta |
| `report_error(name, kind, error)` | emit a `TraceEvent::Error` and count an error |
| `tick_child(index, child)` | parent composites: tick a child with correct tracing |
| `abort_child(index, child, reason)` | parent composites: abort a child with a reason |

`tick_child` / `abort_child` exist for custom composites. They keep trace paths,
error routing, and abort semantics consistent with built-in composites.

```rust
ctx.blackboard().set(&TARGET, 7_u32);
if ctx.blackboard_ref().get(&TARGET).is_some() {
    ctx.report_error("scanner", Kind::Action, "target already acquired");
}
```

### `Time`

```rust
let t = Time::from_millis(1_000, 16); // now = 1s, dt = 16ms
let t = Time::new(Duration::from_secs(1), Duration::from_millis(16));
```

`Time` is virtual: the driver decides what "now" means. Cooldown, Timeout, Wait,
and Throttle use it exclusively, so tests are deterministic and do not sleep.

## 2. Typed blackboard

`Blackboard` stores values by type and static key name. A key mismatch is a
compile error, not a runtime downcast.

```rust
use playact::{Blackboard, Key};

static TARGET: Key<u32> = Key::new("combat.target");
static PLAN: Key<Vec<u32>> = Key::new("combat.plan");

let mut board = Blackboard::new();
board.set(&TARGET, 42);
assert_eq!(board.get(&TARGET), Some(&42));

// `board.get(&OTHER_KEY)` where OTHER_KEY: Key<String> would not compile:
// `get` returns `Option<&T>`, so the key type pins the value type.
```

| Method | Purpose |
|---|---|
| `set(&Key<T>, value) -> Option<T>` | insert/replace, returning the old value |
| `get(&Key<T>) -> Option<&T>` | read |
| `get_mut(&Key<T>) -> Option<&mut T>` | mutate in place |
| `get_or_insert_with(&Key<T>, f) -> &mut T` | lazily initialize |
| `contains(&Key<T>) -> bool` | presence check |
| `remove(&Key<T>) -> Option<T>` | take the value |
| `clear_key(&Key<T>)` | drop one key |
| `clear()`, `len()`, `is_empty()` | whole-board operations |

The `key!` / `event_key!` macros are shortcuts for static keys:

```rust
static TARGET: Key<u32> = playact::key!("combat.target");
static HIT: EventKey<u32> = playact::event_key!("combat.hit");
```

Scenario: a scouting action writes `TARGET`; an attack condition reads it. Since
both use `Key<u32>`, neither can accidentally read a string or a different
entity type.

## 3. `BehaviorTree`

A tree owns the root node, optional services, the blackboard, and an optional
tracer. Ticks are synchronous.

```rust
let mut tree = BehaviorTreeBuilder::<World>::new("player")
    .reactive_selector(|b| {
        b.sequence(|b| {
            b.condition("low hp", |ctx| ctx.world_ref().hp < 30.0)
                .action("heal", |ctx| {
                    ctx.world().hp += 20.0;
                    Status::Success
                })
        })
        .action("patrol", |_| Status::Running)
    })
    .build()?;

let mut world = World::default();
let status = tree.tick(&mut world, Time::from_millis(0, 16));
```

| Method | Purpose |
|---|---|
| `new(root)`, `with_name(name)`, `name()` | construction and naming |
| `tick(&mut world, time) -> Status` | run one frame with the internal blackboard |
| `tick_with_blackboard(&mut world, time, &mut board)` | run with an external blackboard |
| `abort(&mut world, time)` | abandon a Running tree; calls `abort` then `reset` |
| `abort_with_blackboard(...)` | same with an external blackboard |
| `reset()` | clear node state without calling `abort` |
| `blackboard()` / `blackboard_mut()` | inspect or seed typed state |
| `last_status()`, `is_running()` | inspect the previous frame |
| `with_service(node)`, `add_service(node)`, `services()`, `service_count()` | services that run before the root every tick |
| `set_tracer(Option<Box<dyn Tracer + Send>>)`, `with_tracer(...)` | install observability |
| `describe() -> TreeDescription`, `describe_services()` | structural snapshot for UI/tools |
| `duplicate_node_id()`, `duplicate_node_id_path()` | audit IDs after hand-built trees |
| `snapshot()`, `restore(&snapshot)` | structural state capture/restore |
| `snapshot_by_id()`, `restore_by_id`, `restore_by_id_matching` | ID-keyed state, tolerant of reordering |
| `into_root()` | reuse this tree as a subtree root |

Abort/reset semantics:

- `abort` only invokes node `abort` when the previous root status was
  `Running`; a terminal tree has no Running subtree to clean up.
- Services are always reset; services that last returned `Running` are aborted
  first.
- `reset()` never calls `abort`; call `abort` first if external cleanup matters.

Services are ordinary nodes. They run every tick before the root and share the
blackboard, which makes them suitable for sensing, event pumping, or cooldown
bookkeeping:

```rust
let tree = BehaviorTreeBuilder::<World>::new("ai")
    .action("think", |_| Status::Success)
    .build()?
    .with_service(Box::new(MyScanNode)); // any BoxedNode<World>
```

## 4. Builder

`BehaviorTreeBuilder<W, S>` uses typestate to make tree shape errors compile
errors: an empty root cannot be built, a decorator accepts exactly one child,
and a composite's children are scoped by the closure.

```rust
let mut tree = BehaviorTreeBuilder::<World>::new("guard")
    .reactive_selector(|b| {
        b.sequence(|b| {
            b.condition("enemy near", |ctx| ctx.world_ref().enemy_near)
                .cooldown(Duration::from_millis(800), |b| {
                    b.action("attack", |_| Status::Success)
                })
        })
        .action("idle", |_| Status::Running)
    })
    .id("root")
    .build()?;
```

Leaves:

| Builder method | Node produced |
|---|---|
| `action(name, FnMut(&mut Context) -> Status)` | `Action` |
| `try_action(name, FnMut(&mut Context) -> Result<Status, E>)` | `TryAction`; `Err` emits a trace error and returns `Failure` |
| `async_action(name, FnMut(&mut Context) -> Box<dyn AsyncTask>)` | `AsyncAction` |
| `condition(name, Fn(&Context) -> bool)` | `Condition` |
| `wait(duration)` | `Wait` (virtual time) |
| `wait_until(name, Fn(&Context) -> bool)` | `WaitUntil` |
| `wait_for_event(EventKey<T>)` | `WaitForEvent` |
| `set_value(name, &Key<T>, value)` | `SetValue` |
| `raw(BoxedNode<W>)` | any pre-built node, including custom structs |
| `subtree(&SubtreeFactory<W>)` | a fresh subtree instance |

Composites: `sequence`, `reactive_sequence`, `selector`, `reactive_selector`,
`random_selector(seed, ...)`, `weighted_random_selector(children, seed)`,
`utility_selector(children)`, `parallel(success, failure, ...)`,
`mem_parallel(success, failure, ...)`, `if_then_else(condition, then, else)`,
`race(...)`.

Decorators: `inverter`, `always_succeed`, `always_fail`, `repeat(times, ...)`,
`retry(attempts, ...)`, `until_success`, `until_failure`, `guard(condition, ...)`,
`probability(chance, seed, ...)`, `while_do(condition, ...)`,
`cooldown(duration, ...)`, `timeout(duration, ...)`, `limiter(max, ...)`,
`throttle(window, max, ...)`, and `scoped(name, inputs, outputs, ...)`.

Stable IDs:

```rust
let tree = BehaviorTreeBuilder::<World>::new("ai")
    .sequence(|b| {
        b.action("scan", |_| Status::Success)
            .id("scan")
            .action("attack", |_| Status::Success)
            .id("attack")
    })
    .id("root")
    .build()?;
```

IDs enable snapshots, per-node stats, diffs, duplicate-ID validation, and
state-preserving reloads. Duplicate IDs in one tree are rejected at `build()`.

## 5. Composites

| Node | Semantics | Typical scenario |
|---|---|---|
| `Sequence` | memory sequence: remembers the Running child; earlier steps are not re-run | combos, multi-step interactions |
| `ReactiveSequence` | re-checks from the first child every tick; preempts and aborts the old Running child | gate conditions that must stay true |
| `Selector` | memory selector: resumes the Running child | prioritized task queue |
| `ReactiveSelector` | re-evaluates higher-priority branches every tick; aborts a preempted branch | interruptible combat/pickups |
| `RandomSelector` | selects a child deterministically by seed, keeps it while Running | varied behavior without external RNG |
| `WeightedRandomSelector` | weighted choice; zero weight is never selected | loot/attack variety |
| `UtilitySelector` | scores all branches each tick, picks the max; optional hysteresis | tactical choice by situation score |
| `Parallel` | ticks every child in one frame; resolves via `success` / `failure` thresholds; aborts the rest on resolution | run animation + movement together |
| `MemParallel` | accumulates terminal results across ticks | long-running parallel goals |
| `Race` | first terminal child wins, others are aborted | "whichever finishes first" |
| `IfThenElse` | picks a branch each tick; switching aborts the old Running branch | binary decisions with state |
| `Switch` | picks a branch by a typed blackboard key; key change aborts the old branch | boss phases, quest stages |

`Threshold` values are `Threshold::All`, `Threshold::Any`, and
`Threshold::AtLeast(NonZeroUsize)`.

Interruptible combat:

```rust
let mut tree = BehaviorTreeBuilder::<World>::new("combat")
    .reactive_selector(|b| {
        b.sequence(|b| {
            b.condition("low hp", |ctx| ctx.world_ref().hp < 30.0)
                .action("heal", |_| Status::Success)
        })
        .sequence(|b| {
            b.condition("enemy near", |ctx| ctx.world_ref().enemy_near)
                .action("attack", |_| Status::Success)
        })
        .action("patrol", |_| Status::Running)
    })
    .build()?;
```

Utility selection:

```rust
use playact::{UtilityChild, UtilitySelector};

let heal_branch = BehaviorTreeBuilder::<World>::new("heal branch")
    .action("heal", |_| Status::Success)
    .build()?
    .into_root();
let attack_branch = BehaviorTreeBuilder::<World>::new("attack branch")
    .action("attack", |_| Status::Success)
    .build()?
    .into_root();

let select = UtilitySelector::new(vec![
    UtilityChild::new(
        |ctx: &Context<'_, World>| if ctx.world_ref().hp < 30.0 { 100.0 } else { 0.0 },
        heal_branch,
    ),
    UtilityChild::new(
        |ctx: &Context<'_, World>| if ctx.world_ref().enemy_near { 50.0 } else { 1.0 },
        attack_branch,
    ),
])
.with_hysteresis(10.0);

let tree = BehaviorTreeBuilder::<World>::new("tactics")
    .raw(Box::new(select))
    .build()?;
```

`UtilityChild::new(score, child)` takes a `Fn(&Context<W>) -> f32` score and a
pre-built child. Build each branch normally, then call `into_root()`.

Parallel with thresholds:

```rust
let mut tree = BehaviorTreeBuilder::<World>::new("move-and-shoot")
    .parallel(Threshold::All, Threshold::Any, |b| {
        b.action("move", |_| Status::Running)
            .action("shoot", |_| Status::Success)
    })
    .build()?;
```

## 6. Decorators

| Decorator | Effect | Scenario |
|---|---|---|
| `Inverter` | swaps Success/Failure, passes Running | "not detected" conditions |
| `AlwaysSucceed` / `AlwaysFail` | forces the terminal result | optional steps / forced failure probes |
| `Repeat(n)` | runs the child `n` successful times; bounded per tick by `MAX_REPEAT_STEPS_PER_TICK` | combos, multi-hit attacks |
| `Retry(n)` | retries failures up to `n` attempts, one attempt per tick | flaky network/interaction steps |
| `UntilSuccess` / `UntilFailure` | keeps the child Running until the target result | wait for an event to stop |
| `Probability(chance, seed)` | deterministic random gate | miss chance, variation |
| `While(predicate)` | repeats the child while true; aborts when false | hold-position loops |
| `Guard(predicate)` | returns Failure without ticking the child when false; aborts a Running child when it turns false | prerequisite checks |
| `Cooldown(duration)` | after a Success, returns Failure for `duration` without ticking the child | attack cooldowns |
| `Timeout(duration)` | aborts the child and fails after `duration` | bounded interactions |
| `Limiter(max)` | allows at most `max` terminal results | consumables, one-shot quest steps |
| `Throttle(window, max)` | at most `max` terminal results per time window | rate limits |

```rust
let mut tree = BehaviorTreeBuilder::<World>::new("attack")
    .guard(|ctx: &Context<'_, World>| ctx.world_ref().enemy_near, |b| {
        b.cooldown(Duration::from_secs(1), |b| {
            b.action("strike", |_| Status::Success)
        })
    })
    .build()?;
```

All time-based decorators use virtual `Time`, so tests advance time explicitly.

## 7. Leaves

| Leaf | Constructor / builder | Notes |
|---|---|---|
| `Action<W>` | `Action::new(name, tick)` / `.action(...)` | closure state is per node |
| `Condition<W>` | `Condition::new(name, predicate)` / `.condition(...)` | read-only, returns Success/Failure |
| `Wait<W>` | `Wait::new(duration)` / `.wait(...)` | Running until virtual time elapses |
| `WaitUntil<W>` | `WaitUntil::new(name, predicate)` / `.wait_until(...)` | polls a predicate |
| `WaitForEvent<W, T>` | `WaitForEvent::new(EventKey<T>)` / `.wait_for_event(...)` | consumes events FIFO |
| `TryAction<W, E>` | `TryAction::new(name, fallible_tick)` / `.try_action(...)` | `Err` -> trace error + Failure |
| `SetValue<W, T>` | `SetValue::new(name, &Key<T>, value)` / `.set_value(...)` | writes every tick |

Action cleanup callbacks:

```rust
let mut action = Action::<World>::new("aim", |ctx| {
    ctx.world().aiming = true;
    Status::Running
})
.with_abort(|ctx| {
    ctx.world().aiming = false;
})
.with_reset(|| {
    // reset closure-owned counters/phase here
});
```

`with_abort` runs when a Running subtree is preempted and receives a context so
it can mutate the world. `with_reset` runs on tree reset and takes no context;
use it for state owned by the tick closure. World-side cleanup belongs in
`with_abort` (preemption) or in the driver's reset path, because `Node::reset`
also has no context.

Error reporting:

```rust
let mut tree = BehaviorTreeBuilder::<World>::new("interact")
    .try_action("open chest", |ctx| {
        if ctx.world_ref().enemy_near {
            return Err("enemy nearby");
        }
        Ok(Status::Success)
    })
    .build()?;
```

Install a tracer (`StatsTracer`, `TraceRecorder`, or a custom `Tracer`) to see
the emitted `TraceEvent::Error`.

`WaitForEvent` consumes typed events:

```rust
static HIT: EventKey<u32> = playact::event_key!("combat.hit");

let mut tree = BehaviorTreeBuilder::<World>::new("react")
    .sequence(|b| {
        b.wait_for_event(HIT)
            .action("apply damage", |ctx| {
                let damage = HIT.pop(ctx).unwrap_or_default();
                ctx.world().hp -= damage as f32;
                Status::Success
            })
    })
    .build()?;
```

## 8. Events and commands

### Event queues

| API | Purpose |
|---|---|
| `EventKey::new(name)` / `event_key!` | static typed event key |
| `key.push(ctx, event)` | enqueue from a node |
| `key.pop(ctx) -> Option<T>` | consume FIFO |
| `key.len(ctx)` / `is_empty(ctx)` / `clear(ctx)` | inspect/clear the queue |

Events are stored on the current blackboard under the key name. They are ideal
for decoupling producers (network/UI) from consumers (behavior tree leaves).

### Command buffers

Business commands should not mutate the world directly if the application needs
a clean commit point. Store a typed `CommandBuffer<C>` in the blackboard and have
the driver drain it after the tick:

```rust
#[derive(Clone, Copy, Debug)]
enum Command {
    Move { x: f32, y: f32 },
    Attack(u32),
}

static COMMANDS: Key<CommandBuffer<Command>> = playact::key!("commands");

let mut tree = BehaviorTreeBuilder::<World>::new("ai")
    .action("attack", |ctx| {
        ctx.blackboard()
            .get_or_insert_with(&COMMANDS, CommandBuffer::new)
            .push(Command::Attack(7));
        Status::Success
    })
    .build()?;

// After tree.tick(...):
for command in tree
    .blackboard_mut()
    .get_mut(&COMMANDS)
    .map(CommandBuffer::drain)
    .into_iter()
    .flatten()
{
    apply(command);
}
```

## 9. Async and remote actions

`AsyncAction` starts a task on first tick, polls it on later ticks, and cancels
it on abort/reset/drop. `tick` never blocks.

| Type | Use |
|---|---|
| `AsyncTask` trait | implement `poll() -> Status` and `cancel()` |
| `ThreadTask::spawn(FnOnce(CancellationToken) -> Status)` | blocking file/network work on a thread |
| `CancellationToken` | cooperative cancellation flag |
| `TokioTask::spawn(future)` | spawn on the current Tokio runtime |
| `TokioTask::spawn_on(&handle, future)` | explicit runtime handle |
| `AsyncAction::new(name, start)` | leaf wrapper; `start` returns a task |

```rust
let mut tree = BehaviorTreeBuilder::<World>::new("remote")
    .async_action("fetch plan", |ctx| {
        let target = ctx.blackboard_ref().get(&TARGET).copied().unwrap_or_default();
        Box::new(ThreadTask::spawn(move |cancel| {
            if cancel.is_cancelled() {
                return Status::Failure;
            }
            // blocking work with `target`; check the token periodically
            Status::Success
        }))
    })
    .build()?;
```

Tokio variant (requires `tokio`):

```rust
.async_action("http", |_| {
    Box::new(TokioTask::spawn(async move {
        match call_remote().await {
            Ok(_) => Status::Success,
            Err(_) => Status::Failure,
        }
    }))
})
```

Business errors must be mapped to `Status` by the task. Calling
`TokioTask::spawn` outside a runtime returns a task that reports `Failure`
instead of panicking.

## 10. Reuse and boundaries

| API | Purpose | Scenario |
|---|---|---|
| `SubtreeFactory::new(|| BoxedNode)` | cloneable blueprint; `instantiate()` gives fresh state | spawn many goblins with the same AI |
| `SubtreeSlot::new(name, &Key<K>, factory)` | switches subtree when a typed key changes; aborts the old one | boss phases, quest stages |
| `SubtreeContract::new(name, child)` | requires input keys; clears declared outputs on activation/abort | explicit interface between subtrees |
| `ScopedSubtree::new(name, child)` | private blackboard; copies declared inputs in and outputs back | isolate a minion's state |
| nested `BehaviorTree` | a tree implements `Node<W>` | share services/tracers |

```rust
let goblin = SubtreeFactory::<World>::new(|| {
    BehaviorTreeBuilder::<World>::new("goblin")
        .action("chase", |_| Status::Running)
        .build()
        .expect("valid subtree")
        .into_root()
});

let tree = BehaviorTreeBuilder::<World>::new("pair")
    .sequence(|b| b.subtree(&goblin).subtree(&goblin))
    .build()?;
```

```rust
static PHASE: Key<u32> = playact::key!("boss.phase");

let slot = SubtreeSlot::<World, u32>::new("phase", &PHASE, |phase| match phase {
    1 => build_phase_one(),
    2 => build_phase_two(),
    _ => build_enraged(),
});

let tree = BehaviorTreeBuilder::<World>::new("boss")
    .raw(Box::new(slot))
    .build()?;
```

Contracts and scopes:

```rust
let strike = SubtreeContract::new("strike", attack_root())
    .require(&TARGET)   // missing -> trace error + Failure
    .provide(&PLAN);    // cleared on activation/abort

let isolated = ScopedSubtree::new("minion", minion_root())
    .input(&TARGET)     // requires Key<T>: T: Clone
    .output(&PLAN);
```

Builder form for scopes:

```rust
let tree = BehaviorTreeBuilder::<World>::new("minion")
    .scoped("minion", [&TARGET], [&PLAN], |b| {
        b.action("plan", |_| Status::Success)
    })
    .build()?;
```

Tracer events inside a `ScopedSubtree` are suppressed; the scope node itself
still appears in the parent trace.

## 11. State, persistence, and testing

### Structural snapshots

```rust
let snapshot = tree.snapshot();          // version + root/services bytes
tree.restore(&snapshot)?;                // same structure required
```

Snapshots exclude the world, blackboard values, closure captures, `AsyncAction`
tasks, and dynamic `SubtreeSlot` state. Persist `snapshot.version`,
`snapshot.root`, and `snapshot.services` with your serializer, or enable
`snapshot-serde` for `TreeSnapshot::to_json` / `from_json`.

### ID-keyed snapshots

```rust
let snapshot = tree.snapshot_by_id()?;   // BTreeMap<NodeId, Vec<u8>>
tree.restore_by_id(&snapshot)?;          // strict: unknown IDs are errors
let restored = tree.restore_by_id_matching(&snapshot)?; // tolerate removed IDs

let json = snapshot.to_json();           // snapshot-serde; IDs become object keys
let snapshot = NodeIdSnapshot::from_json(&json)?;
```

Use ID-keyed snapshots when node order may change. Custom stateful nodes must
implement `save_own_state` / `load_own_state`; custom composites must also
expose `children_mut()`.

### Blackboard persistence

```rust
let mut codec = BlackboardCodec::new();
codec.register(&TARGET)?;
let bytes = codec.encode(tree.blackboard())?;
codec.decode(tree.blackboard_mut(), &bytes)?;
```

Only registered keys are saved. Unknown keys or malformed values produce typed
errors.

### `TestDriver`

```rust
let mut driver = TestDriver::new(tree, World::default()).with_step(Duration::from_millis(16));
assert_eq!(driver.tick(), Status::Running);

let statuses = driver.tick_n(10);
let outcome = driver.run_until_terminal(600);
let (tree, world) = driver.into_parts();
```

| Method | Purpose |
|---|---|
| `tick()`, `tick_at(time)`, `tick_n(n)` | advance virtual time and tick |
| `run_until_terminal(max)`, `run_until_status(status, max)` | bounded loop with a result |
| `advance(delta)`, `now()`, `time()` | time control |
| `install_trace_recorder()` | installs and returns a `TraceRecorder` |
| `tree()`, `tree_mut()`, `world()`, `world_mut()`, `blackboard()` | inspect state |

### Trace replay

```rust
let recorder = TraceRecorder::new();
tree.set_tracer(Some(Box::new(recorder.clone())));
// run the scenario
let baseline = recorder.records();

recorder.clear();
// run the same scenario again
if let Some(divergence) = recorder.diff(&baseline) {
    eprintln!("trace diverged at {:?}", divergence);
}
```

`TraceDiff` reports the first differing event (including length mismatch).
Replay compares observable node behavior, not world state. With `trace-serde`,
`recorder.to_json()` and `TraceRecorder::from_json()` persist a baseline across
processes; custom node kinds cannot be deserialized.

## 12. Observability

### Tracer events

| Event | Fields | Use |
|---|---|---|
| `TraceEvent::Enter` | id, name, kind, path, time | call stacks, ordering |
| `TraceEvent::Exit` | id, name, kind, path, status, time | status timelines |
| `TraceEvent::Abort` | id, name, kind, path, reason, time | preemption debugging |
| `TraceEvent::Error` | id, name, kind, path, message, time | recoverable business errors |

`AbortReason` values: `Preempted`, `Timeout`, `ParallelDone`, `RaceDone`,
`TreeAbort`.

Custom tracer:

```rust
struct LogTracer;

impl Tracer for LogTracer {
    fn event(&mut self, event: TraceEvent<'_>) {
        if let TraceEvent::Exit { name, status, .. } = event {
            println!("{name} -> {status:?}");
        }
    }
}

tree.set_tracer(Some(Box::new(LogTracer)));
```

`MultiTracer` fans events out to several tracers:

```rust
let mut multi = MultiTracer::new();
multi.push(Box::new(stats.clone()));
multi.push(Box::new(recorder.clone()));
tree.set_tracer(Some(Box::new(multi)));
```

### `StatsTracer`

| Method | Purpose |
|---|---|
| `nodes()` | all observed paths and stats |
| `node(&[usize])` | stats by structural path |
| `node_by_id(&NodeId)` | stats by stable ID |
| `render_prometheus()` | Prometheus text |
| `render_tree(&tree)` | ASCII tree with last statuses |
| `clear()` | reset counters |

Stats are keyed by path but also indexed by `NodeId`, so attaching `.id(...)`
makes metrics stable across structural edits.

### HTML inspector

```rust
use playact::inspector::{Inspector, InspectorServer};

let stats = StatsTracer::new();
tree.set_tracer(Some(Box::new(stats.clone())));

Inspector::new(&tree)
    .with_stats(&stats)
    .with_diff_history(&history)
    .with_title("combat ai")
    .write_to("tree.html")?;
```

Live server:

```rust
let mut server = InspectorServer::bind("127.0.0.1:7878")?;
server.poll_with_history(&tree, Some(&stats), &history)?;

// Include restored node count and any restore error.
let report = watcher.last_reload();
server.poll_with_report(watcher.tree(), Some(&stats), watcher.diff_history(), report)?;
```

| Endpoint | Response |
|---|---|
| `/` | HTML report with tree, statuses, services, NodeId audit, history, restore metadata |
| `/metrics` | Prometheus text |
| `/tree` | ASCII status tree |
| `/status.json` | root status, node/stats counts, history length, oldest/latest revision, latest timestamp, restored count, restore error |
| `/diff.json` | latest `TreeDiff` JSON (`diff-serde`) |
| `/diffs.json[?since=N]` | revision entries, optionally only newer than N (`diff-serde`) |
| `/health` | `ok` |

The server is a debug tool: no authentication, loopback only.

### Tree diffs and history

```rust
let diff = TreeDiff::between(&old_tree, &new_tree);
println!("{diff}");

let mut history = DiffHistory::new(16);
history.push_now(diff);              // revision + wall-clock timestamp
let latest = history.latest_entry();
```

| Method | Purpose |
|---|---|
| `TreeDiff::between(old, new)` | added/removed/moved/renamed by `NodeId` |
| `diff.is_empty()`, `diff.to_json()`, `from_json()` | inspect/serialize/parse |
| `DiffHistory::push`, `push_at`, `push_now` | append with optional timestamp |
| `latest`, `latest_entry`, `latest_revision`, `oldest_revision` | cursors |
| `entries`, `entries_since(rev)`, `iter` | traverse |
| `to_json`, `to_json_since(rev)`, `from_json` | JSON arrays of `{revision, timestamp?, diff}` |
| `pop_latest`, `set_limit`, `clear` | maintenance |

A node that both moved and changed name or kind appears in `moved` and
`renamed`. A polling client should remember `latest_revision` and request only
newer entries; `oldest_revision` tells it when eviction may have skipped data.

## 13. Configuration

The config layer is pure data plus a build-time registry. Configuration never
contains closures; parameters are deserialized into concrete types once. See
the [configuration reference](config.md) for every node and field.

```rust
use playact::config::{NodeRegistry, TreeSpec};

let mut registry = NodeRegistry::<World>::new();
registry.action::<()>("attack", |_| {
    move |ctx: &mut Context<'_, World>| {
        ctx.world().hp -= 1.0;
        Status::Success
    }
})?;
registry.condition::<()>("enemy_near", |_| {
    |ctx: &Context<'_, World>| ctx.world_ref().enemy_near
})?;

let spec = TreeSpec::from_json_str(
    r#"{
        "root": {
            "type": "reactive_selector",
            "children": [
                {
                    "type": "sequence",
                    "children": [
                        {"type": "condition", "name": "enemy_near"},
                        {"type": "action", "name": "attack"}
                    ]
                },
                {"type": "action", "name": "attack"}
            ]
        }
    }"#,
)?;
let mut tree = spec.build(&registry)?;
```

### Registry methods

| Method | Registers |
|---|---|
| `action::<P>(name, factory)` | action; factory receives typed params per node |
| `condition::<P>(name, factory)` | condition leaf and predicate of the same name |
| `predicate::<P>(name, factory)` | predicate only, for guard/while/if_then_else |
| `score::<P>(name, factory)` | utility score |
| `try_action::<P, E>(...)` | fallible action; `Err` -> trace error + Failure |
| `async_action::<P>(...)` | async action returning a task |
| `custom::<P>(kind, factory)` | arbitrary node factory with typed params and children |
| `subtree(name, factory)` | code-built `SubtreeFactory` template for `subtree` nodes |
| `subtree_with_params::<P>(name, factory)` | same, with `"params"` deserialized into `P` per node |
| `switch_key(key)` / `set_value_key(key)` / `event_key(key)` | typed keys referenced by config |
| `contract_key(key)` / `scoped_key(key)` | typed keys for `contract` / `scoped` boundaries; scoped values must be `Clone` |

Factory shape: `Fn(P) -> TickClosure`. The outer factory runs once per node, so
closure state is isolated between instances. When `params` is omitted it is
deserialized from `null`; use `P = ()` for parameterless registrations.

### Supported config nodes

Composites: `sequence`, `reactive_sequence`, `selector`, `reactive_selector`,
`random_selector`, `weighted_random_selector`, `parallel`, `mem_parallel`,
`race`, `switch`, `utility_selector`, `if_then_else`.

Decorators: `inverter`, `always_succeed`, `always_fail`, `repeat`, `retry`,
`until_success`, `until_failure`, `probability`, `while`, `guard`, `cooldown`,
`timeout`, `limiter`, `throttle`.

Leaves: `wait`, `wait_until`, `wait_for_event`, `set_value`, `action`,
`condition`, `try_action`, `async_action`.

Structure: `include`, `subtree`, `custom`, `identified`, `scoped`, `contract`.

Examples:

```json
{
  "version": 1,
  "name": "boss",
  "root": {"type": "identified", "id": "root", "child": {
    "type": "switch",
    "key": "boss.phase",
    "values": [1, 2],
    "children": [
      {"type": "action", "name": "attack"},
      {"type": "action", "name": "enrage"}
    ]
  }},
  "services": [
    {"type": "action", "name": "scan"}
  ]
}
```

```json
{
  "type": "parallel",
  "success": "all",
  "failure": "any",
  "children": [
    {"type": "action", "name": "move"},
    {"type": "action", "name": "shoot"}
  ]
}
```

```json
{
  "type": "if_then_else",
  "condition": "low_hp",
  "then": {"type": "action", "name": "heal"},
  "else": {"type": "include", "path": "combat.json"}
}
```

```json
{
  "type": "scoped",
  "name": "sandbox",
  "inputs": ["combat.target"],
  "outputs": ["combat.plan"],
  "child": {"type": "action", "name": "plan"}
}
```

```json
{
  "type": "contract",
  "required": ["combat.target"],
  "provided": ["combat.plan"],
  "child": {"type": "action", "name": "plan"}
}
```

```json
{
  "type": "subtree",
  "template": "goblin_patrol",
  "name": "patrol",
  "params": {"speed": 3}
}
```

Top-level `version` defaults to `1`; unsupported versions are rejected.
Composites, decorators, and structural nodes accept an optional display `name`;
leaf names come from their registry entry. `identified` adds a stable `id` used
by snapshots and diffs. Unknown fields are rejected.

### Includes

```rust
use playact::config::FsIncludeResolver;

let resolver = FsIncludeResolver::json("config/")?;
let tree = spec.build_with_resolver(&registry, &resolver)?;
```

`FsIncludeResolver` confines paths to its base directory and rejects `..`,
absolute paths, and symlink escapes. Depth and cycles are guarded. It also
records resolved files for hot reload; custom resolvers can implement
`watched_paths()` / `reset_watched_paths()`.

### Validation and schema

```rust
use playact::config::ValidationLimits;

spec.validate(&ValidationLimits::default())?; // depth 64, nodes 10_000
let schema = TreeSpec::json_schema();         // requires `schema`
let schema = registry.json_schema();          // adds the names this registry accepts
```

`NodeRegistry::json_schema()` starts from `TreeSpec::json_schema()` and turns the
action, condition, predicate, score, key, scoped/contract key, custom, and
subtree-template fields into `enum` constraints. Editors then complete only the
names that this registry can actually build, which catches typos before the
configuration is loaded.

Descriptions and parameter schemas are opt-in:

```rust
use playact::config::NodeCategory;

registry.describe(NodeCategory::Action, "patrol", "Walk the patrol route.")?;
registry.describe_params::<PatrolParams>(
    NodeCategory::Action,
    "patrol",
    "Walk the patrol route.",
)?;
```

`describe` adds hover text. `describe_params::<P>` also derives a schema from
`P` (via `playact::schemars::JsonSchema`) and attaches it under `params`,
including field descriptions and validation keywords. Recursive parameter types
are supported. Key fields are covered as well:

```rust
registry.describe(NodeCategory::EventKey, "demo.alarm", "Alarm signal.")?;
registry.describe_switch_key::<String>(MODE, "Current game mode.")?;
registry.describe_set_value_key::<u32>(MORALE, "Written morale value.")?;
```

`describe_switch_key` types `switch.values` items and `describe_set_value_key`
types `set_value.value`. See the
[configuration reference](config.md#editor-completion) for a complete editor
setup.

### Hot reload

```rust
use playact::hot_reload::TreeWatcher;

let mut watcher = TreeWatcher::<World>::json_with_resolver(
    "config/root.json",
    registry,
    FsIncludeResolver::json("config/")?,
)?
.with_diff_history(16)
.with_state_preservation(true);

if watcher.poll()? {
    if let Some(entry) = watcher.diff_history().latest_entry() {
        println!("revision {}: {}", entry.revision, entry.diff);
    }
}
watcher.tree_mut().tick(&mut world, time);
```

- Blackboard data is preserved across reloads; node state resets by default.
- `with_state_preservation(true)` restores own state by `NodeId`.
- `last_diff()`, `last_revision()`, `last_timestamp()`, `diff_history()` expose
  changes. `last_restored()` reports how many nodes loaded own state in the latest
  successful reload (`None` before the first one, `Some(0)` when none matched).
  `last_restore_error()` reports a rejected restore; the new tree is reset.
  `last_reload()` bundles the diff, revision, timestamp, restored count, and
  restore error into one `ReloadReport`.
- Include files reported by the resolver are polled and trigger reloads too.
- Parse/build failures keep the old tree and return an error.

### Remote configuration

```rust
use playact::remote_config::RemoteTreeLoader;

let mut loader = RemoteTreeLoader::<World>::json(url, registry)?
    .with_refresh_interval(Duration::from_secs(30))
    .with_diff_history(16);

loader.poll()?;                 // conditional request; false if not modified
loader.tree_mut().tick(&mut world, time);
```

`RemoteTreeLoader` supports ETag / Last-Modified, bounded response size, UTF-8
validation, includes, and an optional `ResponseVerifier` for signatures or
allowlists. `last_reload()` returns the same `ReloadReport` as hot reload;
`last_restored()`, `last_restore_error()`, `last_revision()`, and
`last_timestamp()` expose refresh metadata individually. Requests are
synchronous on the calling thread.

## 14. Proc-macro DSL

Enable `macros`:

```rust
let tree = playact::bt!(World, "player", reactive_selector {
    sequence {
        condition("low_hp", |ctx: &Context<'_, World>| ctx.world_ref().hp < 0.3),
        action("heal", |_| Status::Success),
    },
    action("patrol", |_| Status::Running),
})?;
```

The macro covers composites, decorators, leaves, and `raw(expression)`, and
still goes through the typestate builder.

`#[derive(BtNode)]` generates the Node boilerplate for a custom struct:

```rust
#[derive(playact::BtNode)]
#[bt(world = World, name = "scan")]
struct Scan {
    #[bt(id)]
    id: playact::NodeId,
    #[bt(children)]
    children: Vec<playact::BoxedNode<World>>,
}

impl Scan {
    fn tick(&mut self, ctx: &mut playact::Context<'_, World>) -> playact::Status {
        playact::Status::Success
    }
}
```

## 15. Scenario recipes

### A. Interruptible combat AI

Use `ReactiveSelector` at the top so low HP and close enemies preempt lower
priorities every frame. Put conditions inside small `Sequence` nodes. Add
`Cooldown` around attacks and `Guard` for prerequisites.

See [`examples/battle_ai.rs`](../examples/battle_ai.rs).

### B. Configuration-driven AI with hot reload and audit

1. Register actions/conditions/scores in `NodeRegistry`.
2. Parse `TreeSpec`, build with `FsIncludeResolver`.
3. Wrap in `TreeWatcher` with `with_diff_history(n)` and
   `with_state_preservation(true)`.
4. Serve `InspectorServer::poll_with_history`.
5. Poll `/status.json` and `/diffs.json?since=<cursor>` from your tooling.

See [`examples/reload_audit.rs`](../examples/reload_audit.rs).

### C. Utility-based tactics

Score each branch every tick (`UtilitySelector`) instead of hard-coding
priority. Use hysteresis to avoid flapping between close scores. Scores are
plain `Fn(&Context<W>) -> f32`; NaN is treated as lowest.

### D. Multi-agent shared blackboard

Use `SharedBlackboard` to share typed state between trees. Each `tick` locks the
board for the duration of that tree's tick; `try_lock` exposes contention.

```rust
let shared = SharedBlackboard::new();
shared.tick(&mut goblin_a, &mut world, time);
shared.tick(&mut goblin_b, &mut world, time);
```

### E. Remote behavior updates

Use `RemoteTreeLoader` with ETag caching and a verifier. Keep the old tree on
refresh errors; diff history tells you what changed on success. Persist
blackboard/node state separately if a restart must resume.

### F. Deterministic regression tests

Use `TestDriver` for virtual time, `StatsTracer` for counters,
`TraceRecorder::diff` for node-level behavior comparison, and `NodeIdSnapshot`
for state restoration. Fixed seeds on random/probability nodes make runs
reproducible.

### G. Save and resume

With `snapshot-serde` and `blackboard-serde`, persist `TreeSnapshot::to_json()`
and `BlackboardCodec::encode()` to files. On startup, rebuild the same tree,
`restore()` the snapshot, `decode()` the blackboard, and resume ticking. World
state is application-specific and must be saved separately. See
[`examples/save_game.rs`](../examples/save_game.rs).

## 16. Module map

| Module | Main types |
|---|---|
| `playact` root | `Status`, `Node`, `BoxedNode`, `Context`, `Time` |
| `blackboard` | `Blackboard`, `Key` |
| `tree` | `BehaviorTree`, `TreeDescription`, `NodePath` |
| `builder` | `BehaviorTreeBuilder`, `BranchBuilder` |
| `composite` | `Sequence`, `ReactiveSequence`, `Selector`, `ReactiveSelector`, `RandomSelector`, `WeightedRandomSelector`, `UtilitySelector`, `Parallel`, `MemParallel`, `Race`, `IfThenElse`, `Switch`, `Threshold` |
| `decorator` | `Inverter`, `AlwaysSucceed`, `AlwaysFail`, `Repeat`, `Retry`, `UntilSuccess`, `UntilFailure`, `Probability`, `While`, `Guard`, `Cooldown`, `Timeout`, `Limiter`, `Throttle` |
| `leaf` | `Action`, `Condition`, `Wait`, `WaitUntil`, `WaitForEvent`, `TryAction`, `SetValue` |
| `event` / `command` | `EventKey`, `EventQueue`, `CommandBuffer` |
| `async_action` / `shared` | `AsyncAction`, `ThreadTask`, `TokioTask`, `SharedBlackboard` |
| `subtree` / `contract` / `scope` | `SubtreeFactory`, `SubtreeSlot`, `SubtreeContract`, `ScopedSubtree` |
| `snapshot` / `persistence` / `replay` | `TreeSnapshot`, `NodeIdSnapshot`, `BlackboardCodec`, `TraceRecorder` |
| `trace` / `stats` / `diff` | `Tracer`, `TraceEvent`, `StatsTracer`, `TreeDiff`, `DiffHistory` |
| `reload` | `ReloadReport` |
| `inspector` | `Inspector`, `InspectorServer` |
| `config` | `TreeSpec`, `NodeRegistry`, `FsIncludeResolver`, `ValidationLimits` |
| `hot_reload` / `remote_config` | `TreeWatcher`, `RemoteTreeLoader` |

For exhaustive signatures and error variants, use
[docs.rs/playact](https://docs.rs/playact).
