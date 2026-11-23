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

Detailed API guide: **[crates/playact/README.md](crates/playact/README.md)**.
中文详细文档：**[crates/playact/README.zh-CN.md](crates/playact/README.zh-CN.md)**。

## Highlights

- **One node contract.** `tick(&mut Context<W>) -> Status` for every node.
  Control nodes orchestrate; leaves perform work. Arbitrary nesting, replacement,
  and reuse come for free.
- **Type-safe state.** Typed `Key<T>` blackboard slots and build-time deserialized
  configuration parameters. No public `Any` or string lookup.
- **Compile-time tree shape.** The typestate builder rejects invalid root and
  decorator child counts before the program runs.
- **Predictable ticks.** No sleeping, no async runtime inside `tick`, no hidden
  threads. Long work returns `Running` and resumes next frame.
- **Interruptible.** Preempted `Running` subtrees receive `abort` for cleanup and
  then `reset`.
- **Optional integrations.** Config formats, hot reload, remote config, Tokio
  tasks, tracing, snapshots, shared blackboards, and a proc-macro DSL.
- **Portable core.** Default `std` build with zero third-party dependencies;
  `no_std + alloc` with default features disabled.

## Workspace layout

| Path | Purpose |
|---|---|
| [`crates/playact`](crates/playact) | The engine library: nodes, builder, blackboard, config, reload, async adapters, snapshots |
| [`crates/playact-macros`](crates/playact-macros) | `bt!` proc-macro DSL |
| [`crates/playact/examples`](crates/playact/examples) | Runnable battle AI examples |
| [`crates/playact/tests`](crates/playact/tests) | Public-API and deterministic stress tests |
| [`crates/playact/benches`](crates/playact/benches) | Dependency-free microbenchmarks |

## Installation

```toml
[dependencies]
playact = "0.1"
```

With optional integrations:

```toml
[dependencies]
playact = { version = "0.1", features = ["config", "hot-reload", "macros"] }
```

For a `no_std` core:

```toml
[dependencies]
playact = { version = "0.1", default-features = false }
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

## Feature overview

| Area | Included |
|---|---|
| Core | `Status`, `Node`, `NodeId`, `Context`, typed `Blackboard`, `Time`, `Kind` |
| Composites | `Sequence`, `ReactiveSequence`, `Selector`, `ReactiveSelector`, `RandomSelector`, `WeightedRandomSelector`, `UtilitySelector`, `Parallel`, `MemParallel`, `Race`, `IfThenElse` |
| Decorators | `Inverter`, `AlwaysSucceed`, `AlwaysFail`, `Repeat`, `Retry`, `UntilSuccess`, `UntilFailure`, `Cooldown`, `Timeout`, `Limiter`, `Throttle`, `Guard` |
| Leaves | `Action`, `Condition`, `Wait`, `WaitUntil`, `WaitForEvent`, `SetValue`, `TryAction`, `AsyncAction` |
| Composition | Typestate builder, `SubtreeFactory`, `SubtreeSlot`, nested `BehaviorTree`, services |
| Observability | `Tracer`, `MultiTracer`, `StatsTracer`, error events, `last_status`, ASCII/DOT `describe` |
| Persistence | Versioned node-state snapshots plus opt-in typed blackboard serialization |
| Shared state | `SharedBlackboard` across trees and threads |
| Configuration | `TreeSpec` + `NodeRegistry`, JSON/TOML/YAML/RON, typed params, includes, JSON Schema, validation, hot reload, remote loading |
| Async | Runtime-agnostic `AsyncTask`, `ThreadTask`, optional `TokioTask` |
| Portability | `no_std + alloc` core; optional integrations gated by features |
| Developer experience | `bt!` DSL, two complete battle AI examples, benchmarks, deterministic stress tests |

Detailed semantics, caveats, and API examples live in the
[crate README](crates/playact/README.md).

## Examples

```text
cargo run -p playact --example battle_ai
cargo run -p playact --example battle_ai_config --features config
```

- `battle_ai`: deterministic 1D duel between a player and three goblins. Demonstrates
  ReactiveSelector interrupts, Cooldown, Timeout, Inverter, Tracer, abort cleanup,
  and CommandBuffer/Executor separation.
- `battle_ai_config`: the same ideas driven by JSON configuration, typed parameters,
  and custom node factories.

## Feature flags

| Feature | Enables |
|---|---|
| default | `std`; pure standard library, zero third-party dependencies |
| `std` | Standard-library integrations; disabling it switches the core to `no_std + alloc` |
| `config` | `TreeSpec`, `NodeRegistry`, JSON parsing, typed parameters, includes (requires `std`) |
| `schema` | JSON Schema generation for `TreeSpec` (requires `config`) |
| `blackboard-serde` | Opt-in typed blackboard serialization (requires `std`) |
| `test-util` | Deterministic `TestDriver` with virtual time (requires `std`) |
| `toml` | TOML parser |
| `yaml` | YAML parser (`serde_yaml_ng`) |
| `ron` | RON parser |
| `tracing` | `TracingTracer` |
| `hot-reload` | `TreeWatcher` with file watching |
| `remote-config` | `RemoteTreeLoader` over HTTP |
| `tokio` | `TokioTask` adapter |
| `macros` | `playact-macros` `bt!` DSL |
| `full` | All of the above |

```text
cargo check -p playact --no-default-features --target thumbv7em-none-eabihf --lib
```

## Development

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo bench -p playact
cargo doc --workspace --all-features --no-deps
cargo check -p playact --no-default-features --target thumbv7em-none-eabihf --lib
```

The test suite covers every node, builder, config layer, reload path, remote loader,
snapshot, async adapter, public-API integration tests, doctests, compile-fail checks,
and deterministic randomized stress tests. Benchmarks are dependency-free and cover
tick paths, blackboard access, construction, and `describe`. CI runs stable and
MSRV 1.92 on Linux and Windows, plus an embedded `no_std` check.

## Documentation

- [English API guide](crates/playact/README.md)
- [Chinese API guide](crates/playact/README.zh-CN.md)
- [API reference on docs.rs](https://docs.rs/playact)
