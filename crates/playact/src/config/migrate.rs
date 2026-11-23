//! Raw document version migration and format-aware configuration loading.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use serde_json::Value;

use super::{CURRENT_VERSION, ConfigError, TreeSpec};

type MigrationFn = dyn Fn(Value) -> Result<Value, ConfigError> + Send + Sync;

/// Supported configuration formats for version-aware loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigFormat {
    /// JSON.
    Json,
    /// TOML.
    Toml,
    /// YAML.
    Yaml,
    /// RON.
    Ron,
}

impl ConfigFormat {
    /// Selects a format from a file extension.
    ///
    /// # Errors
    ///
    /// Returns a message for missing or unsupported extensions.
    pub fn from_path(path: &Path) -> Result<Self, ConfigError> {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase);
        match extension.as_deref() {
            Some("json") => Ok(Self::Json),
            Some("toml") => Ok(Self::Toml),
            Some("yaml" | "yml") => Ok(Self::Yaml),
            Some("ron") => Ok(Self::Ron),
            Some(other) => Err(ConfigError::new(format!(
                "unsupported extension `.{other}`; expected .json, .toml, .yaml, .yml, or .ron"
            ))),
            None => Err(ConfigError::new(
                "configuration path has no extension; expected .json, .toml, .yaml, .yml, or .ron",
            )),
        }
    }

    /// Human-readable format name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Toml => "TOML",
            Self::Yaml => "YAML",
            Self::Ron => "RON",
        }
    }

    fn parse_raw(self, input: &str) -> Result<Value, ConfigError> {
        match self {
            Self::Json => serde_json::from_str(input)
                .map_err(|error| ConfigError::new(format!("invalid JSON: {error}"))),
            #[cfg(feature = "toml")]
            Self::Toml => toml::from_str(input)
                .map_err(|error| ConfigError::new(format!("invalid TOML: {error}"))),
            #[cfg(not(feature = "toml"))]
            Self::Toml => Err(ConfigError::new("TOML parsing requires the `toml` feature")),
            #[cfg(feature = "yaml")]
            Self::Yaml => serde_yaml_ng::from_str(input)
                .map_err(|error| ConfigError::new(format!("invalid YAML: {error}"))),
            #[cfg(not(feature = "yaml"))]
            Self::Yaml => Err(ConfigError::new("YAML parsing requires the `yaml` feature")),
            #[cfg(feature = "ron")]
            Self::Ron => ron::from_str(input)
                .map_err(|error| ConfigError::new(format!("invalid RON: {error}"))),
            #[cfg(not(feature = "ron"))]
            Self::Ron => Err(ConfigError::new("RON parsing requires the `ron` feature")),
        }
    }
}

/// Ordered migration pipeline for raw configuration documents.
///
/// A migration receives the document at version `N` and returns the document at
/// version `N + 1`. The pipeline stamps the resulting version, so migration
/// functions only transform the structure. The final document is deserialized
/// as a [`TreeSpec`] after the chain reaches [`CURRENT_VERSION`].
///
/// Migrations run on the raw JSON representation, which every supported format
/// parses into. They are application-supplied and the engine never interprets
/// business fields.
#[derive(Clone, Default)]
pub struct ConfigMigrator {
    migrations: BTreeMap<u32, Arc<MigrationFn>>,
}

impl ConfigMigrator {
    /// Creates an empty migration pipeline.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            migrations: BTreeMap::new(),
        }
    }

    /// Registers the migration from `from` to `from + 1`.
    ///
    /// # Errors
    ///
    /// Returns an error when `from` is not below [`CURRENT_VERSION`] or a
    /// migration from the same version is already registered.
    pub fn register(
        &mut self,
        from: u32,
        migration: impl Fn(Value) -> Result<Value, ConfigError> + Send + Sync + 'static,
    ) -> Result<(), ConfigError> {
        if from >= CURRENT_VERSION {
            return Err(ConfigError::new(format!(
                "migration source version {from} must be lower than {CURRENT_VERSION}"
            )));
        }
        if self.migrations.contains_key(&from) {
            return Err(ConfigError::new(format!(
                "a migration from version {from} is already registered"
            )));
        }
        self.migrations.insert(from, Arc::new(migration));
        Ok(())
    }

    /// Builder form of [`ConfigMigrator::register`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`ConfigMigrator::register`].
    pub fn with_migration(
        mut self,
        from: u32,
        migration: impl Fn(Value) -> Result<Value, ConfigError> + Send + Sync + 'static,
    ) -> Result<Self, ConfigError> {
        self.register(from, migration)?;
        Ok(self)
    }

    /// Upgrades a raw document to [`CURRENT_VERSION`].
    ///
    /// A missing `version` field defaults to [`CURRENT_VERSION`], matching
    /// [`TreeSpec`]. Every migration step overwrites the document's `version`
    /// with the next version after it succeeds.
    ///
    /// # Errors
    ///
    /// Returns an error for a malformed or future version, a missing migration
    /// step, or a migration that fails or does not return an object.
    pub fn upgrade(&self, document: Value) -> Result<Value, ConfigError> {
        self.upgrade_to(document, CURRENT_VERSION)
    }

    fn upgrade_to(&self, mut document: Value, target: u32) -> Result<Value, ConfigError> {
        let mut version = document_version(&document)?;
        if version > target {
            return Err(ConfigError::with_path(
                "version",
                format!("unsupported version {version}; supported is {target}"),
            ));
        }
        while version < target {
            let next = version + 1;
            let migration = self.migrations.get(&version).ok_or_else(|| {
                ConfigError::with_path(
                    "version",
                    format!("no migration registered from version {version} to {next}"),
                )
            })?;
            let migrated =
                migration(document).map_err(|error| migration_error(version, next, &error))?;
            document = migrated;
            let object = document.as_object_mut().ok_or_else(|| {
                ConfigError::with_path(
                    "version",
                    format!("migration from version {version} to {next} must return an object"),
                )
            })?;
            object.insert(String::from("version"), Value::from(next));
            version = next;
        }
        Ok(document)
    }

    /// Parses JSON and applies the migration chain.
    ///
    /// # Errors
    ///
    /// Returns parse, migration, or final [`TreeSpec`] deserialization errors.
    pub fn parse_json_str(&self, input: &str) -> Result<TreeSpec, ConfigError> {
        self.parse_str(ConfigFormat::Json, input)
    }

    /// Parses JSON bytes and applies the migration chain.
    ///
    /// # Errors
    ///
    /// Returns parse, migration, or final [`TreeSpec`] deserialization errors.
    pub fn parse_json_slice(&self, input: &[u8]) -> Result<TreeSpec, ConfigError> {
        let document = serde_json::from_slice(input)
            .map_err(|error| ConfigError::new(format!("invalid JSON: {error}")))?;
        self.parse_value(document)
    }

    /// Parses TOML and applies the migration chain.
    ///
    /// # Errors
    ///
    /// Returns parse, migration, or final [`TreeSpec`] deserialization errors.
    #[cfg(feature = "toml")]
    pub fn parse_toml_str(&self, input: &str) -> Result<TreeSpec, ConfigError> {
        self.parse_str(ConfigFormat::Toml, input)
    }

    /// Parses YAML and applies the migration chain.
    ///
    /// # Errors
    ///
    /// Returns parse, migration, or final [`TreeSpec`] deserialization errors.
    #[cfg(feature = "yaml")]
    pub fn parse_yaml_str(&self, input: &str) -> Result<TreeSpec, ConfigError> {
        self.parse_str(ConfigFormat::Yaml, input)
    }

    /// Parses RON and applies the migration chain.
    ///
    /// # Errors
    ///
    /// Returns parse, migration, or final [`TreeSpec`] deserialization errors.
    #[cfg(feature = "ron")]
    pub fn parse_ron_str(&self, input: &str) -> Result<TreeSpec, ConfigError> {
        self.parse_str(ConfigFormat::Ron, input)
    }

    /// Parses a specific format and applies the migration chain.
    ///
    /// # Errors
    ///
    /// Returns an error when the format's feature is disabled, the input is
    /// malformed, a migration fails, or the final document is not a valid
    /// [`TreeSpec`].
    pub fn parse_str(&self, format: ConfigFormat, input: &str) -> Result<TreeSpec, ConfigError> {
        let document = format.parse_raw(input)?;
        self.parse_value(document)
    }

    /// Selects a format from `path` and applies the migration chain.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`ConfigFormat::from_path`] and
    /// [`ConfigMigrator::parse_str`].
    pub fn parse_path(&self, path: &Path, input: &str) -> Result<TreeSpec, ConfigError> {
        self.parse_str(ConfigFormat::from_path(path)?, input)
    }

    /// Applies the migration chain to an already parsed document.
    ///
    /// # Errors
    ///
    /// Returns migration or final [`TreeSpec`] deserialization errors.
    pub fn parse_value(&self, document: Value) -> Result<TreeSpec, ConfigError> {
        let document = self.upgrade(document)?;
        let json = serde_json::to_string(&document).map_err(|error| {
            ConfigError::new(format!("serialize migrated configuration: {error}"))
        })?;
        TreeSpec::from_json_str(&json)
    }

    /// Creates a text parser for `format` that owns a clone of this pipeline.
    ///
    /// The closure is suitable for hot-reload and remote-loader constructors.
    pub fn parser(
        &self,
        format: ConfigFormat,
    ) -> impl Fn(&str) -> Result<TreeSpec, ConfigError> + Send + Sync + 'static {
        let migrator = self.clone();
        move |input| migrator.parse_str(format, input)
    }
}

impl fmt::Debug for ConfigMigrator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigMigrator")
            .field(
                "registered_from",
                &self.migrations.keys().collect::<Vec<&u32>>(),
            )
            .finish()
    }
}

fn document_version(document: &Value) -> Result<u32, ConfigError> {
    let Some(version) = document.get("version") else {
        return Ok(CURRENT_VERSION);
    };
    let Some(version) = version.as_u64() else {
        return Err(ConfigError::with_path(
            "version",
            "version must be a non-negative integer",
        ));
    };
    u32::try_from(version).map_err(|_| {
        ConfigError::with_path("version", format!("version {version} is out of range"))
    })
}

fn migration_error(from: u32, to: u32, error: &ConfigError) -> ConfigError {
    let path = error
        .path()
        .filter(|path| !path.is_empty())
        .unwrap_or("version");
    ConfigError::with_path(
        path,
        format!(
            "migration from version {from} to {to} failed: {}",
            error.message()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rename_tree_to_root() -> ConfigMigrator {
        ConfigMigrator::new()
            .with_migration(0, |mut document| {
                let object = document
                    .as_object_mut()
                    .ok_or_else(|| ConfigError::new("migration input must be an object"))?;
                let root = object
                    .remove("tree")
                    .ok_or_else(|| ConfigError::with_path("tree", "missing legacy tree field"))?;
                object.insert(String::from("root"), root);
                Ok(document)
            })
            .expect("register migration")
    }

    #[test]
    fn upgrades_documents_and_stamps_the_version() {
        let spec = rename_tree_to_root()
            .parse_json_str(r#"{"version":0,"tree":{"type":"action","name":"ok"}}"#)
            .expect("migrated spec");
        assert_eq!(spec.version, CURRENT_VERSION);
        assert!(matches!(spec.root, super::super::SpecNode::Action { .. }));
    }

    #[test]
    fn runs_multiple_steps_in_order() {
        fn mark(index: u32) -> Arc<MigrationFn> {
            Arc::new(move |mut document: Value| {
                let object = document
                    .as_object_mut()
                    .ok_or_else(|| ConfigError::new("migration input must be an object"))?;
                object.insert(format!("step{index}"), Value::Bool(true));
                Ok(document)
            })
        }

        let mut migrator = ConfigMigrator::new();
        migrator.migrations.insert(0, mark(0));
        migrator.migrations.insert(1, mark(1));
        migrator.migrations.insert(2, mark(2));

        let upgraded = migrator
            .upgrade_to(serde_json::json!({"version": 0, "root": {}}), 3)
            .expect("three migration steps");
        assert_eq!(upgraded["version"], 3);
        assert_eq!(upgraded["step0"], true);
        assert_eq!(upgraded["step1"], true);
        assert_eq!(upgraded["step2"], true);
    }

    #[test]
    fn missing_step_is_reported_at_version() {
        let error = ConfigMigrator::new()
            .parse_json_str(r#"{"version":0,"root":{"type":"action","name":"ok"}}"#)
            .expect_err("missing migration");
        assert_eq!(error.path(), Some("version"));
        assert!(
            error.to_string().contains("no migration registered"),
            "{error}"
        );
    }

    #[test]
    fn future_and_malformed_versions_are_rejected() {
        let migrator = rename_tree_to_root();
        let future = migrator
            .parse_json_str(r#"{"version":2,"root":{"type":"action","name":"ok"}}"#)
            .expect_err("future version");
        assert_eq!(future.path(), Some("version"));
        assert!(
            future.to_string().contains("unsupported version"),
            "{future}"
        );

        let malformed = migrator
            .parse_json_str(r#"{"version":"one","root":{"type":"action","name":"ok"}}"#)
            .expect_err("malformed version");
        assert_eq!(malformed.path(), Some("version"));
        assert!(
            malformed.to_string().contains("non-negative integer"),
            "{malformed}"
        );
    }

    #[test]
    fn migration_errors_keep_the_inner_path() {
        let migrator = ConfigMigrator::new()
            .with_migration(0, |_| {
                Err(ConfigError::with_path("root", "cannot rewrite root"))
            })
            .expect("register migration");
        let error = migrator
            .parse_json_str(r#"{"version":0,"root":{"type":"action","name":"ok"}}"#)
            .expect_err("migration error");
        assert_eq!(error.path(), Some("root"));
        assert!(
            error.to_string().contains("migration from version 0 to 1"),
            "{error}"
        );
        assert!(error.to_string().contains("cannot rewrite root"), "{error}");
    }

    #[test]
    fn migrations_must_return_an_object() {
        let migrator = ConfigMigrator::new()
            .with_migration(0, |_| Ok(Value::Null))
            .expect("register migration");
        let error = migrator
            .parse_json_str(r#"{"version":0,"root":{"type":"action","name":"ok"}}"#)
            .expect_err("non-object migration");
        assert_eq!(error.path(), Some("version"));
        assert!(
            error.to_string().contains("must return an object"),
            "{error}"
        );
    }

    #[test]
    fn duplicate_and_current_version_registrations_are_rejected() {
        let mut migrator = rename_tree_to_root();
        assert!(
            migrator
                .register(0, Ok)
                .expect_err("duplicate")
                .to_string()
                .contains("already registered")
        );
        assert!(
            migrator
                .register(CURRENT_VERSION, Ok)
                .expect_err("current version")
                .to_string()
                .contains("must be lower")
        );
    }

    #[test]
    fn parses_paths_and_exposes_a_parser_closure() {
        let migrator = rename_tree_to_root();
        let spec = migrator
            .parse_path(
                Path::new("tree.JSON"),
                r#"{"version":0,"tree":{"type":"action","name":"ok"}}"#,
            )
            .expect("path parse");
        assert_eq!(spec.version, CURRENT_VERSION);

        let parser = migrator.parser(ConfigFormat::Json);
        let spec = parser(r#"{"version":0,"tree":{"type":"action","name":"ok"}}"#)
            .expect("parser closure");
        assert_eq!(spec.version, CURRENT_VERSION);
    }

    #[test]
    fn unsupported_paths_are_rejected() {
        let error = ConfigFormat::from_path(Path::new("tree.txt")).expect_err("extension");
        assert!(
            error.to_string().contains("unsupported extension"),
            "{error}"
        );
        let error = ConfigFormat::from_path(Path::new("tree")).expect_err("missing extension");
        assert!(error.to_string().contains("no extension"), "{error}");
    }

    #[cfg(feature = "toml")]
    #[test]
    fn parses_toml_with_migrations() {
        let spec = rename_tree_to_root()
            .parse_toml_str("version = 0\n[tree]\ntype = \"action\"\nname = \"ok\"\n")
            .expect("TOML migration");
        assert_eq!(spec.version, CURRENT_VERSION);
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn parses_yaml_with_migrations() {
        let spec = rename_tree_to_root()
            .parse_yaml_str("version: 0\ntree:\n  type: action\n  name: ok\n")
            .expect("YAML migration");
        assert_eq!(spec.version, CURRENT_VERSION);
    }

    #[cfg(feature = "ron")]
    #[test]
    fn parses_ron_with_migrations() {
        let spec = rename_tree_to_root()
            .parse_ron_str("(version: 0, tree: (type: \"action\", name: \"ok\"))")
            .expect("RON migration");
        assert_eq!(spec.version, CURRENT_VERSION);
    }
}
