//! Preflight tree configurations with schema validation, then build them.
//!
//! Run:
//!
//! ```text
//! cargo run --example config_lint --features config,schema
//! cargo run --example config_lint --features config,schema -- a.json b.json
//! cargo run --example config_lint --features config,schema -- --json a.json
//! ```
//!
//! Without file arguments the example checks `examples/battle_ai.json`. The
//! demo registry matches that file; replace [`build_registry`] with your own
//! registrations to lint other configurations.
//!
//! Schema validation runs first and drills into each node so errors point at
//! the failing field. Building then reports the precise configuration path for
//! failures such as unknown names, bad parameters, or duplicate IDs. The
//! process exits nonzero when any file is invalid, and `--json` prints a
//! machine-readable report for CI.

use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use playact::config::{NodeCategory, NodeRegistry, TreeSpec};
use playact::prelude::*;

const MAX_NODE_DEPTH: usize = 128;
const USAGE: &str = "\
usage: config_lint [--json] [CONFIG...]

Checks each JSON configuration against the demo registry schema, then builds it.
Without CONFIG, examples/battle_ai.json is checked. --json prints a
machine-readable report.";

// The fields are read through the derived `JsonSchema` and `Deserialize` impls.
#[allow(dead_code)]
#[derive(Debug, Deserialize, playact::schemars::JsonSchema)]
struct HealParams {
    /// Healing amount.
    amount: f32,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, playact::schemars::JsonSchema)]
struct HpBelowParams {
    /// Health ratio threshold.
    ratio: f32,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, playact::schemars::JsonSchema)]
struct MultiStrikeParams {
    /// Damage per hit.
    damage: f32,
    /// Number of hits.
    hits: u32,
}

#[derive(Debug, Serialize)]
struct LintIssue {
    source: &'static str,
    path: String,
    message: String,
}

impl LintIssue {
    fn new(source: &'static str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            source,
            path: path.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Serialize)]
struct LintReport {
    file: String,
    valid: bool,
    issues: Vec<LintIssue>,
}

struct Options {
    json: bool,
    files: Vec<PathBuf>,
}

enum Parsed {
    Run(Options),
    Help,
}

fn main() -> ExitCode {
    let options = match parse_options() {
        Ok(Parsed::Run(options)) => options,
        Ok(Parsed::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("config_lint: {error}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let registry = match build_registry() {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("config_lint: {error}");
            return ExitCode::FAILURE;
        }
    };
    let schema = registry.json_schema();
    let reports: Vec<LintReport> = options
        .files
        .iter()
        .map(|path| lint_file(&registry, &schema, path))
        .collect();
    let invalid = reports.iter().filter(|report| !report.valid).count();

    if options.json {
        match serde_json::to_string_pretty(&reports) {
            Ok(report) => println!("{report}"),
            Err(error) => {
                eprintln!("config_lint: cannot serialize report: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        for report in &reports {
            if report.valid {
                println!("{}: valid", report.file);
            } else {
                eprintln!("{}:", report.file);
                for issue in &report.issues {
                    eprintln!(
                        "  {} error at `{}`: {}",
                        issue.source, issue.path, issue.message
                    );
                }
            }
        }
        if reports.len() > 1 {
            eprintln!("{} of {} configuration(s) invalid", invalid, reports.len());
        }
    }

    if invalid == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn parse_options() -> Result<Parsed, String> {
    let mut json_output = false;
    let mut options_done = false;
    let mut files = Vec::new();

    for argument in env::args_os().skip(1) {
        if !options_done {
            match argument.to_str() {
                Some("--json") => {
                    json_output = true;
                    continue;
                }
                Some("-h" | "--help") => return Ok(Parsed::Help),
                Some("--") => {
                    options_done = true;
                    continue;
                }
                Some(option) if option.starts_with('-') => {
                    return Err(format!("unknown option `{option}`"));
                }
                _ => {}
            }
        }
        files.push(PathBuf::from(argument));
    }

    if files.is_empty() {
        files.push(default_path());
    }
    Ok(Parsed::Run(Options {
        json: json_output,
        files,
    }))
}

fn default_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("battle_ai.json")
}

fn build_registry() -> Result<NodeRegistry<()>, Box<dyn Error>> {
    let mut registry = NodeRegistry::<()>::new();

    registry.action::<HealParams>("heal", |_| |_: &mut Context<'_, ()>| Status::Success)?;
    registry.action::<()>("wait", |_| |_: &mut Context<'_, ()>| Status::Success)?;
    registry.condition::<HpBelowParams>("hp_below", |_| |_: &Context<'_, ()>| true)?;
    registry.condition::<()>("enemy_alive", |_| |_: &Context<'_, ()>| true)?;
    registry.custom::<MultiStrikeParams>(
        "multi_strike",
        |_params: MultiStrikeParams, children: Vec<BoxedNode<()>>| {
            Ok(Box::new(Selector::new(children)))
        },
    )?;

    registry.describe_params::<HealParams>(NodeCategory::Action, "heal", "Restore health.")?;
    registry.describe(NodeCategory::Action, "wait", "Wait for the next tick.")?;
    registry.describe_params::<HpBelowParams>(
        NodeCategory::Condition,
        "hp_below",
        "Check the health ratio.",
    )?;
    registry.describe(
        NodeCategory::Condition,
        "enemy_alive",
        "Check whether the enemy survives.",
    )?;
    registry.describe_params::<MultiStrikeParams>(
        NodeCategory::Custom,
        "multi_strike",
        "Strike several times.",
    )?;

    Ok(registry)
}

fn lint_file(registry: &NodeRegistry<()>, schema: &Value, path: &Path) -> LintReport {
    let file = path.display().to_string();
    let mut issues = Vec::new();

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
    match serde_json::from_str::<Value>(&text) {
        Ok(document) => issues.extend(schema_issues(schema, &document)),
        Err(error) => {
            issues.push(LintIssue::new(
                "parse",
                "",
                format!("invalid JSON: {error}"),
            ));
            return LintReport {
                file,
                valid: false,
                issues,
            };
        }
    }

    match TreeSpec::from_json_str(&text) {
        Ok(spec) => {
            if let Err(error) = spec.build(registry) {
                issues.push(LintIssue::new(
                    "build",
                    error.path().unwrap_or(""),
                    error.kind().to_string(),
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
            let enum_paths: std::collections::HashSet<String> = node_errors
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

    fn schema() -> Value {
        build_registry().expect("demo registry").json_schema()
    }

    #[test]
    fn reports_unknown_action_at_node_path() {
        let issues = schema_issues(
            &schema(),
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
        assert_eq!(
            issues
                .iter()
                .filter(|issue| issue.path == "/root/children/0/name")
                .count(),
            1,
            "the enum error replaces its annotation-only oneOf"
        );
    }

    #[test]
    fn reports_wrong_params_at_field_path() {
        let issues = schema_issues(
            &schema(),
            &json!({
                "root": {
                    "type": "action",
                    "name": "heal",
                    "params": {"amount": "lots"}
                }
            }),
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.path == "/root/params/amount"),
            "{issues:?}"
        );
    }

    #[test]
    fn accepts_example_configuration() {
        let text = fs::read_to_string(default_path()).expect("example config");
        let document: Value = serde_json::from_str(&text).expect("valid JSON");
        assert!(schema_issues(&schema(), &document).is_empty());
    }

    #[test]
    fn json_report_contains_issue_fields() {
        let issue = LintIssue::new("build", "root.children[0]", "unknown action `missing`");
        let report = LintReport {
            file: String::from("config.json"),
            valid: false,
            issues: vec![issue],
        };
        let value = serde_json::to_value(report).expect("serialize report");
        assert_eq!(value["valid"], false);
        assert_eq!(value["issues"][0]["source"], "build");
        assert_eq!(value["issues"][0]["path"], "root.children[0]");
    }
}
