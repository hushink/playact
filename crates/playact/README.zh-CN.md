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
- **可选能力齐全。** 类型化配置、热重载、远程配置、Tokio 任务、tracing、快照、
  HTML inspector 与 proc-macro DSL 都按 feature 提供。
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
- 组合节点允许为空（空 `Sequence`/`Parallel` 成功，空 `Selector` 失败）；
- `build()` 会拒绝根子树中重复的稳定 `NodeId`，并报告第二次出现的结构路径。

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
| `Switch` | 按 typed 黑板 key 选择子节点；key 缺失或无匹配返回 Failure |

关键细节：

- `Parallel` 阈值支持 `All`、`Any`、`AtLeast(n)`；`AtLeast(n)` 在构建期校验。
  阈值只针对单个 tick，不跨 tick 累计。
- `MemParallel` 在一次激活内累计终态，结论确定后复位；适合不同子节点在不同 tick
  完成的情况。
- `RandomSelector` 与 `WeightedRandomSelector` 在相同 seed、相同 tick 序列下可复现；
  `reset()` 恢复初始 RNG。
- `UtilitySelector` 的评分闭包只读 `Context`；`NaN` 视为最低分，同分取靠前分支。
- `Race` 同一 tick 多个终态时按子节点顺序取第一个。
- `Switch` 用 typed 分支构造，并通过 `.raw(Box::new(...))` 挂载：
  ```rust
  let switch = Switch::new("phase", &PHASE, vec![
      (1, phase_one),
      (2, phase_two),
  ]);
  ```
  key 改变时会先 abort 仍 Running 的旧分支，再 tick 新分支。

### 装饰节点

`Inverter`、`AlwaysSucceed`、`AlwaysFail`、`Repeat`、`Retry`、`UntilSuccess`、
`UntilFailure`、`While`、`Probability`、`Cooldown`、`Timeout`、`Limiter`、`Throttle`、
`Guard`。

- `Cooldown` 只在子节点成功后开始计时。
- `Timeout` 只对 Running 计时；`duration = 0` 时首次 tick 直接失败，不 tick 子节点。
- `Retry`、`UntilSuccess`、`UntilFailure` 每 tick 最多尝试一次。
- `Repeat` 单 tick 最多推进 `MAX_REPEAT_STEPS_PER_TICK` 次。
- `Limiter` 是“每次激活”的配额；全局 reset 或 `abort → reset` 后清零。
- `Throttle` 按时间窗口限制终态次数；Running 不消耗配额。
- `Guard` 条件为假时直接失败，并 abort 之前 Running 的子节点；它是反应式
  `condition → child` 的直接形式。
- `While` 每个 tick 最多执行一次子节点；子节点成功后返回 `Running`，条件为假时
  返回 `Success`，并先 abort 仍为 Running 的子节点。
- `Probability` 每次激活用确定性种子掷一次概率；子节点 Running 期间保持同一决定，
  下次激活重新掷骰。

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

### 子树契约

`SubtreeContract` 为子树声明必需输入与输出 key：

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

- 缺少必需 key 时通过 Tracer 报错并返回 `Failure`
- 每次新激活开始时清理声明的输出 key
- abort 契约会 abort Running 子节点并清理输出
- 终态后输出保留，父节点可以读取
- 它是校验/生命周期边界，不是独立的黑板命名空间

### 作用域黑板

`ScopedSubtree` 让子树在私有 `Blackboard` 上运行：

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

- 激活时，声明的输入被复制到全新的作用域黑板，声明的输出从父黑板移除。
- 子树进入终态时，声明的输出复制回父黑板。
- 子节点写入但未声明为输出的 key 不会泄漏到父黑板。
- 输入在激活时快照；若要让子节点看到新值，请在下次激活前修改父黑板。
- 作用域子树不接收 Tracer，因此内部 `Enter`/`Exit` 事件不会产生；
  `ScopedSubtree` 节点本身仍会出现在父级 trace 中。
- 快照会恢复子节点状态，但作用域黑板只存在于运行时；恢复后的下一次 tick
  会重新复制输入。

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

- `Tracer` 接收 `Enter` / `Exit` / `Abort` / `Error` 事件，携带子索引路径与可选的
  稳定 `NodeId`。
- `MultiTracer` 将事件按注册顺序广播。
- `StatsTracer` 按路径统计 tick、终态、abort、错误、最后状态与 wall-clock 耗时，
  需要 `std`；可用 `node_by_id` 按稳定 ID 查询，或用 `render_prometheus()` 导出
  可抓取的 Prometheus 文本。
- `last_status()` / `is_running()` 暴露上一次顶层结果。
- `describe()` 输出 ASCII 树、Graphviz DOT 或 Mermaid `graph TD`；builder/配置中的
  `NodeId` 会出现在 `TreeDescription::id` 和图中。
- `StatsTracer::render_tree(&tree)` 输出带每个节点最后状态的 ASCII 树。

### HTML inspector

启用 `inspector` feature 后，可以生成自包含 HTML 报告，并通过本地 HTTP 端点实时查看：

```rust
use playact::inspector::{Inspector, InspectorServer};

// 静态报告
Inspector::new(&tree)
    .with_stats(&stats)
    .write_to("tree.html")?;

// 实时报告：每帧 poll 一次，然后打开 http://127.0.0.1:7878/
let mut server = InspectorServer::bind("127.0.0.1:7878")?;
server.poll(&tree, Some(&stats))?;
```

页面展示树结构、各节点最后状态、计数与耗时、服务节点、带结构路径的 `NodeId` 审计
表与 Mermaid 源码，仅使用内联 CSS。`/metrics`、`/tree`、`/health` 分别提供
Prometheus 文本、ASCII 状态树与就绪探针。服务器没有鉴权，并会暴露树元数据；请只
绑定 loopback，并仅在 debug 构建中使用。

### 树差异

`TreeDiff::between(&old, &new)` 按稳定 `NodeId` 比较两棵树：

```rust
let diff = TreeDiff::between(&old_tree, &new_tree);
println!("{diff}");
```

- `added` / `removed` 报告 `NodeId`、名称、类型与 `NodePath`
- `moved` 报告旧路径与新路径
- 主树与配置中的 services 共享同一个 ID 命名空间
- 无 ID 节点只参与路径计算，不进入报告
- 同一路径上的名称/类型变化不会进入报告

热重载与远程刷新通过 `last_diff()` 暴露最近一次差异。

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
如果树结构可能变化，可为节点分配 ID 并使用与顺序无关的快照：

```rust
let snapshot = tree.snapshot_by_id()?; // NodeIdSnapshot
let mut rebuilt = build_reordered_tree();
rebuilt.restore_by_id(&snapshot)?;
```

内建的有状态节点实现了 `save_own_state` / `load_own_state` 与 `children_mut`，
因此即使子节点重排，也会按 ID 恢复自身进度。自定义节点需要实现相同方法才能参与。
ID 快照不会遍历 `SubtreeSlot` 的动态子节点或嵌套树的 services。

如果新树可能改名或删除 ID，使用 `restore_by_id_matching`：它只应用仍存在 ID 的
条目，忽略其余条目，并返回恢复的节点数量。builder 与 `TreeSpec` 构建都会自动拒绝重复 ID；通过 `BehaviorTree::new` 手工组装的
树可用 `BehaviorTree::duplicate_node_id()` 或 `duplicate_node_id_path()` 检查，
后者返回类似 `root.children[1]` 的 `NodePath`。

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

### Trace 录制与 replay 对比

`trace-record` feature 保存 owned trace 事件，并报告两次运行的首个差异：

```rust
use playact::TraceRecorder;

let mut driver = TestDriver::new(tree, world);
let recorder = driver.install_trace_recorder();
let _ = driver.tick_n(100);
let expected = recorder.records();

// 之后使用相同的确定性 world 场景：
let mut driver = TestDriver::new(build_tree(), same_world);
let recorder = driver.install_trace_recorder();
let _ = driver.tick_n(100);
if let Some(diff) = recorder.diff(&expected) {
    eprintln!("behavior diverged at event {}: {diff:?}", diff.index);
}
```

它记录的是树的可见行为，不是 world。复现时需要提供相同的 world 输入，并用节点
快照 / `BlackboardCodec` 处理有状态数据。事件是 owned 的，可保存用于回归测试。

## 配置驱动

可选 `config` feature 从数据构建行为树，同时保持参数类型安全：

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

- JSON、TOML、YAML、RON 解析器随对应 feature 提供。
- 配置拒绝未知字段；错误路径形如 `root.children[0]`。
- 支持 sequence、selector、随机/加权/效用选择、race、parallel、mem_parallel、
  `switch`、`if_then_else`、装饰器（含 `guard`、`while`、throttle 与 probability）、
  wait、`wait_until`、`wait_for_event`、`set_value`、action、condition、
  `try_action`、`async_action` 与 custom 工厂。
- `try_action` 注册返回可失败的 tick 闭包；错误通过 tracer 上报并转为 `Failure`。
  `async_action` 注册返回 `AsyncTask` 启动器，可接线程、Tokio 或自定义远程任务。
- `condition` 注册同时提供 `guard`、`while`、`if_then_else`、`wait_until` 可用的
  谓词；如果谓词不需要作为 condition 叶子出现，可用 `registry.predicate` 注册。
  谓词参数与 action 参数一样在构建期反序列化为具体类型。
- `switch`、`set_value`、`wait_for_event` 配置引用
  `registry.switch_key` / `set_value_key` / `event_key` 注册的 typed key；配置值在
  构建期反序列化为 key 的 value 或 event 类型。
- `utility_selector` 的分支引用 `registry.score` 注册的评分函数；评分函数接收
  类型化参数，并在每个 tick 重新计算。
- 构建出的根节点与服务中存在重复 `NodeId` 时，构建会直接失败，并给出第二次出现的
  配置路径。

`TreeSpec.services` 声明在每个 tick 的根节点之前运行的服务节点：

```json
{
  "root": {"type": "sequence", "children": []},
  "services": [{"type": "action", "name": "sense_enemies"}]
}
```

服务共享 world、黑板与时间，返回值被忽略。热重载与远程刷新会连同根节点一起重建
服务；启用 `with_state_preservation` 后，ID 匹配的服务节点状态也会恢复。
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
    if let Some(diff) = watcher.last_diff() {
        println!("{diff}");
    }
}
```

成功重建会替换旧树（根节点与配置中的服务）、保留黑板数据并重置节点状态。启用
`with_state_preservation(true)` 后，还会按稳定的 `NodeId` 为仍然存在的节点恢复
自身状态；被改名或删除的 ID 从 reset 状态开始。`last_diff()` 按 `NodeId` 与
`NodePath` 报告新增、删除和移动的节点。解析/构建失败保留旧树并返回错误。默认
200ms 去抖。include 使用 `json_with_resolver` / `with_parser_and_resolver`。

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
保留旧树。成功刷新会重建根节点与配置中的服务、保留黑板并重置节点状态；
`with_state_preservation(true)` 还会按稳定 `NodeId` 恢复仍存在节点的状态，与热重载
行为一致。`last_diff()` 提供与热重载相同的 `TreeDiff`，列出新增、删除和移动的
节点。请求在调用线程同步执行；高延迟环境请放到工作线程或设置 Client 超时。
include 使用 `json_with_resolver` / `with_client_and_resolver`。

响应体限制为 `MAX_REMOTE_CONFIG_BYTES`（1 MiB），必须是 UTF-8，并在构建前通过
`TreeSpec::validate`（默认 `ValidationLimits`）校验；过大或结构过深的配置会被拒绝。

可以用 `ResponseVerifier` 在解析前执行签名、MAC 或 allowlist 校验：

```rust
struct SignatureVerifier;

impl playact::ResponseVerifier for SignatureVerifier {
    fn verify(
        &self,
        headers: &playact::remote_config::reqwest::header::HeaderMap,
        body: &[u8],
    ) -> Result<(), String> {
        // 校验 header 中对 body 的 Ed25519/HMAC 签名
        Ok(())
    }
}

let mut loader =
    RemoteTreeLoader::<World>::json_with_verifier(url, registry, SignatureVerifier)?;
```

首次请求与每次刷新都会执行校验；未通过校验的 payload 不会进入解析。

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

### 派生自定义节点

`#[derive(BtNode)]` 为自定义结构体生成 `Node` 样板代码。结构体提供一个同名
`tick` 方法：

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

- `world = Type` 必填，可以是具体类型或结构体泛型参数
- `name` / `kind` 覆盖默认值（结构体名）
- `#[bt(id)]` 接受 `NodeId` 或 `Option<NodeId>`
- `#[bt(children)]` 生成 `children` / `children_mut`
- 结构体级 flag `abort`、`reset`、`own_state`、`state` 会委托给同名 inherent
  方法

## no_std

关闭默认 feature 后，核心使用 `alloc`：

```toml
[dependencies]
playact = { version = "0.1", default-features = false }
```

核心可用：节点、`BehaviorTree`（含 services 与快照）、builder、`Blackboard`
（内部 `BTreeMap`）、组合/装饰/叶子、`CommandBuffer`、`SubtreeFactory`、
`SubtreeSlot`、`SubtreeContract`、`ScopedSubtree`、`EventQueue` / `EventKey`、
`WaitUntil` / `WaitForEvent` / `TryAction`、`MultiTracer` 与 `describe`。

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
| `trace-record` | owned `TraceRecord` / `TraceRecorder` 与 trace 对比（依赖 `std`） |
| `test-util` | 虚拟时间与 run-until 辅助的 `TestDriver`（依赖 `std` + `trace-record`） |
| `inspector` | 带实时状态/耗时的 HTML 报告与零依赖调试 HTTP 服务（依赖 `std`） |
| `toml` | TOML 解析器 |
| `yaml` | YAML 解析器（`serde_yaml_ng`） |
| `ron` | RON 解析器 |
| `tracing` | `TracingTracer` |
| `hot-reload` | `TreeWatcher` + `notify` 文件监听 |
| `remote-config` | `RemoteTreeLoader` + `reqwest::blocking` |
| `tokio` | `TokioTask` 适配器 |
| `macros` | `playact-macros` 的 `bt!` DSL 与 `BtNode` derive |
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
cargo run -p playact --example battle_ai_showcase --features hot-reload
cargo run -p playact --example live_inspector --features inspector
```

- `battle_ai`：确定性的 1D 玩家 vs 三只哥布林战斗，展示 ReactiveSelector 打断、
  Cooldown、Timeout、Inverter、Tracer、abort 清理与 CommandBuffer/Executor 分离。
- `battle_ai_config`：同一套思路的 JSON 配置驱动版本，展示 typed 参数与 custom
  节点工厂。
- `battle_ai_showcase`：完全配置驱动的 AI，覆盖 services、`utility_selector`、
  `guard`、`try_action`、事件与状态保持热重载。
- `live_inspector`：在 `http://127.0.0.1:7878/` 提供实时状态、耗时、`/metrics`
  与 `/tree` 端点的演示树。

## 测试与基准

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --all-features --examples
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
