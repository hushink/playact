//! Include resolution and a sandboxed filesystem resolver.

use std::fs;
use std::path::{Path, PathBuf};

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
}

impl<F> IncludeResolver for F
where
    F: Fn(&str) -> Result<TreeSpec, ConfigError>,
{
    fn resolve(&self, path: &str) -> Result<TreeSpec, ConfigError> {
        self(path)
    }
}

/// Resolver that reads include files from a base directory.
///
/// Target paths are canonicalized and must still reside inside the base
/// directory, which rejects `..`, absolute paths, and symlink escapes. The base
/// directory is canonicalized and checked for existence at construction time.
pub struct FsIncludeResolver {
    base: PathBuf,
    parser: TreeParser,
}

impl FsIncludeResolver {
    /// Creates a resolver from a base directory and parser; the base directory
    /// must exist and be a directory.
    pub fn new(base: impl Into<PathBuf>, parser: TreeParser) -> Result<Self, ConfigError> {
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
        Ok(Self { base, parser })
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

        let text = fs::read_to_string(&canonical)
            .map_err(|error| ConfigError::with_path(path, format!("read include: {error}")))?;
        let spec = (self.parser)(&text)
            .map_err(|error| ConfigError::with_path(path, format!("parse include: {error}")))?;
        Ok(spec)
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
    fn missing_file_reports_path() {
        let base = temp_dir("missing");
        let resolver = FsIncludeResolver::json(&base).expect("resolver");
        let error = resolver.resolve("nope.json").expect_err("missing error");
        assert!(error.to_string().contains("nope.json"));

        fs::remove_dir_all(&base).expect("cleanup");
    }
}
