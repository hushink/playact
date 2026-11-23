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

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "playact-cli-dir-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create temporary directory");
        Self { path }
    }

    fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
        }
        fs::write(&path, contents).expect("write temporary file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
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
fn lint_resolves_includes_and_reports_child_errors() {
    let directory = TempDir::new();
    directory.write("bad.json", r#"{"root":{"type":"no_such_node"}}"#);
    let root = directory.write(
        "root.json",
        r#"{"root":{"type":"include","path":"bad.json"}}"#,
    );

    let output = run(&["lint", root.to_str().expect("root path")]);
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("bad.json"), "{stderr}");
    assert!(stderr.contains("/root/type"), "{stderr}");
    assert!(stderr.contains("no_such_node"), "{stderr}");
}

#[test]
fn lint_accepts_valid_includes() {
    let directory = TempDir::new();
    directory.write(
        "child.json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let root = directory.write(
        "root.json",
        r#"{"root":{"type":"include","path":"child.json"}}"#,
    );

    let output = run(&["lint", root.to_str().expect("root path")]);
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn lint_defaults_the_include_base_to_a_bare_file_parent() {
    let directory = TempDir::new();
    directory.write(
        "child.json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    directory.write(
        "root.json",
        r#"{"root":{"type":"include","path":"child.json"}}"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_playact"))
        .current_dir(&directory.path)
        .args(["lint", "root.json"])
        .output()
        .expect("run playact");
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn lint_detects_include_cycles() {
    let directory = TempDir::new();
    directory.write("a.json", r#"{"root":{"type":"include","path":"b.json"}}"#);
    directory.write("b.json", r#"{"root":{"type":"include","path":"a.json"}}"#);
    let root = directory.path.join("a.json");

    let output = run(&["lint", root.to_str().expect("root path")]);
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cycle"), "{stderr}");
}

#[test]
fn lint_rejects_include_path_escapes() {
    let directory = TempDir::new();
    directory.write(
        "outside.json",
        r#"{"root":{"type":"action","name":"anything"}}"#,
    );
    let root = directory.write(
        "config/root.json",
        r#"{"root":{"type":"include","path":"../outside.json"}}"#,
    );

    let output = run(&["lint", root.to_str().expect("root path")]);
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("escapes"), "{stderr}");
}

#[test]
fn lint_accepts_an_explicit_include_base() {
    let directory = TempDir::new();
    directory.write(
        "shared.json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let root = directory.write(
        "config/root.json",
        r#"{"root":{"type":"include","path":"shared.json"}}"#,
    );

    let output = run(&[
        "lint",
        "--include-base",
        directory.path.to_str().expect("directory path"),
        root.to_str().expect("root path"),
    ]);
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn convert_validates_includes_before_writing() {
    let directory = TempDir::new();
    directory.write("bad.json", r#"{"root":{"type":"no_such_node"}}"#);
    let root = directory.write(
        "root.json",
        r#"{"root":{"type":"include","path":"bad.json"}}"#,
    );
    let output = TempFile::new("toml", "");

    let converted = run(&[
        "convert",
        root.to_str().expect("root path"),
        output.as_str(),
    ]);
    assert!(!converted.status.success(), "{converted:?}");
    let text = fs::read_to_string(&output.path).expect("output file");
    assert!(text.is_empty(), "{text}");
}

#[test]
fn bundle_copies_root_and_includes() {
    let source = TempDir::new();
    let destination = TempDir::new().path.join("bundle");
    source.write(
        "child.toml",
        "version = 1\n[root]\ntype = \"action\"\nname = \"anything\"\n",
    );
    let root = source.write(
        "root.json",
        r#"{"root":{"type":"include","path":"child.toml"}}"#,
    );

    let output = run(&[
        "bundle",
        root.to_str().expect("root path"),
        destination.to_str().expect("destination path"),
    ]);
    assert!(output.status.success(), "{output:?}");
    assert!(destination.join("root.json").is_file());
    assert!(destination.join("child.toml").is_file());

    let bundled = destination.join("root.json");
    let linted = run(&["lint", bundled.to_str().expect("bundled root")]);
    assert!(linted.status.success(), "{linted:?}");
}

#[test]
fn bundle_preserves_parent_segments_in_include_paths() {
    let source = TempDir::new();
    let destination = TempDir::new().path.join("bundle");
    source.write(
        "child.json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let root = source.write(
        "config/root.json",
        r#"{"root":{"type":"include","path":"config/../child.json"}}"#,
    );

    let output = run(&[
        "bundle",
        "--include-base",
        source.path.to_str().expect("source path"),
        root.to_str().expect("root path"),
        destination.to_str().expect("destination path"),
    ]);
    assert!(output.status.success(), "{output:?}");
    assert!(destination.join("config").is_dir());
    assert!(destination.join("child.json").is_file());

    let bundled = destination.join("root.json");
    let linted = run(&["lint", bundled.to_str().expect("bundled root")]);
    assert!(linted.status.success(), "{linted:?}");
}

#[test]
fn bundle_rejects_invalid_includes_without_creating_output() {
    let source = TempDir::new();
    let destination = TempDir::new().path.join("bundle");
    source.write("bad.json", r#"{"root":{"type":"no_such_node"}}"#);
    let root = source.write(
        "root.json",
        r#"{"root":{"type":"include","path":"bad.json"}}"#,
    );

    let output = run(&[
        "bundle",
        root.to_str().expect("root path"),
        destination.to_str().expect("destination path"),
    ]);
    assert!(!output.status.success(), "{output:?}");
    assert!(!destination.exists());
}

#[test]
fn bundle_requires_an_empty_output_directory() {
    let source = TempDir::new();
    let destination = TempDir::new();
    let existing = destination.write("existing.txt", "keep me");
    let root = source.write(
        "root.json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );

    let output = run(&[
        "bundle",
        root.to_str().expect("root path"),
        destination.path.to_str().expect("destination path"),
    ]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(existing).expect("existing file"),
        "keep me"
    );
}

#[test]
fn bundle_rejects_output_inside_the_include_base() {
    let source = TempDir::new();
    let root = source.write(
        "root.json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let destination = source.path.join("bundle");

    let output = run(&[
        "bundle",
        root.to_str().expect("root path"),
        destination.to_str().expect("destination path"),
    ]);
    assert!(!output.status.success(), "{output:?}");
    assert!(!destination.exists());
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
