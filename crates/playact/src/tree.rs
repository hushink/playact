//! 行为树驱动外壳与结构描述。

use core::fmt::{self, Write};

use crate::blackboard::Blackboard;
use crate::context::Context;
use crate::node::{BoxedNode, Kind, Node};
use crate::status::Status;
use crate::time::Time;
use crate::trace::{AbortReason, Tracer};

/// 一棵完整的行为树。
///
/// 每个 AI 持有自己的实例；树持有黑板与可选 Tracer，world 由驱动方持有。
/// 树不能 `Clone`：需要多 AI 时用工厂函数分别构建，或把 root 通过
/// [`BehaviorTree::into_root`] 挂到别的树上。
pub struct BehaviorTree<W> {
    name: String,
    root: BoxedNode<W>,
    blackboard: Blackboard,
    tracer: Option<Box<dyn Tracer + Send>>,
    last_status: Option<Status>,
}

impl<W> BehaviorTree<W> {
    /// 用根节点构造，默认名为 `BehaviorTree`。
    #[must_use]
    pub fn new(root: BoxedNode<W>) -> Self {
        Self {
            name: String::from("BehaviorTree"),
            root,
            blackboard: Blackboard::new(),
            tracer: None,
            last_status: None,
        }
    }

    /// 覆盖树名。
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// 安装 Tracer；传 `None` 卸载。
    pub fn set_tracer(&mut self, tracer: Option<Box<dyn Tracer + Send>>) {
        self.tracer = tracer;
    }

    /// 链式安装 Tracer。
    #[must_use]
    pub fn with_tracer(mut self, tracer: Box<dyn Tracer + Send>) -> Self {
        self.tracer = Some(tracer);
        self
    }

    /// 驱动一次逻辑帧。
    pub fn tick(&mut self, world: &mut W, time: Time) -> Status {
        let status = {
            let Self {
                root,
                blackboard,
                tracer,
                ..
            } = self;
            let tracer_ref: Option<&mut (dyn Tracer + Send)> = match tracer.as_mut() {
                Some(tracer) => Some(&mut **tracer),
                None => None,
            };
            let mut ctx = Context::with_tracer(world, blackboard, time, tracer_ref);
            ctx.tick_root(root)
        };

        self.last_status = Some(status);
        status
    }

    /// 用外部黑板 tick；用于 [`crate::SharedBlackboard`] 或自定义共享状态。
    ///
    /// 本次 tick 不使用树自带的黑板；树自带的黑板保持不变。Tracer 仍然生效。
    /// 不要在 Running 中途切换黑板来源：节点可能已缓存黑板数据；需要切换时
    /// 应先 `abort` 并 `reset` 整棵树。
    pub fn tick_with_blackboard(
        &mut self,
        world: &mut W,
        time: Time,
        blackboard: &mut Blackboard,
    ) -> Status {
        let status = {
            let Self { root, tracer, .. } = self;
            let tracer_ref: Option<&mut (dyn Tracer + Send)> = match tracer.as_mut() {
                Some(tracer) => Some(&mut **tracer),
                None => None,
            };
            let mut ctx = Context::with_tracer(world, blackboard, time, tracer_ref);
            ctx.tick_root(root)
        };

        self.last_status = Some(status);
        status
    }

    /// 只复位节点内部状态，不清空黑板，也不会触发 `abort`。用于角色复活、
    /// 战斗重开。若当前是 Running 且需要外部清理，应先调用 [`BehaviorTree::abort`]。
    pub fn reset(&mut self) {
        self.root.reset();
        self.last_status = None;
    }

    /// 放弃整棵树：仅当上一次 tick 返回 `Running` 时，调用根节点 `abort`
    /// 做外部清理，再 `reset`。终态树没有需要清理的 Running 子树，本方法
    /// 只清除记录的状态。
    pub fn abort(&mut self, world: &mut W, time: Time) {
        if self.last_status != Some(Status::Running) {
            self.last_status = None;
            return;
        }

        {
            let Self {
                root,
                blackboard,
                tracer,
                ..
            } = self;
            let tracer_ref: Option<&mut (dyn Tracer + Send)> = match tracer.as_mut() {
                Some(tracer) => Some(&mut **tracer),
                None => None,
            };
            let mut ctx = Context::with_tracer(world, blackboard, time, tracer_ref);
            ctx.abort_root(root, AbortReason::TreeAbort);
        }

        self.last_status = None;
    }

    /// 用外部黑板放弃整棵树；语义与 [`BehaviorTree::abort`] 相同。
    pub fn abort_with_blackboard(
        &mut self,
        world: &mut W,
        time: Time,
        blackboard: &mut Blackboard,
    ) {
        if self.last_status != Some(Status::Running) {
            self.last_status = None;
            return;
        }

        {
            let Self { root, tracer, .. } = self;
            let tracer_ref: Option<&mut (dyn Tracer + Send)> = match tracer.as_mut() {
                Some(tracer) => Some(&mut **tracer),
                None => None,
            };
            let mut ctx = Context::with_tracer(world, blackboard, time, tracer_ref);
            ctx.abort_root(root, AbortReason::TreeAbort);
        }

        self.last_status = None;
    }

    /// 结构描述，用于调试与可视化。
    #[must_use]
    pub fn describe(&self) -> TreeDescription {
        TreeDescription::from_node(self.root.as_ref())
    }

    /// 只读黑板。
    #[must_use]
    pub fn blackboard(&self) -> &Blackboard {
        &self.blackboard
    }

    /// 可变黑板。
    pub fn blackboard_mut(&mut self) -> &mut Blackboard {
        &mut self.blackboard
    }

    /// 树名。
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 拆出根节点，用于把整棵树作为子树复用。
    ///
    /// 注意：会丢弃黑板与 Tracer，但保留 root 的内部状态。若 root 是
    /// Running 且依赖黑板数据（例如目标索引），挂到新树前需要重新准备
    /// 黑板；需要保留黑板/状态的子树复用请用工厂函数或 v0.2 的 Subtree。
    #[must_use]
    pub fn into_root(self) -> BoxedNode<W> {
        self.root
    }
}

/// 节点结构描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeDescription {
    /// 节点显示名。
    pub name: String,
    /// 节点类型标签。
    pub kind: Kind,
    /// 子节点描述。
    pub children: Vec<TreeDescription>,
}

impl TreeDescription {
    fn from_node<W>(node: &(dyn Node<W> + Send)) -> Self {
        Self {
            name: node.name().to_string(),
            kind: node.kind(),
            children: node
                .children()
                .iter()
                .map(|child| Self::from_node(child.as_ref()))
                .collect(),
        }
    }

    /// 生成 Graphviz DOT 字符串。
    #[must_use]
    pub fn to_dot(&self) -> String {
        fn visit(
            node: &TreeDescription,
            parent: Option<usize>,
            next: &mut usize,
            out: &mut String,
        ) {
            let id = *next;
            *next += 1;
            let _ = writeln!(
                out,
                "  n{id} [label=\"{}: {}\"];",
                escape_dot(node.kind.as_str()),
                escape_dot(&node.name)
            );
            if let Some(parent) = parent {
                let _ = writeln!(out, "  n{parent} -> n{id};");
            }
            for child in &node.children {
                visit(child, Some(id), next, out);
            }
        }

        let mut out = String::from("digraph playact {\n  rankdir=TB;\n");
        let mut next = 0;
        visit(self, None, &mut next, &mut out);
        out.push_str("}\n");
        out
    }
}

impl fmt::Display for TreeDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn write_node(
            node: &TreeDescription,
            f: &mut fmt::Formatter<'_>,
            prefix: &str,
            last: bool,
            root: bool,
        ) -> fmt::Result {
            if root {
                writeln!(f, "{} \"{}\"", node.kind, node.name)?;
            } else {
                writeln!(
                    f,
                    "{prefix}{}{} \"{}\"",
                    if last { "└── " } else { "├── " },
                    node.kind,
                    node.name
                )?;
            }

            let next_prefix = if root {
                String::new()
            } else {
                format!("{prefix}{}", if last { "    " } else { "│   " })
            };

            for (index, child) in node.children.iter().enumerate() {
                write_node(
                    child,
                    f,
                    &next_prefix,
                    index + 1 == node.children.len(),
                    false,
                )?;
            }
            Ok(())
        }

        write_node(self, f, "", true, true)
    }
}

fn escape_dot(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// 只改显示名、不改变状态语义的透明包装。
#[cfg_attr(not(feature = "config"), allow(dead_code))]
pub(crate) struct Named<W> {
    name: String,
    child: BoxedNode<W>,
}

#[cfg_attr(not(feature = "config"), allow(dead_code))]
impl<W> Named<W> {
    pub(crate) fn new(name: impl Into<String>, child: BoxedNode<W>) -> Self {
        Self {
            name: name.into(),
            child,
        }
    }
}

impl<W> Node<W> for Named<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        ctx.tick_child(0, &mut self.child)
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        ctx.abort_child(0, &mut self.child, AbortReason::Preempted);
    }

    fn reset(&mut self) {
        self.child.reset();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        self.child.kind()
    }

    fn children(&self) -> &[BoxedNode<W>] {
        core::slice::from_ref(&self.child)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    use crate::builder::BehaviorTreeBuilder;
    use crate::leaf::Action;
    use crate::trace::TraceEvent;

    use super::*;

    #[test]
    fn describe_and_dot_contain_structure() {
        let tree = BehaviorTreeBuilder::<bool>::new("demo")
            .sequence(|b| {
                b.condition("gate", |ctx: &Context<'_, bool>| *ctx.world_ref())
                    .action("act", |_| Status::Success)
            })
            .build()
            .expect("tree");

        let description = tree.describe();
        assert_eq!(description.kind, Kind::Sequence);

        let text = description.to_string();
        assert!(text.contains("condition \"gate\""));
        assert!(text.contains("action \"act\""));

        let dot = description.to_dot();
        assert!(dot.starts_with("digraph playact"));
        assert!(dot.contains("gate"));
        assert!(dot.contains("act"));
    }

    #[derive(Clone, Default)]
    struct CollectTrace(Arc<Mutex<Vec<String>>>);

    impl Tracer for CollectTrace {
        fn event(&mut self, event: TraceEvent<'_>) {
            self.0
                .lock()
                .expect("trace lock")
                .push(format!("{event:?}"));
        }
    }

    #[test]
    fn tracer_reports_preemption_abort() {
        let sink = CollectTrace::default();

        let mut tree = BehaviorTreeBuilder::<bool>::new("tracer_demo")
            .reactive_selector(|b| {
                b.sequence(|b| {
                    b.condition("high_gate", |ctx: &Context<'_, bool>| *ctx.world_ref())
                        .action("high_action", |_| Status::Success)
                })
                .action("low_action", |_| Status::Running)
            })
            .build()
            .expect("tree");
        tree.set_tracer(Some(Box::new(sink.clone())));

        let mut world = false;
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Running);

        world = true;
        assert_eq!(
            tree.tick(&mut world, Time::from_millis(250, 250)),
            Status::Success
        );

        let events = sink.0.lock().expect("trace lock");
        assert!(
            events
                .iter()
                .any(|event| event.contains("Abort") && event.contains("low_action")),
            "expected abort event, got {events:?}"
        );
    }

    #[test]
    fn tree_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<BehaviorTree<()>>();
    }

    #[test]
    fn tree_is_send_even_if_world_is_not() {
        struct NotSend {
            _rc: std::rc::Rc<()>,
        }
        fn assert_send<T: Send>() {}
        assert_send::<BehaviorTree<NotSend>>();
    }

    #[test]
    fn abort_only_propagates_from_running_tree() {
        let terminal_aborts = Arc::new(AtomicU32::new(0));
        let terminal_cb = Arc::clone(&terminal_aborts);
        let mut terminal = BehaviorTree::new(Box::new(
            Action::<()>::new("root", |_| Status::Success).with_abort(
                move |_: &mut Context<'_, ()>| {
                    terminal_cb.fetch_add(1, Ordering::Relaxed);
                },
            ),
        ));

        let mut world = ();
        assert_eq!(terminal.tick(&mut world, Time::ZERO), Status::Success);
        terminal.abort(&mut world, Time::ZERO);
        assert_eq!(terminal_aborts.load(Ordering::Relaxed), 0);

        let running_aborts = Arc::new(AtomicU32::new(0));
        let running_cb = Arc::clone(&running_aborts);
        let mut running = BehaviorTree::new(Box::new(
            Action::<()>::new("root", |_| Status::Running).with_abort(
                move |_: &mut Context<'_, ()>| {
                    running_cb.fetch_add(1, Ordering::Relaxed);
                },
            ),
        ));

        assert_eq!(running.tick(&mut world, Time::ZERO), Status::Running);
        running.abort(&mut world, Time::ZERO);
        assert_eq!(running_aborts.load(Ordering::Relaxed), 1);

        // 第二次 abort 是幂等的；上一次状态已清空。
        running.abort(&mut world, Time::ZERO);
        assert_eq!(running_aborts.load(Ordering::Relaxed), 1);
    }
}
