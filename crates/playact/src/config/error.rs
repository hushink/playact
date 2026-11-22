//! 配置层错误类型。

use core::fmt;

use crate::error::BuildError;

/// JSON/TOML/YAML/RON 解析错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    path: Option<String>,
    message: String,
}

impl ConfigError {
    /// 构造一个无路径错误。
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            path: None,
            message: message.into(),
        }
    }

    /// 构造一个带配置路径的错误。
    #[must_use]
    pub fn with_path(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: Some(path.into()),
            message: message.into(),
        }
    }

    /// 配置路径，例如 `root.children[0]`。
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// 错误消息。
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(path) => write!(f, "{path}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ConfigError {}

/// 从 `TreeSpec` 构建行为树时的错误。
#[derive(Debug)]
pub struct ConfigBuildError {
    path: Option<String>,
    kind: ConfigBuildErrorKind,
}

impl ConfigBuildError {
    /// 构造一个未绑定路径的错误。
    #[must_use]
    pub fn new(kind: ConfigBuildErrorKind) -> Self {
        Self { path: None, kind }
    }

    /// 绑定配置路径；已有路径时保留原路径。
    #[must_use]
    pub fn at(mut self, path: impl Into<String>) -> Self {
        if self.path.is_none() {
            self.path = Some(path.into());
        }
        self
    }

    /// 配置路径。
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// 错误种类。
    #[must_use]
    pub fn kind(&self) -> &ConfigBuildErrorKind {
        &self.kind
    }
}

impl fmt::Display for ConfigBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(path) => write!(f, "{path}: {}", self.kind),
            None => self.kind.fmt(f),
        }
    }
}

impl std::error::Error for ConfigBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ConfigBuildErrorKind::InvalidThreshold(error) => Some(error),
            _ => None,
        }
    }
}

/// 配置构建错误的具体种类。
#[derive(Debug)]
pub enum ConfigBuildErrorKind {
    /// 不支持的 spec 版本。
    UnsupportedVersion {
        /// 配置中的版本。
        found: u32,
        /// 当前支持的版本。
        supported: u32,
    },
    /// 未注册的 action 名字。
    UnknownAction {
        /// action 名字。
        name: String,
    },
    /// 未注册的 condition 名字。
    UnknownCondition {
        /// condition 名字。
        name: String,
    },
    /// 未注册的 custom kind。
    UnknownCustom {
        /// custom kind。
        kind: String,
    },
    /// 叶子参数反序列化失败。
    InvalidParams {
        /// 错误消息。
        message: String,
    },
    /// `Parallel` 阈值不合法。
    InvalidThreshold(BuildError),
    /// 树结构不合法。
    InvalidTree {
        /// 错误消息。
        message: String,
    },
    /// 自定义节点工厂返回的错误。
    Custom {
        /// 错误消息。
        message: String,
    },
    /// include 解析失败。
    Include {
        /// include 路径。
        path: String,
        /// 错误消息。
        message: String,
    },
}

impl fmt::Display for ConfigBuildErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion { found, supported } => write!(
                f,
                "unsupported spec version {found}; supported version is {supported}"
            ),
            Self::UnknownAction { name } => write!(f, "unknown action `{name}`"),
            Self::UnknownCondition { name } => write!(f, "unknown condition `{name}`"),
            Self::UnknownCustom { kind } => write!(f, "unknown custom kind `{kind}`"),
            Self::InvalidParams { message } => write!(f, "invalid params: {message}"),
            Self::InvalidThreshold(error) => error.fmt(f),
            Self::InvalidTree { message } => f.write_str(message),
            Self::Custom { message } => f.write_str(message),
            Self::Include { path, message } => {
                write!(f, "include `{path}` failed: {message}")
            }
        }
    }
}

/// 注册节点工厂时的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// 名字为空或只包含空白。
    EmptyName {
        /// `action` / `condition` / `custom`。
        category: &'static str,
    },
    /// 同一类别中名字重复。
    Duplicate {
        /// `action` / `condition` / `custom`。
        category: &'static str,
        /// 重复的名字。
        name: String,
    },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName { category } => {
                write!(f, "{category} name must not be empty")
            }
            Self::Duplicate { category, name } => {
                write!(f, "duplicate {category} name `{name}`")
            }
        }
    }
}

impl std::error::Error for RegistryError {}
