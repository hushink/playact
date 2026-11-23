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

详细 API 文档：**[crates/playact/README.zh-CN.md](crates/playact/README.zh-CN.md)**。
English guide: **[crates/playact/README.md](crates/playact/README.md)**。

## 核心特性

- **唯一节点契约。** 所有节点实现 `tick(&mut Context<W>) -> Status`；控制节点
  负责编排，叶子节点真正干活。任意嵌套、替换与复用都自然成立。
- **类型安全。** `Key<T>` 黑板槽与构建期反序列化的配置参数；公共 API 不暴露
  `Any`，也不做字符串查表。
- **编译期树形校验。** typestate 构建器在编译期拒绝非法的根节点/装饰器子节点数量。
- **可预测的 tick。** tick 内不 sleep、不引入 async runtime、不隐藏线程；长动作
  返回 `Running`，下一帧继续。
- **可中断。** Running 子树被抢占时先 `abort` 清理，再 `reset`。
- **可选集成。** 配置格式、热重载、远程配置、Tokio 任务、tracing、快照、共享
  黑板与 proc-macro DSL。
- **可移植核心。** 默认 `std` 构建零第三方依赖；关闭默认 feature 后使用
  `no_std + alloc`。

## 工作区结构

| 路径 | 用途 |
|---|---|
| [`crates/playact`](crates/playact) | 引擎库：节点、构建器、黑板、配置、重载、异步适配器、快照 |
| [`crates/playact-macros`](crates/playact-macros) | `bt!` proc-macro DSL |
| [`crates/playact/examples`](crates/playact/examples) | 可运行的战斗 AI 示例 |
| [`crates/playact/tests`](crates/playact/tests) | 公共 API 与确定性压力测试 |
| [`crates/playact/benches`](crates/playact/benches) | 零第三方依赖微基准 |

## 安装

```toml
[dependencies]
playact = "0.1"
```

启用可选集成：

```toml
[dependencies]
playact = { version = "0.1", features = ["config", "hot-reload", "macros"] }
```

使用 `no_std` 核心：

```toml
[dependencies]
playact = { version = "0.1", default-features = false }
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

## 能力概览

| 领域 | 内容 |
|---|---|
| 核心 | `Status`、`Node`、`NodeId`、`Context`、类型安全 `Blackboard`、`Time`、`Kind` |
| 组合节点 | `Sequence`、`ReactiveSequence`、`Selector`、`ReactiveSelector`、`RandomSelector`、`WeightedRandomSelector`、`UtilitySelector`、`Parallel`、`MemParallel`、`Race`、`IfThenElse` |
| 装饰节点 | `Inverter`、`AlwaysSucceed`、`AlwaysFail`、`Repeat`、`Retry`、`UntilSuccess`、`UntilFailure`、`Cooldown`、`Timeout`、`Limiter`、`Throttle`、`Guard` |
| 叶子节点 | `Action`、`Condition`、`Wait`、`WaitUntil`、`WaitForEvent`、`SetValue`、`TryAction`、`AsyncAction` |
| 组合复用 | typestate 构建器、`SubtreeFactory`、`SubtreeSlot`、嵌套 `BehaviorTree`、服务 |
| 可观测性 | `Tracer`、`MultiTracer`、`StatsTracer`、错误事件、`last_status`、ASCII/DOT `describe` |
| 持久化 | 带版本的节点状态快照与可选 typed 黑板序列化 |
| 共享状态 | 跨树/线程 `SharedBlackboard` |
| 配置 | `TreeSpec` + `NodeRegistry`、JSON/TOML/YAML/RON、typed 参数、include、JSON Schema、校验、热重载、远程加载 |
| 异步 | 运行时无关 `AsyncTask`、`ThreadTask`、可选 `TokioTask` |
| 可移植性 | `no_std + alloc` 核心；可选集成按 feature 门控 |
| 开发体验 | `bt!` DSL、两个完整战斗 AI 示例、基准与确定性压力测试 |

详细语义、注意事项与 API 示例见
[crate 中文文档](crates/playact/README.zh-CN.md)。

## 示例

```text
cargo run -p playact --example battle_ai
cargo run -p playact --example battle_ai_config --features config
```

- `battle_ai`：确定性的 1D 玩家 vs 三只哥布林战斗，展示 ReactiveSelector 打断、
  Cooldown、Timeout、Inverter、Tracer、abort 清理与 CommandBuffer/Executor 分离。
- `battle_ai_config`：同一套思路的 JSON 配置驱动版本，展示 typed 参数与 custom
  节点工厂。

## Feature 矩阵

| Feature | 内容 |
|---|---|
| 默认 | `std`；纯标准库，零第三方依赖 |
| `std` | 标准库集成；关闭后核心切换为 `no_std + alloc` |
| `config` | `TreeSpec`、`NodeRegistry`、JSON、typed 参数、include（依赖 `std`） |
| `schema` | `TreeSpec` 的 JSON Schema 生成（依赖 `config`） |
| `blackboard-serde` | 可选的 typed 黑板序列化（依赖 `std`） |
| `test-util` | 确定性虚拟时间 `TestDriver`（依赖 `std`） |
| `toml` | TOML 解析器 |
| `yaml` | YAML 解析器（`serde_yaml_ng`） |
| `ron` | RON 解析器 |
| `tracing` | `TracingTracer` |
| `hot-reload` | `TreeWatcher` 文件监听 |
| `remote-config` | HTTP `RemoteTreeLoader` |
| `tokio` | `TokioTask` 适配器 |
| `macros` | `playact-macros` 的 `bt!` DSL |
| `full` | 以上全部 |

```text
cargo check -p playact --no-default-features --target thumbv7em-none-eabihf --lib
```

## 开发

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo bench -p playact
cargo doc --workspace --all-features --no-deps
cargo check -p playact --no-default-features --target thumbv7em-none-eabihf --lib
```

测试覆盖所有节点、构建器、配置层、重载、远程加载、快照、异步适配器、公共 API
集成测试、doctest、compile-fail 检查与确定性随机压力测试。基准零第三方依赖，
覆盖 tick、黑板读写、构建与 `describe`。CI 覆盖 stable 与 MSRV 1.92、Linux 与
Windows，以及嵌入式 `no_std` 检查。

## 文档

- [英文 API 指南](crates/playact/README.md)
- [中文 API 指南](crates/playact/README.zh-CN.md)
- [docs.rs API 参考](https://docs.rs/playact)
