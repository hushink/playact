//! Declarative configuration migration driven by a manifest.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use playact::config::{ConfigError, ConfigMigrator, MAX_INCLUDE_DEPTH, TreeSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::PendingWrite;
use crate::format::Format;
use crate::lint::{self, LintIssue, collect_includes, display_path, resolve_base, resolve_include};

const MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    #[serde(default = "default_manifest_version")]
    version: u32,
    migrations: Vec<MigrationStep>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MigrationStep {
    from: u32,
    operations: Vec<Operation>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", deny_unknown_fields)]
enum Operation {
    #[serde(rename = "rename_field")]
    Rename {
        #[serde(default)]
        at: Option<String>,
        #[serde(default, rename = "where")]
        r#where: Option<BTreeMap<String, Value>>,
        from: String,
        to: String,
    },
    #[serde(rename = "remove_field")]
    Remove {
        #[serde(default)]
        at: Option<String>,
        #[serde(default, rename = "where")]
        r#where: Option<BTreeMap<String, Value>>,
        field: String,
    },
    #[serde(rename = "set_field")]
    Set {
        #[serde(default)]
        at: Option<String>,
        #[serde(default, rename = "where")]
        r#where: Option<BTreeMap<String, Value>>,
        field: String,
        value: Value,
        #[serde(default)]
        overwrite: bool,
    },
    #[serde(rename = "rename_value")]
    RenameValue {
        #[serde(default)]
        at: Option<String>,
        #[serde(default, rename = "where")]
        r#where: Option<BTreeMap<String, Value>>,
        field: String,
        from: Value,
        to: Value,
    },
    #[serde(rename = "map_values")]
    MapValues {
        #[serde(default)]
        at: Option<String>,
        #[serde(default, rename = "where")]
        r#where: Option<BTreeMap<String, Value>>,
        field: String,
        map: BTreeMap<String, Value>,
    },
    #[serde(rename = "scale")]
    Scale {
        #[serde(default)]
        at: Option<String>,
        #[serde(default, rename = "where")]
        r#where: Option<BTreeMap<String, Value>>,
        field: String,
        multiply: f64,
        #[serde(default)]
        add: f64,
    },
}

impl Operation {
    fn name(&self) -> &'static str {
        match self {
            Self::Rename { .. } => "rename_field",
            Self::Remove { .. } => "remove_field",
            Self::Set { .. } => "set_field",
            Self::RenameValue { .. } => "rename_value",
            Self::MapValues { .. } => "map_values",
            Self::Scale { .. } => "scale",
        }
    }

    fn target(&self) -> (Option<&str>, Option<&BTreeMap<String, Value>>) {
        match self {
            Self::Rename { at, r#where, .. }
            | Self::Remove { at, r#where, .. }
            | Self::Set { at, r#where, .. }
            | Self::RenameValue { at, r#where, .. }
            | Self::MapValues { at, r#where, .. }
            | Self::Scale { at, r#where, .. } => (at.as_deref(), r#where.as_ref()),
        }
    }

    fn validate(&self) -> Result<(), String> {
        let (at, matcher) = self.target();
        if at.is_some() && matcher.is_some() {
            return Err(format!(
                "{}: `at` and `where` are mutually exclusive",
                self.name()
            ));
        }
        if let Some(at) = at
            && !at.is_empty()
            && !at.starts_with('/')
        {
            return Err(format!(
                "{}: `at` must be an empty string or a JSON Pointer starting with `/`",
                self.name()
            ));
        }
        if matcher.is_some_and(BTreeMap::is_empty) {
            return Err(format!("{}: `where` cannot be empty", self.name()));
        }
        match self {
            Self::Rename { from, to, .. } => {
                if from.is_empty() || to.is_empty() {
                    return Err(String::from("rename_field: field names cannot be empty"));
                }
                if from == to {
                    return Err(String::from("rename_field: `from` and `to` must differ"));
                }
            }
            Self::Remove { field, .. } => {
                if field.is_empty() {
                    return Err(String::from("remove_field: field name cannot be empty"));
                }
            }
            Self::Set { field, .. } => {
                if field.is_empty() {
                    return Err(String::from("set_field: field name cannot be empty"));
                }
            }
            Self::RenameValue {
                field, from, to, ..
            } => {
                if field.is_empty() {
                    return Err(String::from("rename_value: field name cannot be empty"));
                }
                if from == to {
                    return Err(String::from("rename_value: `from` and `to` must differ"));
                }
            }
            Self::MapValues { field, map, .. } => {
                if field.is_empty() {
                    return Err(String::from("map_values: field name cannot be empty"));
                }
                if map.is_empty() {
                    return Err(String::from("map_values: `map` cannot be empty"));
                }
            }
            Self::Scale {
                field,
                multiply,
                add,
                ..
            } => {
                if field.is_empty() {
                    return Err(String::from("scale: field name cannot be empty"));
                }
                if !multiply.is_finite() || !add.is_finite() {
                    return Err(String::from(
                        "scale: `multiply` and `add` must be finite numbers",
                    ));
                }
            }
        }
        Ok(())
    }

    fn apply(&self, document: &mut Value) -> Result<(), ConfigError> {
        self.validate().map_err(ConfigError::new)?;
        let (at, matcher) = self.target();
        match self {
            Self::Rename { from, to, .. } => {
                apply_to_target(document, at, matcher, self.name(), |object, path| {
                    if !object.contains_key(from) {
                        return Ok(());
                    }
                    if object.contains_key(to) {
                        return Err(ConfigError::with_path(
                            path,
                            format!("cannot rename `{from}` to `{to}`: `{to}` already exists"),
                        ));
                    }
                    let value = object.remove(from).expect("source field was checked");
                    object.insert(to.clone(), value);
                    Ok(())
                })
            }
            Self::Remove { field, .. } => {
                apply_to_target(document, at, matcher, self.name(), |object, _| {
                    object.remove(field);
                    Ok(())
                })
            }
            Self::Set {
                field,
                value,
                overwrite,
                ..
            } => apply_to_target(document, at, matcher, self.name(), |object, _| {
                if *overwrite || !object.contains_key(field) {
                    object.insert(field.clone(), value.clone());
                }
                Ok(())
            }),
            Self::RenameValue {
                field, from, to, ..
            } => apply_to_target(document, at, matcher, self.name(), |object, _| {
                if object.get(field) == Some(from) {
                    object.insert(field.clone(), to.clone());
                }
                Ok(())
            }),
            Self::MapValues { field, map, .. } => {
                let mut apply = |object: &mut Map<String, Value>, _: &str| {
                    let mapped = match object.get(field) {
                        Some(Value::String(current)) => map.get(current).cloned(),
                        _ => None,
                    };
                    if let Some(mapped) = mapped {
                        object.insert(field.clone(), mapped);
                    }
                    Ok(())
                };
                if at.is_none() && matcher.is_none() {
                    visit_objects(document, "", &mut apply)
                } else {
                    apply_to_target(document, at, matcher, self.name(), apply)
                }
            }
            Self::Scale {
                field,
                multiply,
                add,
                ..
            } => apply_to_target(document, at, matcher, self.name(), |object, path| {
                let Some(current) = object.get(field).cloned() else {
                    return Ok(());
                };
                let Some(number) = current.as_f64() else {
                    return Err(ConfigError::with_path(
                        child_pointer(path, field),
                        format!("scale: field `{field}` is not a number"),
                    ));
                };
                let value = scale_value(&current, number * multiply + add).map_err(|message| {
                    ConfigError::with_path(child_pointer(path, field), format!("scale: {message}"))
                })?;
                object.insert(field.clone(), value);
                Ok(())
            }),
        }
    }
}

fn scale_value(original: &Value, scaled: f64) -> Result<Value, String> {
    if !scaled.is_finite() {
        return Err(String::from("result is not finite"));
    }
    if scaled.fract() == 0.0 {
        if original.is_u64() && scaled >= 0.0 && scaled <= u64::MAX as f64 {
            return Ok(Value::from(scaled as u64));
        }
        if original.is_i64() && scaled >= i64::MIN as f64 && scaled <= i64::MAX as f64 {
            return Ok(Value::from(scaled as i64));
        }
    }
    serde_json::Number::from_f64(scaled)
        .map(Value::Number)
        .ok_or_else(|| String::from("result is not a finite JSON number"))
}

fn apply_operations(mut document: Value, operations: &[Operation]) -> Result<Value, ConfigError> {
    for (index, operation) in operations.iter().enumerate() {
        operation.apply(&mut document).map_err(|error| {
            let message = format!(
                "operation {} ({}): {}",
                index + 1,
                operation.name(),
                error.message()
            );
            match error.path().filter(|path| !path.is_empty()) {
                Some(path) => ConfigError::with_path(path, message),
                None => ConfigError::new(message),
            }
        })?;
    }
    Ok(document)
}

fn apply_to_target(
    document: &mut Value,
    at: Option<&str>,
    matcher: Option<&BTreeMap<String, Value>>,
    operation: &str,
    mut action: impl FnMut(&mut Map<String, Value>, &str) -> Result<(), ConfigError>,
) -> Result<(), ConfigError> {
    if let Some(matcher) = matcher {
        return visit_matching(document, "", matcher, &mut action);
    }
    let pointer = at.unwrap_or("");
    let target = document.pointer_mut(pointer).ok_or_else(|| {
        ConfigError::with_path(pointer, format!("{operation}: target does not exist"))
    })?;
    let object = target.as_object_mut().ok_or_else(|| {
        ConfigError::with_path(pointer, format!("{operation}: target is not an object"))
    })?;
    action(object, pointer)
}

fn visit_objects(
    value: &mut Value,
    pointer: &str,
    action: &mut impl FnMut(&mut Map<String, Value>, &str) -> Result<(), ConfigError>,
) -> Result<(), ConfigError> {
    match value {
        Value::Object(object) => {
            action(object, pointer)?;
            for (field, child) in object.iter_mut() {
                let child_pointer = child_pointer(pointer, field);
                visit_objects(child, &child_pointer, action)?;
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter_mut().enumerate() {
                visit_objects(child, &format!("{pointer}/{index}"), action)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn visit_matching(
    value: &mut Value,
    pointer: &str,
    matcher: &BTreeMap<String, Value>,
    action: &mut impl FnMut(&mut Map<String, Value>, &str) -> Result<(), ConfigError>,
) -> Result<(), ConfigError> {
    match value {
        Value::Object(object) => {
            if matcher
                .iter()
                .all(|(field, expected)| object.get(field) == Some(expected))
            {
                action(object, pointer)?;
            }
            for (field, child) in object.iter_mut() {
                let child_pointer = child_pointer(pointer, field);
                visit_matching(child, &child_pointer, matcher, action)?;
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter_mut().enumerate() {
                visit_matching(child, &format!("{pointer}/{index}"), matcher, action)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn child_pointer(pointer: &str, field: &str) -> String {
    let field = field.replace('~', "~0").replace('/', "~1");
    format!("{pointer}/{field}")
}

fn default_manifest_version() -> u32 {
    MANIFEST_VERSION
}

/// Loads a declarative migration manifest and builds its version pipeline.
pub(crate) fn load_manifest(path: &Path) -> Result<ConfigMigrator, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("read manifest `{}`: {error}", display_path(path)))?;
    let format = Format::from_path(path)?;
    let document = format
        .parse_document(&text)
        .map_err(|error| format!("parse manifest `{}`: {error}", display_path(path)))?;
    let manifest: Manifest = serde_json::from_value(document)
        .map_err(|error| format!("manifest `{}`: {error}", display_path(path)))?;
    if manifest.version != MANIFEST_VERSION {
        return Err(format!(
            "unsupported manifest version {}; supported is {MANIFEST_VERSION}",
            manifest.version
        ));
    }

    let mut migrator = ConfigMigrator::new();
    for step in manifest.migrations {
        for (index, operation) in step.operations.iter().enumerate() {
            operation.validate().map_err(|error| {
                format!(
                    "manifest migration from version {} operation {}: {error}",
                    step.from,
                    index + 1
                )
            })?;
        }
        let from = step.from;
        let operations = Arc::new(step.operations);
        migrator
            .register(from, move |document| {
                apply_operations(document, &operations)
            })
            .map_err(|error| format!("manifest migration from version {from}: {error}"))?;
    }
    Ok(migrator)
}

/// Type of a field-level migration change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChangeKind {
    Added,
    Removed,
    Changed,
}

/// One field-level difference between the original and migrated document.
#[derive(Debug, Serialize)]
pub(crate) struct FieldChange {
    /// File containing the field.
    pub file: String,
    /// JSON Pointer to the field; empty means the document root.
    pub path: String,
    /// Kind of change.
    pub kind: ChangeKind,
    /// Previous value for removed or changed fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<Value>,
    /// New value for added or changed fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Value>,
}

/// Result of migrating one root configuration and its include graph.
#[derive(Debug, Serialize)]
pub(crate) struct MigrationReport {
    /// Root path as provided on the command line.
    pub file: String,
    /// Whether the migrated root and its includes passed validation.
    pub valid: bool,
    /// Validation and migration findings.
    pub issues: Vec<LintIssue>,
    /// Files that were migrated (or would be, with `--check`).
    pub migrated: Vec<String>,
    /// Field-level changes across the root and its includes.
    pub changes: Vec<FieldChange>,
}

/// Result of migrating every supplied root.
pub(crate) struct MigrationOutcome {
    pub reports: Vec<MigrationReport>,
    pub writes: Vec<PendingWrite>,
}

/// Migrates roots and their include graphs, validating every final document.
pub(crate) fn migrate_files(
    migrator: &ConfigMigrator,
    schema: &Value,
    roots: &[PathBuf],
    include_base: Option<&Path>,
) -> MigrationOutcome {
    let mut writes = BTreeMap::new();
    let mut reports = Vec::with_capacity(roots.len());

    for root in roots {
        let mut state = RootState {
            base: PathBuf::new(),
            migrator,
            schema,
            writes: &mut writes,
            issues: Vec::new(),
            migrated: BTreeSet::new(),
            changes: Vec::new(),
            visited: HashSet::new(),
            stack: Vec::new(),
        };
        match resolve_base(root, include_base) {
            Ok(base) => {
                state.base = base;
                state.process_file(root, None, 0, true);
            }
            Err(message) => state.issues.push(LintIssue::new("include", "", message)),
        }
        let valid = state.issues.is_empty();
        reports.push(MigrationReport {
            file: display_path(root),
            valid,
            issues: state.issues,
            migrated: state.migrated.into_iter().collect(),
            changes: state.changes,
        });
    }

    MigrationOutcome {
        reports,
        writes: writes.into_values().collect(),
    }
}

struct RootState<'a> {
    base: PathBuf,
    migrator: &'a ConfigMigrator,
    schema: &'a Value,
    writes: &'a mut BTreeMap<PathBuf, PendingWrite>,
    issues: Vec<LintIssue>,
    migrated: BTreeSet<String>,
    changes: Vec<FieldChange>,
    visited: HashSet<PathBuf>,
    stack: Vec<PathBuf>,
}

impl RootState<'_> {
    fn process_file(&mut self, path: &Path, format: Option<Format>, depth: usize, is_root: bool) {
        let canonical = match path.canonicalize() {
            Ok(canonical) => canonical,
            Err(error) => {
                let display = display_path(path);
                self.push_issue(
                    LintIssue::new("io", "", format!("canonicalize: {error}")),
                    &display,
                    is_root,
                );
                return;
            }
        };
        let display = display_path(path);
        self.visited.insert(canonical.clone());
        self.stack.push(canonical.clone());
        self.process_file_inner(&canonical, display, format, depth, is_root);
        self.stack.pop();
    }

    fn process_file_inner(
        &mut self,
        canonical: &Path,
        display: String,
        format: Option<Format>,
        depth: usize,
        is_root: bool,
    ) {
        let text = match fs::read_to_string(canonical) {
            Ok(text) => text,
            Err(error) => {
                self.push_issue(
                    LintIssue::new("io", "", error.to_string()),
                    &display,
                    is_root,
                );
                return;
            }
        };
        let format = match format.map_or_else(|| Format::from_path(canonical), Ok) {
            Ok(format) => format,
            Err(message) => {
                self.push_issue(LintIssue::new("format", "", message), &display, is_root);
                return;
            }
        };
        let document = match format.parse_document(&text) {
            Ok(document) => document,
            Err(error) => {
                self.push_issue(LintIssue::new("parse", "", error), &display, is_root);
                return;
            }
        };
        let original = document.clone();
        let migrated = match self.migrator.upgrade(document) {
            Ok(document) => document,
            Err(error) => {
                self.push_issue(
                    LintIssue::new("migration", error.path().unwrap_or(""), error.message()),
                    &display,
                    is_root,
                );
                return;
            }
        };
        let changed = migrated != original;
        self.changes
            .extend(collect_changes(&original, &migrated, &display));
        let issue_count = self.issues.len();
        for mut issue in lint::validate_value(self.schema, &migrated) {
            if !is_root {
                issue.file = Some(display.clone());
            }
            self.issues.push(issue);
        }
        let file_valid = self.issues.len() == issue_count;

        if changed && file_valid {
            match serialize_migrated(&migrated, format) {
                Ok(bytes) => {
                    let original_bytes = text.into_bytes();
                    if bytes != original_bytes {
                        match self.writes.get(canonical) {
                            Some(existing) if existing.formatted != bytes => {
                                self.push_issue(
                                    LintIssue::new(
                                        "migration",
                                        "",
                                        "conflicting migrated content for the same file",
                                    ),
                                    &display,
                                    is_root,
                                );
                            }
                            Some(_) => {}
                            None => {
                                self.writes.insert(
                                    canonical.to_path_buf(),
                                    PendingWrite {
                                        path: canonical.to_path_buf(),
                                        display: display.clone(),
                                        original: original_bytes,
                                        formatted: bytes,
                                    },
                                );
                            }
                        }
                        self.migrated.insert(display.clone());
                    }
                }
                Err(message) => {
                    self.push_issue(LintIssue::new("migration", "", message), &display, is_root)
                }
            }
        }

        let fallback_format = format;
        for (location, include) in collect_includes(&migrated) {
            if depth >= MAX_INCLUDE_DEPTH {
                self.push_issue(
                    LintIssue::new(
                        "include",
                        location,
                        format!("include nesting exceeds {MAX_INCLUDE_DEPTH} levels"),
                    ),
                    &display,
                    is_root,
                );
                continue;
            }
            let (target, target_format) =
                match resolve_include(&self.base, &include, fallback_format) {
                    Ok(resolved) => resolved,
                    Err(message) => {
                        self.push_issue(
                            LintIssue::new("include", location, message),
                            &display,
                            is_root,
                        );
                        continue;
                    }
                };
            if self.stack.contains(&target) {
                self.push_issue(
                    LintIssue::new(
                        "include",
                        location,
                        format!("include cycle through `{}`", display_path(&target)),
                    ),
                    &display,
                    is_root,
                );
                continue;
            }
            if self.visited.contains(&target) {
                continue;
            }
            self.process_file(&target, Some(target_format), depth + 1, false);
        }
    }

    fn push_issue(&mut self, mut issue: LintIssue, file: &str, is_root: bool) {
        if !is_root {
            issue.file = Some(file.to_owned());
        }
        self.issues.push(issue);
    }
}

fn collect_changes(from: &Value, to: &Value, file: &str) -> Vec<FieldChange> {
    let mut changes = Vec::new();
    diff_values(from, to, "", file, &mut changes);
    changes
}

fn diff_values(from: &Value, to: &Value, path: &str, file: &str, changes: &mut Vec<FieldChange>) {
    if from == to {
        return;
    }
    match (from, to) {
        (Value::Object(old), Value::Object(new)) => {
            let fields: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
            for field in fields {
                let child = child_pointer(path, field);
                match (old.get(field), new.get(field)) {
                    (Some(before), Some(after)) => {
                        diff_values(before, after, &child, file, changes);
                    }
                    (Some(before), None) => changes.push(FieldChange {
                        file: file.to_owned(),
                        path: child,
                        kind: ChangeKind::Removed,
                        from: Some(before.clone()),
                        to: None,
                    }),
                    (None, Some(after)) => changes.push(FieldChange {
                        file: file.to_owned(),
                        path: child,
                        kind: ChangeKind::Added,
                        from: None,
                        to: Some(after.clone()),
                    }),
                    (None, None) => {}
                }
            }
        }
        (Value::Array(old), Value::Array(new)) => {
            let shared = old.len().min(new.len());
            for index in 0..shared {
                diff_values(
                    &old[index],
                    &new[index],
                    &format!("{path}/{index}"),
                    file,
                    changes,
                );
            }
            for (index, value) in old.iter().enumerate().skip(shared) {
                changes.push(FieldChange {
                    file: file.to_owned(),
                    path: format!("{path}/{index}"),
                    kind: ChangeKind::Removed,
                    from: Some(value.clone()),
                    to: None,
                });
            }
            for (index, value) in new.iter().enumerate().skip(shared) {
                changes.push(FieldChange {
                    file: file.to_owned(),
                    path: format!("{path}/{index}"),
                    kind: ChangeKind::Added,
                    from: None,
                    to: Some(value.clone()),
                });
            }
        }
        _ => changes.push(FieldChange {
            file: file.to_owned(),
            path: path.to_owned(),
            kind: ChangeKind::Changed,
            from: Some(from.clone()),
            to: Some(to.clone()),
        }),
    }
}

fn serialize_migrated(document: &Value, format: Format) -> Result<Vec<u8>, String> {
    let json = serde_json::to_string(document)
        .map_err(|error| format!("serialize migrated document: {error}"))?;
    let spec = TreeSpec::from_json_str(&json).map_err(|error| error.to_string())?;
    format.serialize_spec(&spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rename_root() -> Operation {
        Operation::Rename {
            at: Some(String::new()),
            r#where: None,
            from: String::from("tree"),
            to: String::from("root"),
        }
    }

    #[test]
    fn renames_fields_at_json_pointers() {
        let document = json!({
            "version": 0,
            "tree": {"type": "sequence", "children": [{"tree": "nested"}]}
        });
        let migrated = apply_operations(document, &[rename_root()]).expect("rename");
        assert_eq!(migrated["root"]["type"], "sequence");
        assert_eq!(migrated["root"]["children"][0]["tree"], "nested");
        assert!(migrated.get("tree").is_none());
    }

    #[test]
    fn recursively_matches_objects_for_field_operations() {
        let operations = vec![
            Operation::Rename {
                at: None,
                r#where: Some(BTreeMap::from([(
                    String::from("type"),
                    Value::String(String::from("wait")),
                )])),
                from: String::from("seconds"),
                to: String::from("duration_ms"),
            },
            Operation::Remove {
                at: None,
                r#where: Some(BTreeMap::from([(
                    String::from("type"),
                    Value::String(String::from("wait")),
                )])),
                field: String::from("legacy"),
            },
        ];
        let document = json!({
            "root": {
                "type": "sequence",
                "children": [
                    {"type": "wait", "seconds": 1, "legacy": true},
                    {"type": "action", "name": "keep", "seconds": 2}
                ]
            }
        });
        let migrated = apply_operations(document, &operations).expect("recursive ops");
        assert_eq!(migrated["root"]["children"][0]["duration_ms"], 1);
        assert!(migrated["root"]["children"][0].get("seconds").is_none());
        assert!(migrated["root"]["children"][0].get("legacy").is_none());
        assert_eq!(migrated["root"]["children"][1]["seconds"], 2);
    }

    #[test]
    fn set_field_defaults_and_overwrites() {
        let default = Operation::Set {
            at: None,
            r#where: Some(BTreeMap::from([(
                String::from("type"),
                Value::String(String::from("wait")),
            )])),
            field: String::from("duration_ms"),
            value: json!(100),
            overwrite: false,
        };
        let overwrite = Operation::Set {
            at: Some(String::from("/root")),
            r#where: None,
            field: String::from("name"),
            value: json!("replaced"),
            overwrite: true,
        };
        let document = json!({
            "root": {
                "type": "wait",
                "duration_ms": 250,
                "name": "keep"
            }
        });
        let migrated = apply_operations(document, &[default, overwrite]).expect("set fields");
        assert_eq!(migrated["root"]["duration_ms"], 250);
        assert_eq!(migrated["root"]["name"], "replaced");
    }

    #[test]
    fn at_pointer_targets_one_object() {
        let operation = Operation::Rename {
            at: Some(String::from("/root")),
            r#where: None,
            from: String::from("label"),
            to: String::from("name"),
        };
        let document = json!({
            "root": {"label": "outer", "children": [{"label": "inner"}]}
        });
        let migrated = apply_operations(document, &[operation]).expect("pointer op");
        assert_eq!(migrated["root"]["name"], "outer");
        assert_eq!(migrated["root"]["children"][0]["label"], "inner");
    }

    #[test]
    fn rename_collisions_report_the_json_pointer() {
        let operation = Operation::Rename {
            at: None,
            r#where: Some(BTreeMap::from([(
                String::from("type"),
                Value::String(String::from("wait")),
            )])),
            from: String::from("seconds"),
            to: String::from("duration_ms"),
        };
        let document = json!({
            "root": {
                "type": "sequence",
                "children": [
                    {"type": "wait", "seconds": 1, "duration_ms": 1000}
                ]
            }
        });
        let error = apply_operations(document, &[operation]).expect_err("collision");
        assert_eq!(error.path(), Some("/root/children/0"));
        assert!(error.message().contains("already exists"), "{error}");
    }

    #[test]
    fn renames_values_and_scales_numbers() {
        let operations = vec![
            Operation::RenameValue {
                at: None,
                r#where: Some(BTreeMap::from([(
                    String::from("type"),
                    Value::String(String::from("wait_legacy")),
                )])),
                field: String::from("type"),
                from: Value::String(String::from("wait_legacy")),
                to: Value::String(String::from("wait")),
            },
            Operation::Scale {
                at: None,
                r#where: Some(BTreeMap::from([(
                    String::from("type"),
                    Value::String(String::from("wait")),
                )])),
                field: String::from("duration"),
                multiply: 1000.0,
                add: 0.0,
            },
        ];
        let document = json!({
            "root": {
                "type": "sequence",
                "children": [
                    {"type": "wait_legacy", "duration": 2},
                    {"type": "action", "name": "keep", "duration": 2}
                ]
            }
        });
        let migrated = apply_operations(document, &operations).expect("value and scale ops");
        assert_eq!(migrated["root"]["children"][0]["type"], "wait");
        assert_eq!(migrated["root"]["children"][0]["duration"], 2000);
        assert_eq!(migrated["root"]["children"][1]["duration"], 2);
    }

    #[test]
    fn maps_multiple_values_across_all_objects() {
        let operation = Operation::MapValues {
            at: None,
            r#where: None,
            field: String::from("type"),
            map: BTreeMap::from([
                (
                    String::from("legacy_wait"),
                    Value::String(String::from("wait")),
                ),
                (
                    String::from("legacy_action"),
                    Value::String(String::from("action")),
                ),
            ]),
        };
        let document = json!({
            "root": {
                "type": "sequence",
                "children": [
                    {"type": "legacy_wait"},
                    {"type": "legacy_action", "name": "keep"},
                    {"type": "identified", "child": {"type": "legacy_wait"}}
                ]
            }
        });
        let migrated = apply_operations(document, &[operation]).expect("map values");
        assert_eq!(migrated["root"]["children"][0]["type"], "wait");
        assert_eq!(migrated["root"]["children"][1]["type"], "action");
        assert_eq!(migrated["root"]["children"][2]["child"]["type"], "wait");
    }

    #[test]
    fn scale_rejects_non_numeric_fields() {
        let operation = Operation::Scale {
            at: Some(String::from("/root")),
            r#where: None,
            field: String::from("duration"),
            multiply: 1000.0,
            add: 0.0,
        };
        let document = json!({"root": {"type": "wait", "duration": "soon"}});
        let error = apply_operations(document, &[operation]).expect_err("not a number");
        assert_eq!(error.path(), Some("/root/duration"));
        assert!(error.message().contains("not a number"), "{error}");
    }

    #[test]
    fn field_changes_cover_added_removed_and_changed_values() {
        let from = json!({
            "version": 0,
            "tree": {"type": "action", "name": "old"},
            "gone": true
        });
        let to = json!({
            "version": 1,
            "root": {"type": "action", "name": "new"},
            "added": 2
        });
        let changes = collect_changes(&from, &to, "root.json");
        assert!(changes.iter().any(|change| {
            change.path == "/version"
                && change.kind == ChangeKind::Changed
                && change.from == Some(json!(0))
                && change.to == Some(json!(1))
        }));
        assert!(changes.iter().any(|change| {
            change.path == "/tree"
                && change.kind == ChangeKind::Removed
                && change.file == "root.json"
        }));
        assert!(
            changes
                .iter()
                .any(|change| { change.path == "/root" && change.kind == ChangeKind::Added })
        );
        assert!(
            changes
                .iter()
                .any(|change| { change.path == "/gone" && change.kind == ChangeKind::Removed })
        );
        assert!(
            changes
                .iter()
                .any(|change| { change.path == "/added" && change.kind == ChangeKind::Added })
        );
    }

    #[test]
    fn invalid_operations_are_rejected() {
        let cases = [
            Operation::Rename {
                at: Some(String::from("/root")),
                r#where: Some(BTreeMap::from([(
                    String::from("type"),
                    Value::String(String::from("wait")),
                )])),
                from: String::from("a"),
                to: String::from("b"),
            },
            Operation::Rename {
                at: Some(String::from("root")),
                r#where: None,
                from: String::from("a"),
                to: String::from("b"),
            },
            Operation::Remove {
                at: None,
                r#where: Some(BTreeMap::new()),
                field: String::from("a"),
            },
            Operation::Rename {
                at: None,
                r#where: None,
                from: String::from("a"),
                to: String::from("a"),
            },
            Operation::Set {
                at: None,
                r#where: None,
                field: String::new(),
                value: Value::Null,
                overwrite: false,
            },
            Operation::RenameValue {
                at: None,
                r#where: None,
                field: String::new(),
                from: json!("a"),
                to: json!("b"),
            },
            Operation::RenameValue {
                at: None,
                r#where: None,
                field: String::from("type"),
                from: json!("a"),
                to: json!("a"),
            },
            Operation::Scale {
                at: None,
                r#where: None,
                field: String::from("duration"),
                multiply: f64::NAN,
                add: 0.0,
            },
            Operation::MapValues {
                at: None,
                r#where: None,
                field: String::new(),
                map: BTreeMap::from([(String::from("a"), json!("b"))]),
            },
            Operation::MapValues {
                at: None,
                r#where: None,
                field: String::from("type"),
                map: BTreeMap::new(),
            },
        ];
        for operation in cases {
            assert!(operation.validate().is_err(), "{operation:?}");
        }
    }
}
