//! 异步/远程动作：tick 内只轮询任务，不阻塞逻辑帧。
//!
//! 引擎不绑定任何 async runtime：
//! - [`AsyncTask`] 是运行时无关的任务句柄，用户可为 Tokio、futures、远程
//!   RPC 等实现它；
//! - [`AsyncAction`] 每个 tick 调用一次 `poll`，`Running` 继续、终态结束；
//! - 被抢占时调用 `cancel`，实现方应尽量协作取消远端任务；
//! - 内置 [`ThreadTask`] 用标准库线程跑阻塞调用/远程请求，并用
//!   [`CancellationToken`] 做协作取消。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use crate::context::Context;
use crate::node::{Kind, Node};
use crate::status::Status;

/// 运行时无关的异步任务句柄。
///
/// `poll` 每个逻辑帧调用一次；`cancel` 在动作被抢占/复位时调用。
/// 取消是协作式的：实现应尽快停止远端工作并丢弃结果。
pub trait AsyncTask: Send {
    /// 推进任务；`Running` 表示尚未结束。
    fn poll(&mut self) -> Status;

    /// 请求取消。引擎随后会 drop 任务；实现应幂等。
    fn cancel(&mut self);
}

/// 协作取消信号。
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    flag: Arc<AtomicBool>,
}

impl CancellationToken {
    /// 创建未取消的 token。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 请求取消；幂等。
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
    }

    /// 是否已取消。
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

/// 用标准库线程执行阻塞/远程工作的任务。
///
/// 线程只负责执行 `work` 并通过 channel 回报 `Status`；tick 侧只做
/// `try_recv`，不会阻塞。`cancel` 设置 [`CancellationToken`]，线程应检查
/// token 后尽快退出。若线程长时间不检查 token，drop 会 detach 线程而不是
/// 强制终止它。
pub struct ThreadTask {
    result: Receiver<Status>,
    cancel: CancellationToken,
    _handle: Option<thread::JoinHandle<()>>,
}

impl ThreadTask {
    /// 启动一个工作线程。
    ///
    /// 线程创建失败会退化为一个立即 `Failure` 的任务（不会 panic）。
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

type StartFn<W> = Box<dyn for<'a> FnMut(&mut Context<'a, W>) -> Box<dyn AsyncTask> + Send>;

/// 异步/远程动作叶子。
///
/// 第一次 tick 调用 `start` 创建任务；之后每个 tick 轮询任务。任务返回
/// 终态后动作结束，下一次激活重新创建任务。被抢占/复位时调用 `cancel`。
pub struct AsyncAction<W> {
    name: String,
    start: StartFn<W>,
    task: Option<Box<dyn AsyncTask>>,
}

impl<W> AsyncAction<W> {
    /// 用名字和任务工厂构造。
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

    /// 覆盖显示名。
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

        // 新激活后再取消。
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
}
