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

    /// Serializes a `TreeSpec` in this format.
    ///
    /// # Errors
    ///
    /// Returns the format serializer's message.
    pub fn serialize_spec(self, spec: &TreeSpec) -> Result<Vec<u8>, String> {
        match self {
            Self::Json => serde_json::to_vec_pretty(spec).map_err(|error| error.to_string()),
            Self::Toml => serialize_toml(spec),
            Self::Yaml => serde_yaml_ng::to_string(spec)
                .map(String::into_bytes)
                .map_err(|error| error.to_string()),
            Self::Ron => ron::ser::to_string_pretty(spec, ron::ser::PrettyConfig::default())
                .map(String::into_bytes)
                .map_err(|error| error.to_string()),
        }
    }
}

fn serialize_toml(spec: &TreeSpec) -> Result<Vec<u8>, String> {
    let value = serde_json::to_value(spec).map_err(|error| error.to_string())?;
    let value = strip_nulls(value)?;
    let value: toml::Value = serde_json::from_value(value)
        .map_err(|error| format!("TOML cannot represent this configuration: {error}"))?;
    toml::to_string_pretty(&value)
        .map(String::into_bytes)
        .map_err(|error| error.to_string())
}

fn strip_nulls(value: Value) -> Result<Value, String> {
    match value {
        Value::Object(object) => {
            let mut cleaned = serde_json::Map::new();
            for (key, value) in object {
                if !value.is_null() {
                    cleaned.insert(key, strip_nulls(value)?);
                }
            }
            Ok(Value::Object(cleaned))
        }
        Value::Array(values) => values
            .into_iter()
            .map(strip_nulls)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Null => Err(String::from("TOML cannot represent null values")),
        other => Ok(other),
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
    fn serializes_specs_in_all_formats() {
        let spec = TreeSpec::from_json_str(r#"{"root":{"type":"action","name":"wait"}}"#)
            .expect("valid TreeSpec");
        for format in [Format::Json, Format::Toml, Format::Yaml, Format::Ron] {
            let bytes = format
                .serialize_spec(&spec)
                .unwrap_or_else(|error| panic!("{format:?}: {error}"));
            let text = std::str::from_utf8(&bytes).expect("UTF-8 output");
            let reparsed = format
                .parse_spec(text)
                .unwrap_or_else(|error| panic!("{format:?}: {error}\n{text}"));
            assert_eq!(reparsed.version, spec.version, "{format:?}");
        }
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
