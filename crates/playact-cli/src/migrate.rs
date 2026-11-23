//! Declarative configuration migration driven by a manifest.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use playact::config::{ConfigError, ConfigMigrator, MAX_INCLUDE_DEPTH, TreeSpec};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::PendingWrite;
use crate::bundle::relative_include_path;
use crate::convert::{cleanup_output, prepare_output};
use crate::format::Format;
use crate::lint::{self, LintIssue, collect_includes, display_path, resolve_base, resolve_include};

const MANIFEST_VERSION: u32 = 1;

/// Declarative migration manifest.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Manifest {
    /// Manifest format version. Only `1` is supported.
    #[serde(default = "default_manifest_version")]
    version: u32,
    /// Ordered list of versioned migration steps.
    migrations: Vec<MigrationStep>,
}

/// One versioned migration step.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MigrationStep {
    /// Source configuration version; the step upgrades it to `from + 1`.
    from: u32,
    /// Operations applied in order to the raw document.
    operations: Vec<Operation>,
}

/// One declarative migration operation.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "op", deny_unknown_fields)]
enum Operation {
    /// Rename an object field.
    #[serde(rename = "rename_field")]
    Rename {
        /// Optional JSON Pointer targeting one object. Mutually exclusive with `where`.
        #[serde(default)]
        at: Option<String>,
        /// Recursively apply to objects whose listed fields equal these values.
        #[serde(default, rename = "where")]
        r#where: Option<BTreeMap<String, Value>>,
        /// Existing field name.
        from: String,
        /// New field name.
        to: String,
    },
    /// Remove an object field.
    #[serde(rename = "remove_field")]
    Remove {
        /// Optional JSON Pointer targeting one object. Mutually exclusive with `where`.
        #[serde(default)]
        at: Option<String>,
        /// Recursively apply to objects whose listed fields equal these values.
        #[serde(default, rename = "where")]
        r#where: Option<BTreeMap<String, Value>>,
        /// Field to remove.
        field: String,
    },
    /// Insert a field when missing, or overwrite it when `overwrite` is true.
    #[serde(rename = "set_field")]
    Set {
        /// Optional JSON Pointer targeting one object. Mutually exclusive with `where`.
        #[serde(default)]
        at: Option<String>,
        /// Recursively apply to objects whose listed fields equal these values.
        #[serde(default, rename = "where")]
        r#where: Option<BTreeMap<String, Value>>,
        /// Field to insert.
        field: String,
        /// Value to insert.
        value: Value,
        /// Replace an existing field instead of keeping it.
        #[serde(default)]
        overwrite: bool,
    },
    /// Replace one field value when it equals `from`.
    #[serde(rename = "rename_value")]
    RenameValue {
        /// Optional JSON Pointer targeting one object. Mutually exclusive with `where`.
        #[serde(default)]
        at: Option<String>,
        /// Recursively apply to objects whose listed fields equal these values.
        #[serde(default, rename = "where")]
        r#where: Option<BTreeMap<String, Value>>,
        /// Field whose value is matched.
        field: String,
        /// Value to replace.
        from: Value,
        /// Replacement value.
        to: Value,
    },
    /// Map several string values of one field.
    #[serde(rename = "map_values")]
    MapValues {
        /// Optional JSON Pointer targeting one object. Mutually exclusive with `where`.
        #[serde(default)]
        at: Option<String>,
        /// Recursively apply to objects whose listed fields equal these values.
        /// When neither `at` nor `where` is set, every object is visited.
        #[serde(default, rename = "where")]
        r#where: Option<BTreeMap<String, Value>>,
        /// Field whose value is mapped.
        field: String,
        /// String value to replacement JSON value.
        map: BTreeMap<String, Value>,
    },
    /// Multiply and optionally offset a numeric field.
    #[serde(rename = "scale")]
    Scale {
        /// Optional JSON Pointer targeting one object. Mutually exclusive with `where`.
        #[serde(default)]
        at: Option<String>,
        /// Recursively apply to objects whose listed fields equal these values.
        #[serde(default, rename = "where")]
        r#where: Option<BTreeMap<String, Value>>,
        /// Numeric field to scale.
        field: String,
        /// Multiplication factor.
        multiply: f64,
        /// Additive offset applied after multiplication.
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

    fn apply(&self, document: &mut Value) -> Result<OperationOutcome, ConfigError> {
        self.validate().map_err(ConfigError::new)?;
        let (at, matcher) = self.target();
        match self {
            Self::Rename { from, to, .. } => {
                apply_to_target(document, at, matcher, self.name(), |object, path| {
                    if !object.contains_key(from) {
                        return Ok(false);
                    }
                    if object.contains_key(to) {
                        return Err(ConfigError::with_path(
                            path,
                            format!("cannot rename `{from}` to `{to}`: `{to}` already exists"),
                        ));
                    }
                    let value = object.remove(from).expect("source field was checked");
                    object.insert(to.clone(), value);
                    Ok(true)
                })
            }
            Self::Remove { field, .. } => {
                apply_to_target(document, at, matcher, self.name(), |object, _| {
                    Ok(object.remove(field).is_some())
                })
            }
            Self::Set {
                field,
                value,
                overwrite,
                ..
            } => apply_to_target(document, at, matcher, self.name(), |object, _| {
                if !*overwrite && object.contains_key(field) {
                    return Ok(false);
                }
                if object.get(field) == Some(value) {
                    return Ok(false);
                }
                object.insert(field.clone(), value.clone());
                Ok(true)
            }),
            Self::RenameValue {
                field, from, to, ..
            } => apply_to_target(document, at, matcher, self.name(), |object, _| {
                if object.get(field) == Some(from) {
                    object.insert(field.clone(), to.clone());
                    return Ok(true);
                }
                Ok(false)
            }),
            Self::MapValues { field, map, .. } => {
                let mut apply = |object: &mut Map<String, Value>, _: &str| {
                    let mapped = match object.get(field) {
                        Some(Value::String(current)) => map.get(current).cloned(),
                        _ => None,
                    };
                    let Some(mapped) = mapped else {
                        return Ok(false);
                    };
                    if object.get(field) == Some(&mapped) {
                        return Ok(false);
                    }
                    object.insert(field.clone(), mapped);
                    Ok(true)
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
                    return Ok(false);
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
                if value == current {
                    return Ok(false);
                }
                object.insert(field.clone(), value);
                Ok(true)
            }),
        }
    }
}

/// Objects selected and changed by one operation in one document.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct OperationOutcome {
    matched: usize,
    changed: usize,
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

fn apply_operations(
    mut document: Value,
    operations: &[Operation],
) -> Result<(Value, Vec<OperationOutcome>), ConfigError> {
    let mut outcomes = Vec::with_capacity(operations.len());
    for (index, operation) in operations.iter().enumerate() {
        let outcome = operation.apply(&mut document).map_err(|error| {
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
        outcomes.push(outcome);
    }
    Ok((document, outcomes))
}

fn apply_to_target(
    document: &mut Value,
    at: Option<&str>,
    matcher: Option<&BTreeMap<String, Value>>,
    operation: &str,
    mut action: impl FnMut(&mut Map<String, Value>, &str) -> Result<bool, ConfigError>,
) -> Result<OperationOutcome, ConfigError> {
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
    let changed = action(object, pointer)?;
    Ok(OperationOutcome {
        matched: 1,
        changed: usize::from(changed),
    })
}

fn visit_objects(
    value: &mut Value,
    pointer: &str,
    action: &mut impl FnMut(&mut Map<String, Value>, &str) -> Result<bool, ConfigError>,
) -> Result<OperationOutcome, ConfigError> {
    let mut outcome = OperationOutcome::default();
    match value {
        Value::Object(object) => {
            outcome.matched += 1;
            if action(object, pointer)? {
                outcome.changed += 1;
            }
            for (field, child) in object.iter_mut() {
                let child_pointer = child_pointer(pointer, field);
                let child_outcome = visit_objects(child, &child_pointer, action)?;
                outcome.matched += child_outcome.matched;
                outcome.changed += child_outcome.changed;
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter_mut().enumerate() {
                let child_outcome = visit_objects(child, &format!("{pointer}/{index}"), action)?;
                outcome.matched += child_outcome.matched;
                outcome.changed += child_outcome.changed;
            }
        }
        _ => {}
    }
    Ok(outcome)
}

fn visit_matching(
    value: &mut Value,
    pointer: &str,
    matcher: &BTreeMap<String, Value>,
    action: &mut impl FnMut(&mut Map<String, Value>, &str) -> Result<bool, ConfigError>,
) -> Result<OperationOutcome, ConfigError> {
    let mut outcome = OperationOutcome::default();
    match value {
        Value::Object(object) => {
            if matcher
                .iter()
                .all(|(field, expected)| object.get(field) == Some(expected))
            {
                outcome.matched += 1;
                if action(object, pointer)? {
                    outcome.changed += 1;
                }
            }
            for (field, child) in object.iter_mut() {
                let child_pointer = child_pointer(pointer, field);
                let child_outcome = visit_matching(child, &child_pointer, matcher, action)?;
                outcome.matched += child_outcome.matched;
                outcome.changed += child_outcome.changed;
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter_mut().enumerate() {
                let child_outcome =
                    visit_matching(child, &format!("{pointer}/{index}"), matcher, action)?;
                outcome.matched += child_outcome.matched;
                outcome.changed += child_outcome.changed;
            }
        }
        _ => {}
    }
    Ok(outcome)
}

fn child_pointer(pointer: &str, field: &str) -> String {
    let field = field.replace('~', "~0").replace('/', "~1");
    format!("{pointer}/{field}")
}

fn default_manifest_version() -> u32 {
    MANIFEST_VERSION
}

#[derive(Debug, Default)]
struct OperationCounter {
    runs: AtomicUsize,
    matched: AtomicUsize,
    changed: AtomicUsize,
}

#[derive(Debug, Clone, Copy)]
struct OperationDefinition {
    from: u32,
    index: usize,
    op: &'static str,
}

#[derive(Debug, Clone, Copy)]
struct OperationSnapshot {
    runs: usize,
    matched: usize,
    changed: usize,
}

/// Loaded manifest with its migration pipeline and operation counters.
pub(crate) struct LoadedManifest {
    migrator: ConfigMigrator,
    definitions: Vec<OperationDefinition>,
    counters: Arc<Vec<OperationCounter>>,
}

impl LoadedManifest {
    fn snapshot(&self) -> Vec<OperationSnapshot> {
        self.counters
            .iter()
            .map(|counter| OperationSnapshot {
                runs: counter.runs.load(Ordering::Relaxed),
                matched: counter.matched.load(Ordering::Relaxed),
                changed: counter.changed.load(Ordering::Relaxed),
            })
            .collect()
    }
}

/// Loads a declarative migration manifest and builds its version pipeline.
pub(crate) fn load_manifest(path: &Path) -> Result<LoadedManifest, String> {
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

    for step in &manifest.migrations {
        for (index, operation) in step.operations.iter().enumerate() {
            operation.validate().map_err(|error| {
                format!(
                    "manifest migration from version {} operation {}: {error}",
                    step.from,
                    index + 1
                )
            })?;
        }
    }

    let mut definitions = Vec::new();
    for step in &manifest.migrations {
        for (index, operation) in step.operations.iter().enumerate() {
            definitions.push(OperationDefinition {
                from: step.from,
                index: index + 1,
                op: operation.name(),
            });
        }
    }
    let counters: Arc<Vec<OperationCounter>> = Arc::new(
        (0..definitions.len())
            .map(|_| OperationCounter::default())
            .collect(),
    );

    let mut migrator = ConfigMigrator::new();
    let mut next_operation = 0;
    for step in manifest.migrations {
        let from = step.from;
        let operations = Arc::new(step.operations);
        let operation_indices: Vec<usize> =
            (next_operation..next_operation + operations.len()).collect();
        next_operation += operations.len();
        let counters = Arc::clone(&counters);
        migrator
            .register(from, move |document| {
                let (document, outcomes) = apply_operations(document, &operations)?;
                for (index, outcome) in operation_indices.iter().copied().zip(outcomes) {
                    let counter = &counters[index];
                    counter.runs.fetch_add(1, Ordering::Relaxed);
                    counter
                        .matched
                        .fetch_add(outcome.matched, Ordering::Relaxed);
                    counter
                        .changed
                        .fetch_add(outcome.changed, Ordering::Relaxed);
                }
                Ok(document)
            })
            .map_err(|error| format!("manifest migration from version {from}: {error}"))?;
    }
    Ok(LoadedManifest {
        migrator,
        definitions,
        counters,
    })
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

/// One manifest operation that ran on a root's configuration graph.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct OperationReport {
    /// Source configuration version.
    pub from: u32,
    /// One-based position within the migration step.
    pub index: usize,
    /// Operation name.
    pub op: &'static str,
    /// Objects selected by the operation's `at` or `where` target, or its
    /// default scope.
    pub matched: usize,
    /// Selected objects the operation actually modified.
    pub changed: usize,
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
    /// Operations whose migration steps ran, with match and change counts.
    pub operations: Vec<OperationReport>,
}

/// Result of migrating every supplied root.
pub(crate) struct MigrationOutcome {
    pub reports: Vec<MigrationReport>,
    pub writes: Vec<PendingWrite>,
}

/// Aggregates per-root operation counters by manifest position.
pub(crate) fn summarize_operations(reports: &[MigrationReport]) -> Vec<OperationReport> {
    let mut summaries: BTreeMap<(u32, usize), OperationReport> = BTreeMap::new();
    for report in reports {
        for operation in &report.operations {
            let summary = summaries
                .entry((operation.from, operation.index))
                .or_insert(OperationReport {
                    from: operation.from,
                    index: operation.index,
                    op: operation.op,
                    matched: 0,
                    changed: 0,
                });
            summary.matched += operation.matched;
            summary.changed += operation.changed;
        }
    }
    summaries.into_values().collect()
}

/// JSON Schema for declarative migration manifests.
pub(crate) fn manifest_schema() -> Value {
    let mut schema = serde_json::to_value(schemars::schema_for!(Manifest))
        .expect("manifest schema serialization cannot fail");
    if let Some(version) = schema.pointer_mut("/properties/version") {
        version["const"] = Value::from(MANIFEST_VERSION);
        version["default"] = Value::from(MANIFEST_VERSION);
    }
    schema
}

/// Starter manifest written by `migrate --init`.
pub(crate) fn manifest_template() -> Value {
    serde_json::json!({
        "version": MANIFEST_VERSION,
        "migrations": [
            {
                "from": 0,
                "operations": []
            }
        ]
    })
}

/// Result of staging a migrated include graph into a directory.
pub(crate) struct StagedOutput {
    /// Number of written files, including the root.
    pub files: usize,
    /// Staged root inside the output directory.
    pub root: PathBuf,
}

/// Copies a migrated root and its complete include graph into `output_dir`.
///
/// Include strings are preserved, so the staged root must be loaded with
/// `output_dir` as its include base. The output directory must not exist or be
/// empty. Only call this after the migration graph validated successfully.
pub(crate) fn stage_output(
    root: &Path,
    output_dir: &Path,
    include_base: Option<&Path>,
    writes: &[PendingWrite],
) -> Result<StagedOutput, String> {
    let base = resolve_base(root, include_base)?;
    let writes: BTreeMap<&Path, &PendingWrite> = writes
        .iter()
        .map(|write| (write.path.as_path(), write))
        .collect();
    let root_source = root
        .canonicalize()
        .map_err(|error| format!("canonicalize root `{}`: {error}", display_path(root)))?;
    let root_name = root
        .file_name()
        .ok_or_else(|| String::from("root path has no file name"))?;
    let root_relative = PathBuf::from(root_name);
    let root_format = Format::from_path(&root_source)?;
    let root_bytes = final_bytes(&root_source, &writes)?;
    let root_document = parse_final(&root_source, &root_bytes, root_format)?;

    let mut state = StageState {
        base: &base,
        writes: &writes,
        files: vec![(root_relative.clone(), root_bytes)],
        destinations: BTreeMap::from([(root_relative.clone(), root_source.clone())]),
        directories: BTreeSet::new(),
        visited: HashSet::from([root_source.clone()]),
        stack: vec![root_source.clone()],
    };
    state.walk(&root_document, root_format, 0)?;
    state.stack.pop();

    let created = prepare_output(output_dir)?;
    if created && let Err(error) = fs::create_dir_all(output_dir) {
        let _ = fs::remove_dir_all(output_dir);
        return Err(format!(
            "create output directory `{}`: {error}",
            display_path(output_dir)
        ));
    }

    let mut written = Vec::with_capacity(state.files.len());
    for directory in &state.directories {
        let path = output_dir.join(directory);
        if let Err(error) = fs::create_dir_all(&path) {
            cleanup_output(output_dir, &written, created);
            return Err(format!(
                "create output directory `{}`: {error}",
                display_path(&path)
            ));
        }
    }
    for (relative, bytes) in &state.files {
        let path = output_dir.join(relative);
        written.push(path.clone());
        if let Some(parent) = path.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            cleanup_output(output_dir, &written, created);
            return Err(format!(
                "create output directory `{}`: {error}",
                display_path(parent)
            ));
        }
        if let Err(error) = fs::write(&path, bytes) {
            cleanup_output(output_dir, &written, created);
            return Err(format!("write `{}`: {error}", display_path(&path)));
        }
    }

    Ok(StagedOutput {
        files: written.len(),
        root: output_dir.join(root_relative),
    })
}

struct StageState<'a> {
    base: &'a Path,
    writes: &'a BTreeMap<&'a Path, &'a PendingWrite>,
    files: Vec<(PathBuf, Vec<u8>)>,
    destinations: BTreeMap<PathBuf, PathBuf>,
    directories: BTreeSet<PathBuf>,
    visited: HashSet<PathBuf>,
    stack: Vec<PathBuf>,
}

impl StageState<'_> {
    fn walk(&mut self, document: &Value, format: Format, depth: usize) -> Result<(), String> {
        for (location, include) in collect_includes(document) {
            if depth >= MAX_INCLUDE_DEPTH {
                return Err(format!(
                    "include at `{location}`: nesting exceeds {MAX_INCLUDE_DEPTH} levels"
                ));
            }
            let normalized = relative_include_path(&include)
                .map_err(|error| format!("include at `{location}`: {error}"))?;
            let (target, target_format) = resolve_include(self.base, &include, format)
                .map_err(|error| format!("include at `{location}`: {error}"))?;
            if self.stack.contains(&target) {
                return Err(format!(
                    "include at `{location}`: cycle through `{}`",
                    display_path(&target)
                ));
            }
            let destination = PathBuf::from(&include);
            self.directories.extend(parent_directories(&destination));
            if let Some(existing) = self.destinations.get(&normalized) {
                if existing != &target {
                    return Err(format!(
                        "include at `{location}`: `{include}` conflicts with another staged file"
                    ));
                }
                continue;
            }
            self.destinations.insert(normalized, target.clone());
            let bytes = final_bytes(&target, self.writes)?;
            let document = parse_final(&target, &bytes, target_format)?;
            self.files.push((destination, bytes));
            if !self.visited.insert(target.clone()) {
                continue;
            }
            self.stack.push(target.clone());
            self.walk(&document, target_format, depth + 1)?;
            self.stack.pop();
        }
        Ok(())
    }
}

fn final_bytes(source: &Path, writes: &BTreeMap<&Path, &PendingWrite>) -> Result<Vec<u8>, String> {
    match writes.get(source) {
        Some(write) => Ok(write.formatted.clone()),
        None => {
            fs::read(source).map_err(|error| format!("read `{}`: {error}", display_path(source)))
        }
    }
}

fn parse_final(source: &Path, bytes: &[u8], format: Format) -> Result<Value, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("`{}` is not UTF-8: {error}", display_path(source)))?;
    format
        .parse_document(text)
        .map_err(|error| format!("parse migrated `{}`: {error}", display_path(source)))
}

fn parent_directories(path: &Path) -> Vec<PathBuf> {
    let components: Vec<_> = path.components().collect();
    let mut directories = Vec::new();
    let mut current = PathBuf::new();
    for (index, component) in components.iter().enumerate() {
        match component {
            std::path::Component::Normal(part) => {
                current.push(part);
                if index + 1 < components.len() {
                    directories.push(current.clone());
                }
            }
            std::path::Component::ParentDir => {
                current.pop();
            }
            std::path::Component::CurDir => {}
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {}
        }
    }
    directories
}

/// Migrates roots and their include graphs, validating every final document.
pub(crate) fn migrate_files(
    manifest: &LoadedManifest,
    schema: &Value,
    roots: &[PathBuf],
    include_base: Option<&Path>,
) -> MigrationOutcome {
    let mut writes = BTreeMap::new();
    let mut reports = Vec::with_capacity(roots.len());

    for root in roots {
        let before = manifest.snapshot();
        let mut state = RootState {
            base: PathBuf::new(),
            manifest,
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
        let operations = operation_reports(&manifest.definitions, &before, &manifest.snapshot());
        reports.push(MigrationReport {
            file: display_path(root),
            valid,
            issues: state.issues,
            migrated: state.migrated.into_iter().collect(),
            changes: state.changes,
            operations,
        });
    }

    MigrationOutcome {
        reports,
        writes: writes.into_values().collect(),
    }
}

fn operation_reports(
    definitions: &[OperationDefinition],
    before: &[OperationSnapshot],
    after: &[OperationSnapshot],
) -> Vec<OperationReport> {
    definitions
        .iter()
        .zip(before)
        .zip(after)
        .filter_map(|((definition, before), after)| {
            let runs = after.runs.saturating_sub(before.runs);
            (runs > 0).then(|| OperationReport {
                from: definition.from,
                index: definition.index,
                op: definition.op,
                matched: after.matched.saturating_sub(before.matched),
                changed: after.changed.saturating_sub(before.changed),
            })
        })
        .collect()
}

struct RootState<'a> {
    base: PathBuf,
    manifest: &'a LoadedManifest,
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
        let migrated = match self.manifest.migrator.upgrade(document) {
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
        let (migrated, _) = apply_operations(document, &[rename_root()]).expect("rename");
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
        let (migrated, _) = apply_operations(document, &operations).expect("recursive ops");
        assert_eq!(migrated["root"]["children"][0]["duration_ms"], 1);
        assert!(migrated["root"]["children"][0].get("seconds").is_none());
        assert!(migrated["root"]["children"][0].get("legacy").is_none());
        assert_eq!(migrated["root"]["children"][1]["seconds"], 2);
    }

    #[test]
    fn counts_matched_and_changed_objects() {
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
            Operation::Set {
                at: None,
                r#where: Some(BTreeMap::from([(
                    String::from("type"),
                    Value::String(String::from("action")),
                )])),
                field: String::from("name"),
                value: json!("anything"),
                overwrite: false,
            },
        ];
        let document = json!({
            "root": {
                "type": "sequence",
                "children": [
                    {"type": "wait", "seconds": 1},
                    {"type": "wait", "seconds": 2},
                    {"type": "action", "name": "anything"},
                    {"type": "action", "name": "other"}
                ]
            }
        });
        let (_, outcomes) = apply_operations(document, &operations).expect("counted ops");
        assert_eq!(
            outcomes[0],
            OperationOutcome {
                matched: 2,
                changed: 2
            }
        );
        assert_eq!(
            outcomes[1],
            OperationOutcome {
                matched: 2,
                changed: 0
            }
        );
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
        let (migrated, _) = apply_operations(document, &[default, overwrite]).expect("set fields");
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
        let (migrated, _) = apply_operations(document, &[operation]).expect("pointer op");
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
        let (migrated, _) = apply_operations(document, &operations).expect("value and scale ops");
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
        let (migrated, outcomes) = apply_operations(document, &[operation]).expect("map values");
        assert_eq!(migrated["root"]["children"][0]["type"], "wait");
        assert_eq!(migrated["root"]["children"][1]["type"], "action");
        assert_eq!(migrated["root"]["children"][2]["child"]["type"], "wait");
        assert_eq!(
            outcomes[0],
            OperationOutcome {
                matched: 6,
                changed: 3
            }
        );
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
