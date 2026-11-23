//! Error types for the configuration layer.

use core::fmt;

use crate::error::BuildError;

/// JSON/TOML/YAML/RON parse error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    path: Option<String>,
    message: String,
}

impl ConfigError {
    /// Creates an error without a path.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            path: None,
            message: message.into(),
        }
    }

    /// Creates an error with a configuration path.
    #[must_use]
    pub fn with_path(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: Some(path.into()),
            message: message.into(),
        }
    }

    /// Configuration path, for example `root.children[0]`.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// Error message.
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

/// Error raised while building a behavior tree from a `TreeSpec`.
#[derive(Debug)]
pub struct ConfigBuildError {
    path: Option<String>,
    kind: ConfigBuildErrorKind,
}

impl ConfigBuildError {
    /// Creates an error with no path bound.
    #[must_use]
    pub fn new(kind: ConfigBuildErrorKind) -> Self {
        Self { path: None, kind }
    }

    /// Binds a configuration path; if one is already set, the original is kept.
    #[must_use]
    pub fn at(mut self, path: impl Into<String>) -> Self {
        if self.path.is_none() {
            self.path = Some(path.into());
        }
        self
    }

    /// Configuration path.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// Error kind.
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

/// Specific kinds of configuration build errors.
#[derive(Debug)]
#[non_exhaustive]
pub enum ConfigBuildErrorKind {
    /// Unsupported spec version.
    UnsupportedVersion {
        /// Version found in the configuration.
        found: u32,
        /// Currently supported version.
        supported: u32,
    },
    /// Unregistered action name.
    UnknownAction {
        /// Action name.
        name: String,
    },
    /// Unregistered condition name.
    UnknownCondition {
        /// Condition name.
        name: String,
    },
    /// Unregistered predicate name used by `guard`, `while`, or `if_then_else`.
    UnknownPredicate {
        /// Predicate name.
        name: String,
    },
    /// Unregistered switch key name.
    UnknownSwitchKey {
        /// Switch key name.
        key: String,
    },
    /// Unregistered set-value key name.
    UnknownSetValueKey {
        /// Set-value key name.
        key: String,
    },
    /// Unregistered event key name.
    UnknownEventKey {
        /// Event key name.
        key: String,
    },
    /// Unregistered subtree-contract key name.
    UnknownContractKey {
        /// Contract key name.
        key: String,
    },
    /// Unregistered scoped-subtree key name.
    UnknownScopedKey {
        /// Scoped key name.
        key: String,
    },
    /// Unregistered utility score name.
    UnknownScore {
        /// Score name.
        name: String,
    },
    /// Unregistered fallible action name.
    UnknownTryAction {
        /// Action name.
        name: String,
    },
    /// Unregistered async action name.
    UnknownAsyncAction {
        /// Action name.
        name: String,
    },
    /// The same stable `NodeId` appears more than once in the built tree.
    DuplicateNodeId {
        /// Duplicate identity.
        id: String,
        /// Configuration path of the second occurrence.
        path: String,
    },
    /// Unregistered custom kind.
    UnknownCustom {
        /// Custom kind.
        kind: String,
    },
    /// Unregistered subtree template.
    UnknownSubtreeTemplate {
        /// Template name.
        template: String,
    },
    /// Leaf parameter deserialization failed.
    InvalidParams {
        /// Error message.
        message: String,
    },
    /// Invalid `Parallel` threshold.
    InvalidThreshold(BuildError),
    /// Invalid tree structure.
    InvalidTree {
        /// Error message.
        message: String,
    },
    /// Error returned by a custom node factory.
    Custom {
        /// Error message.
        message: String,
    },
    /// Include resolution failed.
    Include {
        /// Include path.
        path: String,
        /// Error message.
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
            Self::UnknownPredicate { name } => write!(f, "unknown predicate `{name}`"),
            Self::UnknownSwitchKey { key } => write!(f, "unknown switch key `{key}`"),
            Self::UnknownSetValueKey { key } => write!(f, "unknown set-value key `{key}`"),
            Self::UnknownEventKey { key } => write!(f, "unknown event key `{key}`"),
            Self::UnknownContractKey { key } => write!(f, "unknown contract key `{key}`"),
            Self::UnknownScopedKey { key } => write!(f, "unknown scoped key `{key}`"),
            Self::UnknownScore { name } => write!(f, "unknown utility score `{name}`"),
            Self::UnknownTryAction { name } => write!(f, "unknown fallible action `{name}`"),
            Self::UnknownAsyncAction { name } => write!(f, "unknown async action `{name}`"),
            Self::DuplicateNodeId { id, path } => {
                write!(f, "duplicate node id `{id}` at {path}")
            }
            Self::UnknownCustom { kind } => write!(f, "unknown custom kind `{kind}`"),
            Self::UnknownSubtreeTemplate { template } => {
                write!(f, "unknown subtree template `{template}`")
            }
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

/// Error raised when registering node factories.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistryError {
    /// Name is empty or whitespace only.
    EmptyName {
        /// Registration category, such as `action` or `score`.
        category: &'static str,
    },
    /// Duplicate name within the same category.
    Duplicate {
        /// Registration category, such as `action` or `score`.
        category: &'static str,
        /// The duplicate name.
        name: String,
    },
    /// Documentation was attached to a name that is not registered.
    UnknownName {
        /// Registration category, such as `action` or `score`.
        category: &'static str,
        /// The name that was not registered.
        name: String,
    },
    /// The category cannot carry the requested schema.
    UnsupportedSchema {
        /// Registration category, such as `switch key`.
        category: &'static str,
        /// Schema being attached, such as `params`.
        target: &'static str,
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
            Self::UnknownName { category, name } => {
                write!(f, "cannot describe unknown {category} `{name}`")
            }
            Self::UnsupportedSchema { category, target } => {
                write!(
                    f,
                    "the {category} category does not support a {target} schema"
                )
            }
        }
    }
}

impl std::error::Error for RegistryError {}
