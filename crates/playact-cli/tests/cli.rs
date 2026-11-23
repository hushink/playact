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

fn rename_manifest(directory: &TempDir) -> PathBuf {
    directory.write(
        "migrations.json",
        r#"{"version":1,"migrations":[{"from":0,"operations":[{"op":"rename_field","from":"tree","to":"root"}]}]}"#,
    )
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
fn convert_recursive_rewrites_include_paths_and_converts_nested_formats() {
    let directory = TempDir::new();
    directory.write(
        "root.json",
        r#"{"version":1,"root":{"type":"sequence","children":[{"type":"include","path":"shared/child.toml"}]}}"#,
    );
    directory.write(
        "shared/child.toml",
        "version = 1\n[root]\ntype = \"include\"\npath = \"shared/grand.yaml\"\n",
    );
    directory.write(
        "shared/grand.yaml",
        "version: 1\nroot:\n  type: action\n  name: anything\n",
    );
    let output = directory.path.join("out");

    let converted = run(&[
        "convert",
        "--recursive",
        "--format",
        "yaml",
        directory
            .path
            .join("root.json")
            .to_str()
            .expect("root path"),
        output.to_str().expect("output path"),
    ]);
    assert!(converted.status.success(), "{converted:?}");
    let stdout = String::from_utf8_lossy(&converted.stdout);
    assert!(stdout.contains("converted 3 file(s)"), "{stdout}");

    let root = fs::read_to_string(output.join("root.yaml")).expect("converted root");
    assert!(root.contains("shared/child.yaml"), "{root}");
    let child = fs::read_to_string(output.join("shared/child.yaml")).expect("converted child");
    assert!(child.contains("shared/grand.yaml"), "{child}");
    assert!(output.join("shared/grand.yaml").is_file());

    let linted = run(&[
        "lint",
        output
            .join("root.yaml")
            .to_str()
            .expect("converted root path"),
    ]);
    assert!(linted.status.success(), "{linted:?}");
}

#[test]
fn convert_recursive_supports_ron_targets() {
    let directory = TempDir::new();
    directory.write(
        "child.json",
        r#"{"root":{"type":"action","name":"anything"}}"#,
    );
    let root = directory.write(
        "root.ron",
        r#"(version: 1, root: (type: "include", path: "child.json"))"#,
    );
    let output = directory.path.join("out");

    let converted = run(&[
        "convert",
        "--recursive",
        "--format",
        "ron",
        root.to_str().expect("root path"),
        output.to_str().expect("output path"),
    ]);
    assert!(converted.status.success(), "{converted:?}");
    let text = fs::read_to_string(output.join("root.ron")).expect("converted root");
    assert!(text.contains("child.ron"), "{text}");

    let linted = run(&[
        "lint",
        output
            .join("root.ron")
            .to_str()
            .expect("converted root path"),
    ]);
    assert!(linted.status.success(), "{linted:?}");
}

#[test]
fn convert_recursive_requires_an_explicit_format() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"root":{"type":"action","name":"anything"}}"#,
    );
    let output = directory.path.join("out");

    let converted = run(&[
        "convert",
        "--recursive",
        root.to_str().expect("root path"),
        output.to_str().expect("output path"),
    ]);
    assert!(!converted.status.success(), "{converted:?}");
    let stderr = String::from_utf8_lossy(&converted.stderr);
    assert!(stderr.contains("--format"), "{stderr}");
    assert!(!output.exists());
}

#[test]
fn convert_recursive_requires_an_empty_output_directory() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"root":{"type":"action","name":"anything"}}"#,
    );
    let output = directory.path.join("out");
    fs::create_dir(&output).expect("output directory");
    fs::write(output.join("existing.txt"), "keep").expect("existing output file");

    let converted = run(&[
        "convert",
        "--recursive",
        "--format",
        "yaml",
        root.to_str().expect("root path"),
        output.to_str().expect("output path"),
    ]);
    assert!(!converted.status.success(), "{converted:?}");
    let stderr = String::from_utf8_lossy(&converted.stderr);
    assert!(stderr.contains("is not empty"), "{stderr}");
    assert_eq!(
        fs::read_to_string(output.join("existing.txt")).expect("existing output"),
        "keep"
    );
    assert!(!output.join("root.yaml").exists());
}

#[test]
fn convert_recursive_writes_nothing_when_an_include_is_invalid() {
    let directory = TempDir::new();
    directory.write("bad.json", r#"{"root":{"type":"no_such_node"}}"#);
    let root = directory.write(
        "root.json",
        r#"{"root":{"type":"include","path":"bad.json"}}"#,
    );
    let output = directory.path.join("out");

    let converted = run(&[
        "convert",
        "--recursive",
        "--format",
        "yaml",
        root.to_str().expect("root path"),
        output.to_str().expect("output path"),
    ]);
    assert!(!converted.status.success(), "{converted:?}");
    assert!(!output.exists());
}

#[test]
fn convert_recursive_rejects_destination_collisions() {
    let directory = TempDir::new();
    directory.write("a.json", r#"{"root":{"type":"action","name":"anything"}}"#);
    directory.write(
        "a.yaml",
        "version: 1\nroot:\n  type: action\n  name: anything\n",
    );
    let root = directory.write(
        "root.json",
        r#"{"root":{"type":"sequence","children":[{"type":"include","path":"a.json"},{"type":"include","path":"a.yaml"}]}}"#,
    );
    let output = directory.path.join("out");

    let converted = run(&[
        "convert",
        "--recursive",
        "--format",
        "yaml",
        root.to_str().expect("root path"),
        output.to_str().expect("output path"),
    ]);
    assert!(!converted.status.success(), "{converted:?}");
    let stderr = String::from_utf8_lossy(&converted.stderr);
    assert!(stderr.contains("conversion maps"), "{stderr}");
    assert!(stderr.contains("a.yaml"), "{stderr}");
    assert!(!output.exists());
}

#[test]
fn convert_recursive_rejects_file_and_directory_collisions() {
    let directory = TempDir::new();
    directory.write("a.json", r#"{"root":{"type":"action","name":"anything"}}"#);
    directory.write(
        "a.yaml/b.json",
        r#"{"root":{"type":"action","name":"anything"}}"#,
    );
    let root = directory.write(
        "root.json",
        r#"{"root":{"type":"sequence","children":[{"type":"include","path":"a.json"},{"type":"include","path":"a.yaml/b.json"}]}}"#,
    );
    let output = directory.path.join("out");

    let converted = run(&[
        "convert",
        "--recursive",
        "--format",
        "yaml",
        root.to_str().expect("root path"),
        output.to_str().expect("output path"),
    ]);
    assert!(!converted.status.success(), "{converted:?}");
    let stderr = String::from_utf8_lossy(&converted.stderr);
    assert!(stderr.contains("both a file and a directory"), "{stderr}");
    assert!(!output.exists());
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn convert_recursive_rejects_case_insensitive_destination_collisions() {
    let directory = TempDir::new();
    directory.write("A.json", r#"{"root":{"type":"action","name":"anything"}}"#);
    directory.write(
        "a.yml",
        "version: 1\nroot:\n  type: action\n  name: anything\n",
    );
    let root = directory.write(
        "root.json",
        r#"{"root":{"type":"sequence","children":[{"type":"include","path":"A.json"},{"type":"include","path":"a.yml"}]}}"#,
    );
    let output = directory.path.join("out");

    let converted = run(&[
        "convert",
        "--recursive",
        "--format",
        "yaml",
        root.to_str().expect("root path"),
        output.to_str().expect("output path"),
    ]);
    assert!(!converted.status.success(), "{converted:?}");
    let stderr = String::from_utf8_lossy(&converted.stderr);
    assert!(stderr.contains("conversion maps"), "{stderr}");
    assert!(!output.exists());
}

#[test]
fn convert_rejects_format_without_recursive() {
    let input = TempFile::new("json", r#"{"root":{"type":"action","name":"anything"}}"#);
    let output = TempFile::new("toml", "");

    let converted = run(&[
        "convert",
        "--format",
        "yaml",
        input.as_str(),
        output.as_str(),
    ]);
    assert!(!converted.status.success(), "{converted:?}");
    let stderr = String::from_utf8_lossy(&converted.stderr);
    assert!(
        stderr.contains("--format is only valid with --recursive"),
        "{stderr}"
    );
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
fn graph_renders_mermaid_nodes_and_includes() {
    let directory = TempDir::new();
    directory.write(
        "child.json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let root = directory.write(
        "root.json",
        r#"{"root":{"type":"sequence","children":[{"type":"include","path":"child.json"}]}}"#,
    );

    let output = run(&["graph", root.to_str().expect("root path")]);
    assert!(output.status.success(), "{output:?}");
    let graph = String::from_utf8_lossy(&output.stdout);
    assert!(graph.contains("flowchart LR"), "{graph}");
    assert!(graph.contains("include: child.json"), "{graph}");
    assert!(graph.contains("action: anything"), "{graph}");
    assert!(graph.contains("-.->|include|"), "{graph}");
}

#[test]
fn graph_renders_dot() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );

    let output = run(&[
        "graph",
        "--format",
        "dot",
        root.to_str().expect("root path"),
    ]);
    assert!(output.status.success(), "{output:?}");
    let graph = String::from_utf8_lossy(&output.stdout);
    assert!(graph.contains("digraph config"), "{graph}");
    assert!(graph.contains("label=\"action: anything\""), "{graph}");
}

#[test]
fn graph_marks_missing_includes() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"root":{"type":"include","path":"missing.json"}}"#,
    );

    let output = run(&["graph", root.to_str().expect("root path")]);
    assert!(output.status.success(), "{output:?}");
    let graph = String::from_utf8_lossy(&output.stdout);
    assert!(graph.contains("missing"), "{graph}");
    assert!(graph.contains(":::error"), "{graph}");
}

#[test]
fn graph_marks_include_cycles() {
    let directory = TempDir::new();
    directory.write("a.json", r#"{"root":{"type":"include","path":"b.json"}}"#);
    directory.write("b.json", r#"{"root":{"type":"include","path":"a.json"}}"#);
    let root = directory.path.join("a.json");

    let output = run(&["graph", root.to_str().expect("root path")]);
    assert!(output.status.success(), "{output:?}");
    let graph = String::from_utf8_lossy(&output.stdout);
    assert!(graph.contains("include cycle"), "{graph}");
}

#[test]
fn graph_rejects_unknown_formats() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );

    let output = run(&[
        "graph",
        "--format",
        "svg",
        root.to_str().expect("root path"),
    ]);
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unsupported graph format"), "{stderr}");
}

#[test]
fn fmt_rewrites_configurations_and_check_passes_afterwards() {
    let config = TempFile::new(
        "json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );

    let formatted = run(&["fmt", config.as_str()]);
    assert!(formatted.status.success(), "{formatted:?}");
    let text = fs::read_to_string(&config.path).expect("formatted config");
    assert!(text.contains("\n  \"version\": 1"), "{text}");
    assert!(text.ends_with('\n'), "{text}");

    let checked = run(&["fmt", "--check", config.as_str()]);
    assert!(checked.status.success(), "{checked:?}");
}

#[test]
fn fmt_check_reports_unformatted_files_without_writing() {
    let config = TempFile::new(
        "json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let original = fs::read_to_string(&config.path).expect("original");

    let checked = run(&["fmt", "--check", config.as_str()]);
    assert!(!checked.status.success(), "{checked:?}");
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert!(stderr.contains("would reformat"), "{stderr}");
    assert_eq!(
        fs::read_to_string(&config.path).expect("unchanged"),
        original
    );
}

#[test]
fn fmt_validates_includes_before_writing() {
    let directory = TempDir::new();
    directory.write("bad.json", r#"{"root":{"type":"no_such_node"}}"#);
    let root = directory.write(
        "root.json",
        r#"{"root":{"type":"include","path":"bad.json"}}"#,
    );
    let original = fs::read_to_string(&root).expect("original root");

    let formatted = run(&["fmt", root.to_str().expect("root path")]);
    assert!(!formatted.status.success(), "{formatted:?}");
    assert_eq!(fs::read_to_string(&root).expect("unchanged root"), original);
}

#[test]
fn fmt_recursive_formats_included_files() {
    let directory = TempDir::new();
    let include = directory.write(
        "shared/child.json",
        r#"{"version":1,"root":{"type":"action","name":"child"}}"#,
    );
    let root = directory.write(
        "root.json",
        r#"{"version":1,"root":{"type":"include","path":"shared/child.json"}}"#,
    );

    let formatted = run(&["fmt", "--recursive", root.to_str().expect("root path")]);
    assert!(formatted.status.success(), "{formatted:?}");
    for path in [&root, &include] {
        let text = fs::read_to_string(path).expect("formatted file");
        assert!(text.contains("\n  \"version\": 1"), "{text}");
        assert!(text.ends_with('\n'), "{text}");
    }

    let checked = run(&[
        "fmt",
        "--check",
        "--recursive",
        root.to_str().expect("root path"),
    ]);
    assert!(checked.status.success(), "{checked:?}");
}

#[test]
fn fmt_recursive_check_writes_nothing() {
    let directory = TempDir::new();
    let include = directory.write(
        "child.json",
        r#"{"version":1,"root":{"type":"action","name":"child"}}"#,
    );
    let root = directory.write(
        "root.json",
        r#"{"version":1,"root":{"type":"include","path":"child.json"}}"#,
    );
    let original_root = fs::read_to_string(&root).expect("original root");
    let original_include = fs::read_to_string(&include).expect("original include");

    let checked = run(&[
        "fmt",
        "--check",
        "--recursive",
        root.to_str().expect("root path"),
    ]);
    assert!(!checked.status.success(), "{checked:?}");
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert!(stderr.contains("would reformat"), "{stderr}");
    assert!(
        stderr.contains("2 file(s) would be reformatted"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&root).expect("root"), original_root);
    assert_eq!(
        fs::read_to_string(&include).expect("include"),
        original_include
    );
}

#[test]
fn fmt_recursive_aborts_before_writing_when_an_include_is_invalid() {
    let directory = TempDir::new();
    let include = directory.write("bad.json", r#"{"root":{"type":"no_such_node"}}"#);
    let root = directory.write(
        "root.json",
        r#"{"version":1,"root":{"type":"include","path":"bad.json"}}"#,
    );
    let original_root = fs::read_to_string(&root).expect("original root");
    let original_include = fs::read_to_string(&include).expect("original include");

    let formatted = run(&["fmt", "-r", root.to_str().expect("root path")]);
    assert!(!formatted.status.success(), "{formatted:?}");
    assert_eq!(fs::read_to_string(&root).expect("root"), original_root);
    assert_eq!(
        fs::read_to_string(&include).expect("include"),
        original_include
    );
}

#[test]
fn fmt_recursive_deduplicates_shared_includes() {
    let directory = TempDir::new();
    let shared = directory.write(
        "shared.json",
        r#"{"version":1,"root":{"type":"action","name":"shared"}}"#,
    );
    let first = directory.write(
        "first.json",
        r#"{"version":1,"root":{"type":"include","path":"shared.json"}}"#,
    );
    let second = directory.write(
        "second.json",
        r#"{"version":1,"root":{"type":"include","path":"shared.json"}}"#,
    );

    let formatted = run(&[
        "fmt",
        "--recursive",
        first.to_str().expect("first path"),
        second.to_str().expect("second path"),
    ]);
    assert!(formatted.status.success(), "{formatted:?}");
    let stdout = String::from_utf8_lossy(&formatted.stdout);
    assert!(stdout.contains("formatted 3 file(s)"), "{stdout}");
    let text = fs::read_to_string(&shared).expect("shared file");
    assert!(text.contains("\n  \"version\": 1"), "{text}");
}

#[test]
fn fmt_supports_toml_and_ron() {
    let toml = TempFile::new(
        "toml",
        "version=1\n[root]\ntype=\"action\"\nname=\"anything\"\n",
    );
    let ron = TempFile::new(
        "ron",
        "(version:1,root:(type:\"action\",name:\"anything\"))",
    );

    let formatted = run(&["fmt", toml.as_str(), ron.as_str()]);
    assert!(formatted.status.success(), "{formatted:?}");
    let linted = run(&["lint", toml.as_str(), ron.as_str()]);
    assert!(linted.status.success(), "{linted:?}");
}

#[test]
fn check_accepts_valid_formatted_configurations() {
    let config = TempFile::new(
        "json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let formatted = run(&["fmt", config.as_str()]);
    assert!(formatted.status.success(), "{formatted:?}");
    let original = fs::read_to_string(&config.path).expect("formatted config");

    let checked = run(&["check", config.as_str()]);
    assert!(checked.status.success(), "{checked:?}");
    let stdout = String::from_utf8_lossy(&checked.stdout);
    assert!(stdout.contains("valid and formatted"), "{stdout}");
    assert_eq!(
        fs::read_to_string(&config.path).expect("unchanged"),
        original
    );
}

#[test]
fn check_reports_unformatted_configurations_without_writing() {
    let config = TempFile::new(
        "json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let original = fs::read_to_string(&config.path).expect("original");

    let checked = run(&["check", config.as_str()]);
    assert!(!checked.status.success(), "{checked:?}");
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert!(stderr.contains("would reformat"), "{stderr}");
    assert!(
        stderr.contains("1 file(s) would be reformatted"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&config.path).expect("unchanged"),
        original
    );
}

#[test]
fn check_reports_lint_errors_without_formatting_invalid_files() {
    let config = TempFile::new("json", r#"{"root":{"type":"no_such_node"}}"#);
    let original = fs::read_to_string(&config.path).expect("original");

    let checked = run(&["check", config.as_str()]);
    assert!(!checked.status.success(), "{checked:?}");
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert!(stderr.contains("no_such_node"), "{stderr}");
    assert!(!stderr.contains("would reformat"), "{stderr}");
    assert_eq!(
        fs::read_to_string(&config.path).expect("unchanged"),
        original
    );
}

#[test]
fn check_still_reports_root_formatting_when_an_include_is_invalid() {
    let directory = TempDir::new();
    directory.write("bad.json", r#"{"root":{"type":"no_such_node"}}"#);
    let root = directory.write(
        "root.json",
        r#"{"version":1,"root":{"type":"include","path":"bad.json"}}"#,
    );
    let original_root = fs::read_to_string(&root).expect("original root");
    let original_bad = fs::read_to_string(directory.path.join("bad.json")).expect("original bad");

    let checked = run(&["check", root.to_str().expect("root path")]);
    assert!(!checked.status.success(), "{checked:?}");
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert!(stderr.contains("no_such_node"), "{stderr}");
    assert!(stderr.contains("would reformat"), "{stderr}");
    assert_eq!(
        fs::read_to_string(&root).expect("unchanged root"),
        original_root
    );
    assert_eq!(
        fs::read_to_string(directory.path.join("bad.json")).expect("unchanged bad"),
        original_bad
    );
}

#[test]
fn check_checks_formatting_of_included_files() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"version":1,"root":{"type":"include","path":"child.json"}}"#,
    );
    let child = directory.write(
        "child.json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let formatted = run(&["fmt", root.to_str().expect("root path")]);
    assert!(formatted.status.success(), "{formatted:?}");
    let original_child = fs::read_to_string(&child).expect("original child");

    let checked = run(&["check", root.to_str().expect("root path")]);
    assert!(!checked.status.success(), "{checked:?}");
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert!(stderr.contains("would reformat"), "{stderr}");
    assert!(stderr.contains("child.json"), "{stderr}");
    assert!(
        stderr.contains("1 file(s) would be reformatted"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&child).expect("unchanged child"),
        original_child
    );
}

#[test]
fn check_json_reports_lint_and_formatting_status() {
    let invalid = TempFile::new("json", r#"{"root":{"type":"no_such_node"}}"#);
    let unformatted = TempFile::new(
        "json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let original = fs::read_to_string(&unformatted.path).expect("original");

    let checked = run(&["check", "--json", invalid.as_str(), unformatted.as_str()]);
    assert!(!checked.status.success(), "{checked:?}");
    let report: Value = serde_json::from_slice(&checked.stdout).expect("valid JSON report");
    assert_eq!(report["valid"], false);
    assert_eq!(report["roots"].as_array().map(Vec::len), Some(2));
    assert_eq!(report["roots"][0]["valid"], false);
    assert_eq!(report["roots"][1]["valid"], true);
    assert!(
        report["formatting"]
            .as_array()
            .expect("formatting findings")
            .iter()
            .any(|finding| finding["would_reformat"] == true)
    );
    assert_eq!(
        fs::read_to_string(&unformatted.path).expect("unchanged"),
        original
    );
}

#[test]
fn check_deduplicates_shared_included_files() {
    let directory = TempDir::new();
    let child = directory.write(
        "child.json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let first = directory.write(
        "first.json",
        r#"{"version":1,"root":{"type":"include","path":"child.json"}}"#,
    );
    let second = directory.write(
        "second.json",
        r#"{"version":1,"root":{"type":"include","path":"child.json"}}"#,
    );
    for root in [&first, &second] {
        let formatted = run(&["fmt", root.to_str().expect("root path")]);
        assert!(formatted.status.success(), "{formatted:?}");
    }
    let original_child = fs::read_to_string(&child).expect("original child");

    let checked = run(&[
        "check",
        first.to_str().expect("first path"),
        second.to_str().expect("second path"),
    ]);
    assert!(!checked.status.success(), "{checked:?}");
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert_eq!(stderr.matches("would reformat").count(), 1, "{stderr}");
    assert!(
        stderr.contains("1 file(s) would be reformatted"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&child).expect("unchanged child"),
        original_child
    );
}

#[test]
fn check_reports_includes_that_cannot_be_formatted() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"version":1,"root":{"type":"include","path":"child.txt"}}"#,
    );
    let child = directory.write(
        "child.txt",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let formatted = run(&["fmt", root.to_str().expect("root path")]);
    assert!(formatted.status.success(), "{formatted:?}");
    let original_child = fs::read_to_string(&child).expect("original child");

    let checked = run(&["check", root.to_str().expect("root path")]);
    assert!(!checked.status.success(), "{checked:?}");
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert!(stderr.contains("unsupported extension"), "{stderr}");
    assert!(stderr.contains("child.txt"), "{stderr}");
    assert_eq!(
        fs::read_to_string(&child).expect("unchanged child"),
        original_child
    );
}

#[test]
fn migrate_applies_manifest_to_root_and_includes() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"version":0,"tree":{"type":"sequence","children":[{"type":"include","path":"shared.json"}]}}"#,
    );
    let shared = directory.write(
        "shared.json",
        r#"{"version":0,"tree":{"type":"action","name":"anything"}}"#,
    );
    let manifest = rename_manifest(&directory);

    let migrated = run(&[
        "migrate",
        "--manifest",
        manifest.to_str().expect("manifest path"),
        root.to_str().expect("root path"),
    ]);
    assert!(migrated.status.success(), "{migrated:?}");
    let stdout = String::from_utf8_lossy(&migrated.stdout);
    assert!(stdout.contains("migrated 2 file(s)"), "{stdout}");

    for path in [&root, &shared] {
        let text = fs::read_to_string(path).expect("migrated file");
        let value: Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(value["version"], 1, "{text}");
        assert!(value.get("root").is_some(), "{text}");
        assert!(value.get("tree").is_none(), "{text}");
    }

    let linted = run(&["lint", root.to_str().expect("root path")]);
    assert!(linted.status.success(), "{linted:?}");
}

#[test]
fn migrate_check_reports_without_writing() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"version":0,"tree":{"type":"action","name":"anything"}}"#,
    );
    let manifest = rename_manifest(&directory);
    let original = fs::read_to_string(&root).expect("original");

    let checked = run(&[
        "migrate",
        "--check",
        "--manifest",
        manifest.to_str().expect("manifest path"),
        root.to_str().expect("root path"),
    ]);
    assert!(!checked.status.success(), "{checked:?}");
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert!(stderr.contains("would migrate"), "{stderr}");
    assert!(stderr.contains("1 file(s) would be migrated"), "{stderr}");
    assert_eq!(fs::read_to_string(&root).expect("unchanged"), original);
}

#[test]
fn migrate_leaves_current_documents_byte_identical() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let manifest = rename_manifest(&directory);
    let original = fs::read_to_string(&root).expect("original");

    let migrated = run(&[
        "migrate",
        "--manifest",
        manifest.to_str().expect("manifest path"),
        root.to_str().expect("root path"),
    ]);
    assert!(migrated.status.success(), "{migrated:?}");
    let stdout = String::from_utf8_lossy(&migrated.stdout);
    assert!(stdout.contains("nothing to migrate"), "{stdout}");
    assert_eq!(fs::read_to_string(&root).expect("unchanged"), original);
}

#[test]
fn migrate_writes_nothing_when_validation_fails() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"version":0,"tree":{"type":"sequence","children":[{"type":"include","path":"shared.json"}]}}"#,
    );
    let shared = directory.write(
        "shared.json",
        r#"{"version":0,"tree":{"type":"action","name":"anything"}}"#,
    );
    let manifest = directory.write(
        "break.json",
        r#"{"version":1,"migrations":[{"from":0,"operations":[
            {"op":"rename_field","from":"tree","to":"root"},
            {"op":"set_field","where":{"type":"sequence"},"field":"type","value":"no_such_node","overwrite":true}
        ]}]}"#,
    );
    let original_root = fs::read_to_string(&root).expect("original root");
    let original_shared = fs::read_to_string(&shared).expect("original shared");

    let migrated = run(&[
        "migrate",
        "--manifest",
        manifest.to_str().expect("manifest path"),
        root.to_str().expect("root path"),
    ]);
    assert!(!migrated.status.success(), "{migrated:?}");
    let stderr = String::from_utf8_lossy(&migrated.stderr);
    assert!(stderr.contains("no_such_node"), "{stderr}");
    assert!(stderr.contains("no files were written"), "{stderr}");
    assert_eq!(fs::read_to_string(&root).expect("root"), original_root);
    assert_eq!(
        fs::read_to_string(&shared).expect("shared"),
        original_shared
    );
}

#[test]
fn migrate_reports_missing_includes_without_writing() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"version":0,"tree":{"type":"include","path":"missing.json"}}"#,
    );
    let manifest = rename_manifest(&directory);
    let original = fs::read_to_string(&root).expect("original");

    let migrated = run(&[
        "migrate",
        "--manifest",
        manifest.to_str().expect("manifest path"),
        root.to_str().expect("root path"),
    ]);
    assert!(!migrated.status.success(), "{migrated:?}");
    let stderr = String::from_utf8_lossy(&migrated.stderr);
    assert!(stderr.contains("missing.json"), "{stderr}");
    assert_eq!(fs::read_to_string(&root).expect("unchanged"), original);
}

#[test]
fn migrate_json_reports_migrated_files() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"version":0,"tree":{"type":"sequence","children":[{"type":"include","path":"shared.json"}]}}"#,
    );
    directory.write(
        "shared.json",
        r#"{"version":0,"tree":{"type":"action","name":"anything"}}"#,
    );
    let manifest = rename_manifest(&directory);

    let migrated = run(&[
        "migrate",
        "--json",
        "--manifest",
        manifest.to_str().expect("manifest path"),
        root.to_str().expect("root path"),
    ]);
    assert!(migrated.status.success(), "{migrated:?}");
    let report: Value = serde_json::from_slice(&migrated.stdout).expect("valid JSON report");
    assert_eq!(report[0]["valid"], true);
    assert_eq!(
        report[0]["migrated"].as_array().map(Vec::len),
        Some(2),
        "{report}"
    );
}

#[test]
fn migrate_requires_a_manifest_and_valid_operations() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"version":0,"tree":{"type":"action","name":"anything"}}"#,
    );

    let missing = run(&["migrate", root.to_str().expect("root path")]);
    assert!(!missing.status.success(), "{missing:?}");
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(stderr.contains("--manifest"), "{stderr}");

    let manifest = directory.write(
        "invalid.json",
        r#"{"version":1,"migrations":[{"from":0,"operations":[{"op":"rename_field","from":"x","to":"x"}]}]}"#,
    );
    let invalid = run(&[
        "migrate",
        "--manifest",
        manifest.to_str().expect("manifest path"),
        root.to_str().expect("root path"),
    ]);
    assert!(!invalid.status.success(), "{invalid:?}");
    let stderr = String::from_utf8_lossy(&invalid.stderr);
    assert!(stderr.contains("must differ"), "{stderr}");
}

#[test]
fn migrate_detects_include_cycles_without_writing() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"version":0,"tree":{"type":"include","path":"a.json"}}"#,
    );
    let included = directory.write(
        "a.json",
        r#"{"version":0,"tree":{"type":"include","path":"root.json"}}"#,
    );
    let manifest = rename_manifest(&directory);
    let original_root = fs::read_to_string(&root).expect("original root");
    let original_included = fs::read_to_string(&included).expect("original include");

    let migrated = run(&[
        "migrate",
        "--manifest",
        manifest.to_str().expect("manifest path"),
        root.to_str().expect("root path"),
    ]);
    assert!(!migrated.status.success(), "{migrated:?}");
    let stderr = String::from_utf8_lossy(&migrated.stderr);
    assert!(stderr.contains("include cycle"), "{stderr}");
    assert_eq!(fs::read_to_string(&root).expect("root"), original_root);
    assert_eq!(
        fs::read_to_string(&included).expect("include"),
        original_included
    );
}

#[test]
fn migrate_accepts_yaml_manifests() {
    let directory = TempDir::new();
    let root = directory.write(
        "root.json",
        r#"{"version":0,"tree":{"type":"action","name":"anything"}}"#,
    );
    let manifest = directory.write(
        "migrations.yaml",
        "version: 1\nmigrations:\n  - from: 0\n    operations:\n      - op: rename_field\n        from: tree\n        to: root\n",
    );

    let migrated = run(&[
        "migrate",
        "--manifest",
        manifest.to_str().expect("manifest path"),
        root.to_str().expect("root path"),
    ]);
    assert!(migrated.status.success(), "{migrated:?}");
    let value: Value =
        serde_json::from_str(&fs::read_to_string(&root).expect("migrated")).expect("valid JSON");
    assert_eq!(value["version"], 1);
    assert!(value.get("root").is_some());
}

#[test]
fn diff_reports_no_changes_for_identical_configs() {
    let config = TempFile::new(
        "json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );

    let output = run(&["diff", config.as_str(), config.as_str()]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("no changes"), "{stdout}");
}

#[test]
fn diff_reports_node_and_parameter_changes() {
    let old = TempFile::new(
        "json",
        r#"{"version":1,"root":{"type":"sequence","children":[{"type":"action","name":"heal","params":{"amount":10}}]}}"#,
    );
    let new = TempFile::new(
        "json",
        r#"{"version":1,"root":{"type":"sequence","children":[{"type":"action","name":"heal","params":{"amount":20}},{"type":"action","name":"wait"}]}}"#,
    );

    let output = run(&["diff", old.as_str(), new.as_str()]);
    assert!(!output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("root.children[0].params.amount"),
        "{stdout}"
    );
    assert!(stdout.contains("10 -> 20"), "{stdout}");
    assert!(
        stdout.contains("+ root.children[1]: action: wait"),
        "{stdout}"
    );
}

#[test]
fn diff_json_report_is_machine_readable() {
    let old = TempFile::new(
        "json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let new = TempFile::new(
        "json",
        r#"{"version":1,"name":"new","root":{"type":"action","name":"anything"}}"#,
    );

    let output = run(&["diff", "--json", old.as_str(), new.as_str()]);
    assert!(!output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).expect("valid JSON diff");
    assert_eq!(report["changes"][0]["kind"], "added");
    assert_eq!(report["changes"][0]["path"], "name");
    assert_eq!(report["changes"][0]["new"], "\"new\"");
    assert_eq!(report["includes"].as_array().map(Vec::len), Some(0));
}

#[test]
fn diff_reports_changes_inside_shared_includes() {
    let old_dir = TempDir::new();
    let new_dir = TempDir::new();
    old_dir.write(
        "child.json",
        r#"{"version":1,"root":{"type":"action","name":"anything","params":{"x":1}}}"#,
    );
    new_dir.write(
        "child.json",
        r#"{"version":1,"root":{"type":"action","name":"anything","params":{"x":2}}}"#,
    );
    let old_root = old_dir.write(
        "root.json",
        r#"{"root":{"type":"include","path":"child.json"}}"#,
    );
    let new_root = new_dir.write(
        "root.json",
        r#"{"root":{"type":"include","path":"child.json"}}"#,
    );

    let output = run(&[
        "diff",
        old_root.to_str().expect("old root"),
        new_root.to_str().expect("new root"),
    ]);
    assert!(!output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[child.json]"), "{stdout}");
    assert!(stdout.contains("root.params.x"), "{stdout}");
    assert!(stdout.contains("1 -> 2"), "{stdout}");

    let json = run(&[
        "diff",
        "--json",
        old_root.to_str().expect("old root"),
        new_root.to_str().expect("new root"),
    ]);
    let report: Value = serde_json::from_slice(&json.stdout).expect("valid JSON diff");
    assert!(
        report["changes"]
            .as_array()
            .is_some_and(|changes| { changes.iter().any(|change| change["file"] == "child.json") })
    );
}

#[test]
fn diff_reports_include_file_changes() {
    let directory = TempDir::new();
    directory.write(
        "a.json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    directory.write(
        "b.json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let old = directory.write("old.json", r#"{"root":{"type":"include","path":"a.json"}}"#);
    let new = directory.write("new.json", r#"{"root":{"type":"include","path":"b.json"}}"#);

    let output = run(&[
        "diff",
        old.to_str().expect("old path"),
        new.to_str().expect("new path"),
    ]);
    assert!(!output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("- include a.json"), "{stdout}");
    assert!(stdout.contains("+ include b.json"), "{stdout}");
}

#[test]
fn diff_rejects_invalid_configurations() {
    let old = TempFile::new(
        "json",
        r#"{"version":1,"root":{"type":"action","name":"anything"}}"#,
    );
    let new = TempFile::new("json", r#"{"root":{"type":"no_such_node"}}"#);

    let output = run(&["diff", old.as_str(), new.as_str()]);
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no_such_node"), "{stderr}");
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
