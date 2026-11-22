//! Configuration hot reload: watch file changes, rebuild the behavior tree, and
//! preserve blackboard data.
//!
//! This is the optional `hot-reload` feature, which depends on [`notify`] and
//! does not pull in an async runtime. Have the driver call [`TreeWatcher::poll`]
//! or [`TreeWatcher::tick`] once per frame.
//!
//! Reload semantics:
//! - New configuration parses/builds successfully: the new tree replaces the old
//!   one and **blackboard data is preserved**; node-internal state is reset;
//! - Parse/build failure: the old tree stays untouched and the error is returned
//!   to the caller;
//! - Tracers are not preserved; reinstall them through [`TreeWatcher::tree_mut`]
//!   after a reload;
//! - `include` files are re-resolved on every reload; the watcher still only
//!   watches the root file, so when an include file changes on its own call
//!   [`TreeWatcher::reload_now`] or touch the root file.

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

/// Hot reload error.
#[derive(Debug)]
#[non_exhaustive]
pub enum HotReloadError {
    /// Failed to read the configuration file.
    Io(std::io::Error),
    /// Failed to parse the configuration text.
    Parse(ConfigError),
    /// Configuration parsed but building the behavior tree failed.
    Build(ConfigBuildError),
    /// File watcher error.
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

/// Watches a configuration file and maintains its behavior tree.
///
/// It watches the directory containing the configuration file (filtering events
/// by file name), so atomic saves that write a temp file and then rename it are
/// also observed.
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
    /// Creates a watcher with a custom parser; the initial file must build
    /// successfully.
    pub fn with_parser(
        path: impl Into<PathBuf>,
        registry: NodeRegistry<W>,
        parser: TreeParser,
    ) -> Result<Self, HotReloadError> {
        Self::with_parser_impl(path, registry, parser, None)
    }

    /// Creates a watcher with a custom parser and include resolver.
    ///
    /// The resolver runs again on the initial build and on every reload. Note
    /// that the watcher still only watches the root configuration file: a change
    /// to an included file alone triggers no event, so call
    /// [`TreeWatcher::reload_now`] or touch the root file.
    pub fn with_parser_and_resolver(
        path: impl Into<PathBuf>,
        registry: NodeRegistry<W>,
        parser: TreeParser,
        resolver: impl IncludeResolver + 'static,
    ) -> Result<Self, HotReloadError> {
        Self::with_parser_impl(path, registry, parser, Some(Box::new(resolver)))
    }

    /// Creates a watcher with a JSON configuration and an include resolver.
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

    /// Creates a watcher with a JSON configuration.
    pub fn json(
        path: impl Into<PathBuf>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, HotReloadError> {
        Self::with_parser(path, registry, TreeSpec::from_json_str)
    }

    /// Creates a watcher with a TOML configuration.
    #[cfg(feature = "toml")]
    pub fn toml(
        path: impl Into<PathBuf>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, HotReloadError> {
        Self::with_parser(path, registry, TreeSpec::from_toml_str)
    }

    /// Creates a watcher with a YAML configuration.
    #[cfg(feature = "yaml")]
    pub fn yaml(
        path: impl Into<PathBuf>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, HotReloadError> {
        Self::with_parser(path, registry, TreeSpec::from_yaml_str)
    }

    /// Creates a watcher with a RON configuration.
    #[cfg(feature = "ron")]
    pub fn ron(
        path: impl Into<PathBuf>,
        registry: NodeRegistry<W>,
    ) -> Result<Self, HotReloadError> {
        Self::with_parser(path, registry, TreeSpec::from_ron_str)
    }

    /// Sets the event debounce duration; defaults to 200ms.
    #[must_use]
    pub fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }

    /// The current behavior tree.
    #[must_use]
    pub fn tree(&self) -> &BehaviorTree<W> {
        &self.tree
    }

    /// Mutable behavior tree. Use it to reinstall Tracers after a reload.
    pub fn tree_mut(&mut self) -> &mut BehaviorTree<W> {
        &mut self.tree
    }

    /// The configuration file currently being watched.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The registry used for building and reloading.
    #[must_use]
    pub fn registry(&self) -> &NodeRegistry<W> {
        &self.registry
    }

    /// Mutable registry; newly registered nodes take effect on the next reload.
    pub fn registry_mut(&mut self) -> &mut NodeRegistry<W> {
        &mut self.registry
    }

    /// Processes file events; returns `true` if a reload completed successfully.
    ///
    /// On a parse/build failure it returns an error and the old tree stays
    /// unchanged; retrying requires waiting for the next file event.
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

    /// Reloads from disk once immediately, without waiting for a file event or
    /// debounce.
    pub fn reload_now(&mut self) -> Result<(), HotReloadError> {
        self.pending_since = None;
        self.reload()
    }

    /// Processes hot reload first, then ticks the current tree.
    ///
    /// If the reload fails, this returns an error and does not tick; to keep
    /// using the old tree, tick it directly via [`TreeWatcher::tree_mut`], which
    /// is still valid.
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

        // The old tree remains usable after a failure.
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
