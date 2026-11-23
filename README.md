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
  tasks, tracing, snapshots, shared blackboards, an HTML inspector, and a
  proc-macro DSL.
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
| Composites | `Sequence`, `ReactiveSequence`, `Selector`, `ReactiveSelector`, `RandomSelector`, `WeightedRandomSelector`, `UtilitySelector`, `Parallel`, `MemParallel`, `Race`, `IfThenElse`, `Switch` |
| Decorators | `Inverter`, `AlwaysSucceed`, `AlwaysFail`, `Repeat`, `Retry`, `UntilSuccess`, `UntilFailure`, `While`, `Probability`, `Cooldown`, `Timeout`, `Limiter`, `Throttle`, `Guard` |
| Leaves | `Action`, `Condition`, `Wait`, `WaitUntil`, `WaitForEvent`, `SetValue`, `TryAction`, `AsyncAction` |
| Composition | Typestate builder, `SubtreeFactory`, `SubtreeSlot`, `SubtreeContract`, `ScopedSubtree`, nested `BehaviorTree`, services |
| Observability | `Tracer`, `MultiTracer`, `StatsTracer`, Prometheus text export, live status tree, error events, `last_status`, ASCII/DOT/Mermaid `describe`, structural `TreeDiff`, HTML inspector |
| Persistence | Versioned structural and ID-keyed node snapshots plus opt-in typed blackboard serialization |
| Shared state | `SharedBlackboard` across trees and threads |
| Configuration | `TreeSpec` + `NodeRegistry`, JSON/TOML/YAML/RON, typed params, includes, application-supplied and declarative version migrations, registry-aware JSON Schema with descriptions, params, and key values, Markdown node catalogs, validation, hot reload, remote loading |
| Async | Runtime-agnostic `AsyncTask`, `ThreadTask`, optional `TokioTask` |
| Portability | `no_std + alloc` core; optional integrations gated by features |
| Developer experience | `bt!` DSL, `BtNode` derive, two complete battle AI examples, benchmarks, deterministic stress tests |

Detailed semantics, caveats, and API examples live in the
[crate README](crates/playact/README.md). For method-level usage, tables, and
scenario recipes, see the [API guide](crates/playact/docs/api.md)
([中文](crates/playact/docs/api.zh-CN.md)). For every configuration node and its
fields, see the [configuration reference](crates/playact/docs/config.md)
([中文](crates/playact/docs/config.zh-CN.md)).

## Examples

```text
cargo run -p playact --example battle_ai
cargo run -p playact --example battle_ai_config --features config
cargo run -p playact --example battle_ai_showcase --features hot-reload
cargo run -p playact --example live_inspector --features inspector
cargo run -p playact --example reload_audit --features hot-reload,inspector,diff-serde
cargo run -p playact --example save_game --features blackboard-serde,snapshot-serde
cargo run -p playact --example schema_export --features config,schema
cargo run -p playact --example config_migration --features config
cargo run -p playact --example config_lint --features config,schema,toml,yaml,ron -- crates/playact/examples/battle_ai.json
```

- `battle_ai`: deterministic 1D duel between a player and three goblins. Demonstrates
  ReactiveSelector interrupts, Cooldown, Timeout, Inverter, Tracer, abort cleanup,
  and CommandBuffer/Executor separation.
- `battle_ai_config`: the same ideas driven by JSON configuration, typed parameters,
  and custom node factories.
- `battle_ai_showcase`: fully config-driven AI using services, `utility_selector`,
  `guard`, `try_action`, events, config-level `scoped`/`contract` boundaries,
  state-preserving hot reload, and structural reload diffs.
- `live_inspector`: a small demo tree served at `http://127.0.0.1:7878/` with live
  statuses, timings, `/metrics`, and `/tree` endpoints.
- `reload_audit`: nested include hot reload with state preservation, revision and
  timestamp history, `/status.json`, and incremental `/diffs.json?since=`. Set
  `PLAYACT_AUDIT_SERVE=1` to keep the inspector running while you edit the config.
- `save_game`: persists a stateful tree and typed blackboard to JSON, rebuilds the
  tree, restores both, and verifies that progress resumes without re-running a
  completed step.
- `schema_export`: writes a registry-aware JSON Schema for editor completion of
  action, condition, predicate, key, custom, and subtree names, with optional
  descriptions, derived parameter schemas, typed key values, and a Markdown
  node catalog.
- `config_migration`: registers a v0 -> v1 migration, upgrades a root document
  and its include, builds the migrated tree, and ticks it through completion.
- `config_lint`: validates JSON, TOML, YAML, or RON configurations against the
  registry schema and then builds them, reporting field paths, accepting
  multiple files, and emitting a JSON report with `--json`. Exits nonzero when
  any file is invalid.

## CLI

The workspace includes `playact-cli` (binary `playact`, not published) for
configuration tooling:

```text
cargo run -p playact-cli -- schema > schema.json
cargo run -p playact-cli -- catalog --schema schema.json
cargo run -p playact-cli -- lint --schema schema.json --json config/*.toml
cargo run -p playact-cli -- lint --watch config/*.toml
cargo run -p playact-cli -- convert config/a.json config/b.yaml
cargo run -p playact-cli -- convert --recursive --format yaml config/root.json dist/config
cargo run -p playact-cli -- bundle config/root.json dist/config
cargo run -p playact-cli -- graph --output graph.mmd config/root.json
cargo run -p playact-cli -- graph --format dot config/root.json
cargo run -p playact-cli -- fmt config/*.toml
cargo run -p playact-cli -- fmt --check config/*.toml
cargo run -p playact-cli -- fmt --recursive config/root.json
cargo run -p playact-cli -- fmt --check --recursive config/root.json
cargo run -p playact-cli -- check --schema schema.json config/root.json
cargo run -p playact-cli -- check --json config/root.json
cargo run -p playact-cli -- migrate --manifest migrations.json config/root.json
cargo run -p playact-cli -- migrate --output dist/config --manifest migrations.json config/root.json
cargo run -p playact-cli -- migrate --check --diff --patch --manifest migrations.json config/root.json
cargo run -p playact-cli -- diff config/old.json config/new.json
cargo run -p playact-cli -- diff --json config/old.toml config/new.toml
```

`schema` writes the base `TreeSpec` schema. Pass a registry-aware schema
generated by `NodeRegistry::json_schema()` with `--schema` to include registered
names, descriptions, and parameter schemas; `catalog`, `lint`, `convert`,
`bundle`, `fmt`, `check`, `migrate`, and `diff` accept it. `lint` supports JSON,
TOML, YAML, and RON, recursively resolves and lints `include` files (against each
root's parent directory or `--include-base`), accepts multiple files, can watch
for changes, and exits nonzero when any file is invalid. `convert` validates the input and its includes
before writing the target format; TOML output omits null optional fields because
TOML has no null representation. Without `--recursive`, include paths are
preserved, not copied, so keep the output within the same include base or adjust
paths afterwards. `bundle` validates the root and its include graph, then copies
the root and every included file into an empty output directory without
rewriting include strings. The output directory must be outside the include
base; load the bundled root with that directory as the include base.
`convert --recursive --format FORMAT` writes into a new or empty output
directory, converts every file in the include graph, and rewrites include paths
to the new extensions; load the converted root with that directory as the
include base. `graph` renders node structure and include relationships as
Mermaid (default) or DOT; missing or invalid includes appear as error nodes.
`fmt` validates each input and rewrites it in canonical form; included files are
validated but not rewritten unless passed explicitly or `--recursive` is set,
which formats the entire include graph. Every target is validated and formatted
in memory before the first write, and a write failure restores files already
written. `fmt --check` writes nothing and exits nonzero when files would change.
`check` combines include-aware validation with a non-writing formatting check:
every lint-valid root and included file must already be canonical. It exits
nonzero on lint errors, formatting errors, or files that would change; `--json`
reports root lint findings plus per-file formatting status. `migrate` applies a
versioned declarative manifest of field renames, value mappings, removals,
defaults, and numeric scaling to each configuration and its include graph,
validates the migrated result, then writes changed files in place or stages the
complete graph with `--output DIR`. `--check` reports changes without writing,
`--diff` prints field-level changes, `--patch` prints unified diffs, and `--json`
reports per-root results including changes. Declarative rules cannot express
arbitrary Rust migrations; use `ConfigMigrator` in the application for those. `diff` validates two configurations, then reports path-based node,
parameter, and include-file set changes; child nodes are matched by type and
name/kind/key, so insertions and reorders report moves rather than changing every
later index. Included files with matching relative paths are compared
recursively, and their findings carry a `file` field. It exits nonzero when
differences exist and supports `--json` for review tooling.

## Feature flags

| Feature | Enables |
|---|---|
| default | `std`; pure standard library, zero third-party dependencies |
| `std` | Standard-library integrations; disabling it switches the core to `no_std + alloc` |
| `config` | `TreeSpec`, `NodeRegistry`, JSON parsing, typed parameters, includes, version migrations (requires `std`) |
| `schema` | JSON Schema generation for `TreeSpec` and a populated `NodeRegistry`, with optional descriptions, params schemas, key value schemas, and Markdown node catalogs (requires `config`) |
| `blackboard-serde` | Opt-in typed blackboard serialization (requires `std`) |
| `snapshot-serde` | `TreeSnapshot` / `NodeIdSnapshot` JSON serialization (requires `std`) |
| `trace-record` | Owned trace recording and replay comparison (requires `std`) |
| `trace-serde` | `TraceRecorder` JSON export/import (requires `trace-record`) |
| `test-util` | Deterministic `TestDriver` with virtual time (requires `std` and `trace-record`) |
| `inspector` | Self-contained HTML report and zero-dependency debug HTTP server (requires `std`) |
| `diff-serde` | `TreeDiff` and `DiffHistory` JSON serialization and parsing (requires `std`) |
| `toml` | TOML parser |
| `yaml` | YAML parser (`serde_yaml_ng`) |
| `ron` | RON parser |
| `tracing` | `TracingTracer` |
| `hot-reload` | `TreeWatcher` with file watching |
| `remote-config` | `RemoteTreeLoader` over HTTP |
| `tokio` | `TokioTask` adapter |
| `macros` | `playact-macros` `bt!` DSL and `BtNode` derive |
| `full` | All of the above |

```text
cargo check -p playact --no-default-features --target thumbv7em-none-eabihf --lib
```

## Development

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --all-features --examples
cargo bench -p playact
cargo doc --workspace --all-features --no-deps
cargo check -p playact --no-default-features --target thumbv7em-none-eabihf --lib
```

The test suite covers every node, builder, config layer, reload path, remote loader,
snapshot, async adapter, public-API integration tests, doctests, compile-fail checks,
and deterministic randomized stress tests. Benchmarks are dependency-free and cover
tick paths, blackboard access, construction, and `describe`. A zero-dependency
test also verifies that local Markdown links in the README and `docs/` resolve.
CI runs stable and MSRV 1.92 on Linux and Windows, an embedded `no_std` check,
and warning-free builds for individual feature combinations.

## Documentation

- [English API guide](crates/playact/README.md)
- [Chinese API guide](crates/playact/README.zh-CN.md)
- [API reference on docs.rs](https://docs.rs/playact)
