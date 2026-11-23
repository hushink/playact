//! Configuration linting against a TreeSpec JSON Schema.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use playact::config::ValidationLimits;
use serde::Serialize;
use serde_json::{Value, json};

use crate::format::Format;

const MAX_NODE_DEPTH: usize = 128;

/// One lint finding.
#[derive(Debug, Serialize)]
pub struct LintIssue {
    /// Origin of the issue, such as `schema`, `parse`, `limits`, `format`, or `io`.
    pub source: &'static str,
    /// JSON pointer or configuration path, depending on the source.
    pub path: String,
    /// Human-readable message.
    pub message: String,
}

impl LintIssue {
    pub(crate) fn new(
        source: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            source,
            path: path.into(),
            message: message.into(),
        }
    }
}

/// Result of linting one file.
#[derive(Debug, Serialize)]
pub struct LintReport {
    /// File path as provided on the command line.
    pub file: String,
    /// Whether the file passed every check.
    pub valid: bool,
    /// Findings in reporting order.
    pub issues: Vec<LintIssue>,
}

/// Lints each file against `schema`.
#[must_use]
pub fn lint_files(schema: &Value, files: &[PathBuf]) -> Vec<LintReport> {
    files.iter().map(|path| lint_file(schema, path)).collect()
}

fn lint_file(schema: &Value, path: &Path) -> LintReport {
    let file = path.display().to_string();
    let mut issues = Vec::new();

    let format = match Format::from_path(path) {
        Ok(format) => format,
        Err(message) => {
            return LintReport {
                file,
                valid: false,
                issues: vec![LintIssue::new("format", "", message)],
            };
        }
    };
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            return LintReport {
                file,
                valid: false,
                issues: vec![LintIssue::new("io", "", error.to_string())],
            };
        }
    };
    match format.parse_document(&text) {
        Ok(document) => issues.extend(schema_issues(schema, &document)),
        Err(error) => {
            issues.push(LintIssue::new(
                "parse",
                "",
                format!("invalid {}: {error}", format.name()),
            ));
            return LintReport {
                file,
                valid: false,
                issues,
            };
        }
    }

    match format.parse_spec(&text) {
        Ok(spec) => {
            if let Err(error) = spec.validate(&ValidationLimits::default()) {
                issues.push(LintIssue::new(
                    "limits",
                    error.path().unwrap_or(""),
                    error.message(),
                ));
            }
        }
        Err(error) => issues.push(LintIssue::new(
            "parse",
            error.path().unwrap_or(""),
            error.message(),
        )),
    }

    LintReport {
        file,
        valid: issues.is_empty(),
        issues,
    }
}

fn schema_issues(schema: &Value, document: &Value) -> Vec<LintIssue> {
    let mut issues = Vec::new();

    match document.get("root") {
        Some(root) => lint_node(schema, root, "/root", &mut issues, 0),
        None => issues.push(LintIssue::new("schema", "/root", "missing root node")),
    }
    if let Some(services) = document.get("services") {
        match services.as_array() {
            Some(services) => {
                for (index, service) in services.iter().enumerate() {
                    lint_node(
                        schema,
                        service,
                        &format!("/services/{index}"),
                        &mut issues,
                        0,
                    );
                }
            }
            None => issues.push(LintIssue::new(
                "schema",
                "/services",
                "services must be an array",
            )),
        }
    }

    if let Ok(validator) = jsonschema::validator_for(schema) {
        let full_errors: Vec<_> = validator.iter_errors(document).collect();
        for error in &full_errors {
            let path = error.instance_path().to_string();
            if path.is_empty() || path.starts_with("/root") || path.starts_with("/services") {
                continue;
            }
            issues.push(LintIssue::new("schema", path, error.to_string()));
        }
        if issues.is_empty()
            && let Some(error) = full_errors.first()
        {
            let path = if error.instance_path().is_empty() {
                String::from("#")
            } else {
                error.instance_path().to_string()
            };
            issues.push(LintIssue::new("schema", path, schema_message(error)));
        }
    }

    issues
}

fn lint_node(schema: &Value, node: &Value, path: &str, issues: &mut Vec<LintIssue>, depth: usize) {
    if depth > MAX_NODE_DEPTH {
        issues.push(LintIssue::new(
            "schema",
            path,
            format!("node nesting exceeds {MAX_NODE_DEPTH} levels"),
        ));
        return;
    }
    let Some(object) = node.as_object() else {
        issues.push(LintIssue::new("schema", path, "node must be an object"));
        return;
    };
    let Some(kind) = object.get("type").and_then(Value::as_str) else {
        issues.push(LintIssue::new(
            "schema",
            format!("{path}/type"),
            "missing node type",
        ));
        return;
    };
    let Some(node_schema) = node_schema(schema, kind) else {
        issues.push(LintIssue::new(
            "schema",
            format!("{path}/type"),
            format!("unknown node type `{kind}`"),
        ));
        return;
    };

    match jsonschema::validator_for(&node_schema) {
        Ok(validator) => {
            let node_errors: Vec<_> = validator.iter_errors(node).collect();
            let enum_paths: HashSet<String> = node_errors
                .iter()
                .filter(|error| {
                    matches!(
                        error.kind(),
                        jsonschema::error::ValidationErrorKind::Enum { .. }
                    )
                })
                .map(|error| error.instance_path().to_string())
                .collect();
            for error in &node_errors {
                // Description metadata adds const-based oneOf entries on top of
                // the enum; report the more specific enum error instead.
                if matches!(
                    error.kind(),
                    jsonschema::error::ValidationErrorKind::OneOfNotValid { .. }
                ) && enum_paths.contains(&error.instance_path().to_string())
                {
                    continue;
                }
                issues.push(LintIssue::new(
                    "schema",
                    format!("{path}{}", error.instance_path()),
                    error.to_string(),
                ));
            }
        }
        Err(error) => issues.push(LintIssue::new(
            "schema",
            path,
            format!("cannot compile schema for `{kind}`: {error}"),
        )),
    }

    if let Some(child) = object.get("child") {
        lint_node(schema, child, &format!("{path}/child"), issues, depth + 1);
    }
    if let Some(then) = object.get("then") {
        lint_node(schema, then, &format!("{path}/then"), issues, depth + 1);
    }
    if let Some(otherwise) = object.get("else") {
        lint_node(
            schema,
            otherwise,
            &format!("{path}/else"),
            issues,
            depth + 1,
        );
    }
    if let Some(children) = object.get("children").and_then(Value::as_array) {
        for (index, child) in children.iter().enumerate() {
            lint_node(
                schema,
                child,
                &format!("{path}/children/{index}"),
                issues,
                depth + 1,
            );
        }
    }
    if let Some(branches) = object.get("branches").and_then(Value::as_array) {
        for (index, branch) in branches.iter().enumerate() {
            if let Some(child) = branch.get("child") {
                lint_node(
                    schema,
                    child,
                    &format!("{path}/branches/{index}/child"),
                    issues,
                    depth + 1,
                );
            }
        }
    }
}

fn node_schema(root: &Value, kind: &str) -> Option<Value> {
    let mut schema = root
        .pointer("/$defs/SpecNode/oneOf")?
        .as_array()?
        .iter()
        .find(|variant| {
            variant
                .pointer("/properties/type/const")
                .and_then(Value::as_str)
                == Some(kind)
        })?
        .clone();

    let object = schema.as_object_mut()?;
    if let Some(defs) = root.get("$defs") {
        object.insert(String::from("$defs"), defs.clone());
    }
    if let Some(meta) = root.get("$schema") {
        object.insert(String::from("$schema"), meta.clone());
    }
    neutralize_child_refs(&mut schema);
    Some(schema)
}

/// Replaces recursive SpecNode references with `{}` so one node can be checked
/// in isolation. Child nodes are visited separately with their own paths.
fn neutralize_child_refs(value: &mut Value) {
    if value.get("$ref").and_then(Value::as_str) == Some("#/$defs/SpecNode") {
        *value = json!({});
        return;
    }
    match value {
        Value::Object(object) => {
            for child in object.values_mut() {
                neutralize_child_refs(child);
            }
        }
        Value::Array(array) => {
            for child in array {
                neutralize_child_refs(child);
            }
        }
        _ => {}
    }
}

fn schema_message(error: &jsonschema::ValidationError<'_>) -> String {
    match error.kind() {
        jsonschema::error::ValidationErrorKind::OneOfNotValid { .. }
        | jsonschema::error::ValidationErrorKind::OneOfMultipleValid { .. } => {
            String::from("configuration does not match any known node shape")
        }
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use playact::config::{NodeCategory, NodeRegistry};
    use playact::prelude::*;

    fn registry_schema() -> Value {
        let mut registry = NodeRegistry::<()>::new();
        registry
            .action::<()>("patrol", |_| |_: &mut Context<'_, ()>| Status::Success)
            .unwrap();
        registry
            .condition::<()>("always", |_| |_: &Context<'_, ()>| true)
            .unwrap();
        registry
            .describe(NodeCategory::Action, "patrol", "Walk the route.")
            .unwrap();
        registry.json_schema()
    }

    #[test]
    fn reports_unknown_action_at_node_path() {
        let issues = schema_issues(
            &registry_schema(),
            &json!({
                "root": {
                    "type": "sequence",
                    "children": [{"type": "action", "name": "missing"}]
                }
            }),
        );
        assert!(
            issues.iter().any(|issue| {
                issue.path == "/root/children/0/name" && issue.message.contains("missing")
            }),
            "{issues:?}"
        );
    }

    #[test]
    fn reports_unknown_node_type() {
        let issues = schema_issues(
            &registry_schema(),
            &json!({"root": {"type": "no_such_node"}}),
        );
        assert!(
            issues.iter().any(|issue| {
                issue.path == "/root/type" && issue.message.contains("no_such_node")
            }),
            "{issues:?}"
        );
    }

    #[test]
    fn base_schema_has_no_registered_name_errors() {
        let issues = schema_issues(
            &playact::config::TreeSpec::json_schema(),
            &json!({"root": {"type": "action", "name": "anything"}}),
        );
        assert!(issues.is_empty(), "{issues:?}");
    }
}
