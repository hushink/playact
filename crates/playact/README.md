# playact

> 面向游戏自动化的类型安全、可扩展行为树引擎。

`playact` 只负责“决策编排”：组合节点与装饰节点只流转 `Status`，执行节点由使用方提供。引擎不定义任何业务世界、命令或动作类型，因此同一个 crate 可以同时用于战斗 AI、挂机脚本、任务流程和机器人控制。

## 设计红线

- **业务无关**：引擎只认识 `Status`、`Node`、`Context<W>`、`Blackboard`、`Time`、`Tracer`。`W`（世界）、`C`（命令）、`P`（配置参数）都由使用方提供。
- **类型安全**：黑板使用 `Key<T>`，公共 API 不做字符串查表、不暴露 `Any`；配置参数在构建期反序列化成具体类型，tick 期不再有 `serde_json::Value`。
- **构建期尽量失败**：根节点多个子节点、装饰器子节点数量不对会直接编译失败；配置错误返回带路径的 `ConfigBuildError`。
- **同步、可预测**：tick 内不 sleep、不引入 async runtime、不 `catch_unwind`；长动作返回 `Running`，由下一帧继续。
- **可中断**：Running 子树被抢占时先 `abort` 做外部清理，再 `reset` 清内部状态。
- **可裁剪**：关闭默认 `std` feature 后核心引擎以 `alloc` 编译；std 专有集成按 feature 门控。

## 快速开始

```rust
use playact::prelude::*;

fn main() {
    let mut tree = BehaviorTreeBuilder::<()>::new("demo")
        .reactive_selector(|b| {
            b.sequence(|b| {
                b.condition("ready", |_| true)
                    .action("work", |_| Status::Success)
            })
            .action("idle", |_| Status::Running)
        })
        .build()
        .expect("valid tree");

    let mut world = ();
    assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
}
```

`BehaviorTreeBuilder` 使用闭包作用域与 typestate：

- 根节点必须恰好一个子节点；
- 装饰节点的闭包必须恰好产生一个子节点；
- 组合节点允许空（空 `Sequence`/`Parallel` 成功，空 `Selector` 失败）。

不需要 `.end()`，也不会因为闭合顺序错误在运行时 panic。

## 核心概念

### Status

```rust
pub enum Status {
    Success,
    Failure,
    Running,
}
```

这是节点之间唯一的控制流协议。业务错误由动作自己映射成 `Failure`，诊断信息走 `Tracer`，不污染组合逻辑。

### Node

```rust
pub trait Node<W>: Send {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status;
    fn abort(&mut self, _ctx: &mut Context<'_, W>) {}
    fn reset(&mut self) {}
    fn name(&self) -> &str;
    fn kind(&self) -> Kind;
    fn children(&self) -> &[BoxedNode<W>];
}
```

### 组合语义

| 节点 | 语义 |
|---|---|
| `Sequence` | 记忆式：成功后前进，Running 停留，Failure 整体失败 |
| `ReactiveSequence` | 每 tick 从头评估；旧 Running 子节点被抢占时 abort |
| `Selector` | 记忆式：失败后尝试下一个，Running 停留 |
| `ReactiveSelector` | 每 tick 从头评估；高优先级 Success/Running 抢占并 abort 低优先级分支 |
| `RandomSelector` | 带记忆；需要新分支时用确定性 seed 打乱顺序，Running 分支续跑 |
| `WeightedRandomSelector` | 带记忆 + 权重；按权重无放回抽样，0 权重永不选择，Running 分支续跑 |
| `Parallel` | 全部子节点 tick 后按成功/失败阈值汇总；失败优先；结论确定后 abort 剩余 Running |
| `MemParallel` | 跨 tick 记住终态；已 Success/Failure 的子节点不再 tick，阈值累计 |
| `Race` | 所有子节点并行推进；第一个终态获胜，abort 其余 Running；空 Race 为 Failure |
| `UtilitySelector` | 每 tick 按动态评分选最高分；Running 分支带 hysteresis 粘性，可被高分分支打断 |
| `IfThenElse` | 条件为真执行 then，否则执行 else；切换分支时 abort 旧的 Running 分支 |

`Parallel` 的阈值按**单个 tick 的汇总**计算，不跨 tick 累计终态：不同 tick 分别成功的子节点不会让 `All` 自动满足；`Any` 每次成功都会 abort 其余仍在 Running 的兄弟。需要累积语义请用 `MemParallel`。

`MemParallel` 在一次激活内记住每个子节点的终态，直到成功/失败阈值满足；结论确定后 abort 剩余 Running，并复位到下一次激活。

`RandomSelector` 的 `seed` 决定随机顺序；相同 seed + 相同 tick 序列可复现，`reset()` 恢复初始 RNG。

`WeightedRandomSelector` 用 `WeightedChild::new(weight, child)` 构造，也可用 builder `.weighted_random_selector(children, seed)`；配置层对应 `weights` 数组（省略时全部按 1 处理）。所有分支权重为 0 时直接 `Failure`。

`Race` 在同一 tick 内多个子节点返回终态时按子节点顺序取第一个；空 `Race` 为 `Failure`。

`UtilitySelector` 用 `UtilityChild::new(score, child)` 构造，评分闭包只读 `Context`、每 tick 重新计算，`NaN` 视为最低分；`with_hysteresis` 设置切换所需的分数优势，负值按 0 处理。评分是业务逻辑，因此不进入配置文件。

`Parallel` 阈值支持 `All`、`Any`、`AtLeast(n)`；`AtLeast(n)` 在构建期校验不超过子节点数量。

### 装饰节点

`Inverter`、`AlwaysSucceed`、`AlwaysFail`、`Repeat`、`Retry`、`UntilSuccess`、`UntilFailure`、`Cooldown`、`Timeout`、`Limiter`、`Throttle`、`Guard`。

`Guard` 每 tick 检查条件，条件为假直接 `Failure`；若子节点上次仍在 Running，会先 `abort → reset`。它等价于常用的 `ReactiveSequence(condition, child)` 形态，但语义更直接。

- `Cooldown` 只在子节点成功后开始计时；
- `Timeout` 只对 Running 计时，超时 abort 子节点；`duration = 0` 时首次 tick 直接 Failure，不 tick 子节点；
- `Retry` 每 tick 最多尝试一次，避免同一帧刷副作用；
- `UntilSuccess`/`UntilFailure` 每 tick 只尝试一次，不会自旋；
- `Repeat` 单 tick 最多推进 `MAX_REPEAT_STEPS_PER_TICK`（默认 1024）次，超出返回 `Running`，不会因超大 `times` 卡死逻辑帧；
- `Limiter` 是“每次激活”的配额，全局 `reset` 或抢占后的 `abort→reset` 会清零；跨抢占配额请放黑板；
- `Throttle` 按时间窗口限制终态结果次数，`Running` 不消耗配额；`window = 0` 等价于不限流。

### 叶子

`Action`、`Condition`、`Wait`、`SetValue`、`AsyncAction`。动作是 `FnMut(&mut Context<W>) -> Status`；条件只读 `&Context<W>`。

`SetValue<T>` 每次 tick 把固定值写入 typed key 并返回 `Success`（`T: Clone`）；builder 提供 `.set_value(name, &KEY, value)`。

`AsyncAction` 用运行时无关的 `AsyncTask` 句柄表达异步/远程工作：tick 内只 `poll`，被抢占时 `cancel`。内置 `ThreadTask` 用标准库线程跑阻塞/远程调用，`CancellationToken` 做协作取消。

启用 `tokio` feature 后提供 `TokioTask`：

```rust
use playact::prelude::*;

let action = AsyncAction::<World>::new("fetch", |_ctx| {
    Box::new(TokioTask::spawn(async move {
        // let result = client.call().await;
        Status::Success
    })) as Box<dyn AsyncTask>
});
```

- `TokioTask::spawn` 使用当前 Tokio runtime；不在 runtime 内时返回一个立即 `Failure` 的任务，不会 panic；
- `TokioTask::spawn_on(&handle, future)` 可显式指定 runtime handle；
- `cancel` / drop 调用 `JoinHandle::abort`，在 await 点取消 future；
- future 返回 `Status`，业务错误由 future 自己映射。

需要被抢占时清理按键/技能时：

```rust
let action = Action::new("hold_key", |_| Status::Running)
    .with_abort(|_ctx| {
        // 释放按键
    })
    .with_reset(|| {
        // 清内部状态
    });
```

在 builder 中，带 `abort`/`reset` 回调的动作通过 `.raw(Box::new(action))` 挂载。

## 类型安全黑板

```rust
use playact::prelude::*;

static TARGET: Key<usize> = playact::key!("target");

let mut board = Blackboard::new();
board.set(&TARGET, 42_usize);

assert_eq!(board.get(&TARGET), Some(&42));
// board.get::<u64>(&TARGET)  // 编译错误
```

- `Key<T>` 携带类型；同名不同 `T` 是不同槽位；
- 黑市值要求 `Send + 'static`，树可以移到线程池；
- 不提供 `DynKey` 或运行时字符串查表。

跨节点共享数据用黑板；决策产生的命令可以用业务侧定义的 `CommandBuffer<C>` 放进世界或黑板：

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

动作只读世界、只写命令，主循环再交给执行器，这就是“决策/执行分离”。引擎本身不解释 `BattleCommand`。

## 共享黑板

默认每棵树持有自己的 `Blackboard`；需要多个 AI / 多棵树共享 typed 数据时用 `SharedBlackboard`（`Arc<Mutex<Blackboard>>`，`std` 功能，默认开启）：

```rust
use playact::prelude::*;

static TARGET: Key<usize> = playact::key!("target");

let shared = SharedBlackboard::new();

// 树 A 写入目标
let mut scout = BehaviorTree::new(Box::new(Action::<World>::new("scout", |ctx| {
    ctx.blackboard().set(&TARGET, 3);
    Status::Success
})));

// 树 B 读取同一个目标
let mut attack = BehaviorTree::new(Box::new(Condition::<World>::new("has_target", |ctx| {
    ctx.blackboard_ref().contains(&TARGET)
})));

// world / time 由驱动方提供
shared.tick(&mut scout, &mut world, time);
shared.tick(&mut attack, &mut world, time);

// 树外读写
shared.with(|board| board.get(&TARGET).copied());
shared.with_mut(|board| board.set(&TARGET, 7));
```

- `tick`/`abort` 在整个树 tick 期间持锁，多个树/线程的 tick 会被串行化；
- 节点内部直接用 `ctx.blackboard()` / `ctx.blackboard_ref()`，**不要**在动作里再 lock 同一个 `SharedBlackboard`，否则会死锁；
- 使用共享黑板时树自带的私有黑板不参与本次 tick，但 Tracer 与节点内部状态仍正常；
- `lock()` 返回 `MutexGuard`，锁中毒会取回内部数据继续使用；`try_lock()` 在锁被占用时返回 `None`；`into_inner()` 在引用唯一时取回 `Blackboard`；
- 共享黑板只共享数据，不共享 world：每次 tick 仍借用各自的 `&mut W`。

## Context 与时间

```rust
let mut ctx = Context::new(&mut world, &mut blackboard, Time::from_millis(1000, 250));
let _world: &mut World = ctx.world();
let _board: &mut Blackboard = ctx.blackboard();
let _time: Time = ctx.time();
```

`Time` 由驱动方提供，`Cooldown`/`Timeout`/`Wait` 只依赖它，因此测试可以用虚拟时间完全确定性推进。

`BehaviorTree::reset()` 清除节点内部状态并清除上次 tick 记录；`BehaviorTree::abort(world, time)` 只在**上一次 tick 返回 Running** 时传播 `abort→reset`，终态树没有需要清理的 Running 子树。

`now` 必须单调递增；如果驱动方重置时钟（战斗重开/重连），先调用 `reset()` 或 `abort()`，避免 Cooldown 等绝对时间节点看到时间回退。

## 异步 / 远程动作

`AsyncAction` 依赖 `std`（默认开启），no_std 核心不包含线程任务；不绑定 async runtime，tick 内绝不阻塞：

```rust
use playact::{AsyncAction, AsyncTask, Status, ThreadTask};

let action = AsyncAction::<World>::new("fetch", |ctx| {
    let target = /* 从 ctx.world_ref()/黑板克隆所需数据 */;

    Box::new(ThreadTask::spawn(move |cancel| {
        if cancel.is_cancelled() {
            return Status::Failure;
        }
        // 阻塞调用 / 远程请求
        Status::Success
    })) as Box<dyn AsyncTask>
});
```

- `AsyncTask::{poll, cancel}` 是运行时无关契约；Tokio/futures/RPC 客户端可实现它；
- `ThreadTask` 在线程中执行 `work`，tick 只用 channel `try_recv`；
- `AsyncAction` 在被抢占/复位时调用 `cancel` 并 drop 任务；
- 取消是协作式的：线程/远端任务应检查 `CancellationToken`；引擎不会强杀线程；
- 任务只返回 `Status`，远端结果由业务写入共享状态/黑板/命令通道；
- 配置层可通过 `custom` 工厂返回 `AsyncAction`，不新增配置节点类型。

## Tracer 与 describe

```rust
let mut tree = /* ... */;
tree.set_tracer(Some(Box::new(MyTracer)));

println!("{}", tree.describe());
println!("{}", tree.describe().to_dot());
```

`Tracer` 接收 `Enter`/`Exit`/`Abort` 事件，`path` 是子索引路径（根为 `[]`）。默认未安装 Tracer 时零成本。`tracing` feature 提供 `TracingTracer` 适配器。

需要统计或多个观察者时：

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

- `StatsTracer` 记录每个路径的 tick/终态/abort 次数与 wall-clock 耗时，需要 `std`；
- `MultiTracer` 按注册顺序 fan-out，事件本身是 `Copy`；
- `last_status()` / `is_running()` 供驱动方读取上一次顶层结果，`reset` / `abort` 后为 `None`。

`AbortReason` 取值为 `Preempted`、`Timeout`、`ParallelDone`、`RaceDone`、`TreeAbort`，只出现在 Tracer 事件中，不进入 `Node::abort` 签名。

## 状态快照

`BehaviorTree::snapshot()` / `restore()` 保存和恢复内建节点的内部状态：

```rust
let snapshot = tree.snapshot();
// snapshot.version / snapshot.root / snapshot.services 都是可持久化的数据

let mut rebuilt = /* 用相同配置重建的树 */;
rebuilt.restore(&snapshot)?;
```

- 覆盖 Sequence / Selector / 并行 / 竞速 / 效用 / 装饰器 / Wait 等内建节点，以及嵌套树的服务；
- 不包含 world、黑板值、闭包捕获状态、`AsyncAction` 任务、`SubtreeSlot` 动态子树与 Tracer；
- 自定义节点可实现 `Node::save_state` / `load_state`；组合节点需要递归编码子树状态；
- 快照带 `SNAPSHOT_VERSION`；结构或服务数量变化会返回 `RestoreError`；
- 时间型节点保存的是驱动方 `Time.now` 的绝对时刻；恢复时应继续同一时间轴，或先 `reset` 后重新建立；
- 恢复会先 reset 根与服务；恢复失败时树可能处于部分恢复状态。

## 配置驱动

启用 `config` feature 后，可以从 JSON/TOML/YAML/RON 构建行为树：

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

- 配置只描述树结构、节点名和叶子参数；
- 支持 `random_selector` / `weighted_random_selector`（`seed` 必填，`weights` 与 `children` 对应，省略时全为 1）、`race`、`throttle`、`mem_parallel` 与 `custom` 子节点；
- 叶子参数在构建期解析为具体 `P`，tick 期没有动态值；
- 配置结构拒绝未知字段（`deny_unknown_fields`），拼错的键不会被静默忽略；
- 省略 `params` 等价于 `null`：无参 action/condition 用 `P = ()` 并省略 `params`；带字段的 struct 参数请显式写 `"params": {}`；
- 未知 action/condition、参数错误、版本不支持、阈值非法都返回带 `root.children[i]...` 路径的 `ConfigBuildError`；
- 需要 `abort`/`reset` 清理的配置动作，用 `custom` 工厂返回带回调的 `Action` 或自定义节点；
- `Guard` / `IfThenElse` / `UtilitySelector` 的条件与评分属于业务逻辑，不在配置 schema 中，用代码或 `custom` 工厂构建；
- 配置不定义业务世界、命令或黑板 key。

示例 JSON 见 [`examples/battle_ai.json`](examples/battle_ai.json)。

### 配置 include

`include` 节点把其他配置引入当前树，需要显式提供解析器：

```rust
use playact::config::FsIncludeResolver;

let resolver = FsIncludeResolver::json("ai/")?;
let tree = spec.build_with_resolver(&registry, &resolver)?;
```

```json
{ "root": { "type": "include", "path": "sub.json", "name": "patrol" } }
```

- `IncludeResolver` 是 trait，闭包 `Fn(&str) -> Result<TreeSpec, ConfigError>` 自动实现；
- `FsIncludeResolver` 把路径限制在基准目录内，拒绝 `..`、绝对路径和符号链接逃逸，并校验目标是文件；
- 构建期检测 include 循环；嵌套深度上限 `MAX_INCLUDE_DEPTH = 32`；
- 没有配置 resolver 时使用 `include` 会返回明确的 `ConfigBuildError`。

## 配置热重载

启用 `hot-reload` feature（依赖 `notify`，但仍无 async runtime）：

```rust
use playact::hot_reload::TreeWatcher;

let mut watcher = TreeWatcher::<World>::json("ai.json", registry)?;

loop {
    // 处理文件事件；成功后新树替换旧树
    if watcher.poll()? {
        println!("reloaded: {}", watcher.tree().describe());
    }

    // 旧树在重载失败时仍然有效
    watcher.tree_mut().tick(&mut world, time);
}
```

语义：

- 解析/构建成功：黑板数据保留，节点内部状态重置；Tracer 不保留，可重载后通过 `tree_mut()` 重新安装。
- 解析/构建失败：旧树保持不变，返回 `HotReloadError`；修复文件后再触发一次事件即可重试。
- 默认 200ms 去抖，可 `with_debounce(Duration)` 调整。
- 监听目录并通过文件名过滤目标文件，兼容编辑器“写临时文件再 rename”的保存方式。
- `TreeWatcher::toml` / `yaml` / `ron` 随对应 feature 提供；也可用 `with_parser` 接入自定义格式。
- `TreeWatcher::tick` 在重载失败时不 tick；需要继续旧树时直接使用 `tree_mut().tick`。
- include 用 `TreeWatcher::json_with_resolver` / `with_parser_and_resolver`；resolver 在每次重载时重新执行。
- 监听器只关注根文件：被 include 的文件单独变化不会触发事件，可调用 `reload_now()` 或触碰根文件。

## proc-macro DSL

启用 `macros` feature 后可用 `bt!` 展开为类型安全的 builder 调用：

```rust
use playact::prelude::*;

let tree = playact::bt!(World, "player", reactive_selector {
    sequence {
        condition("low_hp", |ctx: &Context<'_, World>| ctx.world_ref().hp < 0.3),
        action("drink", |_| Status::Success),
    },
    action("patrol", |_| Status::Running),
})?;
```

- 组合：`sequence`、`reactive_sequence`、`selector`、`reactive_selector`、`random_selector(seed)`、`parallel(success, failure)`、`mem_parallel(success, failure)`、`race`；
- 装饰：`inverter`、`always_succeed`、`always_fail`、`repeat(n)`、`retry(n)`、`until_success`、`until_failure`、`cooldown(duration)`、`timeout(duration)`、`limiter(n)`、`throttle(window, max)`、`guard(condition)`，块内必须恰好一个子节点；
- 叶子：`action(name, closure)`、`condition(name, closure)`、`wait(duration)`、`set_value(name, key, value)`、`raw(expression)`；
- 未知节点、叶子带 body、装饰器子节点数量不对都会在宏展开期报错；最后仍走 builder 的 typestate 检查；
- `bt!` 返回 `Result<BehaviorTree<W>, BuildError>`。

## 远程配置

启用 `remote-config` feature（使用 `reqwest::blocking`）：

```rust
use playact::remote_config::RemoteTreeLoader;

let mut loader = RemoteTreeLoader::<World>::json("https://example.com/ai.json", registry)?
    .with_refresh_interval(Duration::from_secs(30));

loop {
    loader.poll()?; // 到间隔才发起请求，304 不重建
    loader.tree_mut().tick(&mut world, time);
}
```

- 初次加载失败直接返回错误；之后刷新失败保留旧树；
- 支持 ETag / Last-Modified 条件请求，304 返回 `false`；
- 刷新成功保留黑板数据、重置节点内部状态；Tracer 不保留；
- `with_client` 可配置代理、超时、TLS；`json`/`toml`/`yaml`/`ron` 随 feature 提供；
- 请求在调用 `poll`/`tick` 的线程同步执行，高延迟环境请放到工作线程，或给 Client 设置超时；
- 不暴露 async runtime；`reqwest::blocking` 的内部运行时不进入 playact API；
- include 用 `RemoteTreeLoader::json_with_resolver` / `with_parser_and_resolver`；304 不会重建树，include 内容变化需要 ETag/Last-Modified 变化或手动重新加载。

## Feature 矩阵

| feature | 内容 |
|---|---|
| 默认 | `std`，纯标准库，零第三方依赖 |
| `std` | 标准库集成：`SharedBlackboard`、`AsyncAction`、配置/热重载/远程配置、`TracingTracer`；关闭后核心以 `no_std + alloc` 编译 |
| `config` | `TreeSpec`、`NodeRegistry`、JSON 解析（serde/serde_json/serde_path_to_error，依赖 `std`） |
| `toml` | `from_toml_str` |
| `yaml` | `from_yaml_str`（serde_yaml_ng） |
| `ron` | `from_ron_str` |
| `tracing` | `TracingTracer`（依赖 `std`） |
| `hot-reload` | `TreeWatcher` + `notify` 文件监听（依赖 `config`） |
| `remote-config` | `RemoteTreeLoader` + `reqwest::blocking` HTTP 配置（依赖 `config`） |
| `tokio` | `TokioTask`：在 Tokio runtime 上运行 async 任务（依赖 `std`） |
| `macros` | `playact-macros` 的 `bt!` proc-macro DSL（no_std 可用） |
| `full` | 以上全部（含 `std`） |

> 发布顺序：`playact` 的 `macros` feature 依赖同版本 `playact-macros`。需要先发布 `playact-macros`，再打包/发布 `playact`；否则 `cargo package -p playact` 会因 registry 中找不到该依赖而失败。

## no_std

关闭默认 `std` feature 后，核心引擎使用 `alloc` 编译：

```toml
[dependencies]
playact = { version = "0.1.1", default-features = false }
```

可用：`Node`、`BehaviorTree`（含 services 与状态快照）、builder、`Blackboard`（内部使用 `BTreeMap`）、组合/装饰/叶子节点、`CommandBuffer`、`SubtreeFactory`、`SubtreeSlot`、`MultiTracer`、`Tracer` 与 `describe`。

需要 `std`：`SharedBlackboard`、`AsyncAction`/`ThreadTask`/`TokioTask`、`StatsTracer`、配置层、热重载、远程配置、`TracingTracer`。

- `SubtreeFactory` 使用 `alloc::sync::Arc`，目标需支持原子指针；已在 `thumbv7em-none-eabihf` 上验证；
- `macros` 不依赖 `std`，生成的代码只引用 playact；
- 验证命令：

```text
cargo check -p playact --no-default-features --target thumbv7em-none-eabihf --lib
```

## 线程模型

- `Node<W>: Send`，`BehaviorTree<W>: Send`；
- 不要求 `Sync`；
- `W` 不必 `Send`/`Sync`，因为树只在一帧内借用 `&mut W`，不存储它；
- `hot-reload` 的 `notify` 回调只在后台线程发送事件；重建与 tick 仍在调用 `poll`/`tick` 的线程同步执行；
- `remote-config` 的 HTTP 请求在调用线程同步执行；高延迟场景请放到工作线程或自定义 `Client` 超时；
- `AsyncAction` 在 tick 线程 poll；`ThreadTask` 的工作线程只用 channel 回报，取消是协作式的；
- 黑板值要求 `Send + 'static`；
- 每个 AI 持有自己的树和黑板，多 AI 用工厂函数分别构建；共享世界由驱动方顺序 tick 或自行同步。
- `SharedBlackboard` 可跨树/线程共享黑板；`tick` 的锁覆盖整棵树，因此多线程 tick 会被串行化。
- `BehaviorTree::into_root()` 会丢弃黑板与 Tracer，但保留 root 的内部状态；如果 Running 子树依赖黑板数据，挂到新树前要重新准备。

### 子树复用

`SubtreeFactory` 是可克隆的子树蓝图；每次 `instantiate()` / builder `.subtree()` 都创建一份全新的节点状态：

```rust
let goblin = SubtreeFactory::new(|| build_goblin_root());
let tree = BehaviorTreeBuilder::<World>::new("pair")
    .sequence(|b| b.subtree(&goblin).subtree(&goblin))
    .build()?;
```

需要保留 Running 子树依赖的黑板数据时，用工厂重新构建，而不是只 `into_root()`。

整棵 `BehaviorTree` 也实现了 `Node<W>`，可以直接作为节点嵌套：

```rust
let mut tree = BehaviorTreeBuilder::<World>::new("outer")
    .sequence(|b| b.raw(Box::new(inner_tree)))
    .build()?;
```

- 嵌套树共享外层的 world / time / 黑板 / Tracer；它自带的黑板与 Tracer 不参与；
- `describe()` 中显示为 `tree "名字"`，其唯一子节点是原根节点；
- 被抢占时 `abort → reset` 会传播到嵌套树根；
- 需要可重复实例化时仍用 `SubtreeFactory`；`into_root()` 只取根节点，丢弃树外壳与黑板。

`SubtreeSlot` 按 typed key 在运行时切换子树：

```rust
static PHASE: Key<u32> = playact::key!("phase");

let slot = SubtreeSlot::new("phase_slot", &PHASE, |phase| match phase {
    1 => build_phase_one(),
    _ => build_phase_two(),
});

let mut tree = BehaviorTreeBuilder::<World>::new("boss")
    .sequence(|b| b.raw(Box::new(slot)))
    .build()?;
```

- key 不变时保留子树内部状态；key 改变时旧子树 `abort → reset`，工厂创建新子树；
- key 缺失返回 `Failure` 并放弃当前子树；`reset()` 后下次 tick 重新实例化。

### 服务节点

服务在每个 tick 的根节点之前运行，共享 world / time / 黑板，返回值被忽略：

```rust
let mut tree = BehaviorTreeBuilder::<World>::new("player")
    .selector(|b| { /* ... */ })
    .build()?;
tree.add_service(Box::new(sense_enemies));

// 或链式：
let tree = tree.with_service(Box::new(update_cooldowns));
```

- 适合感知、缓存、维护外部状态；不影响 `last_status`；
- 服务子树不产生 Tracer 事件（避免与主树路径混淆）；
- 整棵树 `abort` 时，上次仍 Running 的服务会 `abort`，所有服务随后 `reset`；嵌套树也会运行自己的服务。

## 示例

```text
cargo run --example battle_ai
cargo run --example battle_ai_config --features config
```

- `battle_ai`：1D 战场，玩家 vs 三只哥布林，固定 0.25s 逻辑帧，无随机数；展示 ReactiveSelector 保命打断、Cooldown、Timeout、Inverter、Tracer、abort 清理和 CommandBuffer/Executor 分离。
- `battle_ai_config`：同一套思路的配置驱动精简版，展示 typed params、custom 节点工厂和错误路径。

## 测试与质量

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo bench -p playact
cargo doc --workspace --all-features --no-deps
cargo check -p playact --no-default-features --target thumbv7em-none-eabihf --lib
```

包含模块单测、公共 API 集成测试、doctest 和 `compile_fail` doctest（验证根/装饰器子节点数量、黑板 key 类型错误）。CI 覆盖 stable 与 MSRV 1.92、Linux 与 Windows，并检查嵌入式 no_std 目标。

- `benches/engine.rs` 是零第三方依赖的微基准，覆盖 sequence / selector / reactive 抢占 / parallel / mem_parallel / weighted selector / 嵌套树 / services tick、黑板读写、构建与 describe；输出延迟与吞吐，适合观察回归。
- `tests/stress.rs` 用固定 seed 随机生成组合、装饰、叶子、加权/效用选择、竞速、时间限流、SetValue、嵌套树与前置服务，验证 tick / abort / reset / describe / Send，并检查同 seed 的行为可复现；无需 nightly 或 cargo-fuzz（它不是覆盖率引导的 fuzzer）。

## 0.2 开发线

当前 crate 版本号仍为 **0.1.1**，但已包含 0.2 计划中的：

- `RandomSelector`：带记忆、确定性 seed 的随机选择；
- `SubtreeFactory`：可克隆、可重复实例化的子树工厂；
- `MemParallel`：跨 tick 累计终态的并行节点；
- `hot-reload`：`TreeWatcher` 文件监听、去抖重载、黑板保留、失败回退旧树；
- `remote-config`：`RemoteTreeLoader` HTTP 拉取、间隔刷新、ETag/Last-Modified、304 不重建；
- `AsyncAction` / `AsyncTask` / `ThreadTask`：运行时无关异步动作、线程任务与协作取消；
- `tokio`：`TokioTask` 在 Tokio runtime 上运行 async 任务，取消映射到 `JoinHandle::abort`；
- 状态快照：`Node::save_state` / `load_state`、`BehaviorTree::snapshot` / `restore`；
- `macros`：`bt!` proc-macro DSL，展开为 builder 调用；
- `SharedBlackboard`：基于 `Arc<Mutex<Blackboard>>` 的跨树/跨线程共享黑板；
- `BehaviorTree: Node`：整棵树可直接嵌套，共享外层上下文并传播 abort/reset；
- bench/fuzz：零依赖微基准与确定性随机压力测试（非覆盖率引导的 fuzzing）；
- `StatsTracer` / `MultiTracer`、`last_status()`、`SharedBlackboard::try_lock`：运行统计与多路观测；
- `WeightedRandomSelector`、`Throttle`、`SetValue`：加权随机、时间窗限流与黑板赋值；
- `Race`、`UtilitySelector`：首个终态获胜、动态效用评分与 hysteresis 切换；
- `Guard`、`IfThenElse`：条件守卫与双分支切换；
- `tokio`：`TokioTask` async 任务适配器；
- `SubtreeSlot` 与 Services：运行时子树切换、根节点前的前置服务；
- 配置 include：安全文件解析器、循环/深度保护，并接入热重载与远程加载；
- `no_std`：关闭默认 `std` feature 后核心以 `alloc` 编译，std 集成按 feature 门控。

## License

Apache-2.0
