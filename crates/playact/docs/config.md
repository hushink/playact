# playact configuration reference

Reference for the optional `config` feature: `TreeSpec`, every `SpecNode` type,
its fields and defaults, the matching `NodeRegistry` registration, and include,
validation, and schema behavior. For task-oriented usage see the
[API guide](api.md); for the big picture see the [README](../README.md).

- JSON, TOML, YAML, and RON use the same structure; examples below are JSON.
- Unknown fields are rejected. Errors carry paths such as `root.children[0]`.
- Configuration contains no closures. Registered names and typed parameters are
  resolved and deserialized once at build time.

## Top-level `TreeSpec`

```json
{
  "version": 1,
  "name": "player ai",
  "root": {"type": "action", "name": "patrol"},
  "services": [
    {"type": "action", "name": "sense"}
  ]
}
```

| Field | Required | Default | Meaning |
|---|---|---|---|
| `version` | no | `1` | Spec version; unsupported values are rejected |
| `name` | no | `"BehaviorTree"` | Tree display name |
| `root` | yes | - | Root node |
| `services` | no | `[]` | Nodes that run before the root on every tick |

Services share the world, blackboard, time, and tracer. Their returned status is
recorded for tracing but does not replace the root status.

## Parsing and building

| Method | Feature | Purpose |
|---|---|---|
| `TreeSpec::from_json_str` / `from_json_slice` | `config` | JSON |
| `TreeSpec::from_toml_str` | `toml` | TOML |
| `TreeSpec::from_yaml_str` | `yaml` | YAML |
| `TreeSpec::from_ron_str` | `ron` | RON |
| `spec.build(&registry)` | `config` | build without includes |
| `spec.build_with_resolver(&registry, &resolver)` | `config` | build with `include` |
| `spec.validate(&limits)` | `config` | version/depth/node-count check without building |
| `TreeSpec::json_schema()` | `schema` | JSON Schema for editors |

```rust
use playact::config::{NodeRegistry, TreeSpec, ValidationLimits};

let mut registry = NodeRegistry::<World>::new();
registry.action::<()>("patrol", |_| {
    move |_: &mut Context<'_, World>| Status::Running
})?;

let spec = TreeSpec::from_json_str(json)?;
spec.validate(&ValidationLimits::default())?;
let mut tree = spec.build(&registry)?;
```

`ValidationLimits` defaults to `max_depth: 64` and `max_nodes: 10_000`; includes
count as one node and are not expanded by `validate`.

## Name, parameters, and IDs

- `name` on composites/decorators/structural nodes is an optional display name.
- Actions, conditions, fallible/async actions, and custom kinds use their
  registry name as the display name.
- `params` is optional for every registered factory. Omitted params deserialize
  from JSON `null`; parameterless registrations use `P = ()`. A parameter struct
  needs an explicit `"params": {...}` object (use `{}` when every field has a
  default).
- Wrap any node in `identified` to attach a stable `NodeId` for snapshots, diff,
  metrics, and state-preserving reloads. Duplicate IDs are rejected at build.

## Composites

| `type` | Required | Optional (default) | Semantics |
|---|---|---|---|
| `sequence` | - | `name`, `children` (`[]`) | memory sequence |
| `reactive_sequence` | - | `name`, `children` (`[]`) | re-evaluates from the first child each tick |
| `selector` | - | `name`, `children` (`[]`) | memory selector |
| `reactive_selector` | - | `name`, `children` (`[]`) | re-evaluates priorities and preempts |
| `random_selector` | `seed` | `name`, `children` (`[]`) | deterministic random choice with memory |
| `weighted_random_selector` | `seed` | `name`, `weights` (`[]`), `children` (`[]`) | weighted choice; weight `0` never selected |
| `parallel` | - | `name`, `success` (`all`), `failure` (`any`), `children` (`[]`) | single-tick threshold aggregation |
| `mem_parallel` | - | `name`, `success` (`all`), `failure` (`any`), `children` (`[]`) | accumulates terminal states across ticks |
| `race` | - | `name`, `children` (`[]`) | first terminal child wins |
| `switch` | `key` | `name`, `values` (`[]`), `children` (`[]`) | branch by a registered typed key |
| `utility_selector` | - | `name`, `hysteresis` (`0.0`), `branches` (`[]`) | scores branches every tick |
| `if_then_else` | `condition`, `then`, `else` | `name`, `params` (`null`) | predicate-selected branch |

Thresholds:

```json
"all"
"any"
{"at_least": 1}
```

`switch` pairs `values[i]` with `children[i]`; both arrays must have the same
length. `utility_selector` branches use:

```json
{"score": "heal_urgency", "params": {"threshold": 0.3}, "child": {...}}
```

Compact examples:

```json
{"type": "sequence", "children": [{"type": "action", "name": "step"}]}
{"type": "reactive_selector", "name": "priority", "children": [{"type": "action", "name": "flee"}]}
{"type": "random_selector", "seed": 7, "children": [{"type": "action", "name": "step"}]}
{"type": "weighted_random_selector", "seed": 7, "weights": [1, 3], "children": [{"type": "action", "name": "step"}, {"type": "action", "name": "step"}]}
{"type": "parallel", "success": "all", "failure": "any", "children": [{"type": "action", "name": "step"}]}
{"type": "mem_parallel", "success": {"at_least": 1}, "children": [{"type": "action", "name": "step"}]}
{"type": "race", "children": [{"type": "action", "name": "step"}]}
{"type": "switch", "key": "combat.phase", "values": [1, 2], "children": [{"type": "action", "name": "step"}, {"type": "action", "name": "step"}]}
{"type": "utility_selector", "hysteresis": 0.2, "branches": [{"score": "idle_value", "child": {"type": "action", "name": "step"}}]}
{"type": "if_then_else", "condition": "low_hp", "params": {"ratio": 0.3}, "then": {"type": "action", "name": "heal"}, "else": {"type": "action", "name": "fight"}}
```

## Decorators

| `type` | Required | Optional (default) | Effect |
|---|---|---|---|
| `inverter` | `child` | `name` | swaps Success/Failure |
| `always_succeed` | `child` | `name` | forces Success |
| `always_fail` | `child` | `name` | forces Failure |
| `repeat` | `times` (`> 0`), `child` | `name` | `times` successful runs |
| `retry` | `max_attempts` (`> 0`), `child` | `name` | one attempt per tick |
| `until_success` | `child` | `name` | Running until Success |
| `until_failure` | `child` | `name` | Running until Failure |
| `probability` | `chance` (`0..=1`), `seed`, `child` | `name` | deterministic gate |
| `while` | `condition`, `child` | `name`, `params` (`null`) | repeats while predicate holds |
| `guard` | `condition`, `child` | `name`, `params` (`null`) | blocks/aborts child when false |
| `cooldown` | `duration_ms`, `child` | `name` | cooldown after Success |
| `timeout` | `duration_ms`, `child` | `name` | virtual-time timeout |
| `limiter` | `max` (`> 0`), `child` | `name` | terminal-result quota |
| `throttle` | `window_ms`, `max` (`> 0`), `child` | `name` | terminal-result rate limit |

`duration_ms` and `window_ms` are milliseconds of virtual `Time`.

```json
{"type": "inverter", "child": {"type": "condition", "name": "enemy_near"}}
{"type": "always_succeed", "child": {"type": "action", "name": "optional_step"}}
{"type": "always_fail", "child": {"type": "action", "name": "probe"}}
{"type": "repeat", "times": 3, "child": {"type": "action", "name": "combo_step"}}
{"type": "retry", "max_attempts": 5, "child": {"type": "try_action", "name": "interact"}}
{"type": "until_success", "child": {"type": "action", "name": "poll"}}
{"type": "until_failure", "child": {"type": "action", "name": "hold"}}
{"type": "probability", "chance": 0.25, "seed": 11, "child": {"type": "action", "name": "critical_hit"}}
{"type": "while", "condition": "holding_position", "child": {"type": "action", "name": "block"}}
{"type": "guard", "condition": "enemy_near", "child": {"type": "action", "name": "attack"}}
{"type": "cooldown", "duration_ms": 1000, "child": {"type": "action", "name": "attack"}}
{"type": "timeout", "duration_ms": 5000, "child": {"type": "action", "name": "interact"}}
{"type": "limiter", "max": 3, "child": {"type": "action", "name": "use_item"}}
{"type": "throttle", "window_ms": 1000, "max": 2, "child": {"type": "action", "name": "shoot"}}
```

`guard`, `while`, and `if_then_else` accept predicates registered through
`NodeRegistry::predicate` or `NodeRegistry::condition`.

## Leaves

| `type` | Required | Optional (default) | Meaning |
|---|---|---|---|
| `action` | `name` | `params` (`null`) | registered action |
| `condition` | `name` | `params` (`null`) | registered read-only condition |
| `try_action` | `name` | `params` (`null`) | fallible action; `Err` -> trace error + Failure |
| `async_action` | `name` | `params` (`null`) | registered async/remote action |
| `wait` | `duration_ms` | `name` | virtual-time wait |
| `wait_until` | `condition` | `name`, `params` (`null`) | waits for a predicate |
| `wait_for_event` | `event` | `name` | consumes the registered event queue FIFO |
| `set_value` | `key`, `value` | `name` | writes a typed blackboard value every tick |

```json
{"type": "action", "name": "sense", "params": {"radius": 10}}
{"type": "condition", "name": "enemy_near"}
{"type": "try_action", "name": "open_chest"}
{"type": "async_action", "name": "fetch_plan"}
{"type": "wait", "duration_ms": 500, "name": "settle"}
{"type": "wait_until", "condition": "target_visible", "name": "acquire"}
{"type": "wait_for_event", "event": "combat.hit"}
{"type": "set_value", "key": "combat.phase", "value": 2}
```

## Structural nodes

| `type` | Required | Optional (default) | Meaning |
|---|---|---|---|
| `identified` | `id`, `child` | - | attaches a stable `NodeId` |
| `include` | `path` | `name` | included spec; resolver required |
| `subtree` | `template` | `name`, `params` (`null`) | instantiates a registered `SubtreeFactory` |
| `custom` | `kind` | `name`, `params` (`null`), `children` (`[]`) | registered custom factory |
| `scoped` | `child` | `name`, `inputs` (`[]`), `outputs` (`[]`) | private blackboard with copied inputs/outputs |
| `contract` | `child` | `name`, `required` (`[]`), `provided` (`[]`) | required/provided key boundary |

```json
{"type": "identified", "id": "attack", "child": {"type": "action", "name": "attack"}}
{"type": "include", "path": "combat.json", "name": "combat"}
{"type": "subtree", "template": "goblin_patrol", "name": "patrol", "params": {"speed": 3}}
{"type": "custom", "kind": "patrol_route", "params": {"points": 4}, "children": []}
{"type": "scoped", "name": "sandbox", "inputs": ["combat.target"], "outputs": ["combat.plan"], "child": {"type": "action", "name": "plan"}}
{"type": "contract", "name": "strike", "required": ["combat.target"], "provided": ["combat.plan"], "child": {"type": "action", "name": "plan"}}
```

`scoped` copies `inputs` into a private blackboard on activation and copies
`outputs` back on terminal status. `contract` checks `required` keys before the
child runs and clears `provided` keys on activation and abort. Both require key
names registered with `NodeRegistry::scoped_key` / `NodeRegistry::contract_key`.

## Registry mapping

| Configuration | Registration |
|---|---|
| `action` | `registry.action::<P>(name, factory)` |
| `condition` | `registry.condition::<P>(name, factory)`; also a predicate |
| `try_action` | `registry.try_action::<P, E>(name, factory)` |
| `async_action` | `registry.async_action::<P>(name, factory)` |
| predicates in `guard` / `while` / `if_then_else` / `wait_until` | `registry.condition` or `registry.predicate::<P>` |
| `utility_selector` branch `score` | `registry.score::<P>(name, factory)` |
| `switch` | `registry.switch_key::<K>(key)` |
| `set_value` | `registry.set_value_key::<T>(key)` |
| `wait_for_event` | `registry.event_key::<T>(key)` |
| `scoped` | `registry.scoped_key(&KEY)` (value must be `Clone`) |
| `contract` | `registry.contract_key(&KEY)` |
| `subtree` | `registry.subtree(name, factory)` or `registry.subtree_with_params::<P>` |
| `custom` | `registry.custom::<P>(kind, factory)` |

Factories receive typed params once per node and return that node's independent
tick/condition/score closure. `condition` names are also predicates; use
`predicate` when no condition leaf is needed.

Factory parameter rules: omitted `params` deserialize from `null`; `P = ()`
therefore needs no `params`, while struct params need a `"params": {...}` object.

## Includes

```rust
use playact::config::FsIncludeResolver;

let resolver = FsIncludeResolver::json("config/")?;
let tree = spec.build_with_resolver(&registry, &resolver)?;
```

- `FsIncludeResolver` confines paths to its base directory and rejects `..`,
  absolute paths, and symlink escapes.
- Cycles and nesting deeper than `MAX_INCLUDE_DEPTH` (32) are rejected.
- `include` without `build_with_resolver` is a build error.
- The watcher polls resolved include files for hot reload; custom resolvers can
  implement `watched_paths()` / `reset_watched_paths()`.

## Errors and schema

- Parse errors are `ConfigError` with an optional path.
- Build errors are `ConfigBuildError` with a path and a typed
  `ConfigBuildErrorKind` such as `UnknownAction`, `UnknownSwitchKey`,
  `UnknownSubtreeTemplate`, `InvalidParams`, or `DuplicateNodeId`.
- `TreeSpec::json_schema()` (feature `schema`) returns a JSON Schema for editor
  completion and external validation.
- `NodeRegistry::json_schema()` (feature `schema`) returns the same schema with
  the registered action, condition, predicate, score, key, custom, and subtree
  names injected as `enum` values, so completion matches what actually builds.
  `describe` adds hover descriptions, `describe_params::<P>` adds the derived
  params schema, and `describe_switch_key` / `describe_set_value_key` add typed
  key values.
- The optional hot-reload and remote-config loaders rebuild through the same
  `TreeSpec` pipeline, so validation and path errors apply there too.

## Editor completion

Export the schema from the same registry that builds the tree, then point the
editor at the generated file. The repository includes a small exporter:

```text
cargo run -p playact --example schema_export --features config,schema
```

The default output is `target/playact-schema.json`; pass a path as the first
argument to write it elsewhere. VS Code can map it in `.vscode/settings.json`:

```json
{
  "json.schemas": [
    {
      "fileMatch": ["config/*.json"],
      "url": "./target/playact-schema.json"
    }
  ]
}
```

With this mapping, `action`, `try_action`, `async_action`, `condition`,
predicate fields, keys, scoped/contract keys, custom kinds, and subtree
templates complete from the registry. Descriptions and parameter schemas are
opt-in:

```rust
use playact::config::NodeCategory;

#[derive(serde::Deserialize, playact::schemars::JsonSchema)]
struct PatrolParams {
    /// Waypoints visited before the loop restarts.
    waypoints: u32,
    speed: f32,
}

// After registering the action factory:
registry.describe_params::<PatrolParams>(
    NodeCategory::Action,
    "patrol",
    "Walk the patrol route and report completion.",
)?;
registry.describe(NodeCategory::Action, "attack", "Strike the current target.")?;
```

`describe` adds hover text; `describe_params::<P>` also derives a schema from
`P` and embeds it under `params`, so field descriptions, types, required fields,
and validation keywords apply in the editor. `playact::schemars` is re-exported
by the `schema` feature, so the derive does not need a separate dependency.
Recursive parameter types are supported: their definitions are moved into the
root schema and references are rewritten. Empty descriptions are ignored.

Key and event fields accept descriptions too, and switch/set-value keys can
carry their value schema:

```rust
registry.describe(NodeCategory::EventKey, "demo.alarm", "Raised when the alarm fires.")?;
registry.describe_switch_key::<String>(MODE, "Current game mode.")?;
registry.describe_set_value_key::<u32>(MORALE, "Written morale value.")?;
```

`describe_switch_key` types the matching `switch.values` items, and
`describe_set_value_key` types the matching `set_value.value`. Scoped and
contract key lists use `describe` with `ScopedKey` / `ContractKey` for
descriptions. Event keys have no configurable payload, so only their
description is injected.

Regenerate the file whenever the registry changes, for example as a build or
pre-commit step. If the schema is committed, relative `url` values resolve
against the workspace folder.
