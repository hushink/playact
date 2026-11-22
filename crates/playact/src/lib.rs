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
//! 核心 crate 默认零依赖；配置层、文件格式与 `tracing` 集成都是可选 feature。
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

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(rust_2018_idioms)]

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
pub mod status;
pub mod subtree;
pub mod time;
pub mod trace;
pub mod tree;

pub use async_action::{AsyncAction, AsyncTask, CancellationToken, ThreadTask};
pub use blackboard::{Blackboard, Key};
pub use builder::{BehaviorTreeBuilder, BranchBuilder};
pub use command::CommandBuffer;
pub use composite::{
    MemParallel, Parallel, RandomSelector, ReactiveSelector, ReactiveSequence, Selector, Sequence,
    Threshold,
};
pub use context::Context;
pub use decorator::{
    AlwaysFail, AlwaysSucceed, Cooldown, Inverter, Limiter, MAX_REPEAT_STEPS_PER_TICK, Repeat,
    Retry, Timeout, UntilFailure, UntilSuccess,
};
pub use error::BuildError;
#[cfg(feature = "hot-reload")]
pub use hot_reload::{HotReloadError, TreeParser, TreeWatcher};
pub use leaf::{Action, Condition, Wait};
pub use node::{BoxedNode, Kind, Node};
pub use status::Status;
pub use subtree::SubtreeFactory;
pub use time::Time;
pub use trace::{AbortReason, TraceEvent, Tracer};
pub use tree::{BehaviorTree, TreeDescription};

#[cfg(feature = "tracing")]
pub use trace::TracingTracer;

/// 常用类型与宏的集中导入。
pub mod prelude {
    pub use crate::key;
    pub use crate::{
        AbortReason, Action, AlwaysFail, AlwaysSucceed, AsyncAction, AsyncTask, BehaviorTree,
        BehaviorTreeBuilder, Blackboard, BoxedNode, BranchBuilder, BuildError, CancellationToken,
        CommandBuffer, Condition, Context, Cooldown, Inverter, Key, Kind, Limiter,
        MAX_REPEAT_STEPS_PER_TICK, MemParallel, Node, Parallel, RandomSelector, ReactiveSelector,
        ReactiveSequence, Repeat, Retry, Selector, Sequence, Status, SubtreeFactory, ThreadTask,
        Threshold, Time, Timeout, TraceEvent, Tracer, TreeDescription, UntilFailure, UntilSuccess,
        Wait,
    };
    #[cfg(feature = "hot-reload")]
    pub use crate::{HotReloadError, TreeParser, TreeWatcher};
}
