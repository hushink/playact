//! Async/remote actions: ticks only poll the task and never block the logic
//! frame.
//!
//! The engine is not tied to any async runtime:
//! - [`AsyncTask`] is a runtime-agnostic task handle; users can implement it for
//!   Tokio, futures, remote RPC, and so on;
//! - [`AsyncAction`] calls `poll` once per tick, continuing on `Running` and
//!   finishing on a terminal status;
//! - `cancel` is called on preemption; implementations should cooperatively
//!   cancel remote work where possible;
//! - the built-in [`ThreadTask`] runs blocking calls or remote requests on a
//!   standard-library thread and uses [`CancellationToken`] for cooperative
//!   cancellation.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use crate::context::Context;
use crate::node::{Kind, Node};
use crate::status::Status;

/// A runtime-agnostic async task handle.
///
/// `poll` is called once per logic frame; `cancel` is called when the action is
/// preempted or reset. Cancellation is cooperative: implementations should stop
/// remote work as soon as possible and discard its result.
pub trait AsyncTask: Send {
    /// Advances the task; `Running` means it has not finished yet.
    fn poll(&mut self) -> Status;

    /// Requests cancellation. The engine drops the task afterwards;
    /// implementations should be idempotent.
    fn cancel(&mut self);
}

/// A cooperative cancellation signal.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    flag: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Creates a token that is not yet cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation; idempotent.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
    }

    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

/// A task that performs blocking or remote work on a standard-library thread.
///
/// The thread only runs `work` and reports the `Status` through a channel; the
/// tick side only calls `try_recv`, so it never blocks. `cancel` sets the
/// [`CancellationToken`], and the thread should check the token and exit
/// promptly. If the thread does not check the token for a long time, drop
/// detaches it rather than force-terminating it.
pub struct ThreadTask {
    result: Receiver<Status>,
    cancel: CancellationToken,
    _handle: Option<thread::JoinHandle<()>>,
}

impl ThreadTask {
    /// Spawns a worker thread.
    ///
    /// If thread creation fails, this degrades to a task that immediately
    /// returns `Failure` (without panicking).
    #[must_use]
    pub fn spawn<F>(work: F) -> Self
    where
        F: FnOnce(CancellationToken) -> Status + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        let cancel = CancellationToken::new();
        let token = cancel.clone();

        let handle = thread::Builder::new()
            .spawn(move || {
                let status = work(token);
                let _ = sender.send(status);
            })
            .ok();

        Self {
            result: receiver,
            cancel,
            _handle: handle,
        }
    }
}

impl AsyncTask for ThreadTask {
    fn poll(&mut self) -> Status {
        match self.result.try_recv() {
            Ok(status) => status,
            Err(TryRecvError::Empty) => Status::Running,
            Err(TryRecvError::Disconnected) => Status::Failure,
        }
    }

    fn cancel(&mut self) {
        self.cancel.cancel();
    }
}

impl Drop for ThreadTask {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// A task handle that runs async work on a Tokio runtime.
///
/// The task runs inside Tokio and the tick side only calls `try_recv`;
/// `cancel` / drop call `JoinHandle::abort`. The future must map business errors
/// to `Status` itself.
///
/// Calling [`TokioTask::spawn`] outside a Tokio runtime does not panic; it
/// returns a task that immediately returns `Failure`. Alternatively, pass an
/// explicit handle to [`TokioTask::spawn_on`].
#[cfg(feature = "tokio")]
pub struct TokioTask {
    result: tokio::sync::oneshot::Receiver<Status>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

#[cfg(feature = "tokio")]
impl TokioTask {
    /// Spawns on the current Tokio runtime.
    #[must_use]
    pub fn spawn<F>(future: F) -> Self
    where
        F: core::future::Future<Output = Status> + Send + 'static,
    {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => Self::spawn_on(&handle, future),
            Err(_) => Self::failed(),
        }
    }

    /// Spawns on the given runtime handle; the runtime must still be alive.
    #[must_use]
    pub fn spawn_on<F>(handle: &tokio::runtime::Handle, future: F) -> Self
    where
        F: core::future::Future<Output = Status> + Send + 'static,
    {
        let (sender, result) = tokio::sync::oneshot::channel();
        let join = handle.spawn(async move {
            let status = future.await;
            let _ = sender.send(status);
        });

        Self {
            result,
            handle: Some(join),
        }
    }

    fn failed() -> Self {
        let (sender, result) = tokio::sync::oneshot::channel::<Status>();
        drop(sender);
        Self {
            result,
            handle: None,
        }
    }
}

#[cfg(feature = "tokio")]
impl AsyncTask for TokioTask {
    fn poll(&mut self) -> Status {
        match self.result.try_recv() {
            Ok(status) => status,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => Status::Running,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => Status::Failure,
        }
    }

    fn cancel(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

#[cfg(feature = "tokio")]
impl Drop for TokioTask {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }
}

type StartFn<W> = Box<dyn for<'a> FnMut(&mut Context<'a, W>) -> Box<dyn AsyncTask> + Send>;

/// An async/remote action leaf.
///
/// The first tick calls `start` to create the task; subsequent ticks poll it.
/// The action ends once the task returns a terminal status, and a new task is
/// created on the next activation. `cancel` is called on preemption or reset.
pub struct AsyncAction<W> {
    name: String,
    start: StartFn<W>,
    task: Option<Box<dyn AsyncTask>>,
}

impl<W> AsyncAction<W> {
    /// Constructs from a name and a task factory.
    pub fn new<F>(name: impl Into<String>, start: F) -> Self
    where
        F: for<'a> FnMut(&mut Context<'a, W>) -> Box<dyn AsyncTask> + Send + 'static,
    {
        Self {
            name: name.into(),
            start: Box::new(start),
            task: None,
        }
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<W> Node<W> for AsyncAction<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        if self.task.is_none() {
            self.task = Some((self.start)(ctx));
        }

        let status = match &mut self.task {
            Some(task) => task.poll(),
            None => Status::Failure,
        };

        if status.is_terminal() {
            self.task = None;
        }

        status
    }

    fn abort(&mut self, _ctx: &mut Context<'_, W>) {
        if let Some(mut task) = self.task.take() {
            task.cancel();
        }
    }

    fn reset(&mut self) {
        if let Some(mut task) = self.task.take() {
            task.cancel();
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::AsyncAction
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use crate::blackboard::Blackboard;

    use super::*;

    struct FakeTask {
        polls: u32,
        cancelled: Arc<AtomicBool>,
    }

    impl AsyncTask for FakeTask {
        fn poll(&mut self) -> Status {
            self.polls += 1;
            if self.polls < 2 {
                Status::Running
            } else {
                Status::Success
            }
        }

        fn cancel(&mut self) {
            self.cancelled.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn async_action_polls_and_cancels() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_for_start = Arc::clone(&cancelled);

        let mut action = AsyncAction::<()>::new("fake", move |_| {
            Box::new(FakeTask {
                polls: 0,
                cancelled: Arc::clone(&cancelled_for_start),
            }) as Box<dyn AsyncTask>
        });

        let mut world = ();
        let mut board = Blackboard::new();
        let mut ctx = Context::new(&mut world, &mut board, crate::time::Time::ZERO);

        assert_eq!(action.tick(&mut ctx), Status::Running);
        assert_eq!(action.tick(&mut ctx), Status::Success);

        // Cancel after a new activation.
        assert_eq!(action.tick(&mut ctx), Status::Running);
        action.abort(&mut ctx);
        assert!(cancelled.load(Ordering::Relaxed));
    }

    #[test]
    fn thread_task_completes_without_blocking_tick() {
        let mut action = AsyncAction::<()>::new("thread", |_| {
            Box::new(ThreadTask::spawn(|_| Status::Success)) as Box<dyn AsyncTask>
        });

        let mut world = ();
        let mut board = Blackboard::new();
        let mut ctx = Context::new(&mut world, &mut board, crate::time::Time::ZERO);

        let mut status = action.tick(&mut ctx);
        for _ in 0..2_000 {
            if status.is_terminal() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
            status = action.tick(&mut ctx);
        }

        assert_eq!(status, Status::Success);
    }

    #[cfg(feature = "tokio")]
    #[test]
    fn tokio_task_completes_on_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");

        runtime.block_on(async {
            let mut task = TokioTask::spawn(async { Status::Success });
            let mut status = task.poll();
            for _ in 0..64 {
                if status.is_terminal() {
                    break;
                }
                tokio::task::yield_now().await;
                status = task.poll();
            }
            assert_eq!(status, Status::Success);
        });
    }

    #[cfg(feature = "tokio")]
    #[test]
    fn tokio_task_cancels() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");

        runtime.block_on(async {
            let mut task = TokioTask::spawn(std::future::pending::<Status>());
            task.cancel();

            let mut status = task.poll();
            for _ in 0..64 {
                if status.is_terminal() {
                    break;
                }
                tokio::task::yield_now().await;
                status = task.poll();
            }
            assert_eq!(status, Status::Failure);
        });
    }

    #[cfg(feature = "tokio")]
    #[test]
    fn tokio_task_outside_runtime_fails() {
        let mut task = TokioTask::spawn(async { Status::Success });
        assert_eq!(task.poll(), Status::Failure);
    }
}
