//! 行为树驱动外壳与结构描述。

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
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
    services: Vec<BoxedNode<W>>,
    service_status: Vec<Option<Status>>,
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
            services: Vec::new(),
            service_status: Vec::new(),
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

    /// 追加一个服务节点。
    ///
    /// 服务在每个 tick 的根节点之前运行，与主树共享 world / time / 黑板；
    /// 返回值被忽略，适合感知、缓存或维护外部状态。服务子树不产生 Tracer
    /// 事件，也不影响 [`BehaviorTree::last_status`]。
    ///
    /// 服务的 `abort` / `reset` 随整棵树一起传播；服务需要自己管理终态，
    /// 引擎只记录状态用于判断是否在 tree abort 时清理。
    #[must_use]
    pub fn with_service(mut self, service: BoxedNode<W>) -> Self {
        self.add_service(service);
        self
    }

    /// 追加一个服务节点。
    pub fn add_service(&mut self, service: BoxedNode<W>) {
        self.services.push(service);
        self.service_status.push(None);
    }

    /// 服务节点列表。
    #[must_use]
    pub fn services(&self) -> &[BoxedNode<W>] {
        &self.services
    }

    /// 服务节点数量。
    #[must_use]
    pub fn service_count(&self) -> usize {
        self.services.len()
    }

    /// 驱动一次逻辑帧。
    pub fn tick(&mut self, world: &mut W, time: Time) -> Status {
        let status = {
            let Self {
                root,
                blackboard,
                tracer,
                services,
                service_status,
                ..
            } = self;
            let tracer_ref: Option<&mut (dyn Tracer + Send)> = match tracer.as_mut() {
                Some(tracer) => Some(&mut **tracer),
                None => None,
            };
            let mut ctx = Context::with_tracer(world, blackboard, time, tracer_ref);

            for (service, status) in services.iter_mut().zip(service_status.iter_mut()) {
                let mut service_ctx = ctx.service_context();
                *status = Some(service.tick(&mut service_ctx));
            }

            ctx.tick_root(root)
        };

        self.last_status = Some(status);
        status
    }

    /// 用外部黑板 tick；用于 `SharedBlackboard` 或自定义共享状态。
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
            let Self {
                root,
                tracer,
                services,
                service_status,
                ..
            } = self;
            let tracer_ref: Option<&mut (dyn Tracer + Send)> = match tracer.as_mut() {
                Some(tracer) => Some(&mut **tracer),
                None => None,
            };
            let mut ctx = Context::with_tracer(world, blackboard, time, tracer_ref);

            for (service, status) in services.iter_mut().zip(service_status.iter_mut()) {
                let mut service_ctx = ctx.service_context();
                *status = Some(service.tick(&mut service_ctx));
            }

            ctx.tick_root(root)
        };

        self.last_status = Some(status);
        status
    }

    /// 只复位节点内部状态，不清空黑板，也不会触发 `abort`。用于角色复活、
    /// 战斗重开。若当前是 Running 且需要外部清理，应先调用 [`BehaviorTree::abort`]。
    pub fn reset(&mut self) {
        self.root.reset();
        for service in &mut self.services {
            service.reset();
        }
        self.service_status.fill(None);
        self.last_status = None;
    }

    /// 放弃整棵树：根节点仅在最后一次 tick 返回 `Running` 时执行 `abort`，
    /// 随后 `reset`；服务节点中上次返回 `Running` 的也会 `abort`，所有服务
    /// 都会 `reset`。终态树没有需要清理的 Running 子树。
    pub fn abort(&mut self, world: &mut W, time: Time) {
        let root_running = self.last_status == Some(Status::Running);

        {
            let Self {
                root,
                blackboard,
                tracer,
                services,
                service_status,
                ..
            } = self;
            let tracer_ref: Option<&mut (dyn Tracer + Send)> = match tracer.as_mut() {
                Some(tracer) => Some(&mut **tracer),
                None => None,
            };
            let mut ctx = Context::with_tracer(world, blackboard, time, tracer_ref);

            if root_running {
                ctx.abort_root(root, AbortReason::TreeAbort);
            }

            for (service, status) in services.iter_mut().zip(service_status.iter()) {
                let mut service_ctx = ctx.service_context();
                if *status == Some(Status::Running) {
                    service.abort(&mut service_ctx);
                }
                service.reset();
            }
        }

        self.service_status.fill(None);
        self.last_status = None;
    }

    /// 用外部黑板放弃整棵树；语义与 [`BehaviorTree::abort`] 相同。
    pub fn abort_with_blackboard(
        &mut self,
        world: &mut W,
        time: Time,
        blackboard: &mut Blackboard,
    ) {
        let root_running = self.last_status == Some(Status::Running);

        {
            let Self {
                root,
                tracer,
                services,
                service_status,
                ..
            } = self;
            let tracer_ref: Option<&mut (dyn Tracer + Send)> = match tracer.as_mut() {
                Some(tracer) => Some(&mut **tracer),
                None => None,
            };
            let mut ctx = Context::with_tracer(world, blackboard, time, tracer_ref);

            if root_running {
                ctx.abort_root(root, AbortReason::TreeAbort);
            }

            for (service, status) in services.iter_mut().zip(service_status.iter()) {
                let mut service_ctx = ctx.service_context();
                if *status == Some(Status::Running) {
                    service.abort(&mut service_ctx);
                }
                service.reset();
            }
        }

        self.service_status.fill(None);
        self.last_status = None;
    }

    /// 结构描述，用于调试与可视化。
    #[must_use]
    pub fn describe(&self) -> TreeDescription {
        TreeDescription::from_node(self.root.as_ref())
    }

    /// 服务节点的结构描述，保持注册顺序。
    #[must_use]
    pub fn describe_services(&self) -> Vec<TreeDescription> {
        self.services
            .iter()
            .map(|service| TreeDescription::from_node(service.as_ref()))
            .collect()
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

    /// 上一次 tick 的顶层状态。
    ///
    /// 尚未 tick 或刚执行过 `reset` / `abort` 时返回 `None`。
    #[must_use]
    pub fn last_status(&self) -> Option<Status> {
        self.last_status
    }

    /// 上一次 tick 是否返回 `Running`。
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.last_status == Some(Status::Running)
    }

    /// 拆出根节点，用于把整棵树作为子树复用。
    ///
    /// 注意：会丢弃黑板、Tracer 与服务节点，但保留 root 的内部状态。若 root
    /// 是 Running 且依赖黑板数据（例如目标索引），挂到新树前需要重新准备
    /// 黑板；需要保留黑板/状态的子树复用请用工厂函数或 `SubtreeFactory`。
    #[must_use]
    pub fn into_root(self) -> BoxedNode<W> {
        self.root
    }
}

/// 整棵行为树可以直接作为节点嵌套进另一棵树。
///
/// 嵌套树共享外层 tick 的 world、time、黑板与 Tracer：
/// - 它自带的黑板不参与嵌套 tick；跨树共享数据请用外层黑板或
///   `SharedBlackboard`；
/// - 它自带的 Tracer 不生效，外层 Tracer 会看到嵌套树根及其子树；
/// - 被外层抢占时按 `abort → reset` 传播到嵌套树根；
/// - 在 `describe()` 中显示为 `tree "<名字>"`，其唯一子节点是原根节点。
///
/// 通过 builder 的 `.raw(Box::new(tree))` 挂载；需要可重复实例化时用
/// [`crate::SubtreeFactory`]。
impl<W> Node<W> for BehaviorTree<W> {
    fn tick(&mut self, ctx: &mut Context<'_, W>) -> Status {
        for (service, status) in self.services.iter_mut().zip(self.service_status.iter_mut()) {
            let mut service_ctx = ctx.service_context();
            *status = Some(service.tick(&mut service_ctx));
        }

        let status = ctx.tick_child(0, &mut self.root);
        self.last_status = Some(status);
        status
    }

    fn abort(&mut self, ctx: &mut Context<'_, W>) {
        ctx.abort_child(0, &mut self.root, AbortReason::Preempted);

        for index in 0..self.services.len() {
            let running = self.service_status.get(index) == Some(&Some(Status::Running));
            let mut service_ctx = ctx.service_context();
            if running {
                self.services[index].abort(&mut service_ctx);
            }
            self.services[index].reset();
        }
        self.service_status.fill(None);
        self.last_status = None;
    }

    fn reset(&mut self) {
        self.root.reset();
        for service in &mut self.services {
            service.reset();
        }
        self.service_status.fill(None);
        self.last_status = None;
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> Kind {
        Kind::Tree
    }

    fn children(&self) -> &[BoxedNode<W>] {
        core::slice::from_ref(&self.root)
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
                alloc::format!("{prefix}{}", if last { "    " } else { "│   " })
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

    use crate::blackboard::Key;
    use crate::builder::BehaviorTreeBuilder;
    use crate::leaf::{Action, Condition};
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
    fn last_status_tracks_tick_and_reset() {
        let mut tree = BehaviorTree::new(Box::new(Action::<()>::new("hold", |_| Status::Running)));
        let mut world = ();

        assert_eq!(tree.last_status(), None);
        assert!(!tree.is_running());

        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Running);
        assert_eq!(tree.last_status(), Some(Status::Running));
        assert!(tree.is_running());

        tree.reset();
        assert_eq!(tree.last_status(), None);
        assert!(!tree.is_running());
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

    #[test]
    fn service_runs_before_root_and_shares_blackboard() {
        static SEEN: Key<bool> = crate::key!("service_seen");

        let service = Action::<()>::new("sense", |ctx| {
            ctx.blackboard().set(&SEEN, true);
            Status::Success
        });
        let root = Condition::<()>::new("check", |ctx| {
            ctx.blackboard_ref().get(&SEEN) == Some(&true)
        });

        let mut tree = BehaviorTree::new(Box::new(root)).with_service(Box::new(service));
        assert_eq!(tree.service_count(), 1);

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        assert_eq!(tree.describe_services()[0].name, "sense");
    }

    #[test]
    fn tree_abort_and_reset_reach_services() {
        let service_aborts = Arc::new(AtomicU32::new(0));
        let service_aborts_cb = Arc::clone(&service_aborts);
        let service_resets = Arc::new(AtomicU32::new(0));
        let service_resets_cb = Arc::clone(&service_resets);

        let service = Action::<()>::new("monitor", |_| Status::Running)
            .with_abort(move |_| {
                service_aborts_cb.fetch_add(1, Ordering::Relaxed);
            })
            .with_reset(move || {
                service_resets_cb.fetch_add(1, Ordering::Relaxed);
            });

        let mut tree = BehaviorTree::new(Box::new(Action::<()>::new("root", |_| Status::Success)))
            .with_service(Box::new(service));

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        assert_eq!(tree.last_status(), Some(Status::Success));

        // 根节点已终止，但 Running 服务仍需清理。
        tree.abort(&mut world, Time::from_millis(16, 16));
        assert_eq!(service_aborts.load(Ordering::Relaxed), 1);

        let resets_after_abort = service_resets.load(Ordering::Relaxed);
        assert!(resets_after_abort >= 1);
        tree.reset();
        assert!(service_resets.load(Ordering::Relaxed) > resets_after_abort);
    }

    #[test]
    fn nested_tree_runs_its_services() {
        static SEEN: Key<u32> = crate::key!("nested_service_seen");

        let service = Action::<()>::new("nested_sense", |ctx| {
            ctx.blackboard().set(&SEEN, 3);
            Status::Success
        });
        let root = Condition::<()>::new("nested_check", |ctx| {
            ctx.blackboard_ref().get(&SEEN) == Some(&3)
        });
        let inner = BehaviorTree::new(Box::new(root)).with_service(Box::new(service));

        let mut outer = BehaviorTreeBuilder::<()>::new("outer")
            .sequence(|b| b.raw(Box::new(inner)))
            .build()
            .expect("outer tree");

        let mut world = ();
        assert_eq!(outer.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn behavior_tree_can_be_nested_as_node() {
        static MARK: Key<u32> = crate::key!("nested_mark");

        let inner = BehaviorTreeBuilder::<u32>::new("inner")
            .sequence(|b| {
                b.action("mark", |ctx| {
                    ctx.blackboard().set(&MARK, 5);
                    Status::Success
                })
                .action("inc", |ctx| {
                    *ctx.world() += 1;
                    Status::Success
                })
            })
            .build()
            .expect("inner tree");

        let mut outer = BehaviorTreeBuilder::<u32>::new("outer")
            .sequence(|b| {
                b.raw(Box::new(inner)).action("read_mark", |ctx| {
                    assert_eq!(ctx.blackboard_ref().get(&MARK), Some(&5));
                    *ctx.world() += 1;
                    Status::Success
                })
            })
            .build()
            .expect("outer tree");

        let description = outer.describe();
        assert_eq!(description.kind, Kind::Sequence);
        assert_eq!(description.children[0].kind, Kind::Tree);
        assert_eq!(description.children[0].kind.as_str(), "tree");
        assert_eq!(description.children[0].name, "inner");
        assert_eq!(description.children[0].children.len(), 1);

        let mut world = 0;
        assert_eq!(outer.tick(&mut world, Time::ZERO), Status::Success);
        assert_eq!(world, 2);
        assert_eq!(outer.blackboard().get(&MARK), Some(&5));
    }

    #[test]
    fn nested_tree_propagates_abort_to_running_root() {
        let aborts = Arc::new(AtomicU32::new(0));
        let abort_cb = Arc::clone(&aborts);

        let inner = BehaviorTree::new(Box::new(
            Action::<bool>::new("hold", |_| Status::Running).with_abort(
                move |_: &mut Context<'_, bool>| {
                    abort_cb.fetch_add(1, Ordering::Relaxed);
                },
            ),
        ))
        .with_name("inner");

        let mut outer = BehaviorTreeBuilder::<bool>::new("outer")
            .reactive_selector(|b| {
                b.condition("preempt", |ctx: &Context<'_, bool>| *ctx.world_ref())
                    .raw(Box::new(inner))
            })
            .build()
            .expect("outer tree");

        let mut world = false;
        assert_eq!(outer.tick(&mut world, Time::ZERO), Status::Running);
        assert_eq!(aborts.load(Ordering::Relaxed), 0);

        world = true;
        assert_eq!(
            outer.tick(&mut world, Time::from_millis(100, 100)),
            Status::Success
        );
        assert_eq!(aborts.load(Ordering::Relaxed), 1);
    }
}
