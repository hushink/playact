# playact

**面向游戏自动化的类型安全、可扩展行为树引擎。**

[English](README.md) | 简体中文

[![Crates.io](https://img.shields.io/crates/v/playact.svg)](https://crates.io/crates/playact)
[![Docs.rs](https://img.shields.io/docsrs/playact)](https://docs.rs/playact)
[![MSRV](https://img.shields.io/badge/MSRV-1.92-blue)](Cargo.toml)
[![CI](https://github.com/hushink/playact/actions/workflows/ci.yml/badge.svg)](https://github.com/hushink/playact/actions/workflows/ci.yml)

`playact` 是行为树**引擎库**，不是游戏框架。它只认识 `Status`、`Node`、
`Context<W>`、`Blackboard`、`Time` 与 `Tracer`；世界、命令和配置参数类型都由
使用方提供，业务与引擎的边界始终清晰。

> [!IMPORTANT]
> 本库不定义任何业务类型。`W`（世界）、`C`（命令）、`P`（配置参数）均由应用
> 提供。公共 API 不暴露 `Any`，也不做字符串查表。

## 为什么选择 playact

- **唯一契约。** 所有节点实现 `tick(&mut Context<W>) -> Status`。控制节点只编排
  `Status` 流转，叶子节点真正干活；任意嵌套、替换与复用都自然成立。
- **类型安全。** `Key<T>` 让黑板类型错误变成编译错误；配置参数在构建期反序列化
  成具体类型。
- **编译期树形校验。** typestate 构建器在编译期拒绝“根节点多个子节点”和
  “装饰器子节点数量不对”。
- **可预测的 tick。** tick 内不 sleep、不引入 async runtime、不隐藏线程；长动作
  返回 `Running`，下一帧继续。
- **可中断。** Running 子树被抢占时先 `abort` 做外部清理，再 `reset` 清内部状态。
- **可选能力齐全。** 类型化配置、热重载、远程配置、Tokio 任务、tracing、快照与
  proc-macro DSL 都按 feature 提供。
- **核心足够小。** 默认构建是纯 `std`、零第三方依赖；关闭默认 feature 后以
  `no_std + alloc` 编译。

## 安装

```toml
[dependencies]
playact = "0.1"
```

最低 Rust 版本：**1.92**（edition 2024）。

## 快速开始

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

## 核心模型

### Status

```rust
pub enum Status {
    Success,
    Failure,
    Running,
}
```

`Status` 是节点之间唯一的控制流协议。业务错误由动作映射成 `Failure`；诊断信息
交给 `Tracer`。

### Node

```rust
pub trait Node<W>: Send {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status;

    fn abort(&mut self, _ctx: &mut Context<'_, W>) {}
    fn reset(&mut self) {}

    fn name(&self) -> &str;
    fn kind(&self) -> Kind;
    fn children(&self) -> &[BoxedNode<W>];

    // 可选快照钩子
    fn save_state(&self) -> Option<Vec<u8>> { None }
    fn load_state(&mut self, _state: &[u8]) -> bool { false }
}
```

- `abort` 在 Running 子树被放弃时释放外部副作用。
- `reset` 清除内部记忆；两个钩子都必须幂等。
- `Kind` 是内建节点的固定枚举，外加 `Kind::Custom(&'static str)`。
- 节点是 `Send`，行为树可以移到其他线程或线程池。

### Context 与时间

```rust
let mut ctx = Context::new(&mut world, &mut blackboard, Time::from_millis(1000, 250));
let _world: &mut World = ctx.world();
let _board: &mut Blackboard = ctx.blackboard();
let _time: Time = ctx.time();
```

`Time { now, dt }` 完全由驱动方提供。`Cooldown`、`Timeout`、`Wait` 和 `Throttle`
只依赖它，因此测试可以用虚拟时间确定性地推进。`now` 必须单调递增；驱动方重置
时钟时应先 reset 整棵树。

### 类型安全黑板

```rust
use playact::prelude::*;

static TARGET: Key<usize> = playact::key!("target");

let mut board = Blackboard::new();
board.set(&TARGET, 42_usize);

assert_eq!(board.get(&TARGET), Some(&42));
// board.get::<u64>(&TARGET); // 编译错误
```

Key 携带值类型，黑市值要求 `Send + 'static`。不提供 `DynKey`、运行时字符串查表
或迭代 API。

### 决策/执行分离

动作读取世界并写命令；驱动方在 tick 外 drain 后交给执行器：

```rust
static COMMANDS: Key<CommandBuffer<BattleCommand>> = playact::key!("commands");

// 动作中
ctx.blackboard()
    .get_or_insert_with(&COMMANDS, CommandBuffer::new)
    .push(BattleCommand::Attack);

// 驱动方
let commands: Vec<_> = tree
    .blackboard_mut()
    .get_mut(&COMMANDS)
    .map(|buffer| buffer.drain().collect())
    .unwrap_or_default();
```

引擎本身不解释 `BattleCommand`。

## 构建器

`BehaviorTreeBuilder` 使用闭包作用域与 typestate：

- 根节点必须恰好一个子节点；
- 装饰器闭包必须恰好一个子节点；
- 组合节点允许为空（空 `Sequence`/`Parallel` 成功，空 `Selector` 失败）。

不需要 `.end()`，也不会因为闭合顺序错误在运行时 panic。

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

## 节点

### 组合节点

| 节点 | 语义 |
|---|---|
| `Sequence` | 记忆式：成功后前进，Running 停留，Failure 整体失败 |
| `ReactiveSequence` | 每 tick 从头评估；旧的 Running 子节点被抢占时 abort |
| `Selector` | 记忆式：失败后尝试下一个，Running 停留 |
| `ReactiveSelector` | 每 tick 从头评估；高优先级 Success/Running 抢占低优先级分支 |
| `RandomSelector` | 带记忆；激活时用确定性 seed 打乱剩余分支 |
| `WeightedRandomSelector` | 按权重无放回抽样；0 权重永不选择；Running 分支续跑 |
| `UtilitySelector` | 每 tick 重新评分并选最高分；可用 hysteresis 控制切换 |
| `Parallel` | 所有子节点 tick 后按成功/失败阈值汇总；失败优先；单 tick 汇总 |
| `MemParallel` | 跨 tick 记住终态，直到阈值满足 |
| `Race` | 所有子节点并行推进；第一个终态获胜，其余 Running 被 abort |
| `IfThenElse` | 条件为真执行 then，否则 else；条件翻转时 abort 旧的 Running 分支 |

关键细节：

- `Parallel` 阈值支持 `All`、`Any`、`AtLeast(n)`；`AtLeast(n)` 在构建期校验。
  阈值只针对单个 tick，不跨 tick 累计。
- `MemParallel` 在一次激活内累计终态，结论确定后复位；适合不同子节点在不同 tick
  完成的情况。
- `RandomSelector` 与 `WeightedRandomSelector` 在相同 seed、相同 tick 序列下可复现；
  `reset()` 恢复初始 RNG。
- `UtilitySelector` 的评分闭包只读 `Context`；`NaN` 视为最低分，同分取靠前分支。
- `Race` 同一 tick 多个终态时按子节点顺序取第一个。

### 装饰节点

`Inverter`、`AlwaysSucceed`、`AlwaysFail`、`Repeat`、`Retry`、`UntilSuccess`、
`UntilFailure`、`Cooldown`、`Timeout`、`Limiter`、`Throttle`、`Guard`。

- `Cooldown` 只在子节点成功后开始计时。
- `Timeout` 只对 Running 计时；`duration = 0` 时首次 tick 直接失败，不 tick 子节点。
- `Retry`、`UntilSuccess`、`UntilFailure` 每 tick 最多尝试一次。
- `Repeat` 单 tick 最多推进 `MAX_REPEAT_STEPS_PER_TICK` 次。
- `Limiter` 是“每次激活”的配额；全局 reset 或 `abort → reset` 后清零。
- `Throttle` 按时间窗口限制终态次数；Running 不消耗配额。
- `Guard` 条件为假时直接失败，并 abort 之前 Running 的子节点；它是反应式
  `condition → child` 的直接形式。

### 叶子节点

`Action`、`Condition`、`Wait`、`WaitUntil`、`WaitForEvent`、`SetValue`、`TryAction`、
`AsyncAction`。

```rust
let action = Action::new("hold_key", |_| Status::Running)
    .with_abort(|_ctx| { /* 释放按键 */ })
    .with_reset(|| { /* 清理内部状态 */ });
```

- `WaitUntil` 在谓词为真前返回 `Running`。
- `WaitForEvent` 从 `EventKey` 对应的 `EventQueue` 消费 typed 事件。
- `TryAction` 返回 `Result<Status, E>`；失败时通过 Tracer 记录 `Error` 事件并返回
  `Failure`。

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

`SetValue<T>` 每次 tick 写入 typed key 并返回 `Success`：

```rust
let leaf = SetValue::<World, u32>::new("set_hp", &HP, 100);
// 或 builder.set_value("set_hp", &HP, 100)
```

## 组合与复用

### 整棵树作为节点

`BehaviorTree<W>` 实现了 `Node<W>`，可以直接嵌套：

```rust
let mut outer = BehaviorTreeBuilder::<World>::new("outer")
    .sequence(|b| b.raw(Box::new(inner_tree)))
    .build()?;
```

嵌套树共享外层的 world / time / 黑板 / Tracer；它自带的黑板与 Tracer 不参与。
`abort → reset` 会传播到它的根节点。

### 可复用子树

`SubtreeFactory` 是可克隆的蓝图；每次 `instantiate()` 都创建全新节点状态：

```rust
let goblin = SubtreeFactory::new(|| build_goblin_root());
let tree = BehaviorTreeBuilder::<World>::new("pair")
    .sequence(|b| b.subtree(&goblin).subtree(&goblin))
    .build()?;
```

### 运行时子树槽

`SubtreeSlot` 根据 typed key 切换子树：

```rust
static PHASE: Key<u32> = playact::key!("phase");

let slot = SubtreeSlot::new("phase_slot", &PHASE, |phase| match phase {
    1 => build_phase_one(),
    _ => build_phase_two(),
});
```

- key 不变：现有子树保留内部状态。
- key 改变：旧子树 `abort → reset`，工厂创建新子树。
- key 缺失：放弃当前子树并返回 `Failure`。
- `reset()` 丢弃子树；下一次 tick 重新实例化。

### 服务节点

服务在每个 tick 的根节点之前运行，共享 world/time/黑板；返回值被忽略：

```rust
let mut tree = BehaviorTreeBuilder::<World>::new("player")
    .selector(|b| { /* ... */ })
    .build()?;
tree.add_service(Box::new(sense_enemies));
```

服务适合感知、缓存和外部状态维护。服务子树不产生 Tracer 事件，也不影响
`last_status`。整棵树 `abort` 时，仍为 Running 的服务会被 abort，所有服务随后
reset；嵌套树同样运行自己的服务。

## 共享黑板

`SharedBlackboard` 基于 `Arc<Mutex<Blackboard>>`，用于多树/多线程共享：

```rust
let shared = SharedBlackboard::new();
shared.tick(&mut tree_a, &mut world, time);
shared.tick(&mut tree_b, &mut world, time);

shared.with(|board| board.get(&TARGET).copied());
shared.with_mut(|board| board.set(&TARGET, 7));
```

> [!WARNING]
> `tick` 在整棵树 tick 期间持有锁。节点内部请使用 `ctx.blackboard()` /
> `ctx.blackboard_ref()`，不要再 lock 同一个 `SharedBlackboard`，否则会死锁。

提供 `lock()`、`try_lock()`、`with()`、`with_mut()` 与 `into_inner()`；锁中毒会
自动取回内部数据。

## 可观测性

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

- `Tracer` 接收 `Enter` / `Exit` / `Abort` / `Error` 事件，路径是子索引路径。
- `MultiTracer` 将事件按注册顺序广播。
- `StatsTracer` 按路径统计 tick、终态、abort、错误、最后状态与 wall-clock 耗时，
  需要 `std`。
- `last_status()` / `is_running()` 暴露上一次顶层结果。
- `describe()` 输出 ASCII 树或 Graphviz DOT；builder/配置中的 `NodeId` 会出现在
  `TreeDescription::id` 和 DOT 标签中。

`AbortReason` 取值为 `Preempted`、`Timeout`、`ParallelDone`、`RaceDone`、
`TreeAbort`；它只出现在 trace 事件中，不进入 `Node::abort` 签名。

## 快照

`BehaviorTree::snapshot()` 保存内建节点状态，用于长时挂机或重启恢复：

```rust
let snapshot = tree.snapshot();
// 持久化 snapshot.version / snapshot.root / snapshot.services

let mut rebuilt = build_same_tree();
rebuilt.restore(&snapshot)?;
```

覆盖 Sequence、Selector、随机/加权/效用选择、Parallel/Race 进度、装饰器、`Wait`、
嵌套树与服务。格式带 `SNAPSHOT_VERSION`；数据损坏、结构变化或服务数量不一致会
返回 `RestoreError`。

自定义节点可实现 `Node::save_state` / `load_state`；自定义组合节点需要递归编码
子树状态。

> [!WARNING]
> 节点快照不包含 world、黑板值、闭包捕获状态、`AsyncAction` 任务、`SubtreeSlot`
> 动态子树或 Tracer 状态，这些需要单独持久化。时间型节点保存的是绝对 `Time.now`，
> 应在同一时间轴上恢复，或 reset 后重新建立。

可选 `blackboard-serde` feature 提供 `BlackboardCodec`，只序列化显式注册的 typed
key：

```rust
use playact::BlackboardCodec;

let mut codec = BlackboardCodec::new();
codec.register(&TARGET)?;
codec.register(&COMMANDS)?;

let node_state = tree.snapshot();
let board_state = codec.encode(tree.blackboard())?;
// 之后：tree.restore(&node_state)?; codec.decode(tree.blackboard_mut(), &board_state)?;
```

未知 key、重复注册、版本不支持和值类型错误都会返回错误；codec 不会做字符串 key
查找或公开 `Any`。

## 测试驱动

可选 `test-util` feature 提供确定性测试驱动：

```rust
use playact::TestDriver;

let mut driver = TestDriver::new(tree, world).with_step(Duration::from_millis(16));
assert_eq!(driver.tick(), Status::Running);
driver.advance(Duration::from_secs(1));
assert_eq!(driver.run_until_terminal(10), Some((1, Status::Success)));
```

`TestDriver` 持有树和 world，暴露黑板与最后状态，不读系统时钟也不 sleep。

## 配置驱动

可选 `config` feature 从数据构建行为树，同时保持参数类型安全：

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

- JSON、TOML、YAML、RON 解析器随对应 feature 提供。
- 配置拒绝未知字段；错误路径形如 `root.children[0]`。
- 支持 sequence、selector、随机/加权选择、race、parallel、mem_parallel、装饰器
  （含 throttle）、wait、action、condition 与 custom 工厂。
- `Guard`、`IfThenElse`、`UtilitySelector` 的谓词/评分属于业务逻辑，保留在代码或
  `custom` 工厂中。
- `TreeSpec::validate(&ValidationLimits)` 可在构建前校验版本、最大深度与节点数，
  适合远程配置或用户提交的配置。
- 可选 `schema` feature 提供 `TreeSpec::json_schema()`，用于编辑器补全与外部校验。
- 用 `identified` 包装配置节点可指定稳定 `NodeId`：
  ```json
  {"type":"identified","id":"root","child":{"type":"sequence","children":[]}}
  ```
- 完整树示例见 [`examples/battle_ai.json`](examples/battle_ai.json)。

### Include

`include` 用于拆分大型配置：

```rust
use playact::config::FsIncludeResolver;

let resolver = FsIncludeResolver::json("ai/")?;
let tree = spec.build_with_resolver(&registry, &resolver)?;
```

```json
{ "root": { "type": "include", "path": "patrol.json", "name": "patrol" } }
```

`FsIncludeResolver` 把路径限制在基准目录内，拒绝 `..`、绝对路径与符号链接逃逸；
构建期检测循环与过深嵌套。闭包 `Fn(&str) -> Result<TreeSpec, ConfigError>` 也
自动实现 `IncludeResolver`，可接文件、HTTP 或内存来源。

### 热重载

```rust
use playact::hot_reload::TreeWatcher;

let mut watcher = TreeWatcher::<World>::json("ai.json", registry)?;
watcher.tree_mut().tick(&mut world, time);

if watcher.poll()? {
    println!("reloaded: {}", watcher.tree().describe());
}
```

成功重建会替换旧树、保留黑板数据并重置节点状态；解析/构建失败保留旧树并返回
错误。默认 200ms 去抖。include 使用 `json_with_resolver` /
`with_parser_and_resolver`。

监听器只关注根文件；被 include 的文件单独变化不会触发事件，可调用
`reload_now()` 或触碰根文件。

### 远程配置

```rust
use playact::remote_config::RemoteTreeLoader;

let mut loader = RemoteTreeLoader::<World>::json(
    "https://example.com/ai.json",
    registry,
)?
.with_refresh_interval(Duration::from_secs(30));

loader.poll()?; // 条件请求；304 不重建
loader.tree_mut().tick(&mut world, time);
```

`RemoteTreeLoader` 使用 `reqwest::blocking`，支持 ETag / Last-Modified，刷新失败
保留旧树。请求在调用线程同步执行；高延迟环境请放到工作线程或设置 Client 超时。
include 使用 `json_with_resolver` / `with_client_and_resolver`。

响应体限制为 `MAX_REMOTE_CONFIG_BYTES`（1 MiB），必须是 UTF-8，并在构建前通过
`TreeSpec::validate`（默认 `ValidationLimits`）校验；过大或结构过深的配置会被拒绝。

## 异步动作

`AsyncTask` 是运行时无关的句柄：

```rust
pub trait AsyncTask: Send {
    fn poll(&mut self) -> Status;
    fn cancel(&mut self);
}
```

`AsyncAction` 每个 tick poll 一次，被抢占时 cancel。内置两种适配器：

```rust
// 标准库线程；协作取消
let action = AsyncAction::<World>::new("fetch", |_| {
    Box::new(ThreadTask::spawn(|cancel| {
        if cancel.is_cancelled() {
            return Status::Failure;
        }
        // 阻塞调用 / 远程请求
        Status::Success
    })) as Box<dyn AsyncTask>
});
```

```rust
// 需要 tokio feature
let action = AsyncAction::<World>::new("fetch", |_| {
    Box::new(TokioTask::spawn(async move {
        // let result = client.call().await;
        Status::Success
    })) as Box<dyn AsyncTask>
});
```

- `TokioTask::spawn` 使用当前 runtime；不在 runtime 内时返回立即失败的任务，不会
  panic。
- `TokioTask::spawn_on(&handle, future)` 指定 runtime handle。
- cancel/drop 在 await 点 abort 任务。
- 业务错误由 future/闭包自行映射为 `Status`。

## Proc-macro DSL

启用 `macros` 后可用 `bt!` 编写类型安全的行为树：

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

DSL 支持 sequence、selector、随机选择、parallel、race、装饰器（含 guard 与
throttle）、`action`、`condition`、`wait`、`set_value` 与 `raw(expression)`。
加权/效用选择使用 `raw(...)`，因为权重与评分是代码表达式。宏展开仍走 typestate
构建器，子节点数量错误依旧是编译错误。

## no_std

关闭默认 feature 后，核心使用 `alloc`：

```toml
[dependencies]
playact = { version = "0.1", default-features = false }
```

核心可用：节点、`BehaviorTree`（含 services 与快照）、builder、`Blackboard`
（内部 `BTreeMap`）、组合/装饰/叶子、`CommandBuffer`、`SubtreeFactory`、
`SubtreeSlot`、`EventQueue` / `EventKey`、`WaitUntil` / `WaitForEvent` /
`TryAction`、`MultiTracer` 与 `describe`。

需要 `std`：`SharedBlackboard`、`AsyncAction` / `ThreadTask` / `TokioTask`、
`StatsTracer`、配置层、黑板序列化、热重载、远程配置、`TracingTracer`。

`SubtreeFactory` 使用 `alloc::sync::Arc`，no_std 目标需支持原子指针；`macros`
feature 可在 no_std 中使用。

```text
cargo check -p playact --no-default-features --target thumbv7em-none-eabihf --lib
```

## Feature 矩阵

| Feature | 内容 |
|---|---|
| 默认 | `std`；纯标准库，零第三方依赖 |
| `std` | 标准库集成；关闭后核心切换为 `no_std + alloc` |
| `config` | `TreeSpec`、`NodeRegistry`、JSON、typed 参数、include（依赖 `std`） |
| `schema` | `TreeSpec::json_schema()` JSON Schema 生成（依赖 `config`） |
| `blackboard-serde` | `BlackboardCodec` typed 黑板序列化（依赖 `std`） |
| `test-util` | 虚拟时间与 run-until 辅助的 `TestDriver`（依赖 `std`） |
| `toml` | TOML 解析器 |
| `yaml` | YAML 解析器（`serde_yaml_ng`） |
| `ron` | RON 解析器 |
| `tracing` | `TracingTracer` |
| `hot-reload` | `TreeWatcher` + `notify` 文件监听 |
| `remote-config` | `RemoteTreeLoader` + `reqwest::blocking` |
| `tokio` | `TokioTask` 适配器 |
| `macros` | `playact-macros` 的 `bt!` DSL |
| `full` | 以上全部 |

## 线程模型

- `Node<W>: Send`、`BehaviorTree<W>: Send`；不要求 `Sync`。
- `W` 不必 `Send`/`Sync`；树只在 tick 期间借用 `&mut W`。
- 黑板值要求 `Send + 'static`。
- `SharedBlackboard` 用互斥锁串行化多树 tick。
- 热重载回调只在后台线程发送事件；重建与 tick 在调用线程执行。
- 远程配置请求在调用线程同步执行。
- 异步任务在 tick 线程 poll；线程/远程取消是协作式的（Tokio 任务会被 abort）。
- 每个 AI 通常持有自己的树与黑板；多 AI 用工厂分别构建。

## 示例

```text
cargo run -p playact --example battle_ai
cargo run -p playact --example battle_ai_config --features config
```

- `battle_ai`：确定性的 1D 玩家 vs 三只哥布林战斗，展示 ReactiveSelector 打断、
  Cooldown、Timeout、Inverter、Tracer、abort 清理与 CommandBuffer/Executor 分离。
- `battle_ai_config`：同一套思路的 JSON 配置驱动版本，展示 typed 参数与 custom
  节点工厂。

## 测试与基准

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo bench -p playact
cargo doc --workspace --all-features --no-deps
cargo check -p playact --no-default-features --target thumbv7em-none-eabihf --lib
```

- 单元测试覆盖所有节点、构建器、配置层、重载、远程加载、快照与异步适配器。
- 公共 API 集成测试与 doctest 以用户视角验证 crate。
- `tests/stress.rs` 用固定 seed 生成随机树（含 services、加权/效用选择、race、
  guard 与快照恢复），验证 tick / abort / reset / describe / Send 不变量。
- `benches/engine.rs` 是零第三方依赖的微基准，覆盖 tick、黑板、构建与 describe。
- CI 覆盖 stable 与 MSRV 1.92、Linux 与 Windows，以及嵌入式 no_std 检查。
