# playact

**A type-safe, extensible behavior-tree engine for game automation.**

English | [简体中文](README.zh-CN.md) · [API guide](docs/api.md) · [API 指南](docs/api.zh-CN.md) · [Config reference](docs/config.md)

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
  Tokio tasks, tracing, snapshots, an HTML inspector, and a proc-macro DSL are
  opt-in features.
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
- composites may be empty (empty `Sequence`/`Parallel` succeed, empty `Selector` fails);
- `build()` rejects duplicate stable `NodeId` values and reports the structural path
  of the second occurrence.

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
| `Switch` | Selects a child by a typed blackboard key; missing/unmatched keys return Failure |

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
- `Switch` is constructed from typed branches and attached with `.raw(Box::new(...))`:
  ```rust
  let switch = Switch::new("phase", &PHASE, vec![
      (1, phase_one),
      (2, phase_two),
  ]);
  ```
  Changing the key aborts a Running old branch before ticking the new one.

### Decorators

`Inverter`, `AlwaysSucceed`, `AlwaysFail`, `Repeat`, `Retry`, `UntilSuccess`,
`UntilFailure`, `While`, `Probability`, `Cooldown`, `Timeout`, `Limiter`, `Throttle`,
`Guard`.

- `Cooldown` starts only after the child succeeds.
- `Timeout` times only Running children; `duration = 0` fails on the first tick without
  ticking the child.
- `Retry`, `UntilSuccess`, and `UntilFailure` make at most one attempt per tick.
- `Repeat` advances at most `MAX_REPEAT_STEPS_PER_TICK` times per tick.
- `Limiter` is a per-activation quota; it resets on global reset and on `abort → reset`.
- `Throttle` limits terminal results per time window; `Running` does not consume quota.
- `Guard` fails immediately when its condition is false and aborts a previously Running
  child. It is the direct form of a reactive `condition → child` sequence.
- `While` performs at most one child step per tick; it returns `Running` after a
  successful step and `Success` once its condition is false (aborting a still-Running
  child first).
- `Probability` rolls once per activation with a deterministic seed; a `Running` child
  keeps the same decision, and the next activation rolls again.

### Leaves

`Action`, `Condition`, `Wait`, `WaitUntil`, `WaitForEvent`, `SetValue`, `TryAction`,
`AsyncAction`.

```rust
let action = Action::new("hold_key", |_| Status::Running)
    .with_abort(|_ctx| { /* release key */ })
    .with_reset(|| { /* clear internal state */ });
```

- `WaitUntil` returns `Running` until a predicate becomes true.
- `WaitForEvent` consumes typed events from an `EventQueue`, created with an `EventKey`.
- `TryAction` returns `Result<Status, E>`; on error it reports an `Error` trace event and
  returns `Failure`.

```rust
static DAMAGE: EventKey<u32> = playact::event_key!("damage");

let tree = BehaviorTreeBuilder::<World>::new("react")
    .sequence(|b| {
        b.action("emit", |ctx| {
            DAMAGE.push(ctx, 10);
            Status::Success
        })
        .wait_for_event(DAMAGE)
    })
    .build()?;

let fallible = BehaviorTreeBuilder::<World>::new("fallible")
    .try_action("fetch", |_ctx| {
        Err::<Status, &str>("network unavailable")
    })
    .build()?;
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

### Subtree contracts

`SubtreeContract` declares required input keys and output keys for a subtree:

```rust
static TARGET: Key<u32> = playact::key!("target");
static RESULT: Key<u32> = playact::key!("result");

let contract = SubtreeContract::new("scan", subtree_root)
    .require(&TARGET)
    .provide(&RESULT);

let tree = BehaviorTreeBuilder::<World>::new("ai")
    .raw(Box::new(contract))
    .build()?;
```

- Missing required keys report an error through the tracer and return `Failure`.
- Declared outputs are cleared when a new activation starts.
- Aborting the contract aborts a Running child and clears outputs.
- Outputs remain after a terminal result so the parent can read them.
- It is a validation/lifecycle boundary, not a separate blackboard namespace.

### Scoped blackboards

`ScopedSubtree` runs a child against a private `Blackboard`:

```rust
static PLAN: Key<u32> = playact::key!("plan");
static RESULT: Key<u32> = playact::key!("result");

let tree = BehaviorTreeBuilder::<World>::new("ai")
    .scoped("planning", [&PLAN], [&RESULT], |b| {
        b.sequence(|b| {
            b.action("prepare", |ctx| {
                let plan = ctx.blackboard_ref().get(&PLAN).copied().unwrap_or(0);
                ctx.blackboard().set(&RESULT, plan + 1);
                Status::Success
            })
            .action("finish", |_| Status::Success)
        })
    })
    .build()?;
```

- On activation, declared inputs are copied into a fresh scoped board and
  declared outputs are removed from the parent.
- When the child reaches a terminal status, declared outputs are copied back.
- Undeclared keys written inside the subtree never reach the parent.
- Inputs are snapshotted at activation; change them in the parent before the
  next activation to affect the child.
- The scoped child runs without a Tracer, so internal `Enter`/`Exit` events are
  not emitted; the `ScopedSubtree` node itself is still visible in the parent
  trace.
- Snapshots restore child node state, but the scoped board is runtime-only;
  inputs are copied again on the next tick after a restore.

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

- `Tracer` receives `Enter` / `Exit` / `Abort` / `Error` events with a child-index path
  and the node's optional stable `NodeId`.
- `MultiTracer` fans events out to multiple tracers.
- `StatsTracer` tracks per-path tick counts, status counts, aborts, reported errors,
  last status, and wall-clock time (requires `std`); query a specific node with
  `node_by_id`, or export scrapable text with `render_prometheus()`.
- `last_status()` and `is_running()` expose the last top-level result.
- `describe()` renders an ASCII tree, Graphviz DOT, or Mermaid `graph TD`;
  builder/app-config IDs appear in `TreeDescription::id` and diagram labels.
- `StatsTracer::render_tree(&tree)` renders an ASCII tree with each node's last status.

### HTML inspector

Enable the `inspector` feature to render a self-contained HTML report and serve it
over a local HTTP endpoint:

```rust
use playact::inspector::{Inspector, InspectorServer};

// Static report (add .with_diff(&diff) or .with_diff_history(&history) after a reload)
Inspector::new(&tree)
    .with_stats(&stats)
    .write_to("tree.html")?;

// Live report: poll once per frame, then open http://127.0.0.1:7878/
let mut server = InspectorServer::bind("127.0.0.1:7878")?;
server.poll(&tree, Some(&stats))?;
// After a reload, use server.poll_with_diff(&tree, Some(&stats), watcher.last_diff())?;
// To show every kept diff, use server.poll_with_history(&tree, Some(&stats), watcher.diff_history())?;
// To include restore audit metadata:
// server.poll_with_report(&tree, Some(&stats), watcher.diff_history(), watcher.last_reload())?;
```

The page shows the tree with last statuses, per-node counters and timings, services,
a `NodeId` audit table with structural paths, an optional reload diff, and Mermaid
source, using inline CSS only. With `poll_with_history` / `with_diff_history`, the
report renders every stored diff as a collapsible `Reload history` list with the
newest entry expanded; entries are labeled by a monotonic revision that is never
reused, plus their UTC timestamp when one was recorded. `poll_with_report` /
`with_reload_report` add a `Restored` card and a state-restore error panel.
`/metrics`, `/tree`, and `/health` serve Prometheus text, the ASCII status tree,
and a readiness probe; `/status.json` returns a machine-readable summary with the
root status, node and statistics counts, history length, the oldest and latest
revisions, the latest timestamp, and restore metadata. With
`diff-serde`, `/diff.json` returns the latest diff and `/diffs.json` the whole
`DiffHistory` as an array of `{"revision", "diff"}` entries (oldest first); append
`?since=N` to fetch only revisions after `N`. The server has no authentication and
exposes tree metadata; bind it to loopback and use it only in debug builds.

### Tree diffs

`TreeDiff::between(&old, &new)` compares nodes by stable `NodeId`:

```rust
let diff = TreeDiff::between(&old_tree, &new_tree);
println!("{diff}");
```

- `added` / `removed` report `NodeId`, name, kind, and `NodePath`
- `moved` reports the old and new structural paths
- `renamed` reports old/new name and kind; a node that also moved appears in both
  `moved` and `renamed`
- the main tree and configured services share one ID namespace
- nodes without IDs are traversed for path accuracy but not reported

Enable `diff-serde` for compact JSON output and parsing:

```rust
let json = diff.to_json();
let parsed = TreeDiff::from_json(&json)?;
```

The same feature supports `DiffHistory::to_json()` / `DiffHistory::from_json()`.
Parsed history entries keep their revisions, and the next push continues after the
largest restored revision, so logs from an earlier process stay comparable. Custom
node kinds serialize to their name but cannot be deserialized: `Kind::Custom` stores a
`'static` name, so `TreeDiff::from_json` rejects payloads that contain one.

Hot reload and remote refresh expose their latest diff through `last_diff()` and keep a
bounded `DiffHistory` when configured with `with_diff_history(n)`. Every stored diff
gets a monotonic revision (starting at 1, never reused across eviction, `pop_latest`,
or `clear`) exposed by `latest_entry()` / `entries()`. `push_at(diff, unix_ms)` also
records a wall-clock timestamp; `push_now` reads the system clock, and the hot-reload
and remote watchers use it automatically. The history serializes to a JSON array of
`{"revision", "diff"}` entries (plus `"timestamp"` when set), oldest first; combine it
with `InspectorServer::poll_with_history` to serve `/diffs.json`. For incremental polling,
`entries_since(rev)` and `to_json_since(rev)` return only revisions above `rev`, and
`/diffs.json?since=rev` exposes the same filter over HTTP. `oldest_revision()` reports
the retention floor: if a client's cursor is below it, the limit already evicted entries
it has not seen. Watchers also expose `last_revision()` and `last_timestamp()` directly.

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

With `snapshot-serde`, both snapshot types serialize to compact JSON:

```rust
let json = snapshot.to_json();
let snapshot = TreeSnapshot::from_json(&json)?;

let id_json = id_snapshot.to_json(); // NodeIdSnapshot: IDs become object keys
```

Custom nodes can opt in with `Node::save_state` / `Node::load_state`. Composite custom
nodes must encode their children recursively.

> [!WARNING]
For trees whose structure may change, assign IDs and use the order-independent snapshot:

```rust
let snapshot = tree.snapshot_by_id()?; // NodeIdSnapshot
let mut rebuilt = build_reordered_tree();
rebuilt.restore_by_id(&snapshot)?;
```

Built-in stateful nodes implement `save_own_state` / `load_own_state` and
`children_mut`, so their own progress is restored by ID even if children are reordered.
Custom nodes must implement the same methods to participate. Dynamic `SubtreeSlot`
children and nested-tree services are not traversed by ID snapshots.

If the new tree may rename or remove IDs, use `restore_by_id_matching`: it applies
entries whose IDs exist, ignores the rest, and returns the number of nodes restored.
Builder and `TreeSpec` builds reject duplicate IDs automatically. Manually assembled
trees via `BehaviorTree::new` can be checked with `BehaviorTree::duplicate_node_id()`
or `duplicate_node_id_path()`, which returns a `NodePath` such as `root.children[1]`.

> Node snapshots do not include the world, blackboard values, closure captures,
> `AsyncAction` tasks, `SubtreeSlot` dynamic subtrees, or Tracer state. Persist those
> separately. Time-based nodes store absolute `Time.now` values, so restore on the same
> time axis or reset and rebuild.

The optional `blackboard-serde` feature adds `BlackboardCodec`, which serializes only
explicitly registered typed keys:

```rust
use playact::BlackboardCodec;

let mut codec = BlackboardCodec::new();
codec.register(&TARGET)?;
codec.register(&COMMANDS)?;

let node_state = tree.snapshot();
let board_state = codec.encode(tree.blackboard())?;
// later: tree.restore(&node_state)?; codec.decode(tree.blackboard_mut(), &board_state)?;
```

Unknown keys, duplicate registration, unsupported versions, and wrong value types are
reported as errors; the codec never performs string-keyed or `Any` lookups.

## Test driver

The optional `test-util` feature provides a deterministic driver:

```rust
use playact::TestDriver;

let mut driver = TestDriver::new(tree, world).with_step(Duration::from_millis(16));
assert_eq!(driver.tick(), Status::Running);
driver.advance(Duration::from_secs(1));
assert_eq!(driver.run_until_terminal(10), Some((1, Status::Success)));
```

`TestDriver` owns the tree and world, exposes the blackboard and last status, and never
reads the system clock or sleeps.

### Trace recording and replay comparison

The `trace-record` feature stores owned trace events and reports the first divergence
between two runs:

```rust
use playact::TraceRecorder;

let mut driver = TestDriver::new(tree, world);
let recorder = driver.install_trace_recorder();
let _ = driver.tick_n(100);
let expected = recorder.records();

// later, with the same deterministic world scenario:
let mut driver = TestDriver::new(build_tree(), same_world);
let recorder = driver.install_trace_recorder();
let _ = driver.tick_n(100);
if let Some(diff) = recorder.diff(&expected) {
    eprintln!("behavior diverged at event {}: {diff:?}", diff.index);
}
```

This records the tree's observable behavior, not the world. Reproduce the same world
inputs and use node snapshots / `BlackboardCodec` for stateful replay. Trace events are
owned, so they can be stored for regression tests.

With `trace-serde`, export and reload a baseline across processes:

```rust
let json = recorder.to_json();
let baseline = TraceRecorder::from_json(&json)?;
if let Some(diff) = recorder.diff(&baseline.records()) {
    eprintln!("diverged at {}", diff.index);
}
```

## Configuration

The optional `config` feature builds trees from data while keeping parameters typed.
See the [configuration reference](docs/config.md) for every node and field:

```rust
use playact::config::{NodeRegistry, TreeSpec};
use playact::Key;

static PHASE: Key<u32> = playact::key!("phase");

let mut registry = NodeRegistry::<World>::new();
registry.condition::<HpBelow>("hp_below", |params: HpBelow| {
    move |ctx: &Context<'_, World>| ctx.world_ref().hp_ratio() < params.ratio
})?;
registry.action::<()>("wait", |_params: ()| {
    move |_: &mut Context<'_, World>| Status::Running
})?;
registry.switch_key(PHASE)?;

let spec = TreeSpec::from_json_str(JSON)?;
let mut tree = spec.build(&registry)?;
```

- JSON, TOML, YAML, and RON parsers are available behind format features.
- Unknown fields are rejected; errors carry paths such as `root.children[0]`.
- Supported node types include sequences, selectors, random/weighted/utility selection,
  race, parallel, mem-parallel, `switch`, `if_then_else`, decorators (including
  `guard`, `while`, throttle, and probability), waits, `wait_until`,
  `wait_for_event`, `set_value`, actions, conditions, `try_action`, `async_action`,
  `custom` factories, and `scoped` / `contract` subtree boundaries.
- `subtree` configuration nodes reference a `SubtreeFactory` registered with
  `registry.subtree(name, factory)`, so complex code-built subtrees can be reused
  by name from JSON/TOML/YAML/RON. `registry.subtree_with_params::<P>(...)`
  deserializes the node's `params` into `P` before creating the template.
- `try_action` registrations return fallible tick closures; errors are reported
  through the tracer and become `Failure`. `async_action` registrations return
  `AsyncTask` starters for thread, Tokio, or custom remote tasks.
- `condition` registrations also provide predicates for `guard`, `while`,
  `if_then_else`, and `wait_until`; use `registry.predicate` for predicates that are
  not condition leaves. Predicate parameters are typed like action parameters.
- `switch`, `set_value`, and `wait_for_event` configurations reference typed keys
  registered with `registry.switch_key` / `set_value_key` / `event_key`; configured
  values deserialize into the key's value or event type at build time.
- `contract` and `scoped` configurations reference keys registered with
  `registry.contract_key` / `registry.scoped_key`. Contract keys require
  `Send + 'static`; scoped keys also require the value to be `Clone`. Key names in
  the config are resolved at build time, and unknown names report a path.
- `utility_selector` branches reference score functions registered with
  `registry.score`; scores receive typed parameters and are re-evaluated every tick.
- A duplicate `NodeId` across the built root and services is rejected at build time;
  the error includes the config path of the second occurrence.

`TreeSpec.services` declares nodes that run before the root on every tick:

```json
{
  "root": {"type": "sequence", "children": []},
  "services": [{"type": "action", "name": "sense_enemies"}]
}
```

Services share the world, blackboard, and time; their return values are ignored. They
are rebuilt with the tree on hot reload and remote refresh, and
`with_state_preservation` restores service state by ID when IDs match.
- `TreeSpec::validate(&ValidationLimits)` checks version, maximum depth, and node count
  before building. This is useful for remote or user-supplied configuration.
- The optional `schema` feature exposes `TreeSpec::json_schema()` for editor completion
  and external validation. `NodeRegistry::json_schema()` additionally injects the
  registered action, condition, predicate, score, key, custom, and subtree names as
  `enum` constraints, so completion matches what the application can build.
  `NodeRegistry::describe` adds hover descriptions, and
  `NodeRegistry::describe_params::<P>` adds a derived params schema with field
  descriptions and validation keywords.
- [`examples/schema_export.rs`](examples/schema_export.rs) writes that schema to
  `target/playact-schema.json`; the
  [configuration reference](docs/config.md#editor-completion) shows a VS Code mapping.
- Wrap a configuration node in `identified` to assign a stable `NodeId`:
  ```json
  {"type":"identified","id":"root","child":{"type":"sequence","children":[]}}
  ```
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
paths, and symlink escapes. Cycles and excessive nesting are detected during build, and
the resolver records every resolved file so hot reload can watch it. A closure
`Fn(&str) -> Result<TreeSpec, ConfigError>` also implements `IncludeResolver` for custom
file, HTTP, or in-memory sources; implement `watched_paths()` when those sources should
trigger reloads.

### Hot reload

```rust
use playact::hot_reload::TreeWatcher;

let mut watcher = TreeWatcher::<World>::json("ai.json", registry)?;
watcher.tree_mut().tick(&mut world, time);

if watcher.poll()? {
    println!("reloaded: {}", watcher.tree().describe());
    if let Some(diff) = watcher.last_diff() {
        println!("{diff}");
    }
}
```

A successful rebuild replaces the tree (root and configured services), preserves
blackboard data, and resets node state. Enable `with_state_preservation(true)` to also
restore own state by stable `NodeId` for nodes that still exist; renamed or removed IDs
start from their reset state. `last_restored()` reports how many nodes actually loaded
state in the most recent successful reload; `last_restore_error()` reports a rejected
restore (the new tree is reset in that case). `last_reload()` bundles the diff, revision,
timestamp, restored count, and restore error into one `ReloadReport`.
`last_diff()` reports added, removed, moved,
and renamed nodes by `NodeId` and `NodePath`; `with_diff_history(n)` keeps recent diffs
in `diff_history()`. Parse/build failures keep the old tree and return an error. Edits are
debounced (200 ms by default). Use `json_with_resolver` / `with_parser_and_resolver`
for includes.

The watcher polls both the root file and the include paths reported by the resolver, so
editing an included file on its own triggers a reload. `FsIncludeResolver` records every
file it resolves; custom resolvers can implement `watched_paths()` and
`reset_watched_paths()` to report their own local files. `include_paths()` exposes the
current watch list.

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
the old tree on refresh errors. A successful refresh rebuilds the root and configured
services, preserves the blackboard, and resets node state; `with_state_preservation(true)`
also restores node state by stable `NodeId`, matching the hot-reload behavior.
`last_diff()` exposes the same added/removed/moved/renamed `TreeDiff` as hot reload;
`last_restored()` reports how many nodes loaded state in the latest refresh;
`last_restore_error()` reports a rejected restore; `last_reload()` bundles the same
metadata as a `ReloadReport`; `with_diff_history(n)` keeps a bounded history. Requests
happen on the calling thread;
use a worker thread or a custom `Client` timeout in high-latency environments. Includes
are available through `json_with_resolver` / `with_client_and_resolver`.

Response bodies are limited to `MAX_REMOTE_CONFIG_BYTES` (1 MiB), must be UTF-8, and
are checked with `TreeSpec::validate` using `ValidationLimits::default()` before build.
Oversized or structurally excessive configurations are rejected.

Install a `ResponseVerifier` to enforce signatures, MACs, or allowlists before parsing:

```rust
struct SignatureVerifier;

impl playact::ResponseVerifier for SignatureVerifier {
    fn verify(
        &self,
        headers: &playact::remote_config::reqwest::header::HeaderMap,
        body: &[u8],
    ) -> Result<(), String> {
        // verify an Ed25519/HMAC signature from a header over `body`
        Ok(())
    }
}

let mut loader =
    RemoteTreeLoader::<World>::json_with_verifier(url, registry, SignatureVerifier)?;
```

The verifier runs on the initial request and every refresh; no unverified payload is
parsed.

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

### Deriving custom nodes

`#[derive(BtNode)]` generates the `Node` boilerplate for a custom struct. The struct
provides an inherent `tick` method:

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
        // ...
        playact::Status::Success
    }
}
```

- `world = Type` is required and may be a concrete type or a struct generic
- `name` / `kind` override the defaults (struct name)
- `#[bt(id)]` accepts `NodeId` or `Option<NodeId>`
- `#[bt(children)]` generates `children` / `children_mut`
- Struct flags `abort`, `reset`, `own_state`, `state` generate delegation to
  inherent methods of the same names

## no_std

Disable default features to build the core with `alloc`:

```toml
[dependencies]
playact = { version = "0.1", default-features = false }
```

Available in the core: nodes, `BehaviorTree` (including services and snapshots),
builder, `Blackboard` (backed by `BTreeMap`), composites, decorators, leaves,
`CommandBuffer`, `SubtreeFactory`, `SubtreeSlot`, `SubtreeContract`,
`ScopedSubtree`, `EventQueue` / `EventKey`, `WaitUntil` / `WaitForEvent` /
`TryAction`, `MultiTracer`, and `describe`.

Requires `std`: `SharedBlackboard`, `AsyncAction` / `ThreadTask` / `TokioTask`,
`StatsTracer`, configuration, blackboard serialization, hot reload, remote config, and
`TracingTracer`.

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
| `schema` | `TreeSpec::json_schema()` and `NodeRegistry::json_schema()` JSON Schema generation, plus `describe` / `describe_params` metadata (requires `config`) |
| `blackboard-serde` | `BlackboardCodec` typed blackboard serialization (requires `std`) |
| `snapshot-serde` | `TreeSnapshot` / `NodeIdSnapshot` JSON serialization (requires `std`) |
| `trace-record` | Owned `TraceRecord` / `TraceRecorder` and trace comparison (requires `std`) |
| `trace-serde` | `TraceRecorder` JSON export/import (requires `trace-record`) |
| `test-util` | `TestDriver` with virtual time and run-until helpers (requires `std` + `trace-record`) |
| `inspector` | HTML report with live statuses/timings and a zero-dependency debug HTTP server (requires `std`) |
| `diff-serde` | `TreeDiff::to_json()` serialization (requires `std`) |
| `toml` | TOML parser |
| `yaml` | YAML parser (`serde_yaml_ng`) |
| `ron` | RON parser |
| `tracing` | `TracingTracer` |
| `hot-reload` | `TreeWatcher` with `notify` file watching |
| `remote-config` | `RemoteTreeLoader` with `reqwest::blocking` |
| `tokio` | `TokioTask` adapter |
| `macros` | `playact-macros` `bt!` proc-macro DSL and `BtNode` derive |
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
cargo run -p playact --example battle_ai_showcase --features hot-reload
cargo run -p playact --example live_inspector --features inspector
cargo run -p playact --example reload_audit --features hot-reload,inspector,diff-serde
cargo run -p playact --example save_game --features blackboard-serde,snapshot-serde
cargo run -p playact --example schema_export --features config,schema
```

- `battle_ai`: deterministic 1D duel between a player and three goblins. Shows
  ReactiveSelector interrupts, Cooldown, Timeout, Inverter, Tracer, abort cleanup,
  and CommandBuffer/Executor separation.
- `battle_ai_config`: the same ideas driven by JSON configuration, typed parameters,
  and custom node factories.
- `battle_ai_showcase`: fully config-driven AI using services, `utility_selector`,
  `guard`, `try_action`, events, config-level `scoped`/`contract` boundaries,
  state-preserving hot reload, and structural reload diffs.
- `live_inspector`: a demo tree served at `http://127.0.0.1:7878/` with live
  statuses, timings, `/metrics`, and `/tree` endpoints.
- `reload_audit`: nested include hot reload with state preservation, revision and
  timestamp history, `/status.json`, and incremental `/diffs.json?since=`. Set
  `PLAYACT_AUDIT_SERVE=1` to keep the inspector running while you edit the config.
- `save_game`: persists a stateful tree and typed blackboard to JSON, rebuilds the
  tree, restores both, and verifies that progress resumes without re-running a
  completed step.
- `schema_export`: writes a registry-aware JSON Schema for editor completion of
  action, condition, predicate, key, custom, and subtree names, with optional
  descriptions and derived parameter schemas.

## Testing and benchmarks

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --all-features --examples
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
- `tests/docs_links.rs` verifies that relative Markdown links in the READMEs and
  `docs/` resolve, skipping external URLs and fenced code.
- `benches/engine.rs` is a dependency-free microbenchmark suite for tick paths,
  blackboard access, construction, and describe output.
- CI runs stable and MSRV 1.92 on Linux and Windows, an embedded no_std check,
  and warning-free builds for individual feature combinations.
