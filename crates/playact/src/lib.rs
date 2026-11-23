//! playact: a type-safe behavior tree engine for game automation.
//!
//! The engine knows only three things:
//!
//! - [`Status`]: `Success` / `Failure` / `Running`, the only control-flow protocol
//!   between nodes;
//! - [`Node`]: every node implements `tick(&mut Context<W>) -> Status`;
//! - [`Context`]: a borrowed world, a type-safe [`Blackboard`], and [`Time`].
//!
//! # Quick start
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
//! # Design boundaries
//!
//! playact defines no `WorldState`, business `Action`, or concrete command type.
//! The world type `W`, command type `C`, and configuration parameters `P` are all
//! supplied by the user; the combat types in the examples exist only in the
//! examples.
//!
//! By default the core crate uses the standard library through the `std` feature
//! and keeps zero third-party dependencies; with that feature off it builds on
//! `alloc`. The config layer, file formats, and `tracing` integration are all
//! optional features.
//!
//! # no_std
//!
//! With the default `std` feature disabled, the core engine builds on `alloc`:
//!
//! ```toml
//! playact = { version = "0.1.1", default-features = false }
//! ```
//!
//! Trees, builders, blackboards, composites/decorators/leaves, command buffers,
//! and `SubtreeFactory` are all available. `SharedBlackboard`, `AsyncAction`,
//! config/hot-reload/remote-config, and tracing integration require `std`.
//! `SubtreeFactory` uses `alloc::sync::Arc`, so the target must support atomic
//! pointers.
//!
//! # Compile-time guarantees
//!
//! A root with multiple children, a decorator with the wrong number of children,
//! or a blackboard key type mismatch all fail to compile outright:
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
pub mod event;
#[cfg(feature = "hot-reload")]
pub mod hot_reload;
pub mod leaf;
pub mod node;
#[cfg(feature = "blackboard-serde")]
pub mod persistence;
#[cfg(feature = "remote-config")]
pub mod remote_config;
#[cfg(feature = "std")]
pub mod shared;
pub mod snapshot;
#[cfg(feature = "std")]
pub mod stats;
pub mod status;
pub mod subtree;
#[cfg(feature = "test-util")]
pub mod testing;
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
pub use config::{TreeParser, ValidationLimits};
pub use context::Context;
pub use decorator::{
    AlwaysFail, AlwaysSucceed, Cooldown, Guard, Inverter, Limiter, MAX_REPEAT_STEPS_PER_TICK,
    Repeat, Retry, Throttle, Timeout, UntilFailure, UntilSuccess,
};
pub use error::BuildError;
pub use event::{EventKey, EventQueue};
#[cfg(feature = "hot-reload")]
pub use hot_reload::{HotReloadError, TreeWatcher};
pub use leaf::{Action, Condition, SetValue, TryAction, Wait, WaitForEvent, WaitUntil};
pub use node::{BoxedNode, IdentifiedNode, Kind, Node, NodeId};
#[cfg(feature = "blackboard-serde")]
pub use persistence::{BLACKBOARD_SNAPSHOT_VERSION, BlackboardCodec, BlackboardCodecError};
#[cfg(feature = "remote-config")]
pub use remote_config::{MAX_REMOTE_CONFIG_BYTES, RemoteConfigError, RemoteTreeLoader};
#[cfg(feature = "std")]
pub use shared::SharedBlackboard;
pub use snapshot::{
    NODE_ID_SNAPSHOT_VERSION, NodeIdSnapshot, NodeIdSnapshotError, RestoreError, SNAPSHOT_VERSION,
    TreeSnapshot,
};
#[cfg(feature = "std")]
pub use stats::{NodeStats, StatsTracer};
pub use status::Status;
pub use subtree::{SubtreeFactory, SubtreeSlot};
#[cfg(feature = "test-util")]
pub use testing::TestDriver;
pub use time::Time;
pub use trace::{AbortReason, MultiTracer, TraceEvent, Tracer};
pub use tree::{BehaviorTree, TreeDescription};

#[cfg(feature = "tracing")]
pub use trace::TracingTracer;

#[cfg(feature = "macros")]
pub use playact_macros::bt;

/// Centralized imports of common types and macros.
pub mod prelude {
    #[cfg(feature = "test-util")]
    pub use crate::TestDriver;
    #[cfg(feature = "tokio")]
    pub use crate::TokioTask;
    #[cfg(feature = "macros")]
    pub use crate::bt;
    pub use crate::event_key;
    pub use crate::key;
    pub use crate::{
        AbortReason, Action, AlwaysFail, AlwaysSucceed, BehaviorTree, BehaviorTreeBuilder,
        Blackboard, BoxedNode, BranchBuilder, BuildError, CommandBuffer, Condition, Context,
        Cooldown, EventKey, EventQueue, Guard, IdentifiedNode, IfThenElse, Inverter, Key, Kind,
        Limiter, MAX_REPEAT_STEPS_PER_TICK, MemParallel, MultiTracer, NODE_ID_SNAPSHOT_VERSION,
        Node, NodeId, NodeIdSnapshot, NodeIdSnapshotError, Parallel, Race, RandomSelector,
        ReactiveSelector, ReactiveSequence, Repeat, RestoreError, Retry, SNAPSHOT_VERSION,
        Selector, Sequence, SetValue, Status, SubtreeFactory, SubtreeSlot, Threshold, Throttle,
        Time, Timeout, TraceEvent, Tracer, TreeDescription, TreeSnapshot, TryAction, UntilFailure,
        UntilSuccess, UtilityChild, UtilitySelector, Wait, WaitForEvent, WaitUntil, WeightedChild,
        WeightedRandomSelector,
    };
    #[cfg(feature = "std")]
    pub use crate::{
        AsyncAction, AsyncTask, CancellationToken, NodeStats, SharedBlackboard, StatsTracer,
        ThreadTask,
    };
    #[cfg(feature = "blackboard-serde")]
    pub use crate::{BLACKBOARD_SNAPSHOT_VERSION, BlackboardCodec, BlackboardCodecError};
    #[cfg(feature = "hot-reload")]
    pub use crate::{HotReloadError, TreeWatcher};
    #[cfg(feature = "remote-config")]
    pub use crate::{MAX_REMOTE_CONFIG_BYTES, RemoteConfigError, RemoteTreeLoader};
    #[cfg(feature = "config")]
    pub use crate::{TreeParser, ValidationLimits};
}
