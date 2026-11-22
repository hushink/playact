//! playact：面向游戏自动化的类型安全行为树引擎。
//!
//! 引擎只认识三类东西：
//!
//! - [`Status`]：`Success` / `Failure` / `Running`，节点之间唯一的控制流协议；
//! - [`Node`]：所有节点统一实现 `tick(&mut Context<W>) -> Status`；
//! - [`Context`]：借用式 world、类型安全 [`Blackboard`] 与 [`Time`]。
//!
//! # 快速开始
//!
//! ```
//! use playact::{BehaviorTreeBuilder, Status, Time};
//!
//! let mut tree = BehaviorTreeBuilder::<()>::new("demo")
//!     .sequence(|b| {
//!         b.condition("ready", |_| true)
//!             .action("work", |_| Status::Success)
//!     })
//!     .build()
//!     .expect("valid tree");
//!
//! let mut world = ();
//! assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
//! ```
//!
//! # 设计边界
//!
//! playact 不定义 `WorldState`、业务 `Action` 或任何具体命令类型。世界类型
//! `W`、命令类型 `C`、配置参数 `P` 都由使用方提供；示例中的战斗类型只
//! 存在于示例里。
//!
//! 核心 crate 默认通过 `std` feature 使用标准库并保持零第三方依赖；关闭后
//! 以 `alloc` 编译。配置层、文件格式与 `tracing` 集成都是可选 feature。
//!
//! # no_std
//!
//! 关闭默认 `std` feature 后，核心引擎改用 `alloc` 编译：
//!
//! ```toml
//! playact = { version = "0.1.1", default-features = false }
//! ```
//!
//! 树、构建器、黑板、组合/装饰/叶子、命令缓冲与 `SubtreeFactory` 均可用。
//! `SharedBlackboard`、`AsyncAction`、配置/热重载/远程配置与 tracing 集成
//! 需要 `std`。`SubtreeFactory` 使用 `alloc::sync::Arc`，目标需支持原子指针。
//!
//! # 编译期保证
//!
//! 根节点多个子节点、装饰器子节点数量不对、黑板 key 类型不匹配都会直接
//! 编译失败：
//!
//! ```compile_fail
//! use playact::prelude::*;
//!
//! let _ = BehaviorTreeBuilder::<()>::new("bad")
//!     .action("first", |_| Status::Success)
//!     .action("second", |_| Status::Success)
//!     .build();
//! ```
//!
//! ```compile_fail
//! use std::time::Duration;
//! use playact::prelude::*;
//!
//! let _ = BehaviorTreeBuilder::<()>::new("bad")
//!     .cooldown(Duration::from_secs(1), |b| {
//!         b.action("first", |_| Status::Success)
//!             .action("second", |_| Status::Success)
//!     })
//!     .build();
//! ```
//!
//! ```compile_fail
//! use playact::prelude::*;
//!
//! static A: Key<u32> = Key::new("a");
//! let mut board = Blackboard::new();
//! board.set(&A, 1_u32);
//! let _: Option<&u64> = board.get(&A);
//! ```

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(rust_2018_idioms)]

extern crate alloc;

#[cfg(feature = "std")]
pub mod async_action;
pub mod blackboard;
pub mod builder;
pub mod command;
pub mod composite;
#[cfg(feature = "config")]
pub mod config;
pub mod context;
pub mod decorator;
pub mod error;
#[cfg(feature = "hot-reload")]
pub mod hot_reload;
pub mod leaf;
pub mod node;
#[cfg(feature = "remote-config")]
pub mod remote_config;
#[cfg(feature = "std")]
pub mod shared;
pub mod snapshot;
#[cfg(feature = "std")]
pub mod stats;
pub mod status;
pub mod subtree;
pub mod time;
pub mod trace;
pub mod tree;

#[cfg(feature = "tokio")]
pub use async_action::TokioTask;
#[cfg(feature = "std")]
pub use async_action::{AsyncAction, AsyncTask, CancellationToken, ThreadTask};
pub use blackboard::{Blackboard, Key};
pub use builder::{BehaviorTreeBuilder, BranchBuilder};
pub use command::CommandBuffer;
pub use composite::{
    IfThenElse, MemParallel, Parallel, Race, RandomSelector, ReactiveSelector, ReactiveSequence,
    Selector, Sequence, Threshold, UtilityChild, UtilitySelector, WeightedChild,
    WeightedRandomSelector,
};
#[cfg(feature = "config")]
pub use config::TreeParser;
pub use context::Context;
pub use decorator::{
    AlwaysFail, AlwaysSucceed, Cooldown, Guard, Inverter, Limiter, MAX_REPEAT_STEPS_PER_TICK,
    Repeat, Retry, Throttle, Timeout, UntilFailure, UntilSuccess,
};
pub use error::BuildError;
#[cfg(feature = "hot-reload")]
pub use hot_reload::{HotReloadError, TreeWatcher};
pub use leaf::{Action, Condition, SetValue, Wait};
pub use node::{BoxedNode, Kind, Node};
#[cfg(feature = "remote-config")]
pub use remote_config::{RemoteConfigError, RemoteTreeLoader};
#[cfg(feature = "std")]
pub use shared::SharedBlackboard;
pub use snapshot::{RestoreError, SNAPSHOT_VERSION, TreeSnapshot};
#[cfg(feature = "std")]
pub use stats::{NodeStats, StatsTracer};
pub use status::Status;
pub use subtree::{SubtreeFactory, SubtreeSlot};
pub use time::Time;
pub use trace::{AbortReason, MultiTracer, TraceEvent, Tracer};
pub use tree::{BehaviorTree, TreeDescription};

#[cfg(feature = "tracing")]
pub use trace::TracingTracer;

#[cfg(feature = "macros")]
pub use playact_macros::bt;

/// 常用类型与宏的集中导入。
pub mod prelude {
    #[cfg(feature = "tokio")]
    pub use crate::TokioTask;
    #[cfg(feature = "config")]
    pub use crate::TreeParser;
    #[cfg(feature = "macros")]
    pub use crate::bt;
    pub use crate::key;
    pub use crate::{
        AbortReason, Action, AlwaysFail, AlwaysSucceed, BehaviorTree, BehaviorTreeBuilder,
        Blackboard, BoxedNode, BranchBuilder, BuildError, CommandBuffer, Condition, Context,
        Cooldown, Guard, IfThenElse, Inverter, Key, Kind, Limiter, MAX_REPEAT_STEPS_PER_TICK,
        MemParallel, MultiTracer, Node, Parallel, Race, RandomSelector, ReactiveSelector,
        ReactiveSequence, Repeat, RestoreError, Retry, SNAPSHOT_VERSION, Selector, Sequence,
        SetValue, Status, SubtreeFactory, SubtreeSlot, Threshold, Throttle, Time, Timeout,
        TraceEvent, Tracer, TreeDescription, TreeSnapshot, UntilFailure, UntilSuccess,
        UtilityChild, UtilitySelector, Wait, WeightedChild, WeightedRandomSelector,
    };
    #[cfg(feature = "std")]
    pub use crate::{
        AsyncAction, AsyncTask, CancellationToken, NodeStats, SharedBlackboard, StatsTracer,
        ThreadTask,
    };
    #[cfg(feature = "hot-reload")]
    pub use crate::{HotReloadError, TreeWatcher};
    #[cfg(feature = "remote-config")]
    pub use crate::{RemoteConfigError, RemoteTreeLoader};
}
