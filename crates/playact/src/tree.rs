//! Behavior tree driver shell and structure description.

use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::{self, Write};

use crate::blackboard::Blackboard;
use crate::context::Context;
use crate::node::{BoxedNode, Kind, Node, NodeId};
use crate::snapshot::{load_children, save_children, state};
use crate::status::Status;
use crate::time::Time;
use crate::trace::{AbortReason, Tracer};

/// A complete behavior tree.
///
/// Each AI owns its own instance; the tree owns the blackboard and an optional
/// tracer, while the world is owned by the driver. The tree is not `Clone`: for
/// multiple AIs, build one per AI with a factory function, or attach the root to
/// another tree via [`BehaviorTree::into_root`].
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
    /// Builds from a root node; the default name is `BehaviorTree`.
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

    /// Overrides the tree name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Installs a tracer; pass `None` to uninstall it.
    pub fn set_tracer(&mut self, tracer: Option<Box<dyn Tracer + Send>>) {
        self.tracer = tracer;
    }

    /// Installs a tracer in builder style.
    #[must_use]
    pub fn with_tracer(mut self, tracer: Box<dyn Tracer + Send>) -> Self {
        self.tracer = Some(tracer);
        self
    }

    /// Appends a service node.
    ///
    /// Services run before the root node on every tick and share the world /
    /// time / blackboard with the main tree; their return values are ignored,
    /// which suits perception, caching, or maintaining external state. Service
    /// subtrees emit no tracer events and do not affect
    /// [`BehaviorTree::last_status`].
    ///
    /// A service's `abort` / `reset` propagates with the whole tree. Services
    /// manage their own terminal state; the engine only records each status to
    /// decide whether cleanup is needed on tree abort.
    #[must_use]
    pub fn with_service(mut self, service: BoxedNode<W>) -> Self {
        self.add_service(service);
        self
    }

    /// Appends a service node.
    pub fn add_service(&mut self, service: BoxedNode<W>) {
        self.services.push(service);
        self.service_status.push(None);
    }

    /// The list of service nodes.
    #[must_use]
    pub fn services(&self) -> &[BoxedNode<W>] {
        &self.services
    }

    /// The number of service nodes.
    #[must_use]
    pub fn service_count(&self) -> usize {
        self.services.len()
    }

    /// Returns the first [`NodeId`] that appears more than once across the root
    /// and services.
    ///
    /// Traversal follows [`Node::children`], matching ID-keyed snapshots. Dynamic
    /// `SubtreeSlot` children and the services of a nested `BehaviorTree` are not
    /// traversed. Use [`BehaviorTree::duplicate_node_id_path`] when the location is
    /// needed as well.
    #[must_use]
    pub fn duplicate_node_id(&self) -> Option<NodeId> {
        self.duplicate_node_id_path().map(|(id, _)| id)
    }

    /// Like [`BehaviorTree::duplicate_node_id`], but also returns the structural
    /// path of the second occurrence.
    #[must_use]
    pub fn duplicate_node_id_path(&self) -> Option<(NodeId, NodePath)> {
        fn visit<W>(
            node: &(dyn Node<W> + Send),
            seen: &mut BTreeSet<NodeId>,
            path: NodePath,
        ) -> Option<(NodeId, NodePath)> {
            if let Some(id) = node.id()
                && !seen.insert(id.clone())
            {
                return Some((id.clone(), path));
            }
            for (index, child) in node.children().iter().enumerate() {
                if let Some(found) = visit(child.as_ref(), seen, path.with_child(index)) {
                    return Some(found);
                }
            }
            None
        }

        let mut seen = BTreeSet::new();
        if let Some(found) = visit(self.root.as_ref(), &mut seen, NodePath::root()) {
            return Some(found);
        }
        for (index, service) in self.services.iter().enumerate() {
            if let Some(found) = visit(service.as_ref(), &mut seen, NodePath::service(index)) {
                return Some(found);
            }
        }
        None
    }

    /// Drives one logic frame.
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

    /// Ticks with an external blackboard; intended for `SharedBlackboard` or
    /// custom shared state.
    ///
    /// This tick does not use the tree's own blackboard, which stays untouched.
    /// The tracer still applies. Do not switch blackboard sources while the
    /// tree is Running: nodes may have cached blackboard data. To switch, first
    /// `abort` and `reset` the whole tree.
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

    /// Resets only internal node state; does not clear the blackboard and does
    /// not trigger `abort`. Intended for respawns and combat restarts. If the
    /// tree is currently Running and external cleanup is needed, call
    /// [`BehaviorTree::abort`] first.
    pub fn reset(&mut self) {
        self.root.reset();
        for service in &mut self.services {
            service.reset();
        }
        self.service_status.fill(None);
        self.last_status = None;
    }

    /// Abandons the whole tree: the root runs `abort` only if its last tick
    /// returned `Running`, and is then `reset`; services that last returned
    /// `Running` are also aborted, and all services are `reset`. A terminal
    /// tree has no Running subtree that needs cleanup.
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

    /// Abandons the whole tree using an external blackboard; semantics match
    /// [`BehaviorTree::abort`].
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

    /// A structural description for debugging and visualization.
    #[must_use]
    pub fn describe(&self) -> TreeDescription {
        TreeDescription::from_node(self.root.as_ref())
    }

    /// Structural descriptions of the service nodes, in registration order.
    #[must_use]
    pub fn describe_services(&self) -> Vec<TreeDescription> {
        self.services
            .iter()
            .map(|service| TreeDescription::from_node(service.as_ref()))
            .collect()
    }

    /// The blackboard, read-only.
    #[must_use]
    pub fn blackboard(&self) -> &Blackboard {
        &self.blackboard
    }

    /// The blackboard, mutable.
    pub fn blackboard_mut(&mut self) -> &mut Blackboard {
        &mut self.blackboard
    }

    /// The tree name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The top-level status of the last tick.
    ///
    /// Returns `None` before the first tick or right after `reset` / `abort`.
    #[must_use]
    pub fn last_status(&self) -> Option<Status> {
        self.last_status
    }

    /// Whether the last tick returned `Running`.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.last_status == Some(Status::Running)
    }

    /// Exports a state snapshot of the root and service nodes.
    ///
    /// The snapshot excludes the world, blackboard values, closure-captured
    /// state, `AsyncAction` tasks, and the dynamic state of `SubtreeSlot`;
    /// callers must persist or rebuild those separately.
    #[must_use]
    pub fn snapshot(&self) -> crate::snapshot::TreeSnapshot {
        crate::snapshot::capture(&self.root, &self.services)
    }

    /// Restores node state from a snapshot.
    ///
    /// The root and service nodes are `reset` first; then each node's state is
    /// restored from the snapshot, and `last_status` plus the service status
    /// records are cleared. Use the same tree structure the snapshot was taken
    /// from; the number of services must match. Nodes that do not support state
    /// snapshots are left in their post-reset state. On failure the tree may be
    /// partially restored, so callers should `reset` again as needed.
    pub fn restore(
        &mut self,
        snapshot: &crate::snapshot::TreeSnapshot,
    ) -> Result<(), crate::snapshot::RestoreError> {
        let result = crate::snapshot::apply(&mut self.root, &mut self.services, snapshot);
        self.last_status = None;
        self.service_status.fill(None);
        result
    }

    /// Exports state keyed by stable [`NodeId`].
    ///
    /// Only nodes that have an ID and implement
    /// [`Node::save_own_state`] contribute entries. The result is independent of
    /// child order, so it can be restored into a reordered configuration.
    pub fn snapshot_by_id(
        &self,
    ) -> Result<crate::snapshot::NodeIdSnapshot, crate::snapshot::NodeIdSnapshotError> {
        crate::snapshot::capture_by_id(&self.root, &self.services)
    }

    /// Restores state from an ID-keyed snapshot.
    ///
    /// The tree is reset first, then each node with a matching ID loads its own
    /// state. Snapshot IDs not present in the tree, duplicate IDs, or rejected
    /// state return an error. Nodes without IDs or without own-state support are
    /// left in their reset state.
    pub fn restore_by_id(
        &mut self,
        snapshot: &crate::snapshot::NodeIdSnapshot,
    ) -> Result<(), crate::snapshot::NodeIdSnapshotError> {
        let result = crate::snapshot::apply_by_id(&mut self.root, &mut self.services, snapshot);
        self.last_status = None;
        self.service_status.fill(None);
        result
    }

    /// Restores state from an ID-keyed snapshot for IDs that exist in this tree.
    ///
    /// Snapshot entries without a matching ID are ignored, which makes this
    /// suitable for applying a snapshot after a configuration change that
    /// renames or removes nodes. The tree is reset first; duplicate IDs in the
    /// tree and rejected state still return an error, and a failed restore may
    /// leave the tree partially restored. Returns the number of nodes that
    /// loaded state.
    pub fn restore_by_id_matching(
        &mut self,
        snapshot: &crate::snapshot::NodeIdSnapshot,
    ) -> Result<usize, crate::snapshot::NodeIdSnapshotError> {
        let result =
            crate::snapshot::apply_by_id_matching(&mut self.root, &mut self.services, snapshot);
        self.last_status = None;
        self.service_status.fill(None);
        result
    }

    /// Extracts the root node so the tree can be reused as a subtree.
    ///
    /// Note: this discards the blackboard, tracer, and service nodes, but keeps
    /// the root's internal state. If the root is Running and depends on
    /// blackboard data (such as a target index), the blackboard must be
    /// prepared again before attaching it to a new tree; to reuse a subtree
    /// together with its blackboard and state, use a factory function or
    /// `SubtreeFactory`.
    #[must_use]
    pub fn into_root(self) -> BoxedNode<W> {
        self.root
    }
}

/// A whole behavior tree can be nested directly into another tree as a node.
///
/// A nested tree shares the outer tick's world, time, blackboard, and tracer:
/// - its own blackboard does not participate in the nested tick; share data
///   across trees through the outer blackboard or `SharedBlackboard`;
/// - its own tracer is inactive; the outer tracer sees the nested tree root and
///   its subtrees;
/// - when preempted by the outer tree, `abort → reset` propagates to the nested
///   tree root;
/// - it shows up in `describe()` as `tree "<name>"`, with the original root
///   node as its only child.
///
/// Attach it with the builder's `.raw(Box::new(tree))`; use
/// [`crate::SubtreeFactory`] when repeatable instantiation is needed.
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

    fn children_mut(&mut self) -> &mut [BoxedNode<W>] {
        core::slice::from_mut(&mut self.root)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        Some(state::encode(&(
            self.root.save_state(),
            save_children(&self.services),
        )))
    }

    fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some((root, services)) =
            state::decode::<(Option<Vec<u8>>, Vec<Option<Vec<u8>>>)>(bytes)
        else {
            return false;
        };
        if services.len() != self.services.len() {
            return false;
        }

        self.root.reset();
        for service in &mut self.services {
            service.reset();
        }
        self.last_status = None;
        self.service_status.fill(None);

        if let Some(root) = root
            && !self.root.load_state(&root)
        {
            return false;
        }
        load_children(&mut self.services, &services)
    }
}

/// A structural path to a node inside a behavior tree.
///
/// The root is `root`; child steps use `.children[i]`. A service path starts at
/// `services[i]` and then follows the same child steps. Paths are diagnostic
/// labels, not stable identities.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodePath {
    service: Option<usize>,
    indices: Vec<usize>,
}

impl NodePath {
    /// Path to the root node.
    #[must_use]
    pub fn root() -> Self {
        Self {
            service: None,
            indices: Vec::new(),
        }
    }

    /// Path to the service at `index`.
    #[must_use]
    pub fn service(index: usize) -> Self {
        Self {
            service: Some(index),
            indices: Vec::new(),
        }
    }

    /// Extends the path with a child index.
    #[must_use]
    pub fn with_child(&self, index: usize) -> Self {
        let mut next = self.clone();
        next.indices.push(index);
        next
    }

    /// The service index, or `None` for the main tree.
    #[must_use]
    pub fn service_index(&self) -> Option<usize> {
        self.service
    }

    /// Child indices below the root or service node.
    #[must_use]
    pub fn indices(&self) -> &[usize] {
        &self.indices
    }

    /// Parses a path produced by [`Display`](fmt::Display).
    ///
    /// Accepts `root`, `services[i]`, and any number of `.children[i]` steps.
    /// Returns `None` for malformed or overflowing paths.
    #[must_use]
    pub fn parse(path: &str) -> Option<Self> {
        let (mut node_path, mut rest) = if let Some(rest) = path.strip_prefix("root") {
            (Self::root(), rest)
        } else {
            let rest = path.strip_prefix("services[")?;
            let (index, rest) = parse_bracket_index(rest)?;
            (Self::service(index), rest)
        };

        while !rest.is_empty() {
            let tail = rest.strip_prefix(".children[")?;
            let (index, tail) = parse_bracket_index(tail)?;
            node_path.indices.push(index);
            rest = tail;
        }
        Some(node_path)
    }
}

fn parse_bracket_index(input: &str) -> Option<(usize, &str)> {
    let (digits, rest) = input.split_once(']')?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((digits.parse().ok()?, rest))
}

impl Default for NodePath {
    fn default() -> Self {
        Self::root()
    }
}

impl fmt::Display for NodePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.service {
            Some(index) => write!(f, "services[{index}]")?,
            None => f.write_str("root")?,
        }
        for index in &self.indices {
            write!(f, ".children[{index}]")?;
        }
        Ok(())
    }
}

#[cfg(feature = "diff-serde")]
impl serde::Serialize for NodePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "diff-serde")]
impl<'de> serde::Deserialize<'de> for NodePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct PathVisitor;

        impl<'de> serde::de::Visitor<'de> for PathVisitor {
            type Value = NodePath;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a node path like `root.children[0]` or `services[1]`")
            }

            fn visit_str<E>(self, value: &str) -> Result<NodePath, E>
            where
                E: serde::de::Error,
            {
                NodePath::parse(value)
                    .ok_or_else(|| E::custom(alloc::format!("invalid node path `{value}`")))
            }
        }

        deserializer.deserialize_str(PathVisitor)
    }
}

/// A node's structural description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeDescription {
    /// The node display name.
    pub name: String,
    /// Stable identity, when the tree author assigned one.
    pub id: Option<NodeId>,
    /// The node kind label.
    pub kind: Kind,
    /// Descriptions of the child nodes.
    pub children: Vec<TreeDescription>,
}

impl TreeDescription {
    fn from_node<W>(node: &(dyn Node<W> + Send)) -> Self {
        Self {
            name: node.name().to_string(),
            id: node.id().cloned(),
            kind: node.kind(),
            children: node
                .children()
                .iter()
                .map(|child| Self::from_node(child.as_ref()))
                .collect(),
        }
    }

    /// Renders a Graphviz DOT string.
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
            let label = match &node.id {
                Some(node_id) => {
                    alloc::format!("{}: {} ({node_id})", node.kind.as_str(), node.name)
                }
                None => alloc::format!("{}: {}", node.kind.as_str(), node.name),
            };
            let _ = writeln!(out, "  n{id} [label=\"{}\"];", escape_dot(&label));
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

    /// Renders a Mermaid `graph TD` diagram.
    #[must_use]
    pub fn to_mermaid(&self) -> String {
        fn visit(
            node: &TreeDescription,
            parent: Option<usize>,
            next: &mut usize,
            out: &mut String,
        ) {
            let id = *next;
            *next += 1;
            let _ = writeln!(out, "  n{id}[\"{}\"];", escape_mermaid(&node.label()));
            if let Some(parent) = parent {
                let _ = writeln!(out, "  n{parent} --> n{id};");
            }
            for child in &node.children {
                visit(child, Some(id), next, out);
            }
        }

        let mut out = String::from("graph TD\n");
        let mut next = 0;
        visit(self, None, &mut next, &mut out);
        out
    }

    fn label(&self) -> String {
        match &self.id {
            Some(node_id) => alloc::format!("{}: {} ({node_id})", self.kind.as_str(), self.name),
            None => alloc::format!("{}: {}", self.kind.as_str(), self.name),
        }
    }
}

fn escape_mermaid(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push('\''),
            '\n' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
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

/// A transparent wrapper that changes only the display name, not state
/// semantics.
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

    fn children_mut(&mut self) -> &mut [BoxedNode<W>] {
        core::slice::from_mut(&mut self.child)
    }

    fn save_own_state(&self) -> Option<Vec<u8>> {
        self.child.save_own_state()
    }

    fn load_own_state(&mut self, state: &[u8]) -> bool {
        self.child.load_own_state(state)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        self.child.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> bool {
        self.child.load_state(state)
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
    fn stable_ids_appear_in_descriptions() {
        let tree = BehaviorTreeBuilder::<()>::new("ids")
            .sequence(|b| {
                b.action("first", |_| Status::Success)
                    .id("first_id")
                    .action("second", |_| Status::Success)
            })
            .id("root_id")
            .build()
            .expect("tree");

        let description = tree.describe();
        assert_eq!(description.id.as_ref().map(NodeId::as_str), Some("root_id"));
        assert_eq!(
            description.children[0].id.as_ref().map(NodeId::as_str),
            Some("first_id")
        );
        assert!(description.children[1].id.is_none());
        assert!(description.to_dot().contains("first_id"));
    }

    #[test]
    fn id_without_a_node_is_a_build_error() {
        let error = BehaviorTreeBuilder::<()>::new("bad")
            .id("orphan")
            .action("node", |_| Status::Success)
            .build()
            .err()
            .expect("id requires a node");
        assert!(error.to_string().contains("id requires"));
    }

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

        let mermaid = description.to_mermaid();
        assert!(mermaid.starts_with("graph TD"));
        assert!(mermaid.contains("gate"));
        assert!(mermaid.contains("-->"));
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
    fn duplicate_node_id_detects_root_and_service_duplicates() {
        let duplicate = BehaviorTree::new(Box::new(crate::composite::Selector::new(vec![
            Box::new(crate::node::IdentifiedNode::new(
                "same",
                Box::new(Action::<()>::new("first", |_| Status::Success)),
            )),
            Box::new(crate::node::IdentifiedNode::new(
                "same",
                Box::new(Action::<()>::new("second", |_| Status::Success)),
            )),
        ])));
        assert_eq!(duplicate.duplicate_node_id(), Some(NodeId::new("same")));
        assert_eq!(
            duplicate.duplicate_node_id_path(),
            Some((NodeId::new("same"), NodePath::root().with_child(1)))
        );

        let mut with_service = BehaviorTreeBuilder::<()>::new("root")
            .action("root", |_| Status::Success)
            .id("shared")
            .build()
            .expect("tree");
        let service = crate::node::IdentifiedNode::new(
            "shared",
            Box::new(Action::<()>::new("service", |_| Status::Success)),
        );
        with_service.add_service(Box::new(service));
        assert_eq!(
            with_service.duplicate_node_id(),
            Some(NodeId::new("shared"))
        );
        assert_eq!(
            with_service.duplicate_node_id_path(),
            Some((NodeId::new("shared"), NodePath::service(0)))
        );

        let unique = BehaviorTreeBuilder::<()>::new("unique")
            .selector(|b| {
                b.action("first", |_| Status::Success)
                    .id("a")
                    .action("second", |_| Status::Success)
                    .id("b")
            })
            .build()
            .expect("tree");
        assert_eq!(unique.duplicate_node_id(), None);
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

        // A second abort is idempotent; the previous status was already cleared.
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

        // The root has terminated, but Running services still need cleanup.
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

    #[test]
    fn node_path_parses_display_forms() {
        assert_eq!(NodePath::parse("root"), Some(NodePath::root()));
        assert_eq!(
            NodePath::parse("root.children[1].children[2]"),
            Some(NodePath::root().with_child(1).with_child(2))
        );
        assert_eq!(NodePath::parse("services[3]"), Some(NodePath::service(3)));
        assert_eq!(
            NodePath::parse("services[3].children[0]"),
            Some(NodePath::service(3).with_child(0))
        );

        let paths = [
            NodePath::root(),
            NodePath::service(0).with_child(4),
            NodePath::root().with_child(0).with_child(1),
        ];
        for path in paths {
            assert_eq!(NodePath::parse(&path.to_string()), Some(path));
        }

        for invalid in [
            "",
            "rootx",
            "root.",
            "root.children[",
            "root.children[]",
            "root.children[-1]",
            "root.children[1]x",
            "services[]",
            "services[0",
            "services[0].child[1]",
            "services[0].children[18446744073709551616]",
        ] {
            assert_eq!(NodePath::parse(invalid), None, "{invalid}");
        }
    }

    #[cfg(feature = "diff-serde")]
    #[test]
    fn node_path_serde_roundtrips() {
        let path = NodePath::service(2).with_child(0).with_child(3);
        let json = serde_json::to_string(&path).expect("serialize path");
        assert_eq!(json, "\"services[2].children[0].children[3]\"");
        let parsed: NodePath = serde_json::from_str(&json).expect("parse path");
        assert_eq!(parsed, path);
        assert!(serde_json::from_str::<NodePath>("\"not a path\"").is_err());
    }
}
