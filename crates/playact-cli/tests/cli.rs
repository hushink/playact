//! End-to-end tests for the `playact` binary.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use playact::config::{NodeCategory, NodeRegistry};
use playact::prelude::*;
use serde_json::Value;

struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn new(extension: &str, contents: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "playact-cli-{}-{}.{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            extension
        ));
        fs::write(&path, contents).expect("write temporary configuration");
        Self { path }
    }

    fn as_str(&self) -> &str {
        self.path.to_str().expect("temporary path is UTF-8")
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_playact"))
        .args(arguments)
        .output()
        .expect("run playact")
}

fn registry_schema() -> String {
    let mut registry = NodeRegistry::<()>::new();
    registry
        .action::<()>("patrol", |_| |_: &mut Context<'_, ()>| Status::Success)
        .unwrap();
    registry
        .describe(NodeCategory::Action, "patrol", "Walk the route.")
        .unwrap();
    serde_json::to_string(&registry.json_schema()).expect("serialize schema")
}

#[test]
fn schema_writes_the_base_tree_spec_schema() {
    let output = run(&["schema"]);
    assert!(output.status.success(), "{output:?}");
    let schema: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert!(
        schema
            .pointer("/$defs/SpecNode/oneOf")
            .and_then(Value::as_array)
            .is_some_and(|variants| !variants.is_empty())
    );
}

#[test]
fn catalog_lists_built_in_node_types() {
    let output = run(&["catalog"]);
    assert!(output.status.success(), "{output:?}");
    let catalog = String::from_utf8_lossy(&output.stdout);
    assert!(catalog.contains("## Node types"));
    assert!(catalog.contains("`sequence`"));
    assert!(catalog.contains("`utility_selector`"));
}

#[test]
fn catalog_accepts_a_registry_schema() {
    let schema = TempFile::new("json", &registry_schema());
    let output = run(&["catalog", "--schema", schema.as_str()]);
    assert!(output.status.success(), "{output:?}");
    let catalog = String::from_utf8_lossy(&output.stdout);
    assert!(catalog.contains("## Registered names"));
    assert!(catalog.contains("`action.name`"));
    assert!(catalog.contains("`patrol`"));
    assert!(catalog.contains("Walk the route."));
}

#[test]
fn lint_accepts_all_supported_formats() {
    let json = TempFile::new(
        "json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let toml = TempFile::new(
        "toml",
        "version = 1\n[root]\ntype = \"action\"\nname = \"anything\"\n",
    );
    let yaml = TempFile::new(
        "yaml",
        "version: 1\nroot:\n  type: action\n  name: anything\n",
    );
    let ron = TempFile::new(
        "ron",
        "(version: 1, root: (type: \"action\", name: \"anything\"))",
    );

    let output = run(&[
        "lint",
        json.as_str(),
        toml.as_str(),
        yaml.as_str(),
        ron.as_str(),
    ]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.matches(": valid").count(), 4, "{stdout}");
}

#[test]
fn lint_reports_nested_schema_errors() {
    let bad = TempFile::new(
        "json",
        r#"{"root":{"type":"sequence","children":[{"type":"no_such_node"}]}}"#,
    );
    let output = run(&["lint", bad.as_str()]);
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("/root/children/0/type"), "{stderr}");
    assert!(stderr.contains("no_such_node"), "{stderr}");
}

#[test]
fn lint_json_report_is_machine_readable() {
    let valid = TempFile::new(
        "json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let output = run(&["lint", "--json", valid.as_str()]);
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).expect("valid JSON report");
    assert_eq!(report[0]["valid"], true);
    assert_eq!(report[0]["issues"].as_array().map(Vec::len), Some(0));
}

#[test]
fn convert_writes_a_valid_configuration() {
    let input = TempFile::new(
        "json",
        r#"{"version":1,"name":"converted","root":{"type":"action","name":"anything"}}"#,
    );
    let output = TempFile::new("toml", "");

    let converted = run(&["convert", input.as_str(), output.as_str()]);
    assert!(converted.status.success(), "{converted:?}");
    let text = fs::read_to_string(&output.path).expect("converted output");
    assert!(text.contains("type = \"action\""), "{text}");

    let linted = run(&["lint", output.as_str()]);
    assert!(linted.status.success(), "{linted:?}");
}

#[test]
fn convert_rejects_invalid_input_without_writing() {
    let input = TempFile::new("json", r#"{"root":{"type":"no_such_node"}}"#);
    let output = TempFile::new("yaml", "");

    let converted = run(&["convert", input.as_str(), output.as_str()]);
    assert!(!converted.status.success(), "{converted:?}");
    let text = fs::read_to_string(&output.path).expect("output file");
    assert!(text.is_empty(), "invalid input must not be written: {text}");
}

#[test]
fn watch_cannot_be_combined_with_json_reports() {
    let valid = TempFile::new(
        "json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let output = run(&["lint", "--json", "--watch", valid.as_str()]);
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--watch cannot be combined with --json"),
        "{stderr}"
    );
}

#[test]
fn lint_uses_registry_names_from_the_schema() {
    let schema = TempFile::new("json", &registry_schema());
    let bad = TempFile::new("json", r#"{"root":{"type":"action","name":"missing"}}"#);
    let output = run(&["lint", "--schema", schema.as_str(), bad.as_str()]);
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("/root/name"), "{stderr}");
    assert!(stderr.contains("missing"), "{stderr}");
}
