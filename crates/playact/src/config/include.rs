//! include 解析与安全的文件系统解析器。

use std::fs;
use std::path::{Path, PathBuf};

use super::{ConfigError, TreeParser, TreeSpec};

/// 把 include 路径解析成一份 [`TreeSpec`]。
///
/// 实现可以读取文件、发起 HTTP 请求或查内存表；引擎只负责递归构建与
/// 深度/循环保护。闭包 `Fn(&str) -> Result<TreeSpec, ConfigError>` 自动
/// 实现本 trait。
pub trait IncludeResolver {
    /// 解析一个 include 路径。
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

/// 从基准目录读取 include 文件的解析器。
///
/// 目标路径会经 `canonicalize` 规范化，并必须仍位于基准目录内，从而拒绝
/// `..`、绝对路径和符号链接逃逸。基准目录在构造时规范化并校验存在。
pub struct FsIncludeResolver {
    base: PathBuf,
    parser: TreeParser,
}

impl FsIncludeResolver {
    /// 用基准目录和 parser 构造；基准目录必须存在且是目录。
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

    /// 从基准目录读取 JSON include。
    pub fn json(base: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        Self::new(base, TreeSpec::from_json_str)
    }

    /// 从基准目录读取 TOML include。
    #[cfg(feature = "toml")]
    pub fn toml(base: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        Self::new(base, TreeSpec::from_toml_str)
    }

    /// 从基准目录读取 YAML include。
    #[cfg(feature = "yaml")]
    pub fn yaml(base: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        Self::new(base, TreeSpec::from_yaml_str)
    }

    /// 从基准目录读取 RON include。
    #[cfg(feature = "ron")]
    pub fn ron(base: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        Self::new(base, TreeSpec::from_ron_str)
    }

    /// 规范化后的基准目录。
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
