//! Configuration format detection and parsing.

use std::path::Path;

use playact::config::{ConfigError, TreeSpec};
use serde_json::Value;

/// Supported configuration formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Toml,
    Yaml,
    Ron,
}

impl Format {
    /// Selects a format from a file extension.
    ///
    /// # Errors
    ///
    /// Returns a message for missing or unsupported extensions.
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase);
        match extension.as_deref() {
            Some("json") => Ok(Self::Json),
            Some("toml") => Ok(Self::Toml),
            Some("yaml" | "yml") => Ok(Self::Yaml),
            Some("ron") => Ok(Self::Ron),
            Some(other) => Err(format!(
                "unsupported extension `.{other}`; expected .json, .toml, .yaml, .yml, or .ron"
            )),
            None => Err(String::from(
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

    /// Parses into a generic JSON value for schema validation.
    ///
    /// # Errors
    ///
    /// Returns the format parser's message.
    pub fn parse_document(self, text: &str) -> Result<Value, String> {
        match self {
            Self::Json => serde_json::from_str(text).map_err(|error| error.to_string()),
            Self::Toml => toml::from_str(text).map_err(|error| error.to_string()),
            Self::Yaml => serde_yaml_ng::from_str(text).map_err(|error| error.to_string()),
            Self::Ron => ron::from_str(text).map_err(|error| error.to_string()),
        }
    }

    /// Parses into a `TreeSpec` with the matching library constructor.
    ///
    /// # Errors
    ///
    /// Returns the configuration parser's error.
    pub fn parse_spec(self, text: &str) -> Result<TreeSpec, ConfigError> {
        match self {
            Self::Json => TreeSpec::from_json_str(text),
            Self::Toml => TreeSpec::from_toml_str(text),
            Self::Yaml => TreeSpec::from_yaml_str(text),
            Self::Ron => TreeSpec::from_ron_str(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_extensions() {
        assert_eq!(Format::from_path(Path::new("tree.JSON")), Ok(Format::Json));
        assert_eq!(Format::from_path(Path::new("tree.toml")), Ok(Format::Toml));
        assert_eq!(Format::from_path(Path::new("tree.yml")), Ok(Format::Yaml));
        assert_eq!(Format::from_path(Path::new("tree.ron")), Ok(Format::Ron));
        assert!(Format::from_path(Path::new("tree.txt")).is_err());
        assert!(Format::from_path(Path::new("tree")).is_err());
    }

    #[test]
    fn parses_all_formats_into_specs() {
        let cases = [
            (Format::Json, r#"{"root":{"type":"action","name":"wait"}}"#),
            (
                Format::Toml,
                "version = 1\n[root]\ntype = \"action\"\nname = \"wait\"\n",
            ),
            (
                Format::Yaml,
                "version: 1\nroot:\n  type: action\n  name: wait\n",
            ),
            (
                Format::Ron,
                "(version: 1, root: (type: \"action\", name: \"wait\"))",
            ),
        ];
        for (format, text) in cases {
            let document = format
                .parse_document(text)
                .unwrap_or_else(|error| panic!("{format:?}: {error}"));
            assert!(document.get("root").is_some(), "{format:?}");
            let spec = format
                .parse_spec(text)
                .unwrap_or_else(|error| panic!("{format:?}: {error}"));
            assert_eq!(spec.version, 1, "{format:?}");
        }
    }
}
