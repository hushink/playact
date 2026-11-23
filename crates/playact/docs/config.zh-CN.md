# playact 配置参考

可选 `config` feature 的完整参考：`TreeSpec`、所有 `SpecNode` 类型、字段与默认值、
对应的 `NodeRegistry` 注册方式，以及 include、版本迁移、校验与 schema。任务式用法见
[API 指南](api.zh-CN.md)，项目概览见 [README](../README.zh-CN.md)。

- JSON、TOML、YAML、RON 结构一致；下文示例使用 JSON。
- 未知字段会被拒绝；错误带路径，如 `root.children[0]`。
- 配置中不包含闭包；注册名与类型化参数只在构建期解析/反序列化一次。

## 顶层 `TreeSpec`

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

| 字段 | 必填 | 默认 | 含义 |
|---|---|---|---|
| `version` | 否 | `1` | 配置版本；不支持的值会被拒绝 |
| `name` | 否 | `"BehaviorTree"` | 树显示名称 |
| `root` | 是 | - | 根节点 |
| `services` | 否 | `[]` | 每帧先于根节点运行的节点 |

Services 共享 world、黑板、时间与 tracer；其返回状态会被 trace 记录，但不替代根状态。

## 解析与构建

| 方法 | Feature | 用途 |
|---|---|---|
| `TreeSpec::from_json_str` / `from_json_slice` | `config` | JSON |
| `TreeSpec::from_toml_str` | `toml` | TOML |
| `TreeSpec::from_yaml_str` | `yaml` | YAML |
| `TreeSpec::from_ron_str` | `ron` | RON |
| `spec.build(&registry)` | `config` | 不含 include 的构建 |
| `spec.build_with_resolver(&registry, &resolver)` | `config` | 含 `include` 的构建 |
| `spec.validate(&limits)` | `config` | 只校验版本/深度/节点数 |
| `TreeSpec::json_schema()` | `schema` | 供编辑器使用的 JSON Schema |

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

`ValidationLimits` 默认 `max_depth: 64`、`max_nodes: 10_000`；`validate` 把 include
计为一个节点，不展开。

## 版本迁移

`TreeSpec` 只接受 `CURRENT_VERSION`（当前为 1），但应用可以注册迁移，在反序列化前
升级旧版原始文档。迁移处理 `serde_json::Value`，与源格式无关；引擎不会解释任何业务
字段。

```rust
use serde_json::Value;
use playact::config::{ConfigError, ConfigMigrator, FsIncludeResolver};

let migrator = ConfigMigrator::new().with_migration(0, |mut document: Value| {
    let object = document
        .as_object_mut()
        .ok_or_else(|| ConfigError::new("legacy document must be an object"))?;
    let tree = object
        .remove("tree")
        .ok_or_else(|| ConfigError::with_path("tree", "missing tree"))?;
    object.insert(String::from("root"), tree);
    Ok(document)
})?;

let spec = migrator.parse_json_str(legacy_json)?;
let resolver = FsIncludeResolver::auto_with_migrator("config/", migrator)?;
let mut tree = spec.build_with_resolver(&registry, &resolver)?;
```

| API | 用途 |
|---|---|
| `ConfigMigrator::new()` | 空迁移链 |
| `register(from, migration)` / `with_migration(from, migration)` | 注册 `from -> from + 1`；每个版本一个步骤 |
| `upgrade(Value)` | 执行迁移链并写入结果 `version`；当前版本文档原样返回 |
| `parse_json_str` / `parse_json_slice` / `parse_toml_str` / `parse_yaml_str` / `parse_ron_str` | 解析指定格式、迁移、再反序列化（TOML/YAML/RON 方法需要对应 feature） |
| `parse_str(format, text)` / `parse_path(path, text)` | 指定格式或按扩展名选择格式 |
| `parse_value(Value)` | 迁移并反序列化已解析文档 |
| `parser(format)` | 供自定义加载器使用的自有闭包 |
| `FsIncludeResolver::with_migrator(base, migrator, format)` / `auto_with_migrator(base, migrator)` | 迁移每个被包含文件 |
| `TreeWatcher::with_migrator` / `with_migrator_and_resolver` | 初次加载与每次热重载时迁移（`hot-reload`） |
| `RemoteTreeLoader::with_migrator` / `with_migrator_and_resolver`（以及 `with_client_*` 变体） | 初次加载与每次刷新时迁移（`remote-config`） |

规则与错误：

- 顶层 `version` 缺失时默认为 `CURRENT_VERSION`，与 `TreeSpec` 一致；旧版文档必须
  显式声明版本。
- 高于 `CURRENT_VERSION` 的版本、非整数与负数会在迁移前以 `version` 路径拒绝。
- 缺少步骤（例如版本 `0` 没有 `0 -> 1` 迁移）会报错。
- 每个迁移必须返回 JSON 对象；成功后管线会把 `version` 覆盖为下一版本。迁移错误
  保留内部配置路径，并附带源版本与目标版本上下文。
- 最终文档按当前 `TreeSpec` 解析，因此未知字段、非法参数与未注册名称仍会以原有
  路径失败。
- include 文件分别迁移，因此 include 图中的每个文件都可以处于任意版本，只要有到
  当前版本的注册路径。

[`examples/config_migration.rs`](../examples/config_migration.rs) 会迁移 v0 根配置与
include，然后构建并 tick。

### 声明式清单

`playact migrate` 无需应用代码即可执行声明式迁移。清单文件与配置一样按扩展名解析，
支持 JSON、TOML、YAML 与 RON：

```json
{
  "version": 1,
  "migrations": [
    {
      "from": 0,
      "operations": [
        {"op": "rename_field", "from": "tree", "to": "root"},
        {"op": "rename_field", "where": {"type": "wait"}, "from": "seconds", "to": "duration_ms"},
        {"op": "remove_field", "where": {"type": "wait"}, "field": "legacy"},
        {"op": "set_field", "at": "/root", "field": "name", "value": "migrated", "overwrite": false}
      ]
    }
  ]
}
```

- `version` 是清单格式版本（`1`），不是配置版本。
- `from` 是源配置版本。规则使用与 `ConfigMigrator` 相同的链式执行与版本写入语义，
  直到 `CURRENT_VERSION`。
- `rename_field` 在源字段不存在时无操作，目标字段已存在时报错；`remove_field` 在字段
  不存在时无操作；`set_field` 只在字段缺失时插入，`"overwrite": true` 时覆盖。
- 目标选择：
  - 不写 `at` 或 `where`：仅根对象。
  - `at`：指向单个对象的 JSON Pointer（`""` 为根，`"/root"` 为根节点）。
  - `where`：递归匹配所有字段与给定值相等的对象，例如 `{"type": "wait"}`。`at` 与
    `where` 互斥。
- 每个根文件与 include 文件都会迁移，然后按 schema 校验；include 图按迁移后的路径
  重新解析。所有文件均有效后才写入；写入具备事务回滚。
- 迁移后的文件按源格式规范化序列化；已处于 `CURRENT_VERSION` 的文件保持字节不变。
- `--check` 只报告将要变更的文件；`--json` 输出逐根报告。任意 Rust 变换仍需应用中
  的 `ConfigMigrator` 闭包。

## 名称、参数与 ID

- 组合/装饰/结构节点上的 `name` 是可选的显示名称。
- Action、condition、可失败/异步 action 与 custom kind 使用注册名作为显示名。
- 每个注册工厂的 `params` 都可省略。省略时按 JSON `null` 反序列化；无参数注册使用
  `P = ()`。参数结构体需要显式 `"params": {...}` 对象（所有字段都有默认值时可写
  `{}`）。
- 用 `identified` 包装任意节点可附加稳定 `NodeId`，供快照、差异、指标与状态保持
  重载使用。重复 ID 会在构建期被拒绝。

## 组合节点

| `type` | 必填 | 可选（默认） | 语义 |
|---|---|---|---|
| `sequence` | - | `name`、`children`（`[]`） | 记忆式顺序 |
| `reactive_sequence` | - | `name`、`children`（`[]`） | 每帧从首个子节点重新评估 |
| `selector` | - | `name`、`children`（`[]`） | 记忆式选择 |
| `reactive_selector` | - | `name`、`children`（`[]`） | 每帧重新评估优先级并抢占 |
| `random_selector` | `seed` | `name`、`children`（`[]`） | 带记忆的确定性随机选择 |
| `weighted_random_selector` | `seed` | `name`、`weights`（`[]`）、`children`（`[]`） | 加权选择；权重 `0` 永不选中 |
| `parallel` | - | `name`、`success`（`all`）、`failure`（`any`）、`children`（`[]`） | 单帧阈值汇总 |
| `mem_parallel` | - | `name`、`success`（`all`）、`failure`（`any`）、`children`（`[]`） | 跨帧累计终态 |
| `race` | - | `name`、`children`（`[]`） | 第一个终态子节点获胜 |
| `switch` | `key` | `name`、`values`（`[]`）、`children`（`[]`） | 按注册的 typed key 选择分支 |
| `utility_selector` | - | `name`、`hysteresis`（`0.0`）、`branches`（`[]`） | 每帧评分选择 |
| `if_then_else` | `condition`、`then`、`else` | `name`、`params`（`null`） | 谓词选择分支 |

阈值写法：

```json
"all"
"any"
{"at_least": 1}
```

`switch` 将 `values[i]` 与 `children[i]` 一一配对，两个数组长度必须相同。
`utility_selector` 分支：

```json
{"score": "heal_urgency", "params": {"threshold": 0.3}, "child": {...}}
```

紧凑示例：

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

## 装饰节点

| `type` | 必填 | 可选（默认） | 效果 |
|---|---|---|---|
| `inverter` | `child` | `name` | 交换 Success/Failure |
| `always_succeed` | `child` | `name` | 强制 Success |
| `always_fail` | `child` | `name` | 强制 Failure |
| `repeat` | `times`（`> 0`）、`child` | `name` | 成功执行 `times` 次 |
| `retry` | `max_attempts`（`> 0`）、`child` | `name` | 每帧一次尝试 |
| `until_success` | `child` | `name` | 直到 Success 前保持 Running |
| `until_failure` | `child` | `name` | 直到 Failure 前保持 Running |
| `probability` | `chance`（`0..=1`）、`seed`、`child` | `name` | 确定性概率门 |
| `while` | `condition`、`child` | `name`、`params`（`null`） | 条件为真时重复 |
| `guard` | `condition`、`child` | `name`、`params`（`null`） | 条件为假时阻断/abort |
| `cooldown` | `duration_ms`、`child` | `name` | 成功后的冷却 |
| `timeout` | `duration_ms`、`child` | `name` | 虚拟时间超时 |
| `limiter` | `max`（`> 0`）、`child` | `name` | 终态次数配额 |
| `throttle` | `window_ms`、`max`（`> 0`）、`child` | `name` | 时间窗口终态限流 |

`duration_ms` 与 `window_ms` 是虚拟 `Time` 的毫秒数。

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

`guard`、`while`、`if_then_else` 使用 `NodeRegistry::predicate` 或
`NodeRegistry::condition` 注册的谓词。

## 叶子节点

| `type` | 必填 | 可选（默认） | 含义 |
|---|---|---|---|
| `action` | `name` | `params`（`null`） | 注册的 action |
| `condition` | `name` | `params`（`null`） | 注册的只读 condition |
| `try_action` | `name` | `params`（`null`） | 可失败 action；`Err` -> trace error + Failure |
| `async_action` | `name` | `params`（`null`） | 注册的异步/远程 action |
| `wait` | `duration_ms` | `name` | 虚拟时间等待 |
| `wait_until` | `condition` | `name`、`params`（`null`） | 等待谓词成立 |
| `wait_for_event` | `event` | `name` | FIFO 消费注册事件队列 |
| `set_value` | `key`、`value` | `name` | 每帧写入 typed 黑板值 |

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

## 结构节点

| `type` | 必填 | 可选（默认） | 含义 |
|---|---|---|---|
| `identified` | `id`、`child` | - | 附加稳定 `NodeId` |
| `include` | `path` | `name` | 引入其他配置；需要 resolver |
| `subtree` | `template` | `name`、`params`（`null`） | 实例化注册的 `SubtreeFactory` |
| `custom` | `kind` | `name`、`params`（`null`）、`children`（`[]`） | 注册的自定义工厂 |
| `scoped` | `child` | `name`、`inputs`（`[]`）、`outputs`（`[]`） | 私有黑板，输入/输出按名称拷贝 |
| `contract` | `child` | `name`、`required`（`[]`）、`provided`（`[]`） | 必需/提供 key 边界 |

```json
{"type": "identified", "id": "attack", "child": {"type": "action", "name": "attack"}}
{"type": "include", "path": "combat.json", "name": "combat"}
{"type": "subtree", "template": "goblin_patrol", "name": "patrol", "params": {"speed": 3}}
{"type": "custom", "kind": "patrol_route", "params": {"points": 4}, "children": []}
{"type": "scoped", "name": "sandbox", "inputs": ["combat.target"], "outputs": ["combat.plan"], "child": {"type": "action", "name": "plan"}}
{"type": "contract", "name": "strike", "required": ["combat.target"], "provided": ["combat.plan"], "child": {"type": "action", "name": "plan"}}
```

`scoped` 在激活时把 `inputs` 拷入私有黑板，终态时把 `outputs` 拷回父黑板。
`contract` 在子节点运行前检查 `required`，并在激活与 abort 时清空 `provided`。
两者都要求 key 通过 `NodeRegistry::scoped_key` / `NodeRegistry::contract_key` 注册。

## 注册表映射

| 配置 | 注册方式 |
|---|---|
| `action` | `registry.action::<P>(name, factory)` |
| `condition` | `registry.condition::<P>(name, factory)`；同时作为 predicate |
| `try_action` | `registry.try_action::<P, E>(name, factory)` |
| `async_action` | `registry.async_action::<P>(name, factory)` |
| `guard` / `while` / `if_then_else` / `wait_until` 的谓词 | `registry.condition` 或 `registry.predicate::<P>` |
| `utility_selector` 分支的 `score` | `registry.score::<P>(name, factory)` |
| `switch` | `registry.switch_key::<K>(key)` |
| `set_value` | `registry.set_value_key::<T>(key)` |
| `wait_for_event` | `registry.event_key::<T>(key)` |
| `scoped` | `registry.scoped_key(&KEY)`（值类型需 `Clone`） |
| `contract` | `registry.contract_key(&KEY)` |
| `subtree` | `registry.subtree(name, factory)` 或 `registry.subtree_with_params::<P>` |
| `custom` | `registry.custom::<P>(kind, factory)` |

Factory 每个节点接收一次类型化参数，并返回该节点独立的 tick/condition/score 闭包。
`condition` 名称同时注册为谓词；不需要 condition 叶子时使用 `predicate`。

Factory 参数规则：省略 `params` 时按 `null` 反序列化；`P = ()` 无需 `params`；
结构体参数需要 `"params": {...}` 对象。

## Include

```rust
use playact::config::FsIncludeResolver;

let resolver = FsIncludeResolver::auto("config/")?;
let tree = spec.build_with_resolver(&registry, &resolver)?;
```

- `FsIncludeResolver::auto` 按被包含文件的扩展名选择解析器：`.json` 始终可用，
  `.toml` / `.yaml` / `.yml` / `.ron` 需要对应 feature。没有受支持扩展名的 include
  会被拒绝。也可用 `json` / `toml` / `yaml` / `ron` 强制使用单一解析器。
- `FsIncludeResolver` 把路径限制在基准目录内，拒绝 `..`、绝对路径与符号链接逃逸。
- 循环与超过 `MAX_INCLUDE_DEPTH`（32）的嵌套会被拒绝。
- 未使用 `build_with_resolver` 时，`include` 会构建失败。
- watcher 会轮询解析过的 include 文件以触发热重载；自定义 resolver 可实现
  `watched_paths()` / `reset_watched_paths()`。

## 错误与 schema

- 解析错误为 `ConfigError`，带可选路径。
- 构建错误为 `ConfigBuildError`，带路径与类型化的 `ConfigBuildErrorKind`，例如
  `UnknownAction`、`UnknownSwitchKey`、`UnknownSubtreeTemplate`、`InvalidParams`、
  `DuplicateNodeId`。
- `TreeSpec::json_schema()`（`schema` feature）返回 JSON Schema，供编辑器补全与
  外部校验使用。
- `NodeRegistry::json_schema()`（`schema` feature）在相同 schema 中把已注册的
  action、condition、predicate、score、key、custom 与 subtree 名称注入为 `enum`，
  使补全内容与实际可构建内容一致。`describe` 添加悬停描述，
  `describe_params::<P>` 添加派生的参数 schema，
  `describe_switch_key` / `describe_set_value_key` 添加带类型的 key 值。
- 可选的热重载与远程配置加载器走同一套 `TreeSpec` 流程，因此校验与路径错误同样适用。

## 编辑器补全

用实际构建树的同一个注册表导出 schema，再让编辑器指向生成文件。仓库自带一个
导出示例：

```text
cargo run -p playact --example schema_export --features config,schema
```

默认输出为 `target/playact-schema.json`；把路径作为第一个参数传入可写到其他位置。
VS Code 可在 `.vscode/settings.json` 中配置：

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

配置后，`action`、`try_action`、`async_action`、`condition`、predicate 字段、
各类 key、scoped/contract key、custom kind 与 subtree template 都会按注册表补全。
描述与参数 schema 是可选的：

```rust
use playact::config::NodeCategory;

#[derive(serde::Deserialize, playact::schemars::JsonSchema)]
struct PatrolParams {
    /// Waypoints visited before the loop restarts.
    waypoints: u32,
    speed: f32,
}

// 注册 action 工厂之后：
registry.describe_params::<PatrolParams>(
    NodeCategory::Action,
    "patrol",
    "Walk the patrol route and report completion.",
)?;
registry.describe(NodeCategory::Action, "attack", "Strike the current target.")?;
```

`describe` 只添加悬停描述；`describe_params::<P>` 还会从 `P` 生成 schema 并放入
`params`，因此字段描述、类型、必填项与校验关键字在编辑器中同样生效。`schema`
feature 会重导出 `playact::schemars`，无需额外声明依赖。递归参数类型也受支持：
其定义会迁移到根 schema 并重写引用。空描述会被忽略。

key 与 event 字段同样支持描述，switch/set-value key 还可携带值 schema：

```rust
registry.describe(NodeCategory::EventKey, "demo.alarm", "Raised when the alarm fires.")?;
registry.describe_switch_key::<String>(MODE, "Current game mode.")?;
registry.describe_set_value_key::<u32>(MORALE, "Written morale value.")?;
```

`describe_switch_key` 为匹配的 `switch.values` 元素提供类型，
`describe_set_value_key` 为匹配的 `set_value.value` 提供类型。scoped/contract
的 key 列表可用 `describe` 配合 `ScopedKey` / `ContractKey` 添加描述。event key
没有可配置的载荷，因此只注入描述。

注册表变化后重新生成即可，例如放入构建或 pre-commit 步骤。若把 schema 提交到仓库，
相对 `url` 会按工作区目录解析。

## 节点目录

`NodeRegistry::markdown_catalog()`（`schema` feature）把相同元数据渲染为 Markdown，
便于团队配置文档：

```rust
std::fs::write("docs/node-catalog.md", registry.markdown_catalog())?;
```

条目按分类分组、按名称排序，因此输出确定，可在 CI 中生成或在 diff 中审阅。每行包含
注册名称、`describe` 描述，以及来自 `describe_params`、`describe_switch_key` 与
`describe_set_value_key` 的紧凑参数或 key 值摘要。condition 只出现在 Conditions
分类中；它仍可作为 predicate 使用，但不会在 Predicates 中重复。

[`examples/schema_export.rs`](../examples/schema_export.rs) 会同时写出 schema 与
节点目录。测试套件使用 JSON Schema 校验器验证仓库中的示例配置，确保 schema 与解析器
保持一致。

## 预检

[`examples/config_lint.rs`](../examples/config_lint.rs) 先用注册表 schema 校验文件，
再解析并构建，因此参数反序列化、未知名称、include 与重复 ID 也会被检查：

```text
cargo run -p playact --example config_lint --features config,schema,toml,yaml,ron -- config/a.json config/b.toml
cargo run -p playact --example config_lint --features config,schema,toml,yaml,ron -- --json config/*
```

格式按扩展名选择：`.json`、`.toml`、`.yaml`/`.yml` 或 `.ron`。示例使用
`battle_ai.json` 对应的演示注册表；请把其中的 `build_registry` 替换为自己的注册。
schema 校验会逐节点深入，错误会指向具体字段；构建错误会带精确的配置路径。`--json`
输出包含 `source`、`path` 与 `message` 的机器可读报告。任一文件无效时以非零状态退出，
可作为部署或配置发布流程的门禁。

### CLI

工作区 `playact-cli` crate 提供 `playact` 二进制，用于配置工具链：

```text
cargo run -p playact-cli -- schema > schema.json
cargo run -p playact-cli -- catalog --schema schema.json
cargo run -p playact-cli -- lint --schema schema.json config/a.toml config/b.yaml
cargo run -p playact-cli -- lint --watch --schema schema.json config/*.toml
cargo run -p playact-cli -- convert --schema schema.json config/a.json config/b.yaml
cargo run -p playact-cli -- convert --recursive --format yaml --schema schema.json config/root.json dist/config
cargo run -p playact-cli -- bundle --schema schema.json config/root.json dist/config
cargo run -p playact-cli -- graph --output graph.mmd config/root.json
cargo run -p playact-cli -- fmt --schema schema.json config/*.toml
cargo run -p playact-cli -- fmt --check --schema schema.json config/*.toml
cargo run -p playact-cli -- fmt --recursive --schema schema.json config/root.json
cargo run -p playact-cli -- fmt --check --recursive --schema schema.json config/root.json
cargo run -p playact-cli -- check --schema schema.json config/root.json
cargo run -p playact-cli -- check --json --schema schema.json config/root.json
cargo run -p playact-cli -- migrate --manifest migrations.json --schema schema.json config/root.json
cargo run -p playact-cli -- migrate --check --manifest migrations.json config/root.json
cargo run -p playact-cli -- diff --schema schema.json config/old.json config/new.json
```

`schema` 输出基础 `TreeSpec` schema。`catalog`、`lint`、`convert`、`bundle`、
`fmt`、`check`、`migrate` 与 `diff` 接受 `NodeRegistry::json_schema()` 生成的
注册表感知 schema；通过 `--schema` 传入即可包含注册名称、描述与参数 schema；未传入
时使用基础 schema。`lint` 会解析并校验限制，但没有应用注册表，因此不会构建树；需要
应用内构建预检时使用 `config_lint` 示例。
`include` 节点会递归解析；路径限制在 `--include-base` 内，未指定时使用每个根文件的
父目录。include 格式按扩展名选择，无法识别时回退到包含它的文件格式。被包含文件的
错误在 JSON 报告中带有 `file` 字段。`--watch` 在根配置、被包含文件或 schema 变化时
重新 lint。`convert` 在写入 JSON/TOML/YAML/RON 前先校验输入及
其 include。TOML 无法表示 null，因此转换为 TOML 时会省略取值为 null 的可选字段。
不使用 `--recursive` 时 include 路径会被保留而不会复制文件，转换后应保持在相同
include 基准目录内或自行调整。`convert --recursive --format FORMAT` 可写入新的或空的
输出目录，转换 include 图中的每个文件，并把 include 路径改写为目标扩展名；加载转换
后的根文件时以该输出目录作为 include 基准目录。
`bundle` 在验证根配置及其 include 后，把根文件与所有被包含文件复制到空输出目录，
不重写 include 字符串。输出目录必须位于 include 基准目录之外；加载 bundle 根文件时
以该输出目录作为 include 基准目录。`graph` 把节点结构与 include 关系渲染为 Mermaid
（默认）或 DOT；缺失或无效的 include 会显示为错误节点。`fmt` 校验输入后按规范格式
重写；被包含文件只做校验，除非显式传入或使用 `--recursive` 格式化整个 include 图。
所有目标都会先在内存中完成校验与格式化，再开始写入；写入失败时会恢复已写入的文件。
`--check` 不写文件，存在需要格式化的文件时非零退出。`check` 把 include 感知的校验
与只读格式检查结合：所有通过 lint 的根文件和被包含文件必须已经是规范格式。存在 lint
错误、无法格式化或需要格式化的文件时非零退出；`--json` 同时输出根配置的 lint 报告
与逐文件格式状态。`migrate` 对每个根文件及其 include 应用声明式清单，校验迁移后的
图，所有文件有效时才原地写入；`--check` 只报告待迁移文件，`--json` 输出逐根报告。
RON 格式化使用解析后的 spec 序列化，因为 RON 的通用 map 语法无法反序列化为
`TreeSpec`；其他格式保留原始字段存在性。`diff` 先校验两个输入，再按路径报告节点、
参数与 include 文件集合变化。子节点按 type 与 name/kind/key 匹配，插入与
重排会报告为 moved，而不是后续索引全部变化。相对路径相同的 include 文件会递归比较，
其变化带有 `file` 字段。存在差异时非零退出并支持 `--json`；include 文件集合使用
`--include-base`，未指定时使用各自输入文件的父目录。
