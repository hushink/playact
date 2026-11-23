//! Mermaid and DOT graphs for configuration files and includes.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use playact::config::MAX_INCLUDE_DEPTH;
use serde_json::{Map, Value};

use crate::format::Format;
use crate::lint::{display_path, resolve_base, resolve_include};

/// Output graph language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GraphFormat {
    Mermaid,
    Dot,
}

impl GraphFormat {
    pub(crate) fn from_name(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "mermaid" | "mmd" => Ok(Self::Mermaid),
            "dot" | "graphviz" => Ok(Self::Dot),
            other => Err(format!(
                "unsupported graph format `{other}`; expected mermaid or dot"
            )),
        }
    }
}

/// Renders a configuration and its include graph.
pub(crate) fn render(
    input: &Path,
    include_base: Option<&Path>,
    format: GraphFormat,
) -> Result<String, String> {
    let input_format = Format::from_path(input)?;
    let text = fs::read_to_string(input)
        .map_err(|error| format!("read `{}`: {error}", display_path(input)))?;
    let document = input_format
        .parse_document(&text)
        .map_err(|error| format!("parse `{}`: {error}", display_path(input)))?;
    let base = resolve_base(input, include_base)?;
    let root = input
        .canonicalize()
        .map_err(|error| format!("resolve `{}`: {error}", display_path(input)))?;

    let mut builder = GraphBuilder::new(&base);
    builder.add_file(root, graph_path(input), input_format, &document)?;
    Ok(match format {
        GraphFormat::Mermaid => builder.render_mermaid(),
        GraphFormat::Dot => builder.render_dot(),
    })
}

struct FileGraph {
    label: String,
    root: Option<String>,
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}

struct GraphNode {
    id: String,
    label: String,
    error: bool,
}

struct GraphEdge {
    from: String,
    to: String,
    label: Option<String>,
    dashed: bool,
}

struct PendingInclude {
    file_index: usize,
    from: String,
    raw: String,
    fallback: Format,
}

struct GraphBuilder<'a> {
    base: &'a Path,
    files: Vec<FileGraph>,
    file_roots: HashMap<PathBuf, String>,
    include_edges: Vec<GraphEdge>,
    stack: Vec<PathBuf>,
}

impl<'a> GraphBuilder<'a> {
    fn new(base: &'a Path) -> Self {
        Self {
            base,
            files: Vec::new(),
            file_roots: HashMap::new(),
            include_edges: Vec::new(),
            stack: Vec::new(),
        }
    }

    fn add_file(
        &mut self,
        path: PathBuf,
        label: String,
        format: Format,
        document: &Value,
    ) -> Result<(), String> {
        if self.file_roots.contains_key(&path) {
            return Ok(());
        }
        let index = self.files.len();
        let mut pending = Vec::new();
        let (file, root) = {
            let mut builder = FileBuilder::new(index, label, format, &mut pending);
            let root = builder.build(document)?;
            (builder.file, root)
        };
        self.files.push(FileGraph {
            root: Some(root.clone()),
            ..file
        });
        self.file_roots.insert(path.clone(), root);
        self.stack.push(path);

        for include in pending {
            self.process_include(include)?;
        }

        self.stack.pop();
        Ok(())
    }

    fn process_include(&mut self, include: PendingInclude) -> Result<(), String> {
        let resolved = resolve_include(self.base, &include.raw, include.fallback);
        let (target, format) = match resolved {
            Ok(resolved) => resolved,
            Err(error) => {
                self.add_error_edge(
                    include.file_index,
                    &include.from,
                    format!("missing: {error}"),
                );
                return Ok(());
            }
        };
        if let Some(root) = self.file_roots.get(&target).cloned() {
            let label = if self.stack.contains(&target) {
                String::from("include cycle")
            } else {
                String::from("include")
            };
            self.include_edges.push(GraphEdge {
                from: include.from,
                to: root,
                label: Some(label),
                dashed: true,
            });
            return Ok(());
        }
        if self.stack.len() >= MAX_INCLUDE_DEPTH {
            self.add_error_edge(
                include.file_index,
                &include.from,
                format!("nesting exceeds {MAX_INCLUDE_DEPTH}"),
            );
            return Ok(());
        }
        let text = match fs::read_to_string(&target) {
            Ok(text) => text,
            Err(error) => {
                self.add_error_edge(
                    include.file_index,
                    &include.from,
                    format!("read error: {error}"),
                );
                return Ok(());
            }
        };
        let document = match format.parse_document(&text) {
            Ok(document) => document,
            Err(error) => {
                self.add_error_edge(
                    include.file_index,
                    &include.from,
                    format!("parse error: {error}"),
                );
                return Ok(());
            }
        };
        let label = target
            .strip_prefix(self.base)
            .map_or_else(|_| graph_path(&target), graph_path);
        if let Err(error) = self.add_file(target.clone(), label, format, &document) {
            self.add_error_edge(include.file_index, &include.from, error);
            return Ok(());
        }
        if let Some(root) = self.file_roots.get(&target).cloned() {
            self.include_edges.push(GraphEdge {
                from: include.from,
                to: root,
                label: Some(String::from("include")),
                dashed: true,
            });
        }
        Ok(())
    }

    fn add_error_edge(&mut self, file_index: usize, from: &str, label: String) {
        let file = &mut self.files[file_index];
        let id = format!("f{file_index}_e{}", file.nodes.len());
        file.nodes.push(GraphNode {
            id: id.clone(),
            label,
            error: true,
        });
        file.edges.push(GraphEdge {
            from: from.to_string(),
            to: id,
            label: None,
            dashed: true,
        });
    }

    fn render_mermaid(&self) -> String {
        let mut output = String::from("flowchart LR\n");
        for (index, file) in self.files.iter().enumerate() {
            output.push_str(&format!(
                "  subgraph f{index}[\"{}\"]\n    direction TB\n",
                escape_mermaid(&file.label)
            ));
            for node in &file.nodes {
                output.push_str(&format!(
                    "    {}[\"{}\"]",
                    node.id,
                    escape_mermaid(&node.label)
                ));
                if node.error {
                    output.push_str(":::error");
                }
                output.push('\n');
            }
            for edge in &file.edges {
                output.push_str(&mermaid_edge(edge, "    "));
            }
            output.push_str("  end\n");
        }
        for edge in &self.include_edges {
            output.push_str(&mermaid_edge(edge, "  "));
        }
        if self
            .files
            .iter()
            .flat_map(|file| &file.nodes)
            .any(|node| node.error)
        {
            output.push_str("  classDef error fill:#f8d7da,stroke:#b02a37,color:#000\n");
        }
        output
    }

    fn render_dot(&self) -> String {
        let mut output = String::from("digraph config {\n  rankdir=LR;\n");
        for (index, file) in self.files.iter().enumerate() {
            output.push_str(&format!(
                "  subgraph cluster_{index} {{\n    label=\"{}\";\n",
                escape_dot(&file.label)
            ));
            for node in &file.nodes {
                output.push_str(&format!(
                    "    {} [label=\"{}\"",
                    node.id,
                    escape_dot(&node.label)
                ));
                if node.error {
                    output.push_str(", color=\"#b02a37\", fontcolor=\"#b02a37\"");
                }
                output.push_str("];\n");
            }
            for edge in &file.edges {
                output.push_str(&dot_edge(edge, "    "));
            }
            output.push_str("  }\n");
        }
        for edge in &self.include_edges {
            output.push_str(&dot_edge(edge, "  "));
        }
        output.push_str("}\n");
        output
    }
}

struct FileBuilder<'a> {
    index: usize,
    format: Format,
    file: FileGraph,
    pending: &'a mut Vec<PendingInclude>,
    next_id: usize,
}

impl FileBuilder<'_> {
    fn new(
        index: usize,
        label: String,
        format: Format,
        pending: &mut Vec<PendingInclude>,
    ) -> FileBuilder<'_> {
        FileBuilder {
            index,
            format,
            file: FileGraph {
                label,
                root: None,
                nodes: Vec::new(),
                edges: Vec::new(),
            },
            pending,
            next_id: 0,
        }
    }

    fn build(&mut self, document: &Value) -> Result<String, String> {
        let root = document
            .get("root")
            .ok_or_else(|| String::from("configuration has no root node"))?;
        let root_id = self.add_node(root, None);
        self.file.root = Some(root_id.clone());

        if let Some(services) = document.get("services").and_then(Value::as_array)
            && !services.is_empty()
        {
            let services_id = self.new_node(String::from("services (run first)"), false);
            for service in services {
                self.add_node(service, Some(&services_id));
            }
        }
        Ok(root_id)
    }

    fn add_node(&mut self, value: &Value, parent: Option<&str>) -> String {
        let Some(object) = value.as_object() else {
            let id = self.new_node(String::from("invalid node"), true);
            if let Some(parent) = parent {
                self.connect(parent.to_string(), id.clone(), true, None);
            }
            return id;
        };

        let label = node_label(object);
        let id = self.new_node(label, false);
        if let Some(parent) = parent {
            self.connect(parent.to_string(), id.clone(), false, None);
        }

        let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "include" {
            if let Some(path) = object.get("path").and_then(Value::as_str) {
                self.pending.push(PendingInclude {
                    file_index: self.index,
                    from: id.clone(),
                    raw: path.to_string(),
                    fallback: self.format,
                });
            }
            return id;
        }

        if let Some(child) = object.get("child") {
            self.add_node(child, Some(&id));
        }
        if let Some(then) = object.get("then") {
            self.add_node(then, Some(&id));
        }
        if let Some(otherwise) = object.get("else") {
            self.add_node(otherwise, Some(&id));
        }
        if let Some(children) = object.get("children").and_then(Value::as_array) {
            for child in children {
                self.add_node(child, Some(&id));
            }
        }
        if let Some(branches) = object.get("branches").and_then(Value::as_array) {
            for branch in branches {
                let score = branch
                    .get("score")
                    .and_then(Value::as_str)
                    .unwrap_or("branch");
                let branch_id = self.new_node(format!("score: {score}"), false);
                self.connect(id.clone(), branch_id.clone(), false, None);
                if let Some(child) = branch.get("child") {
                    self.add_node(child, Some(&branch_id));
                }
            }
        }
        id
    }

    fn new_node(&mut self, label: String, error: bool) -> String {
        let id = format!("f{}_n{}", self.index, self.next_id);
        self.next_id += 1;
        self.file.nodes.push(GraphNode {
            id: id.clone(),
            label,
            error,
        });
        id
    }

    fn connect(&mut self, from: String, to: String, dashed: bool, label: Option<String>) {
        self.file.edges.push(GraphEdge {
            from,
            to,
            label,
            dashed,
        });
    }
}

fn node_label(object: &Map<String, Value>) -> String {
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let detail = match kind {
        "action" | "condition" | "try_action" | "async_action" => {
            object.get("name").and_then(Value::as_str)
        }
        "custom" => object.get("kind").and_then(Value::as_str),
        "subtree" => object.get("template").and_then(Value::as_str),
        "switch" | "set_value" => object.get("key").and_then(Value::as_str),
        "wait_for_event" => object.get("event").and_then(Value::as_str),
        "include" => object.get("path").and_then(Value::as_str),
        "identified" => object.get("id").and_then(Value::as_str),
        _ => object.get("name").and_then(Value::as_str),
    };
    detail.map_or_else(|| kind.to_string(), |detail| format!("{kind}: {detail}"))
}

fn mermaid_edge(edge: &GraphEdge, indent: &str) -> String {
    let label = edge
        .label
        .as_ref()
        .map(|label| format!("|{}|", escape_mermaid(label)))
        .unwrap_or_default();
    if edge.dashed {
        format!("{indent}{} -.->{label} {}\n", edge.from, edge.to)
    } else {
        format!("{indent}{} -->{label} {}\n", edge.from, edge.to)
    }
}

fn dot_edge(edge: &GraphEdge, indent: &str) -> String {
    let mut attributes = Vec::new();
    if edge.dashed {
        attributes.push(String::from("style=dashed"));
    }
    if let Some(label) = &edge.label {
        attributes.push(format!("label=\"{}\"", escape_dot(label)));
    }
    if attributes.is_empty() {
        format!("{indent}{} -> {};\n", edge.from, edge.to)
    } else {
        format!(
            "{indent}{} -> {} [{}];\n",
            edge.from,
            edge.to,
            attributes.join(", ")
        )
    }
}

fn graph_path(path: &Path) -> String {
    display_path(path).replace('\\', "/")
}

fn escape_mermaid(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
        .replace(['\r', '\n'], " ")
}

fn escape_dot(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\r', '\n'], " ")
}
