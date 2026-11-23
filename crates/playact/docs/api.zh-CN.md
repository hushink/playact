# playact API 使用指南

面向任务的 `playact` 0.1.x API 参考：覆盖日常会调用的公开类型与方法，并给出示例代码、使用方式与适用场景。

- [README](../README.zh-CN.md) - 项目概览、feature 与完整配置说明。
- 本文 - API 用法、方法表与场景配方。
- [docs.rs/playact](https://docs.rs/playact) - 逐项 rustdoc，包含全部字段与错误枚举。
- [`examples/`](../examples) - 可直接运行的完整示例。

示例默认：

```rust
use playact::prelude::*;
```

并可能省略 `use std::time::Duration;` 与所在函数的错误处理样板。`W` 是你的世界类型，例如：

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
> 引擎不包含 `WorldState`、业务 `Action` 或命令枚举。这些类型都留在你的 crate 中。
> API 对 `W` 泛型，并通过类型化的 `Key<T>` / `EventKey<T>` 访问状态，而不是字符串查找。

## Feature 一览

大多数 API 始终可用；可选 feature 控制对应模块：

| Feature | 提供内容 |
|---|---|
| 默认（`std`） | `std` 集成；纯标准库、零第三方依赖 |
| `config` | `TreeSpec`、`NodeRegistry`、JSON、include、校验与 schema |
| `toml` / `yaml` / `ron` | 对应格式解析器 |
| `schema` | `TreeSpec::json_schema()` JSON Schema 导出 |
| `hot-reload` | `TreeWatcher`（依赖 `notify`） |
| `remote-config` | `RemoteTreeLoader`（依赖 `reqwest::blocking`） |
| `inspector` | HTML 报告与零依赖 HTTP 服务 |
| `diff-serde` | `TreeDiff` / `DiffHistory` JSON 序列化与解析 |
| `blackboard-serde` | `BlackboardCodec` 类型化黑板持久化 |
| `snapshot-serde` | `TreeSnapshot` / `NodeIdSnapshot` 的 JSON 序列化 |
| `trace-record` | `TraceRecorder` / `TraceRecord` 与 trace 对比 |
| `tracing` | `TracingTracer` 集成 |
| `tokio` | `TokioTask` 异步适配 |
| `macros` | `bt!` 过程宏 DSL 与 `#[derive(BtNode)]` |
| `test-util` | `TestDriver` |
| `full` | 以上全部 |

关闭默认 feature 后，核心使用 `no_std + alloc`。仅 `std` 可用的能力（
`shared`、`stats`、异步 action、配置/热重载/远程配置、inspector）在 `no_std` 下不可用。

## 1. 核心契约

### `Status`

节点之间只通过三个值通信：

| 变体 | 含义 | 辅助方法 |
|---|---|---|
| `Success` | 完成且成功 | `is_success()` |
| `Failure` | 失败且不应继续 | `is_failure()` |
| `Running` | 未完成，下次继续 | `is_running()` |

```rust
let world = World::default();
let status = Status::from_bool(world.hp > 0.0);
assert_eq!(status.is_terminal(), status.is_success() || status.is_failure());
```

`Status::is_terminal()` 对 `Success` / `Failure` 为 true。

### `Node<W>`

唯一必须实现的是 `tick`；其余方法有默认实现，由引擎在需要时调用：

| 方法 | 调用时机 | 典型用途 |
|---|---|---|
| `tick(&mut Context<W>) -> Status` | 每个逻辑帧 | 读写世界/黑板并返回状态 |
| `abort(&mut Context<W>)` | Running 子树被放弃 | 释放锁、停止音效、取消请求 |
| `reset()` | 整树重置或 abort 后 | 清空计时器、下标、缓存目标 |
| `name() -> &str` | 诊断 | 显示名称 |
| `kind() -> Kind` | 诊断/工具 | 固定类型标签或 `Kind::Custom("...")` |
| `id() -> Option<&NodeId>` | 快照/差异/指标 | 稳定身份，通常通过 `.id(...)` |
| `children()` / `children_mut()` | 工具/ID 快照 | 自定义组合节点暴露子节点 |
| `save_own_state()` / `load_own_state()` | ID 快照 | 有状态自定义叶子 |
| `save_state()` / `load_state()` | 结构快照 | 自定义组合/装饰节点 |

自定义叶子示例：

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

通过 `.raw(...)` 挂载：

```rust
let mut tree = BehaviorTreeBuilder::<World>::new("guard")
    .sequence(|b| {
        b.raw(Box::new(WaitForTarget))
            .action("strike", |_| Status::Success)
    })
    .build()?;
```

契约要点：

- 返回 `Success` / `Failure` 后，节点必须能在下次激活时重新开始。
- `abort` 与 `reset` 必须幂等。
- 长任务应返回 `Running` 并在后续帧继续；不要在 `tick` 中阻塞（阻塞/远程工作使用 `AsyncAction`）。
- `Node<W>: Send`；`W` 本身不需要 `Send`，因为树只在 tick 期间借用 `&mut W`。

### `Context<W>`

`Context` 在单次 tick 中借用，向节点提供世界、黑板、时间与子节点辅助方法。

| 方法 | 用途 |
|---|---|
| `world() -> &mut W` | 本帧修改世界 |
| `world_ref() -> &W` | 读取世界 |
| `blackboard() -> &mut Blackboard` | 读写类型化状态 |
| `blackboard_ref() -> &Blackboard` | 只读类型化状态 |
| `time() -> Time` | 当前虚拟时间与帧间隔 |
| `report_error(name, kind, error)` | 发出 `TraceEvent::Error` 并计数错误 |
| `tick_child(index, child)` | 自定义组合：带正确 trace 地 tick 子节点 |
| `abort_child(index, child, reason)` | 自定义组合：带原因 abort 子节点 |

`tick_child` / `abort_child` 用于自定义组合节点，保证 trace 路径、错误路由与 abort 语义与内建组合一致。

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

`Time` 是虚拟时间，由驱动方决定 "now" 的含义。Cooldown、Timeout、Wait、Throttle
都只使用它，因此测试无需 sleep 且完全确定。

## 2. 类型化黑板

`Blackboard` 按类型与静态 key 名称存值。key 类型不匹配是编译错误，而不是运行时 downcast。

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

| 方法 | 用途 |
|---|---|
| `set(&Key<T>, value) -> Option<T>` | 插入/替换，返回旧值 |
| `get(&Key<T>) -> Option<&T>` | 读取 |
| `get_mut(&Key<T>) -> Option<&mut T>` | 原地修改 |
| `get_or_insert_with(&Key<T>, f) -> &mut T` | 懒初始化 |
| `contains(&Key<T>) -> bool` | 是否存在 |
| `remove(&Key<T>) -> Option<T>` | 取出 |
| `clear_key(&Key<T>)` | 删除单个 key |
| `clear()`、`len()`、`is_empty()` | 整板操作 |

`key!` / `event_key!` 宏用于静态 key：

```rust
static TARGET: Key<u32> = playact::key!("combat.target");
static HIT: EventKey<u32> = playact::event_key!("combat.hit");
```

场景：索敌 action 写入 `TARGET`，攻击 condition 读取它；两边都使用 `Key<u32>`，
不可能误读字符串或其他实体类型。

## 3. `BehaviorTree`

树持有根节点、可选 services、黑板与可选 tracer；tick 同步执行。

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

| 方法 | 用途 |
|---|---|
| `new(root)`、`with_name(name)`、`name()` | 构造与命名 |
| `tick(&mut world, time) -> Status` | 用内部黑板运行一帧 |
| `tick_with_blackboard(&mut world, time, &mut board)` | 使用外部黑板 |
| `abort(&mut world, time)` | 放弃 Running 树：先 `abort` 再 `reset` |
| `abort_with_blackboard(...)` | 外部黑板版本 |
| `reset()` | 仅清空节点状态，不调用 `abort` |
| `blackboard()` / `blackboard_mut()` | 检查或播种类型化状态 |
| `last_status()`、`is_running()` | 查看上一帧 |
| `with_service(node)`、`add_service(node)`、`services()`、`service_count()` | 每帧先于根节点运行的 service |
| `set_tracer(...)`、`with_tracer(...)` | 安装可观测性 |
| `describe()` / `describe_services()` | 生成结构快照供 UI/工具使用 |
| `duplicate_node_id()` / `duplicate_node_id_path()` | 手工构建后审计 ID |
| `snapshot()` / `restore(&snapshot)` | 结构状态捕获/恢复 |
| `snapshot_by_id()`、`restore_by_id`、`restore_by_id_matching` | 按 ID 的状态，允许重排 |
| `into_root()` | 把树作为子树根复用 |

abort/reset 语义：

- 仅当上一帧根状态为 `Running` 时才调用节点 `abort`；终态树没有需要清理的 Running 子树。
- services 总会 reset；上一帧返回 `Running` 的 service 会先 abort。
- `reset()` 不会调用 `abort`；若需要外部清理，先调用 `abort`。

Service 是普通节点，每帧先于根节点执行并共享黑板，适合做感知、事件泵或冷却记账：

```rust
let tree = BehaviorTreeBuilder::<World>::new("ai")
    .action("think", |_| Status::Success)
    .build()?
    .with_service(Box::new(MyScanNode)); // any BoxedNode<World>
```

## 4. 构建器

`BehaviorTreeBuilder<W, S>` 用 typestate 把树形错误变成编译错误：空根无法 build，
装饰节点只能有一个子节点，组合节点的子节点由闭包作用域限制。

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

叶子：

| 构建器方法 | 产生的节点 |
|---|---|
| `action(name, FnMut(&mut Context) -> Status)` | `Action` |
| `try_action(name, FnMut(&mut Context) -> Result<Status, E>)` | `TryAction`；`Err` 发 trace error 并返回 `Failure` |
| `async_action(name, FnMut(&mut Context) -> Box<dyn AsyncTask>)` | `AsyncAction` |
| `condition(name, Fn(&Context) -> bool)` | `Condition` |
| `wait(duration)` | `Wait`（虚拟时间） |
| `wait_until(name, Fn(&Context) -> bool)` | `WaitUntil` |
| `wait_for_event(EventKey<T>)` | `WaitForEvent` |
| `set_value(name, &Key<T>, value)` | `SetValue` |
| `raw(BoxedNode<W>)` | 任意预构建节点，包括自定义 struct |
| `subtree(&SubtreeFactory<W>)` | 新的子树实例 |

组合：`sequence`、`reactive_sequence`、`selector`、`reactive_selector`、
`random_selector(seed, ...)`、`weighted_random_selector(children, seed)`、
`utility_selector(children)`、`parallel(success, failure, ...)`、
`mem_parallel(success, failure, ...)`、`if_then_else(condition, then, else)`、
`race(...)`。

装饰：`inverter`、`always_succeed`、`always_fail`、`repeat(times, ...)`、
`retry(attempts, ...)`、`until_success`、`until_failure`、`guard(condition, ...)`、
`probability(chance, seed, ...)`、`while_do(condition, ...)`、`cooldown(duration, ...)`、
`timeout(duration, ...)`、`limiter(max, ...)`、`throttle(window, max, ...)`、
`scoped(name, inputs, outputs, ...)`。

稳定 ID：

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

ID 用于快照、按节点统计、差异、重复 ID 校验与状态保持重载。同一棵树中的重复 ID
会在 `build()` 时被拒绝。

## 5. 组合节点

| 节点 | 语义 | 典型场景 |
|---|---|---|
| `Sequence` | 记忆式顺序：记住 Running 子节点，不重跑已成功步骤 | 连招、多步交互 |
| `ReactiveSequence` | 每帧从头检查；抢占并 abort 旧的 Running 子节点 | 必须持续成立的前置条件 |
| `Selector` | 记忆式选择：恢复 Running 子节点 | 有优先级的任务队列 |
| `ReactiveSelector` | 每帧重新评估高优先级分支；抢占时 abort 旧分支 | 可打断的战斗/拾取 |
| `RandomSelector` | 按 seed 确定性选择，Running 期间保持 | 无需外部 RNG 的行为变化 |
| `WeightedRandomSelector` | 按权重选择；权重 0 永不选中 | 掉落/攻击多样性 |
| `UtilitySelector` | 每帧给所有分支打分取最高；支持 hysteresis | 按局势做战术选择 |
| `Parallel` | 同一帧 tick 全部子节点，按 success/failure 阈值结算；结算后 abort 其余 | 同时播放动画与移动 |
| `MemParallel` | 跨帧累计终态 | 长时间并行目标 |
| `Race` | 第一个终态子节点获胜，其余 abort | "谁先完成算谁" |
| `IfThenElse` | 每帧选择分支；切换时 abort 旧 Running 分支 | 带状态的二选一 |
| `Switch` | 按类型化黑板 key 选分支；key 变化时 abort 旧分支 | Boss 阶段、任务阶段 |

`Threshold` 取值：`Threshold::All`、`Threshold::Any`、`Threshold::AtLeast(NonZeroUsize)`。

可打断战斗：

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

Utility 选择：

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

`UtilityChild::new(score, child)` 接受 `Fn(&Context<W>) -> f32` 与预构建子节点。
分支用普通 builder 构建后调用 `into_root()`。

带阈值的 Parallel：

```rust
let mut tree = BehaviorTreeBuilder::<World>::new("move-and-shoot")
    .parallel(Threshold::All, Threshold::Any, |b| {
        b.action("move", |_| Status::Running)
            .action("shoot", |_| Status::Success)
    })
    .build()?;
```

## 6. 装饰节点

| 装饰器 | 效果 | 场景 |
|---|---|---|
| `Inverter` | 交换 Success/Failure，Running 透传 | "未发现"条件 |
| `AlwaysSucceed` / `AlwaysFail` | 强制终态 | 可选步骤/强制失败探针 |
| `Repeat(n)` | 子节点成功 n 次；每帧受 `MAX_REPEAT_STEPS_PER_TICK` 限制 | 连招、多段攻击 |
| `Retry(n)` | 失败后重试，最多 n 次，每帧一次尝试 | 不稳定的网络/交互步骤 |
| `UntilSuccess` / `UntilFailure` | 持续 Running 直到目标结果 | 等待事件结束 |
| `Probability(chance, seed)` | 确定性随机门 | 命中率、行为变化 |
| `While(predicate)` | 条件为真时重复子节点；变假时 abort | 持续压制循环 |
| `Guard(predicate)` | 条件为假时不 tick 子节点并返回 Failure；Running 期间变假会 abort | 前置条件检查 |
| `Cooldown(duration)` | 成功后 `duration` 内直接 Failure，不 tick 子节点 | 攻击冷却 |
| `Timeout(duration)` | 超时 abort 子节点并失败 | 有界交互 |
| `Limiter(max)` | 最多允许 max 个终态结果 | 消耗品、一次性任务 |
| `Throttle(window, max)` | 时间窗口内最多 max 个终态结果 | 限流 |

```rust
let mut tree = BehaviorTreeBuilder::<World>::new("attack")
    .guard(|ctx: &Context<'_, World>| ctx.world_ref().enemy_near, |b| {
        b.cooldown(Duration::from_secs(1), |b| {
            b.action("strike", |_| Status::Success)
        })
    })
    .build()?;
```

所有时间类装饰器都使用虚拟 `Time`，测试中显式推进时间即可。

## 7. 叶子节点

| 叶子 | 构造 / 构建器 | 说明 |
|---|---|---|
| `Action<W>` | `Action::new(name, tick)` / `.action(...)` | 闭包状态按节点独立 |
| `Condition<W>` | `Condition::new(name, predicate)` / `.condition(...)` | 只读，返回 Success/Failure |
| `Wait<W>` | `Wait::new(duration)` / `.wait(...)` | 虚拟时间到达前 Running |
| `WaitUntil<W>` | `WaitUntil::new(name, predicate)` / `.wait_until(...)` | 轮询谓词 |
| `WaitForEvent<W, T>` | `WaitForEvent::new(EventKey<T>)` / `.wait_for_event(...)` | FIFO 消费事件 |
| `TryAction<W, E>` | `TryAction::new(name, fallible_tick)` / `.try_action(...)` | `Err` -> trace error + Failure |
| `SetValue<W, T>` | `SetValue::new(name, &Key<T>, value)` / `.set_value(...)` | 每帧写值 |

Action 清理回调：

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

`with_abort` 在 Running 子树被抢占时执行，接收 context，可修改世界。
`with_reset` 在整树 reset 时执行，不接收 context，适合重置 tick 闭包内部状态。
世界侧清理应放在 `with_abort`（抢占）或驱动方的 reset 流程中，因为 `Node::reset`
同样没有 context。

错误上报：

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

安装 tracer（`StatsTracer`、`TraceRecorder` 或自定义 `Tracer`）即可看到 `TraceEvent::Error`。

`WaitForEvent` 消费类型化事件：

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

## 8. 事件与命令

### 事件队列

| API | 用途 |
|---|---|
| `EventKey::new(name)` / `event_key!` | 静态类型化事件 key |
| `key.push(ctx, event)` | 节点内入队 |
| `key.pop(ctx) -> Option<T>` | FIFO 消费 |
| `key.len(ctx)` / `is_empty(ctx)` / `clear(ctx)` | 检查/清空队列 |

事件按 key 名称存储在当前黑板中，适合解耦生产者（网络/UI）与消费者（行为树叶子）。

### 命令缓冲

若应用需要干净的提交点，业务命令不应直接修改世界。把类型化的
`CommandBuffer<C>` 放进黑板，驱动方在 tick 后 drain：

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

// after tree.tick(...):
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

## 9. 异步与远程 action

`AsyncAction` 在首次 tick 启动任务，后续 tick 轮询，abort/reset/drop 时取消；
`tick` 永不阻塞。

| 类型 | 用途 |
|---|---|
| `AsyncTask` trait | 实现 `poll() -> Status` 与 `cancel()` |
| `ThreadTask::spawn(FnOnce(CancellationToken) -> Status)` | 在线程中做阻塞文件/网络工作 |
| `CancellationToken` | 协作式取消标志 |
| `TokioTask::spawn(future)` | 在当前 Tokio runtime 上启动 |
| `TokioTask::spawn_on(&handle, future)` | 指定 runtime handle |
| `AsyncAction::new(name, start)` | 叶子包装；`start` 返回任务 |

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

Tokio 版本（需要 `tokio` feature）：

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

业务错误必须由任务映射成 `Status`。在 runtime 之外调用 `TokioTask::spawn`
会返回立即 `Failure` 的任务，而不是 panic。

## 10. 复用与边界

| API | 用途 | 场景 |
|---|---|---|
| `SubtreeFactory::new(|| BoxedNode)` | 可克隆蓝图；`instantiate()` 产生新状态 | 同种哥布林批量生成 |
| `SubtreeSlot::new(name, &Key<K>, factory)` | 类型化 key 变化时切换子树并 abort 旧子树 | Boss 阶段、任务阶段 |
| `SubtreeContract::new(name, child)` | 要求输入 key；激活/abort 时清理声明的输出 | 子树之间的显式接口 |
| `ScopedSubtree::new(name, child)` | 私有黑板；声明输入拷入、输出拷回 | 隔离小怪状态 |
| 嵌套 `BehaviorTree` | 树本身实现 `Node<W>` | 共享 service/tracer |

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

契约与作用域：

```rust
let strike = SubtreeContract::new("strike", attack_root())
    .require(&TARGET)   // missing -> trace error + Failure
    .provide(&PLAN);    // cleared on activation/abort

let isolated = ScopedSubtree::new("minion", minion_root())
    .input(&TARGET)     // requires Key<T>: T: Clone
    .output(&PLAN);
```

构建器形式：

```rust
let tree = BehaviorTreeBuilder::<World>::new("minion")
    .scoped("minion", [&TARGET], [&PLAN], |b| {
        b.action("plan", |_| Status::Success)
    })
    .build()?;
```

`ScopedSubtree` 内部的 trace 事件会被抑制；作用域节点本身仍出现在父级 trace 中。

## 11. 状态、持久化与测试

### 结构快照

```rust
let snapshot = tree.snapshot();          // version + root/services bytes
tree.restore(&snapshot)?;                // same structure required
```

快照不包含世界、黑板值、闭包捕获、`AsyncAction` 任务与 `SubtreeSlot` 动态状态。
请自行序列化 `snapshot.version`、`snapshot.root`、`snapshot.services`；也可以启用
`snapshot-serde` 使用 `TreeSnapshot::to_json` / `from_json`。

### ID 快照

```rust
let snapshot = tree.snapshot_by_id()?;   // BTreeMap<NodeId, Vec<u8>>
tree.restore_by_id(&snapshot)?;          // strict: unknown IDs are errors
let restored = tree.restore_by_id_matching(&snapshot)?; // tolerate removed IDs

let json = snapshot.to_json();           // snapshot-serde；ID 作为对象 key
let snapshot = NodeIdSnapshot::from_json(&json)?;
```

节点顺序可能变化时使用 ID 快照。自定义有状态节点需实现
`save_own_state` / `load_own_state`；自定义组合节点还需暴露 `children_mut()`。

### 黑板持久化

```rust
let mut codec = BlackboardCodec::new();
codec.register(&TARGET)?;
let bytes = codec.encode(tree.blackboard())?;
codec.decode(tree.blackboard_mut(), &bytes)?;
```

只保存已注册的 key；未知 key 或类型不符会返回类型化错误。

### `TestDriver`

```rust
let mut driver = TestDriver::new(tree, World::default()).with_step(Duration::from_millis(16));
assert_eq!(driver.tick(), Status::Running);

let statuses = driver.tick_n(10);
let outcome = driver.run_until_terminal(600);
let (tree, world) = driver.into_parts();
```

| 方法 | 用途 |
|---|---|
| `tick()`、`tick_at(time)`、`tick_n(n)` | 推进虚拟时间并 tick |
| `run_until_terminal(max)`、`run_until_status(status, max)` | 有界循环并返回结果 |
| `advance(delta)`、`now()`、`time()` | 时间控制 |
| `install_trace_recorder()` | 安装并返回 `TraceRecorder` |
| `tree()`、`tree_mut()`、`world()`、`world_mut()`、`blackboard()` | 检查状态 |

### Trace 回放

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

`TraceDiff` 报告第一个不同的事件（包括长度不一致）。回放比较的是可观测节点行为，而不是世界状态。

## 12. 可观测性

### Tracer 事件

| 事件 | 字段 | 用途 |
|---|---|---|
| `TraceEvent::Enter` | id、name、kind、path、time | 调用栈、顺序 |
| `TraceEvent::Exit` | id、name、kind、path、status、time | 状态时间线 |
| `TraceEvent::Abort` | id、name、kind、path、reason、time | 抢占调试 |
| `TraceEvent::Error` | id、name、kind、path、message、time | 可恢复业务错误 |

`AbortReason`：`Preempted`、`Timeout`、`ParallelDone`、`RaceDone`、`TreeAbort`。

自定义 tracer：

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

`MultiTracer` 向多个 tracer 广播：

```rust
let mut multi = MultiTracer::new();
multi.push(Box::new(stats.clone()));
multi.push(Box::new(recorder.clone()));
tree.set_tracer(Some(Box::new(multi)));
```

### `StatsTracer`

| 方法 | 用途 |
|---|---|
| `nodes()` | 所有已观测路径与统计 |
| `node(&[usize])` | 按结构路径查统计 |
| `node_by_id(&NodeId)` | 按稳定 ID 查统计 |
| `render_prometheus()` | Prometheus 文本 |
| `render_tree(&tree)` | 带最后状态的 ASCII 树 |
| `clear()` | 重置计数 |

统计按路径索引，同时按 `NodeId` 建索引；加上 `.id(...)` 后指标在结构调整时仍然稳定。

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

实时服务：

```rust
let mut server = InspectorServer::bind("127.0.0.1:7878")?;
server.poll_with_history(&tree, Some(&stats), &history)?;

// 附带恢复节点数与恢复错误。
let report = watcher.last_reload();
server.poll_with_report(watcher.tree(), Some(&stats), watcher.diff_history(), report)?;
```

| 端点 | 响应 |
|---|---|
| `/` | HTML 报告：树、状态、services、NodeId 审计、历史、恢复元数据 |
| `/metrics` | Prometheus 文本 |
| `/tree` | ASCII 状态树 |
| `/status.json` | 根状态、节点/统计计数、历史长度、最旧/最新 revision、最新时间戳、恢复节点数、恢复错误 |
| `/diff.json` | 最近一次 `TreeDiff` JSON（`diff-serde`） |
| `/diffs.json[?since=N]` | revision 条目，可只取 N 之后（`diff-serde`） |
| `/health` | `ok` |

服务器仅供调试：无鉴权，请只绑定 loopback。

### 树差异与历史

```rust
let diff = TreeDiff::between(&old_tree, &new_tree);
println!("{diff}");

let mut history = DiffHistory::new(16);
history.push_now(diff);              // revision + wall-clock timestamp
let latest = history.latest_entry();
```

| 方法 | 用途 |
|---|---|
| `TreeDiff::between(old, new)` | 按 `NodeId` 报告新增/删除/移动/改名 |
| `diff.is_empty()`、`diff.to_json()`、`from_json()` | 检查/序列化/解析 |
| `DiffHistory::push`、`push_at`、`push_now` | 追加，可带时间戳 |
| `latest`、`latest_entry`、`latest_revision`、`oldest_revision` | 游标 |
| `entries`、`entries_since(rev)`、`iter` | 遍历 |
| `to_json`、`to_json_since(rev)`、`from_json` | `{revision, timestamp?, diff}` 数组 |
| `pop_latest`、`set_limit`、`clear` | 维护 |

同时发生移动与改名/改类型的节点会同时出现在 `moved` 与 `renamed` 中。
轮询方应保存 `latest_revision` 并只请求更新条目；`oldest_revision` 用于判断
是否因淘汰而漏读。

## 13. 配置

配置层是纯数据 + 构建期注册表。配置中不包含闭包；参数只在构建时反序列化为具体类型。

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

### 注册表方法

| 方法 | 注册内容 |
|---|---|
| `action::<P>(name, factory)` | action；factory 按节点接收类型化参数 |
| `condition::<P>(name, factory)` | condition 叶子，同时注册同名 predicate |
| `predicate::<P>(name, factory)` | 仅 predicate，供 guard/while/if_then_else 使用 |
| `score::<P>(name, factory)` | utility 评分 |
| `try_action::<P, E>(...)` | 可失败 action；`Err` -> trace error + Failure |
| `async_action::<P>(...)` | 返回任务的异步 action |
| `custom::<P>(kind, factory)` | 任意节点工厂，接收类型化参数与子节点 |
| `switch_key(key)` / `set_value_key(key)` / `event_key(key)` | 供配置引用的类型化 key |
| `contract_key(key)` / `scoped_key(key)` | 供 `contract` / `scoped` 边界使用的 key；scoped 值需实现 `Clone` |

Factory 形状为 `Fn(P) -> TickClosure`。外层 factory 每个节点执行一次，因此实例之间闭包状态隔离。
省略 `params` 时按 `null` 反序列化；无参数注册使用 `P = ()`。

### 支持的配置节点

组合：`sequence`、`reactive_sequence`、`selector`、`reactive_selector`、
`random_selector`、`weighted_random_selector`、`parallel`、`mem_parallel`、
`race`、`switch`、`utility_selector`、`if_then_else`。

装饰：`inverter`、`always_succeed`、`always_fail`、`repeat`、`retry`、
`until_success`、`until_failure`、`probability`、`while`、`guard`、`cooldown`、
`timeout`、`limiter`、`throttle`。

叶子：`wait`、`wait_until`、`wait_for_event`、`set_value`、`action`、
`condition`、`try_action`、`async_action`。

结构：`include`、`custom`、`identified`、`scoped`、`contract`。

示例：

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

顶层 `version` 默认 `1`；不支持的版本会被拒绝。组合、装饰与结构节点可带可选显示
`name`；叶子的名称来自注册表条目。`identified` 增加供快照与差异使用的稳定 `id`。
未知字段会被拒绝。

### Include

```rust
use playact::config::FsIncludeResolver;

let resolver = FsIncludeResolver::json("config/")?;
let tree = spec.build_with_resolver(&registry, &resolver)?;
```

`FsIncludeResolver` 把路径限制在基准目录内，拒绝 `..`、绝对路径与符号链接逃逸；
构建期检测深度与循环；同时记录解析过的文件供热重载监视。自定义 resolver 可实现
`watched_paths()` / `reset_watched_paths()`。

### 校验与 schema

```rust
use playact::config::ValidationLimits;

spec.validate(&ValidationLimits::default())?; // depth 64, nodes 10_000
let schema = TreeSpec::json_schema();         // requires `schema`
```

### 热重载

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

- 重载保留黑板数据；节点状态默认重置。
- `with_state_preservation(true)` 按 `NodeId` 恢复自身状态。
- `last_diff()`、`last_revision()`、`last_timestamp()`、`diff_history()` 暴露变化。
  `last_restored()` 报告最近一次成功重载恢复了多少节点自身状态（首次重载前为
  `None`，没有匹配节点时为 `Some(0)`）；`last_restore_error()` 报告被拒绝的恢复，
  此时新树会被 reset。`last_reload()` 用 `ReloadReport` 一次返回 diff、revision、
  时间戳、恢复节点数与恢复错误。
- resolver 报告的 include 文件也会被轮询并触发重载。
- 解析/构建失败保留旧树并返回错误。

### 远程配置

```rust
use playact::remote_config::RemoteTreeLoader;

let mut loader = RemoteTreeLoader::<World>::json(url, registry)?
    .with_refresh_interval(Duration::from_secs(30))
    .with_diff_history(16);

loader.poll()?;                 // conditional request; false if not modified
loader.tree_mut().tick(&mut world, time);
```

`RemoteTreeLoader` 支持 ETag / Last-Modified、响应大小上限、UTF-8 校验、include，
以及可选的 `ResponseVerifier`（签名/白名单）。`last_reload()` 返回与热重载相同的
`ReloadReport`；`last_restored()`、`last_restore_error()`、`last_revision()`、
`last_timestamp()` 分别暴露刷新元数据。请求在调用线程同步执行。

## 14. 过程宏 DSL

启用 `macros`：

```rust
let tree = playact::bt!(World, "player", reactive_selector {
    sequence {
        condition("low_hp", |ctx: &Context<'_, World>| ctx.world_ref().hp < 0.3),
        action("heal", |_| Status::Success),
    },
    action("patrol", |_| Status::Running),
})?;
```

宏覆盖组合、装饰、叶子与 `raw(expression)`，并且仍走 typestate builder。

`#[derive(BtNode)]` 为自定义 struct 生成 Node 样板：

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

## 15. 场景配方

### A. 可打断的战斗 AI

顶层使用 `ReactiveSelector`，让低血量与近距离敌人每帧抢占低优先级行为；
条件放进小型 `Sequence`。攻击外加 `Cooldown`，前置条件用 `Guard`。

参考 [`examples/battle_ai.rs`](../examples/battle_ai.rs)。

### B. 配置驱动 AI + 热重载 + 审计

1. 在 `NodeRegistry` 注册 action/condition/score。
2. 解析 `TreeSpec`，用 `FsIncludeResolver` 构建。
3. 包进 `TreeWatcher`，启用 `with_diff_history(n)` 与 `with_state_preservation(true)`。
4. 用 `InspectorServer::poll_with_history` 提供服务。
5. 工具侧轮询 `/status.json` 与 `/diffs.json?since=<cursor>`。

参考 [`examples/reload_audit.rs`](../examples/reload_audit.rs)。

### C. Utility 战术选择

用 `UtilitySelector` 每帧给分支打分，而不是硬编码优先级；用 hysteresis 避免相近分数来回抖动。
评分就是普通 `Fn(&Context<W>) -> f32`；NaN 视为最低分。

### D. 多智能体共享黑板

用 `SharedBlackboard` 在多棵树之间共享类型化状态。每次 `tick` 会在该树 tick 期间加锁；
`try_lock` 可检测竞争。

```rust
let shared = SharedBlackboard::new();
shared.tick(&mut goblin_a, &mut world, time);
shared.tick(&mut goblin_b, &mut world, time);
```

### E. 远程行为更新

使用 `RemoteTreeLoader` + ETag 缓存 + verifier。刷新失败保留旧树；成功时用 diff 历史
观察变化。若重启需要恢复，请另行持久化黑板/节点状态。

### F. 确定性回归测试

用 `TestDriver` 控制虚拟时间，`StatsTracer` 统计计数，`TraceRecorder::diff` 比较节点级行为，
`NodeIdSnapshot` 恢复状态。随机/概率节点使用固定 seed 即可复现。

### G. 存档与恢复

启用 `snapshot-serde` 与 `blackboard-serde` 后，把 `TreeSnapshot::to_json()` 与
`BlackboardCodec::encode()` 写入文件。启动时重建同一棵树，调用 `restore()` 恢复
节点状态、`decode()` 恢复黑板，再继续 tick。世界状态由应用自行保存。参考
[`examples/save_game.rs`](../examples/save_game.rs)。

## 16. 模块索引

| 模块 | 主要类型 |
|---|---|
| `playact` 根 | `Status`、`Node`、`BoxedNode`、`Context`、`Time` |
| `blackboard` | `Blackboard`、`Key` |
| `tree` | `BehaviorTree`、`TreeDescription`、`NodePath` |
| `builder` | `BehaviorTreeBuilder`、`BranchBuilder` |
| `composite` | `Sequence`、`ReactiveSequence`、`Selector`、`ReactiveSelector`、`RandomSelector`、`WeightedRandomSelector`、`UtilitySelector`、`Parallel`、`MemParallel`、`Race`、`IfThenElse`、`Switch`、`Threshold` |
| `decorator` | `Inverter`、`AlwaysSucceed`、`AlwaysFail`、`Repeat`、`Retry`、`UntilSuccess`、`UntilFailure`、`Probability`、`While`、`Guard`、`Cooldown`、`Timeout`、`Limiter`、`Throttle` |
| `leaf` | `Action`、`Condition`、`Wait`、`WaitUntil`、`WaitForEvent`、`TryAction`、`SetValue` |
| `event` / `command` | `EventKey`、`EventQueue`、`CommandBuffer` |
| `async_action` / `shared` | `AsyncAction`、`ThreadTask`、`TokioTask`、`SharedBlackboard` |
| `subtree` / `contract` / `scope` | `SubtreeFactory`、`SubtreeSlot`、`SubtreeContract`、`ScopedSubtree` |
| `snapshot` / `persistence` / `replay` | `TreeSnapshot`、`NodeIdSnapshot`、`BlackboardCodec`、`TraceRecorder` |
| `trace` / `stats` / `diff` | `Tracer`、`TraceEvent`、`StatsTracer`、`TreeDiff`、`DiffHistory` |
| `reload` | `ReloadReport` |
| `inspector` | `Inspector`、`InspectorServer` |
| `config` | `TreeSpec`、`NodeRegistry`、`FsIncludeResolver`、`ValidationLimits` |
| `hot_reload` / `remote_config` | `TreeWatcher`、`RemoteTreeLoader` |

完整签名与错误枚举见 [docs.rs/playact](https://docs.rs/playact)。
