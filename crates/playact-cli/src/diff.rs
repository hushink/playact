//! Structural and include diffs between two configurations.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use playact::config::MAX_INCLUDE_DEPTH;
use serde::Serialize;
use serde_json::Value;

use crate::format::Format;
use crate::lint::{collect_includes, display_path, resolve_base, resolve_include};

/// Complete diff between two configurations.
#[derive(Debug, Serialize)]
pub(crate) struct ConfigDiff {
    pub old: String,
    pub new: String,
    pub changes: Vec<NodeChange>,
    pub includes: Vec<IncludeChange>,
}

impl ConfigDiff {
    pub(crate) fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.includes.is_empty()
    }
}

/// One structural configuration change.
#[derive(Debug, Serialize)]
pub(crate) struct NodeChange {
    pub kind: &'static str,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new: Option<String>,
}

/// One include-file set change.
#[derive(Debug, Serialize)]
pub(crate) struct IncludeChange {
    pub kind: &'static str,
    pub path: String,
}

/// Compares two configurations and their include file sets.
pub(crate) fn diff(
    old_path: &Path,
    new_path: &Path,
    include_base: Option<&Path>,
) -> Result<ConfigDiff, String> {
    let old_format = Format::from_path(old_path)?;
    let new_format = Format::from_path(new_path)?;
    let old_text = fs::read_to_string(old_path)
        .map_err(|error| format!("read `{}`: {error}", display_path(old_path)))?;
    let new_text = fs::read_to_string(new_path)
        .map_err(|error| format!("read `{}`: {error}", display_path(new_path)))?;
    let old_document = old_format
        .parse_document(&old_text)
        .map_err(|error| format!("parse `{}`: {error}", display_path(old_path)))?;
    let new_document = new_format
        .parse_document(&new_text)
        .map_err(|error| format!("parse `{}`: {error}", display_path(new_path)))?;

    let mut changes = Vec::new();
    diff_value(&old_document, &new_document, "", &mut changes);

    let old_includes = include_files(old_path, old_format, &old_document, include_base)?;
    let new_includes = include_files(new_path, new_format, &new_document, include_base)?;
    let mut includes = Vec::new();
    for path in old_includes.difference(&new_includes) {
        includes.push(IncludeChange {
            kind: "removed",
            path: path.clone(),
        });
    }
    for path in new_includes.difference(&old_includes) {
        includes.push(IncludeChange {
            kind: "added",
            path: path.clone(),
        });
    }

    Ok(ConfigDiff {
        old: display_path(old_path),
        new: display_path(new_path),
        changes,
        includes,
    })
}

/// Renders a human-readable diff.
pub(crate) fn render_human(diff: &ConfigDiff) -> String {
    let mut output = format!("{} -> {}\n", diff.old, diff.new);
    if diff.is_empty() {
        output.push_str("no changes\n");
        return output;
    }
    for change in &diff.changes {
        match change.kind {
            "added" => output.push_str(&format!(
                "+ {}: {}\n",
                change.path,
                change.new.as_deref().unwrap_or("")
            )),
            "removed" => output.push_str(&format!(
                "- {}: {}\n",
                change.path,
                change.old.as_deref().unwrap_or("")
            )),
            "moved" => output.push_str(&format!(
                "> {}: {} (moved from {})\n",
                change.path,
                change.new.as_deref().unwrap_or(""),
                change.old.as_deref().unwrap_or("")
            )),
            _ => output.push_str(&format!(
                "~ {}: {} -> {}\n",
                change.path,
                change.old.as_deref().unwrap_or(""),
                change.new.as_deref().unwrap_or("")
            )),
        }
    }
    for include in &diff.includes {
        let sign = if include.kind == "added" { "+" } else { "-" };
        output.push_str(&format!("{sign} include {}\n", include.path));
    }
    output.push_str(&format!(
        "\n{} node change(s), {} include change(s)\n",
        diff.changes.len(),
        diff.includes.len()
    ));
    output
}

fn diff_value(old: &Value, new: &Value, path: &str, changes: &mut Vec<NodeChange>) {
    match (old, new) {
        (Value::Object(old_map), Value::Object(new_map)) => {
            for (key, old_value) in old_map {
                let child = join(path, key);
                match new_map.get(key) {
                    Some(new_value) => diff_value(old_value, new_value, &child, changes),
                    None => changes.push(NodeChange {
                        kind: "removed",
                        path: child,
                        old: Some(value_label(old_value)),
                        new: None,
                    }),
                }
            }
            for (key, new_value) in new_map {
                if !old_map.contains_key(key) {
                    changes.push(NodeChange {
                        kind: "added",
                        path: join(path, key),
                        old: None,
                        new: Some(value_label(new_value)),
                    });
                }
            }
        }
        (Value::Array(old_items), Value::Array(new_items)) => {
            if can_match_arrays(old_items, new_items) {
                diff_matched_array(old_items, new_items, path, changes);
            } else {
                diff_indexed_array(old_items, new_items, path, changes);
            }
        }
        _ => {
            if old != new {
                changes.push(NodeChange {
                    kind: "changed",
                    path: path.to_string(),
                    old: Some(value_label(old)),
                    new: Some(value_label(new)),
                });
            }
        }
    }
}

fn can_match_arrays(old: &[Value], new: &[Value]) -> bool {
    let mut any = false;
    for item in old.iter().chain(new) {
        any = true;
        if array_identity(item).is_none() {
            return false;
        }
    }
    any
}

fn array_identity(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    if let Some(kind) = object.get("type").and_then(Value::as_str) {
        let detail = node_detail(object, kind).unwrap_or("");
        return Some(format!("node:{kind}:{detail}"));
    }
    let score = object.get("score").and_then(Value::as_str)?;
    Some(format!("branch:{score}"))
}

fn array_kind(value: &Value) -> Option<&str> {
    let object = value.as_object()?;
    if let Some(kind) = object.get("type").and_then(Value::as_str) {
        return Some(kind);
    }
    object
        .get("score")
        .and_then(Value::as_str)
        .map(|_| "branch")
}

fn diff_indexed_array(
    old_items: &[Value],
    new_items: &[Value],
    path: &str,
    changes: &mut Vec<NodeChange>,
) {
    let shared = old_items.len().min(new_items.len());
    for index in 0..shared {
        diff_value(
            &old_items[index],
            &new_items[index],
            &format!("{path}[{index}]"),
            changes,
        );
    }
    for (index, item) in old_items.iter().enumerate().skip(shared) {
        changes.push(NodeChange {
            kind: "removed",
            path: format!("{path}[{index}]"),
            old: Some(value_label(item)),
            new: None,
        });
    }
    for (index, item) in new_items.iter().enumerate().skip(shared) {
        changes.push(NodeChange {
            kind: "added",
            path: format!("{path}[{index}]"),
            old: None,
            new: Some(value_label(item)),
        });
    }
}

fn diff_matched_array(
    old_items: &[Value],
    new_items: &[Value],
    path: &str,
    changes: &mut Vec<NodeChange>,
) {
    let old_ids: Vec<Option<String>> = old_items.iter().map(array_identity).collect();
    let new_ids: Vec<Option<String>> = new_items.iter().map(array_identity).collect();
    let mut old_queues: HashMap<String, VecDeque<usize>> = HashMap::new();
    for (index, id) in old_ids.iter().enumerate() {
        if let Some(id) = id {
            old_queues.entry(id.clone()).or_default().push_back(index);
        }
    }

    let mut old_used = vec![false; old_items.len()];
    let mut new_used = vec![false; new_items.len()];
    let mut pairs = Vec::new();

    for (new_index, id) in new_ids.iter().enumerate() {
        let Some(id) = id else {
            continue;
        };
        let Some(queue) = old_queues.get_mut(id) else {
            continue;
        };
        while let Some(old_index) = queue.pop_front() {
            if !old_used[old_index] {
                old_used[old_index] = true;
                new_used[new_index] = true;
                pairs.push((old_index, new_index));
                break;
            }
        }
    }

    let shared = old_items.len().min(new_items.len());
    for index in 0..shared {
        if old_used[index] || new_used[index] {
            continue;
        }
        let old_kind = array_kind(&old_items[index]);
        if old_kind.is_some() && old_kind == array_kind(&new_items[index]) {
            old_used[index] = true;
            new_used[index] = true;
            pairs.push((index, index));
        }
    }

    for (index, item) in old_items.iter().enumerate() {
        if !old_used[index] {
            changes.push(NodeChange {
                kind: "removed",
                path: format!("{path}[{index}]"),
                old: Some(value_label(item)),
                new: None,
            });
        }
    }
    for (index, item) in new_items.iter().enumerate() {
        if !new_used[index] {
            changes.push(NodeChange {
                kind: "added",
                path: format!("{path}[{index}]"),
                old: None,
                new: Some(value_label(item)),
            });
        }
    }

    pairs.sort_by_key(|(_, new_index)| *new_index);
    for (old_index, new_index) in pairs {
        let old_path = format!("{path}[{old_index}]");
        let new_path = format!("{path}[{new_index}]");
        if old_index != new_index {
            changes.push(NodeChange {
                kind: "moved",
                path: new_path.clone(),
                old: Some(old_path),
                new: Some(value_label(&new_items[new_index])),
            });
        }
        diff_value(
            &old_items[old_index],
            &new_items[new_index],
            &new_path,
            changes,
        );
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

fn node_detail<'a>(object: &'a serde_json::Map<String, Value>, kind: &str) -> Option<&'a str> {
    match kind {
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
    }
}

fn value_label(value: &Value) -> String {
    if let Some(object) = value.as_object()
        && let Some(kind) = object.get("type").and_then(Value::as_str)
    {
        return node_detail(object, kind)
            .map_or_else(|| kind.to_string(), |detail| format!("{kind}: {detail}"));
    }
    truncate(&value.to_string(), 160)
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut shortened: String = text.chars().take(limit).collect();
    shortened.push_str("...");
    shortened
}

fn include_files(
    root: &Path,
    format: Format,
    document: &Value,
    include_base: Option<&Path>,
) -> Result<BTreeSet<String>, String> {
    let base = resolve_base(root, include_base)?;
    let mut files = BTreeSet::new();
    let mut visited = HashSet::new();
    let mut stack = Vec::new();
    if let Ok(canonical) = root.canonicalize() {
        visited.insert(canonical.clone());
        stack.push(canonical);
    }
    walk_includes(
        document,
        format,
        &base,
        &mut visited,
        &mut stack,
        &mut files,
        0,
    )?;
    Ok(files)
}

fn walk_includes(
    document: &Value,
    format: Format,
    base: &Path,
    visited: &mut HashSet<PathBuf>,
    stack: &mut Vec<PathBuf>,
    files: &mut BTreeSet<String>,
    depth: usize,
) -> Result<(), String> {
    for (location, include) in collect_includes(document) {
        if depth >= MAX_INCLUDE_DEPTH {
            return Err(format!(
                "include at `{location}`: nesting exceeds {MAX_INCLUDE_DEPTH} levels"
            ));
        }
        let (target, target_format) = resolve_include(base, &include, format)
            .map_err(|error| format!("include at `{location}`: {error}"))?;
        if stack.contains(&target) {
            continue;
        }
        let label = target
            .strip_prefix(base)
            .map_or_else(|_| display_path(&target), display_path)
            .replace('\\', "/");
        files.insert(label);
        if !visited.insert(target.clone()) {
            continue;
        }
        let text = fs::read_to_string(&target)
            .map_err(|error| format!("read include `{}`: {error}", display_path(&target)))?;
        let included_document = target_format
            .parse_document(&text)
            .map_err(|error| format!("parse include `{}`: {error}", display_path(&target)))?;
        stack.push(target);
        walk_includes(
            &included_document,
            target_format,
            base,
            visited,
            stack,
            files,
            depth + 1,
        )?;
        stack.pop();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reports_added_removed_and_changed_values() {
        let old = json!({
            "root": {
                "type": "sequence",
                "children": [
                    {"type": "action", "name": "wait"},
                    {"type": "action", "name": "heal", "params": {"amount": 10}}
                ]
            }
        });
        let new = json!({
            "root": {
                "type": "sequence",
                "children": [
                    {"type": "action", "name": "wait"},
                    {"type": "action", "name": "heal", "params": {"amount": 20}}
                ]
            }
        });
        let mut changes = Vec::new();
        diff_value(&old, &new, "", &mut changes);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "root.children[1].params.amount");
        assert_eq!(changes[0].kind, "changed");
    }

    #[test]
    fn reports_insertions_as_moves_instead_of_index_changes() {
        let old = json!({
            "root": {
                "type": "sequence",
                "children": [
                    {"type": "action", "name": "wait"},
                    {"type": "action", "name": "heal"}
                ]
            }
        });
        let new = json!({
            "root": {
                "type": "sequence",
                "children": [
                    {"type": "action", "name": "attack"},
                    {"type": "action", "name": "wait"},
                    {"type": "action", "name": "heal"}
                ]
            }
        });
        let mut changes = Vec::new();
        diff_value(&old, &new, "", &mut changes);
        assert_eq!(
            changes
                .iter()
                .filter(|change| change.kind == "moved")
                .count(),
            2
        );
        assert!(
            changes
                .iter()
                .any(|change| { change.kind == "added" && change.path == "root.children[0]" })
        );
        assert!(
            changes.iter().all(|change| change.kind != "changed"),
            "{changes:?}"
        );
    }

    #[test]
    fn reports_reorders_as_moves() {
        let old = json!({
            "root": {
                "type": "sequence",
                "children": [
                    {"type": "action", "name": "wait"},
                    {"type": "action", "name": "heal"}
                ]
            }
        });
        let new = json!({
            "root": {
                "type": "sequence",
                "children": [
                    {"type": "action", "name": "heal"},
                    {"type": "action", "name": "wait"}
                ]
            }
        });
        let mut changes = Vec::new();
        diff_value(&old, &new, "", &mut changes);
        assert_eq!(changes.len(), 2);
        assert!(changes.iter().all(|change| change.kind == "moved"));
    }

    #[test]
    fn reports_added_nodes_with_labels() {
        let old = json!({"root": {"type": "sequence", "children": []}});
        let new = json!({
            "root": {
                "type": "sequence",
                "children": [{"type": "action", "name": "wait"}]
            }
        });
        let mut changes = Vec::new();
        diff_value(&old, &new, "", &mut changes);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "added");
        assert_eq!(changes[0].path, "root.children[0]");
        assert_eq!(changes[0].new.as_deref(), Some("action: wait"));
    }
}
