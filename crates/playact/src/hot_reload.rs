//! Configuration hot reload: watch file changes, rebuild the behavior tree, and
//! preserve blackboard data.
//!
//! This is the optional `hot-reload` feature, which depends on [`notify`] and
//! does not pull in an async runtime. Have the driver call [`TreeWatcher::poll`]
//! or [`TreeWatcher::tick`] once per frame.
//!
//! Reload semantics:
//! - New configuration parses/builds successfully: the new tree (root and
//!   configured services) replaces the old one and **blackboard data is
//!   preserved**; node-internal state is reset by default;
//! - With [`TreeWatcher::with_state_preservation`] enabled, node state is also
//!   restored by stable `NodeId` for nodes that still exist in the new tree;
//!   renamed or removed IDs start from their reset state;
//! - Each successful reload records a [`crate::TreeDiff`] with nodes added,
//!   removed, moved, or renamed according to their `NodeId` and structural path;
//!   [`TreeWatcher::with_diff_history`] keeps a bounded history instead of only
//!   the latest diff;
//! - Parse/build failure: the old tree stays untouched and the error is returned
//!   to the caller;
//! - Tracers are not preserved; reinstall them through [`TreeWatcher::tree_mut`]
//!   after a reload;
//! - `include` files are re-resolved on every reload, and the watcher polls the
//!   paths reported by [`IncludeResolver::watched_paths`] (plus the root file),
//!   so editing an include on its own triggers a reload. `FsIncludeResolver`
//!   records every file it resolves; custom resolvers can implement
//!   [`IncludeResolver::watched_paths`] and
//!   [`IncludeResolver::reset_watched_paths`].

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::config::{
    ConfigBuildError, ConfigError, IncludeResolver, NodeRegistry, TreeParser, TreeSpec,
};
use crate::diff::{DiffHistory, TreeDiff};
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

/// A file reported by the include resolver, with its last observed stamp.
#[derive(Debug, Clone)]
struct WatchedInclude {
    path: PathBuf,
    stamp: Option<FileStamp>,
}

/// Cheap file identity used to notice include edits while polling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    fs::metadata(path).ok().map(|metadata| FileStamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
    })
}

fn collect_watched_includes(resolver: &dyn IncludeResolver) -> Vec<WatchedInclude> {
    resolver
        .watched_paths()
        .into_iter()
        .map(|path| WatchedInclude {
            stamp: file_stamp(&path),
            path,
        })
        .collect()
}

/// Watches a configuration file and maintains its behavior tree.
///
/// It watches the directory containing the configuration file (filtering events
/// by file name), so atomic saves that write a temp file and then rename it are
/// also observed. Include files reported by the resolver are polled on every
/// [`TreeWatcher::poll`], and a change to any of them triggers a reload too.
pub struct TreeWatcher<W: 'static> {
    path: PathBuf,
    registry: NodeRegistry<W>,
    parser: TreeParser,
    tree: BehaviorTree<W>,
    resolver: Option<Box<dyn IncludeResolver>>,
    watched_includes: Vec<WatchedInclude>,
    _watcher: RecommendedWatcher,
    events: Receiver<notify::Result<Event>>,
    debounce: Duration,
    pending_since: Option<Instant>,
    preserve_state: bool,
    diff_history: DiffHistory,
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
    /// The resolver runs again on the initial build and on every reload. Its
    /// [`IncludeResolver::watched_paths`] report is polled alongside the root
    /// file, so a resolver that records local files (such as
    /// [`FsIncludeResolver`](crate::config::FsIncludeResolver)) also reloads
    /// when an include changes on its own.
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
            Some(resolver) => {
                resolver.reset_watched_paths();
                spec.build_with_resolver(&registry, resolver)?
            }
            None => spec.build(&registry)?,
        };
        let watched_includes = resolver
            .as_deref()
            .map(collect_watched_includes)
            .unwrap_or_default();

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
            watched_includes,
            _watcher: watcher,
            events,
            debounce: Duration::from_millis(200),
            pending_since: None,
            preserve_state: false,
            diff_history: DiffHistory::default(),
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

    /// Enables or disables node-state preservation across reloads.
    ///
    /// When enabled, the watcher captures an ID-keyed snapshot before rebuilding
    /// and restores state for nodes whose `NodeId` still exists in the new
    /// configuration. Nodes without an ID, nodes whose ID was renamed or
    /// removed, and nodes that do not support own-state snapshots start from
    /// their reset state. The blackboard is preserved regardless of this option.
    #[must_use]
    pub fn with_state_preservation(mut self, preserve: bool) -> Self {
        self.preserve_state = preserve;
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

    /// Include files polled for changes in addition to the root file.
    ///
    /// The list comes from the resolver's watched-path report after each build;
    /// it is empty without a resolver or when the resolver reads no files.
    pub fn include_paths(&self) -> impl Iterator<Item = &Path> {
        self.watched_includes
            .iter()
            .map(|include| include.path.as_path())
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

    /// Keeps up to `limit` recent reload diffs; `0` disables history.
    ///
    /// The default keeps only the most recent diff.
    #[must_use]
    pub fn with_diff_history(mut self, limit: usize) -> Self {
        self.diff_history.set_limit(limit);
        self
    }

    /// Recent structural diffs, ordered oldest to newest.
    #[must_use]
    pub fn diff_history(&self) -> &DiffHistory {
        &self.diff_history
    }

    /// Structural changes from the most recent successful reload.
    #[must_use]
    pub fn last_diff(&self) -> Option<&TreeDiff> {
        self.diff_history.latest()
    }

    /// Revision assigned to the most recent diff, if any.
    #[must_use]
    pub fn last_revision(&self) -> Option<u64> {
        self.diff_history.latest_revision()
    }

    /// Unix-millisecond timestamp of the most recent diff, if any.
    #[must_use]
    pub fn last_timestamp(&self) -> Option<u64> {
        self.diff_history
            .latest_entry()
            .and_then(|entry| entry.timestamp)
    }

    /// Takes the structural changes from the most recent successful reload.
    pub fn take_last_diff(&mut self) -> Option<TreeDiff> {
        self.diff_history.pop_latest()
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
        if self.pending_since.is_none() && self.includes_changed() {
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
        let result = self.reload_inner();
        self.watched_includes = self
            .resolver
            .as_deref()
            .map(collect_watched_includes)
            .unwrap_or_default();
        result
    }

    fn reload_inner(&mut self) -> Result<(), HotReloadError> {
        let text = fs::read_to_string(&self.path)?;
        let spec = (self.parser)(&text)?;
        let mut new_tree = match self.resolver.as_deref() {
            Some(resolver) => {
                resolver.reset_watched_paths();
                spec.build_with_resolver(&self.registry, resolver)?
            }
            None => spec.build(&self.registry)?,
        };

        let snapshot = if self.preserve_state {
            self.tree.snapshot_by_id().ok()
        } else {
            None
        };

        let board = std::mem::take(self.tree.blackboard_mut());
        *new_tree.blackboard_mut() = board;

        if let Some(snapshot) = &snapshot
            && new_tree.restore_by_id_matching(snapshot).is_err()
        {
            new_tree.reset();
        }

        let diff = TreeDiff::between(&self.tree, &new_tree);
        self.tree = new_tree;
        self.diff_history.push_now(diff);

        Ok(())
    }

    fn includes_changed(&self) -> bool {
        self.watched_includes
            .iter()
            .any(|include| file_stamp(&include.path) != include.stamp)
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

    fn write_limiter_config(path: &Path, action: &str) {
        let config = format!(
            r#"{{"root":{{"type":"identified","id":"limit","child":{{"type":"limiter","max":2,"child":{{"type":"action","name":"{action}"}}}}}}}}"#
        );
        fs::write(path, config).expect("write limiter config");
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
    fn reload_rebuilds_configured_services() {
        let path = temp_path();
        fs::write(
            &path,
            r#"{"root":{"type":"action","name":"old"},"services":[{"type":"action","name":"old"}]}"#,
        )
        .expect("write initial");

        let mut watcher = TreeWatcher::<()>::json(&path, registry()).expect("watcher");
        let mut world = ();
        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut watcher), vec!["old", "old"]);

        fs::write(
            &path,
            r#"{"root":{"type":"action","name":"new"},"services":[{"type":"action","name":"new"}]}"#,
        )
        .expect("write new");
        watcher.reload_now().expect("reload");
        assert_eq!(watcher.tree().service_count(), 1);

        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut watcher), vec!["new", "new"]);

        drop(watcher);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reload_reports_structural_diff() {
        let path = temp_path();
        fs::write(
            &path,
            r#"{"root":{"type":"sequence","children":[
                {"type":"identified","id":"a","child":{"type":"action","name":"old"}},
                {"type":"identified","id":"b","child":{"type":"action","name":"old"}}
            ]}}"#,
        )
        .expect("write initial");

        let mut watcher = TreeWatcher::<()>::json(&path, registry()).expect("watcher");
        assert!(watcher.last_diff().is_none());

        fs::write(
            &path,
            r#"{"root":{"type":"sequence","children":[
                {"type":"identified","id":"b","child":{"type":"action","name":"new"}},
                {"type":"identified","id":"c","child":{"type":"action","name":"new"}}
            ]},"services":[
                {"type":"identified","id":"svc","child":{"type":"action","name":"new"}}
            ]}"#,
        )
        .expect("write new");
        watcher.reload_now().expect("reload");

        assert_eq!(watcher.last_revision(), Some(1));
        assert!(watcher.last_timestamp().is_some());

        let diff = watcher.last_diff().expect("diff");
        assert_eq!(diff.added.len(), 2);
        assert_eq!(diff.added[0].id.as_str(), "c");
        assert_eq!(diff.added[1].id.as_str(), "svc");
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.removed[0].id.as_str(), "a");
        assert_eq!(diff.moved.len(), 1);
        assert_eq!(diff.moved[0].id.as_str(), "b");
        assert_eq!(diff.moved[0].from.to_string(), "root.children[1]");
        assert_eq!(diff.moved[0].to.to_string(), "root.children[0]");
        // `b` also changed its action name, so it is reported as moved and renamed.
        assert_eq!(diff.renamed.len(), 1);
        let renamed = &diff.renamed[0];
        assert_eq!(renamed.id.as_str(), "b");
        assert_eq!(renamed.path.to_string(), "root.children[0]");
        assert_eq!(renamed.old_name, "old");
        assert_eq!(renamed.new_name, "new");

        assert!(watcher.take_last_diff().is_some());
        assert!(watcher.last_diff().is_none());

        drop(watcher);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reload_keeps_bounded_diff_history() {
        let path = temp_path();
        fs::write(
            &path,
            r#"{"root":{"type":"sequence","children":[
                {"type":"identified","id":"a","child":{"type":"action","name":"old"}}
            ]}}"#,
        )
        .expect("write initial");

        let mut watcher = TreeWatcher::<()>::json(&path, registry())
            .expect("watcher")
            .with_diff_history(2);

        for (id, action) in [("b", "new"), ("c", "new")] {
            fs::write(
                &path,
                format!(
                    r#"{{"root":{{"type":"sequence","children":[
                        {{"type":"identified","id":"{id}","child":{{"type":"action","name":"{action}"}}}}
                    ]}}}}"#
                ),
            )
            .expect("write config");
            watcher.reload_now().expect("reload");
        }

        assert_eq!(watcher.diff_history().len(), 2);
        assert_eq!(watcher.last_revision(), Some(2));
        assert_eq!(watcher.diff_history().oldest_revision(), Some(1));
        let ids: Vec<&str> = watcher
            .diff_history()
            .iter()
            .map(|diff| diff.added[0].id.as_str())
            .collect();
        assert_eq!(ids, vec!["b", "c"]);
        assert_eq!(
            watcher.last_diff().expect("latest").added[0].id.as_str(),
            "c"
        );

        drop(watcher);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reload_resets_node_state_by_default() {
        let path = temp_path();
        write_limiter_config(&path, "old");

        let mut watcher = TreeWatcher::<()>::json(&path, registry()).expect("watcher");
        let mut world = ();
        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut watcher), vec!["old"]);

        write_limiter_config(&path, "new");
        watcher.reload_now().expect("reload");

        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut watcher), vec!["new"]);
        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut watcher), vec!["new"]);
        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Failure
        );

        drop(watcher);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reload_can_preserve_node_state_by_id() {
        let path = temp_path();
        write_limiter_config(&path, "old");

        let mut watcher = TreeWatcher::<()>::json(&path, registry())
            .expect("watcher")
            .with_state_preservation(true);
        let mut world = ();
        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut watcher), vec!["old"]);

        write_limiter_config(&path, "new");
        watcher.reload_now().expect("reload");

        // The limiter already consumed one of its two runs before the reload.
        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut watcher), vec!["new"]);
        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Failure
        );
        assert!(drain(&mut watcher).is_empty());

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

    #[test]
    fn poll_reloads_when_an_include_changes() {
        use crate::config::FsIncludeResolver;

        let dir = std::env::temp_dir().join(format!(
            "playact_hot_reload_include_poll_{}_{}",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create dir");
        let root = dir.join("root.json");
        let sub = dir.join("sub.json");
        let leaf = dir.join("leaf.json");
        fs::write(&root, r#"{"root":{"type":"include","path":"sub.json"}}"#).expect("write root");
        fs::write(&sub, r#"{"root":{"type":"include","path":"leaf.json"}}"#).expect("write sub");
        fs::write(
            &leaf,
            "{\n  \"root\": { \"type\": \"action\", \"name\": \"old\" }\n}\n",
        )
        .expect("write leaf");

        let resolver = FsIncludeResolver::json(&dir).expect("resolver");
        let mut watcher = TreeWatcher::<()>::json_with_resolver(&root, registry(), resolver)
            .expect("watcher")
            .with_debounce(Duration::ZERO);

        let mut watched: Vec<PathBuf> = watcher.include_paths().map(Path::to_path_buf).collect();
        watched.sort();
        let mut expected = vec![
            sub.canonicalize().expect("canonical sub"),
            leaf.canonicalize().expect("canonical leaf"),
        ];
        expected.sort();
        assert_eq!(watched, expected);

        let mut world = ();
        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut watcher), vec!["old"]);

        // Change only the nested include; neither the root nor filesystem events
        // for it are needed: the watcher polls the recorded include paths.
        fs::write(&leaf, r#"{"root":{"type":"action","name":"new"}}"#).expect("write leaf new");
        assert!(watcher.poll().expect("poll reload"));
        assert_eq!(
            watcher.tree_mut().tick(&mut world, Time::ZERO),
            Status::Success
        );
        assert_eq!(drain(&mut watcher), vec!["new"]);

        drop(watcher);
        let _ = fs::remove_dir_all(dir);
    }
}
