//! 配置热重载：监听文件变化，重建行为树并保留黑板数据。
//!
//! 这是可选的 `hot-reload` feature，依赖 [`notify`]；不引入 async runtime。
//! 驱动方每帧调用一次 [`TreeWatcher::poll`] 或 [`TreeWatcher::tick`] 即可。
//!
//! 重载语义：
//! - 新配置解析/构建成功：新树替换旧树，**黑板数据保留**；节点内部状态重置；
//! - 解析/构建失败：旧树保持不变，错误返回给调用方；
//! - Tracer 不会被保留；重载后可通过 [`TreeWatcher::tree_mut`] 重新安装；
//! - `include` 文件在每次重载时重新解析；监听器仍然只关注根文件，include
//!   文件单独变化时可调用 [`TreeWatcher::reload_now`] 或触碰根文件。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::config::{
    ConfigBuildError, ConfigError, IncludeResolver, NodeRegistry, TreeParser, TreeSpec,
};
use crate::status::Status;
use crate::time::Time;
use crate::tree::BehaviorTree;

/// 热重载错误。
#[derive(Debug)]
#[non_exhaustive]
pub enum HotReloadError {
    /// 读取配置文件失败。
    Io(std::io::Error),
    /// 配置文本解析失败。
    Parse(ConfigError),
    /// 配置通过解析但构建行为树失败。
    Build(ConfigBuildError),
    /// 文件监听器错误。
    Watch(notify::Error),
}

impl std::fmt::Display for HotReloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "hot-reload I/O error: {error}"),
            Self::Parse(error) => write!(f, "hot-reload parse error: {error}"),
            Self::Build(error) => write!(f, "hot-reload build error: {error}"),
            Self::Watch(error) => write!(f, "hot-reload watch error: {error}"),
        }
    }
}

impl std::error::Error for HotReloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::Build(error) => Some(error),
            Self::Watch(error) => Some(error),
        }
    }
}

impl From<std::io::Error> for HotReloadError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<ConfigError> for HotReloadError {
    fn from(value: ConfigError) -> Self {
        Self::Parse(value)
    }
}

impl From<ConfigBuildError> for HotReloadError {
    fn from(value: ConfigBuildError) -> Self {
        Self::Build(value)
    }
}

impl From<notify::Error> for HotReloadError {
    fn from(value: notify::Error) -> Self {
        Self::Watch(value)
    }
}

/// 监听一个配置文件并维护对应行为树。
///
/// 监听的是配置文件所在目录（通过文件名过滤目标文件），因此编辑器采用
/// “写临时文件再 rename”的原子保存方式也能收到事件。
pub struct TreeWatcher<W: 'static> {
    path: PathBuf,
    registry: NodeRegistry<W>,
    parser: TreeParser,
    tree: BehaviorTree<W>,
    resolver: Option<Box<dyn IncludeResolver>>,
    _watcher: RecommendedWatcher,
    events: Receiver<notify::Result<Event>>,
    debounce: Duration,
    pending_since: Option<Instant>,
}

impl<W: 'static> TreeWatcher<W> {
    /// 用自定义解析器构造；初始文件必须能成功构建。
    pub fn with_parser(
        path: impl Into<PathBuf>,
        registry: NodeRegistry<W>,
        parser: TreeParser,
    ) -> Result<Self, HotReloadError> {
        Self::with_parser_impl(path, registry, parser, None)
    }

    /// 用自定义解析器和 include 解析器构造。
    ///
    /// resolver 在初次构建与每次重载时都会重新执行。注意监听器仍然只关注
    /// 根配置文件：被 include 的文件单独变化不会触发事件，可调用
    /// [`TreeWatcher::reload_now`] 或触碰根文件。
    pub fn with_parser_and_resolver(
        path: impl Into<PathBuf>,
        registry: NodeRegistry<W>,
        parser: TreeParser,
        resolver: impl IncludeResolver + 'static,
    ) -> Result<Self, HotReloadError> {
        Self::with_parser_impl(path, registry, parser, Some(Box::new(resolver)))
    }

    /// 用 JSON 配置和 include 解析器构造。
    pub fn json_with_resolver(
        path: impl Into<PathBuf>,
        registry: NodeRegistry<W>,
        resolver: impl IncludeResolver + 'static,
    ) -> Result<Self, HotReloadError> {
        Self::with_parser_and_resolver(path, registry, TreeSpec::from_json_str, resolver)
    }

    fn with_parser_impl(
        path: impl Into<PathBuf>,
        registry: NodeRegistry<W>,
        parser: TreeParser,
        resolver: Option<Box<dyn IncludeResolver>>,
    ) -> Result<Self, HotReloadError> {
        let path = path.into();
        let text = fs::read_to_string(&path)?;
        let spec = parser(&text)?;
        let tree = match resolver.as_deref() {
            Some(resolver) => spec.build_with_resolver(&registry, resolver)?,
            None => spec.build(&registry)?,
        };

        let (sender, events) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })?;

        let watch_root = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        watcher.watch(watch_root, RecursiveMode::NonRecursive)?;

        Ok(Self {
            path,
            registry,
            parser,
            tree,
            resolver,
            _watcher: watcher,
            events,
            debounce: Duration::from_millis(200),
            pending_since: None,
        })
    }

    /// 用 JSON 配置构造。
    pub fn json(
        path: impl Into<PathBuf>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, HotReloadError> {
        Self::with_parser(path, registry, TreeSpec::from_json_str)
    }

    /// 用 TOML 配置构造。
    #[cfg(feature = "toml")]
    pub fn toml(
        path: impl Into<PathBuf>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, HotReloadError> {
        Self::with_parser(path, registry, TreeSpec::from_toml_str)
    }

    /// 用 YAML 配置构造。
    #[cfg(feature = "yaml")]
    pub fn yaml(
        path: impl Into<PathBuf>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, HotReloadError> {
        Self::with_parser(path, registry, TreeSpec::from_yaml_str)
    }

    /// 用 RON 配置构造。
    #[cfg(feature = "ron")]
    pub fn ron(
        path: impl Into<PathBuf>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, HotReloadError> {
        Self::with_parser(path, registry, TreeSpec::from_ron_str)
    }

    /// 设置事件去抖时长，默认 200ms。
    #[must_use]
    pub fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }

    /// 当前行为树。
    #[must_use]
    pub fn tree(&self) -> &BehaviorTree<W> {
        &self.tree
    }

    /// 可变行为树。重载后可用它重新安装 Tracer。
    pub fn tree_mut(&mut self) -> &mut BehaviorTree<W> {
        &mut self.tree
    }

    /// 当前监听的配置文件。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 构建/重载使用的注册表。
    #[must_use]
    pub fn registry(&self) -> &NodeRegistry<W> {
        &self.registry
    }

    /// 可变注册表；注册新节点后下一次重载生效。
    pub fn registry_mut(&mut self) -> &mut NodeRegistry<W> {
        &mut self.registry
    }

    /// 处理文件事件；如果完成一次成功重载返回 `true`。
    ///
    /// 解析/构建失败时返回错误，旧树保持不变；需要等下一次文件事件再重试。
    pub fn poll(&mut self) -> Result<bool, HotReloadError> {
        let mut relevant = false;

        while let Ok(event) = self.events.try_recv() {
            match event {
                Ok(event) if self.is_relevant(&event) => relevant = true,
                Ok(_) => {}
                Err(error) => return Err(HotReloadError::Watch(error)),
            }
        }

        if relevant {
            self.pending_since = Some(Instant::now());
        }

        let Some(since) = self.pending_since else {
            return Ok(false);
        };
        if since.elapsed() < self.debounce {
            return Ok(false);
        }

        self.pending_since = None;
        self.reload()?;
        Ok(true)
    }

    /// 立即从磁盘重载一次，不等待文件事件/去抖。
    pub fn reload_now(&mut self) -> Result<(), HotReloadError> {
        self.pending_since = None;
        self.reload()
    }

    /// 先处理热重载，再 tick 当前树。
    ///
    /// 如果重载失败，本次返回错误且不 tick；需要继续使用旧树时调用
    /// [`TreeWatcher::tree_mut`] 直接 tick，旧树仍然有效。
    pub fn tick(&mut self, world: &mut W, time: Time) -> Result<Status, HotReloadError> {
        self.poll()?;
        Ok(self.tree.tick(world, time))
    }

    fn reload(&mut self) -> Result<(), HotReloadError> {
        let text = fs::read_to_string(&self.path)?;
        let spec = (self.parser)(&text)?;
        let mut new_tree = match self.resolver.as_deref() {
            Some(resolver) => spec.build_with_resolver(&self.registry, resolver)?,
            None => spec.build(&self.registry)?,
        };

        let board = std::mem::take(self.tree.blackboard_mut());
        *new_tree.blackboard_mut() = board;
        self.tree = new_tree;

        Ok(())
    }

    fn is_relevant(&self, event: &Event) -> bool {
        if !matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
            return false;
        }

        match self.path.file_name() {
            Some(name) => event
                .paths
                .iter()
                .any(|event_path| event_path.file_name() == Some(name)),
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::blackboard::Key;
    use crate::command::CommandBuffer;
    use crate::context::Context;
    use crate::status::Status;
    use crate::time::Time;

    use super::*;

    static COMMANDS: Key<CommandBuffer<&'static str>> = crate::key!("commands");
    static MARKER: Key<u32> = crate::key!("marker");
    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "playact_hot_reload_{}_{}.json",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn registry() -> NodeRegistry<()> {
        let mut registry = NodeRegistry::<()>::new();

        registry
            .action::<()>("old", |_| {
                move |ctx: &mut Context<'_, ()>| {
                    push(ctx, "old");
                    Status::Success
                }
            })
            .expect("register old");
        registry
            .action::<()>("new", |_| {
                move |ctx: &mut Context<'_, ()>| {
                    push(ctx, "new");
                    Status::Success
                }
            })
            .expect("register new");

        registry
    }

    fn push(ctx: &mut Context<'_, ()>, value: &'static str) {
        ctx.blackboard()
            .get_or_insert_with(&COMMANDS, CommandBuffer::new)
            .push(value);
    }

    fn drain(watcher: &mut TreeWatcher<()>) -> Vec<&'static str> {
        watcher
            .tree_mut()
            .blackboard_mut()
            .get_mut(&COMMANDS)
            .map(|buffer| buffer.drain().collect())
            .unwrap_or_default()
    }

    #[test]
    fn reload_rebuilds_and_preserves_blackboard() {
        let path = temp_path();
        fs::write(&path, r#"{"root":{"type":"action","name":"old"}}"#).expect("write initial");

        let mut watcher = TreeWatcher::<()>::json(&path, registry()).expect("watcher");
        watcher.tree_mut().blackboard_mut().set(&MARKER, 42);

        let mut world = ();
        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut watcher), vec!["old"]);

        fs::write(&path, r#"{"root":{"type":"action","name":"new"}}"#).expect("write new");
        watcher.reload_now().expect("reload");

        assert_eq!(watcher.tree().blackboard().get(&MARKER), Some(&42));
        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut watcher), vec!["new"]);

        fs::write(&path, "{").expect("write invalid");
        assert!(watcher.reload_now().is_err());

        // 失败时旧树仍然可用。
        assert_eq!(watcher.tree().blackboard().get(&MARKER), Some(&42));
        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut watcher), vec!["new"]);

        drop(watcher);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reload_resolves_includes() {
        use crate::config::FsIncludeResolver;

        let dir = std::env::temp_dir().join(format!(
            "playact_hot_reload_include_{}_{}",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create dir");
        let root = dir.join("root.json");
        let sub = dir.join("sub.json");
        fs::write(&root, r#"{"root":{"type":"include","path":"sub.json"}}"#).expect("write root");
        fs::write(&sub, r#"{"root":{"type":"action","name":"old"}}"#).expect("write sub");

        let resolver = FsIncludeResolver::json(&dir).expect("resolver");
        let mut watcher =
            TreeWatcher::<()>::json_with_resolver(&root, registry(), resolver).expect("watcher");

        let mut world = ();
        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut watcher), vec!["old"]);

        fs::write(&sub, r#"{"root":{"type":"action","name":"new"}}"#).expect("write sub new");
        watcher.reload_now().expect("reload include");
        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut watcher), vec!["new"]);

        drop(watcher);
        let _ = fs::remove_dir_all(dir);
    }
}
