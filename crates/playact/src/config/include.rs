//! Include resolution and a sandboxed filesystem resolver.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::{ConfigError, TreeParser, TreeSpec};

/// Resolves an include path into a [`TreeSpec`].
///
/// Implementations may read files, issue HTTP requests, or look up an in-memory
/// table; the engine only handles recursive building plus depth/cycle
/// protection. The closure `Fn(&str) -> Result<TreeSpec, ConfigError>`
/// implements this trait automatically.
pub trait IncludeResolver {
    /// Resolves one include path.
    fn resolve(&self, path: &str) -> Result<TreeSpec, ConfigError>;

    /// Files whose changes should trigger a hot reload, in addition to the root
    /// configuration file.
    ///
    /// [`TreeWatcher`](crate::hot_reload::TreeWatcher) polls these paths after
    /// every build. Implementations that read local files should record them in
    /// [`reset_watched_paths`](IncludeResolver::reset_watched_paths) and return
    /// them here. The default reports no extra paths.
    fn watched_paths(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    /// Clears the paths reported by [`watched_paths`](IncludeResolver::watched_paths).
    ///
    /// The watcher calls this before every build so the next report reflects
    /// only the current include graph. The default does nothing.
    fn reset_watched_paths(&self) {}
}

impl<F> IncludeResolver for F
where
    F: Fn(&str) -> Result<TreeSpec, ConfigError>,
{
    fn resolve(&self, path: &str) -> Result<TreeSpec, ConfigError> {
        self(path)
    }
}

#[derive(Clone, Copy)]
enum ParserMode {
    Fixed(TreeParser),
    Auto,
}

/// Resolver that reads include files from a base directory.
///
/// Target paths are canonicalized and must still reside inside the base
/// directory, which rejects `..`, absolute paths, and symlink escapes. The base
/// directory is canonicalized and checked for existence at construction time.
pub struct FsIncludeResolver {
    base: PathBuf,
    parser: ParserMode,
    watched: Mutex<Vec<PathBuf>>,
}

impl FsIncludeResolver {
    /// Creates a resolver from a base directory and parser; the base directory
    /// must exist and be a directory.
    pub fn new(base: impl Into<PathBuf>, parser: TreeParser) -> Result<Self, ConfigError> {
        Self::with_parser(base, ParserMode::Fixed(parser))
    }

    /// Reads includes, choosing the parser from each file's extension.
    ///
    /// `.json` is always available. `.toml`, `.yaml`/`.yml`, and `.ron` require
    /// the corresponding `toml`, `yaml`, and `ron` features. Includes without a
    /// supported extension are rejected.
    pub fn auto(base: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        Self::with_parser(base, ParserMode::Auto)
    }

    fn with_parser(base: impl Into<PathBuf>, parser: ParserMode) -> Result<Self, ConfigError> {
        let base = base.into();
        let base = base.canonicalize().map_err(|error| {
            ConfigError::new(format!(
                "include base directory `{}`: {error}",
                base.display()
            ))
        })?;
        if !base.is_dir() {
            return Err(ConfigError::new(format!(
                "include base `{}` is not a directory",
                base.display()
            )));
        }
        Ok(Self {
            base,
            parser,
            watched: Mutex::new(Vec::new()),
        })
    }

    /// Reads JSON includes from the base directory.
    pub fn json(base: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        Self::new(base, TreeSpec::from_json_str)
    }

    /// Reads TOML includes from the base directory.
    #[cfg(feature = "toml")]
    pub fn toml(base: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        Self::new(base, TreeSpec::from_toml_str)
    }

    /// Reads YAML includes from the base directory.
    #[cfg(feature = "yaml")]
    pub fn yaml(base: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        Self::new(base, TreeSpec::from_yaml_str)
    }

    /// Reads RON includes from the base directory.
    #[cfg(feature = "ron")]
    pub fn ron(base: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        Self::new(base, TreeSpec::from_ron_str)
    }

    /// The canonicalized base directory.
    #[must_use]
    pub fn base_dir(&self) -> &Path {
        &self.base
    }
}

impl IncludeResolver for FsIncludeResolver {
    fn resolve(&self, path: &str) -> Result<TreeSpec, ConfigError> {
        let candidate = self.base.join(path);
        let canonical = candidate
            .canonicalize()
            .map_err(|error| ConfigError::with_path(path, format!("include path: {error}")))?;

        if !canonical.starts_with(&self.base) {
            return Err(ConfigError::with_path(
                path,
                "include path escapes the base directory",
            ));
        }
        if !canonical.is_file() {
            return Err(ConfigError::with_path(path, "include path is not a file"));
        }

        if let Ok(mut watched) = self.watched.lock()
            && !watched.contains(&canonical)
        {
            watched.push(canonical.clone());
        }

        let text = fs::read_to_string(&canonical)
            .map_err(|error| ConfigError::with_path(path, format!("read include: {error}")))?;
        let spec = match self.parser {
            ParserMode::Fixed(parser) => parser(&text),
            ParserMode::Auto => parse_auto(&canonical, &text),
        }
        .map_err(|error| ConfigError::with_path(path, format!("parse include: {error}")))?;
        Ok(spec)
    }

    fn watched_paths(&self) -> Vec<PathBuf> {
        self.watched
            .lock()
            .map(|watched| watched.clone())
            .unwrap_or_default()
    }

    fn reset_watched_paths(&self) {
        if let Ok(mut watched) = self.watched.lock() {
            watched.clear();
        }
    }
}

fn parse_auto(path: &Path, text: &str) -> Result<TreeSpec, ConfigError> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("json") => TreeSpec::from_json_str(text),
        #[cfg(feature = "toml")]
        Some("toml") => TreeSpec::from_toml_str(text),
        #[cfg(not(feature = "toml"))]
        Some("toml") => Err(ConfigError::new("TOML includes require the `toml` feature")),
        #[cfg(feature = "yaml")]
        Some("yaml" | "yml") => TreeSpec::from_yaml_str(text),
        #[cfg(not(feature = "yaml"))]
        Some("yaml" | "yml") => Err(ConfigError::new("YAML includes require the `yaml` feature")),
        #[cfg(feature = "ron")]
        Some("ron") => TreeSpec::from_ron_str(text),
        #[cfg(not(feature = "ron"))]
        Some("ron") => Err(ConfigError::new("RON includes require the `ron` feature")),
        Some(other) => Err(ConfigError::new(format!(
            "unsupported include extension `.{other}`; expected .json, .toml, .yaml, .yml, or .ron"
        ))),
        None => Err(ConfigError::new(
            "include path has no extension; expected .json, .toml, .yaml, .yml, or .ron",
        )),
    }
}

impl core::fmt::Debug for FsIncludeResolver {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FsIncludeResolver")
            .field("base", &self.base)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "playact-include-{}-{id}-{label}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    #[test]
    fn resolves_files_inside_base() {
        let base = temp_dir("base");
        fs::write(
            base.join("sub.json"),
            r#"{"root":{"type":"action","name":"ok"}}"#,
        )
        .expect("write include");

        let resolver = FsIncludeResolver::json(&base).expect("resolver");
        let spec = resolver.resolve("sub.json").expect("resolve");
        assert!(matches!(spec.root, super::super::SpecNode::Action { .. }));

        fs::remove_dir_all(&base).expect("cleanup");
    }

    #[test]
    fn rejects_paths_outside_base() {
        let parent = temp_dir("parent");
        let base = parent.join("base");
        fs::create_dir_all(&base).expect("create base");
        fs::write(
            parent.join("outside.json"),
            r#"{"root":{"type":"action","name":"ok"}}"#,
        )
        .expect("write outside");

        let resolver = FsIncludeResolver::json(&base).expect("resolver");
        let error = resolver
            .resolve("../outside.json")
            .expect_err("escape rejected");
        assert!(error.to_string().contains("escapes"));

        fs::remove_dir_all(&parent).expect("cleanup");
    }

    #[test]
    fn records_watched_paths_and_resets() {
        let base = temp_dir("watch");
        let sub = base.join("sub.json");
        fs::write(&sub, r#"{"root":{"type":"action","name":"ok"}}"#).expect("write include");

        let resolver = FsIncludeResolver::json(&base).expect("resolver");
        assert!(resolver.watched_paths().is_empty());

        resolver.resolve("sub.json").expect("resolve");
        resolver.resolve("sub.json").expect("resolve again");
        let expected = sub.canonicalize().expect("canonical include");
        assert_eq!(resolver.watched_paths(), vec![expected]);

        resolver.reset_watched_paths();
        assert!(resolver.watched_paths().is_empty());

        fs::remove_dir_all(&base).expect("cleanup");
    }

    #[test]
    fn auto_detects_supported_extensions() {
        let base = temp_dir("auto");
        fs::write(
            base.join("sub.json"),
            r#"{"version":1,"root":{"type":"action","name":"ok"}}"#,
        )
        .expect("write json");
        #[cfg(feature = "toml")]
        fs::write(
            base.join("sub.toml"),
            "version = 1\n[root]\ntype = \"action\"\nname = \"ok\"\n",
        )
        .expect("write toml");
        #[cfg(feature = "yaml")]
        fs::write(
            base.join("sub.yaml"),
            "version: 1\nroot:\n  type: action\n  name: ok\n",
        )
        .expect("write yaml");
        #[cfg(feature = "ron")]
        fs::write(
            base.join("sub.ron"),
            "(version: 1, root: (type: \"action\", name: \"ok\"))",
        )
        .expect("write ron");

        let resolver = FsIncludeResolver::auto(&base).expect("resolver");
        let mut names = vec!["sub.json"];
        #[cfg(feature = "toml")]
        names.push("sub.toml");
        #[cfg(feature = "yaml")]
        names.push("sub.yaml");
        #[cfg(feature = "ron")]
        names.push("sub.ron");
        for name in names {
            let spec = resolver.resolve(name).expect("resolve");
            assert!(
                matches!(spec.root, super::super::SpecNode::Action { .. }),
                "{name}"
            );
        }

        fs::remove_dir_all(&base).expect("cleanup");
    }

    #[test]
    fn auto_rejects_unsupported_extensions() {
        let base = temp_dir("auto-unsupported");
        fs::write(base.join("sub.txt"), "{}").expect("write txt");
        fs::write(base.join("noext"), "{}").expect("write extensionless");

        let resolver = FsIncludeResolver::auto(&base).expect("resolver");
        let error = resolver
            .resolve("sub.txt")
            .expect_err("unsupported extension");
        assert!(
            error.to_string().contains("unsupported include extension"),
            "{error}"
        );
        let error = resolver.resolve("noext").expect_err("missing extension");
        assert!(error.to_string().contains("no extension"), "{error}");

        fs::remove_dir_all(&base).expect("cleanup");
    }

    #[test]
    fn missing_file_reports_path() {
        let base = temp_dir("missing");
        let resolver = FsIncludeResolver::json(&base).expect("resolver");
        let error = resolver.resolve("nope.json").expect_err("missing error");
        assert!(error.to_string().contains("nope.json"));

        fs::remove_dir_all(&base).expect("cleanup");
    }
}
