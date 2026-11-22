# playact

> 面向游戏自动化的类型安全、可扩展行为树引擎。

`playact` 只负责“决策编排”：组合节点与装饰节点只流转 `Status`，执行节点由使用方提供。引擎不定义任何业务世界、命令或动作类型，因此同一个 crate 可以同时用于战斗 AI、挂机脚本、任务流程和机器人控制。

## 设计红线

- **业务无关**：引擎只认识 `Status`、`Node`、`Context<W>`、`Blackboard`、`Time`、`Tracer`。`W`（世界）、`C`（命令）、`P`（配置参数）都由使用方提供。
- **类型安全**：黑板使用 `Key<T>`，公共 API 不做字符串查表、不暴露 `Any`；配置参数在构建期反序列化成具体类型，tick 期不再有 `serde_json::Value`。
- **构建期尽量失败**：根节点多个子节点、装饰器子节点数量不对会直接编译失败；配置错误返回带路径的 `ConfigBuildError`。
- **同步、可预测**：tick 内不 sleep、不引入 async runtime、不 `catch_unwind`；长动作返回 `Running`，由下一帧继续。
- **可中断**：Running 子树被抢占时先 `abort` 做外部清理，再 `reset` 清内部状态。

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
| `Parallel` | 全部子节点 tick 后按成功/失败阈值汇总；失败优先；结论确定后 abort 剩余 Running |

`Parallel` 的阈值按**单个 tick 的汇总**计算，不跨 tick 累计终态：不同 tick 分别成功的子节点不会让 `All` 自动满足；`Any` 每次成功都会 abort 其余仍在 Running 的兄弟。需要累积语义请用自定义节点（v0.2 计划中的累积式 Parallel）。

`RandomSelector` 的 `seed` 决定随机顺序；相同 seed + 相同 tick 序列可复现，`reset()` 恢复初始 RNG。

`Parallel` 阈值支持 `All`、`Any`、`AtLeast(n)`；`AtLeast(n)` 在构建期校验不超过子节点数量。

### 装饰节点

`Inverter`、`AlwaysSucceed`、`AlwaysFail`、`Repeat`、`Retry`、`UntilSuccess`、`UntilFailure`、`Cooldown`、`Timeout`、`Limiter`。

- `Cooldown` 只在子节点成功后开始计时；
- `Timeout` 只对 Running 计时，超时 abort 子节点；`duration = 0` 时首次 tick 直接 Failure，不 tick 子节点；
- `Retry` 每 tick 最多尝试一次，避免同一帧刷副作用；
- `UntilSuccess`/`UntilFailure` 每 tick 只尝试一次，不会自旋；
- `Repeat` 单 tick 最多推进 `MAX_REPEAT_STEPS_PER_TICK`（默认 1024）次，超出返回 `Running`，不会因超大 `times` 卡死逻辑帧；
- `Limiter` 是“每次激活”的配额，全局 `reset` 或抢占后的 `abort→reset` 会清零；跨抢占配额请放黑板。

### 叶子

`Action`、`Condition`、`Wait`。动作是 `FnMut(&mut Context<W>) -> Status`；条件只读 `&Context<W>`。

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

## Tracer 与 describe

```rust
let mut tree = /* ... */;
tree.set_tracer(Some(Box::new(MyTracer)));

println!("{}", tree.describe());
println!("{}", tree.describe().to_dot());
```

`Tracer` 接收 `Enter`/`Exit`/`Abort` 事件，`path` 是子索引路径（根为 `[]`）。默认未安装 Tracer 时零成本。`tracing` feature 提供 `TracingTracer` 适配器。

`AbortReason` 取值为 `Preempted`、`Timeout`、`ParallelDone`、`TreeAbort`，只出现在 Tracer 事件中，不进入 `Node::abort` 签名。

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
- 支持 `random_selector`（`seed` 必填，确定性）与 `custom` 子节点；
- 叶子参数在构建期解析为具体 `P`，tick 期没有动态值；
- 配置结构拒绝未知字段（`deny_unknown_fields`），拼错的键不会被静默忽略；
- 省略 `params` 等价于 `null`：无参 action/condition 用 `P = ()` 并省略 `params`；带字段的 struct 参数请显式写 `"params": {}`；
- 未知 action/condition、参数错误、版本不支持、阈值非法都返回带 `root.children[i]...` 路径的 `ConfigBuildError`；
- 需要 `abort`/`reset` 清理的配置动作，用 `custom` 工厂返回带回调的 `Action` 或自定义节点；
- 配置不定义业务世界、命令或黑板 key。

示例 JSON 见 [`examples/battle_ai.json`](examples/battle_ai.json)。

## Feature 矩阵

| feature | 内容 |
|---|---|
| 默认 | 纯标准库，零依赖 |
| `config` | `TreeSpec`、`NodeRegistry`、JSON 解析（serde/serde_json/serde_path_to_error） |
| `toml` | `from_toml_str` |
| `yaml` | `from_yaml_str`（serde_yaml_ng） |
| `ron` | `from_ron_str` |
| `tracing` | `TracingTracer` |
| `full` | 以上全部 |

## 线程模型

- `Node<W>: Send`，`BehaviorTree<W>: Send`；
- 不要求 `Sync`；
- `W` 不必 `Send`/`Sync`，因为树只在一帧内借用 `&mut W`，不存储它；
- 黑板值要求 `Send + 'static`；
- 每个 AI 持有自己的树和黑板，多 AI 用工厂函数分别构建；共享世界由驱动方顺序 tick 或自行同步。
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
cargo doc --workspace --all-features --no-deps
```

包含模块单测、公共 API 集成测试、doctest 和 `compile_fail` doctest（验证根/装饰器子节点数量、黑板 key 类型错误）。CI 覆盖 stable 与 MSRV 1.92、Linux 与 Windows。

## 0.2 开发线

当前 crate 版本号仍为 **0.1.1**，但已包含 0.2 计划中的：

- `RandomSelector`：带记忆、确定性 seed 的随机选择；
- `SubtreeFactory`：可克隆、可重复实例化的子树工厂。

仍未实现：累积式 Parallel、async action/远程动作、proc-macro DSL、热重载/远程配置、共享黑板、`BehaviorTree: Node`、bench/fuzz、`no_std`。

## License

Apache-2.0
