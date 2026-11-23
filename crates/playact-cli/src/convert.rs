//! Recursive conversion of a configuration and its include graph.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use playact::config::TreeSpec;
use serde_json::Value;

use crate::format::Format;
use crate::lint::{display_path, resolve_base, resolve_include};

/// Result of a recursive conversion.
pub(crate) struct ConversionOutcome {
    /// Number of written files, including the root.
    pub files: usize,
    /// Converted root inside the output directory.
    pub root: PathBuf,
}

struct PlannedFile {
    source: PathBuf,
    relative: PathBuf,
    format: Format,
}

/// Converts `input` and every included file into `output_dir` as `target`.
///
/// Include strings are rewritten to the converted relative paths. `output_dir`
/// must not exist or must be empty; it becomes the include base for the
/// converted root.
pub(crate) fn convert_recursive(
    input: &Path,
    output_dir: &Path,
    target: Format,
    included: &[PathBuf],
    include_base: Option<&Path>,
) -> Result<ConversionOutcome, String> {
    let base = resolve_base(input, include_base)?;
    let root_source = input
        .canonicalize()
        .map_err(|error| format!("canonicalize input `{}`: {error}", display_path(input)))?;
    let root_relative = root_relative(&root_source, input, &base, target)?;

    let mut plans = BTreeMap::new();
    let mut destinations = BTreeMap::new();
    add_plan(
        &mut plans,
        &mut destinations,
        root_source,
        root_relative.clone(),
        target,
    )?;
    for source in included {
        let relative = source
            .strip_prefix(&base)
            .map_err(|_| {
                format!(
                    "include `{}` is outside the include base",
                    display_path(source)
                )
            })?
            .to_path_buf();
        add_plan(
            &mut plans,
            &mut destinations,
            source.clone(),
            relative,
            target,
        )?;
    }
    validate_destinations(&plans)?;

    let mut pending = Vec::with_capacity(plans.len());
    for plan in plans.values() {
        let text = fs::read_to_string(&plan.source)
            .map_err(|error| format!("read `{}`: {error}", display_path(&plan.source)))?;
        let mut document = plan.format.parse_document(&text).map_err(|error| {
            format!(
                "parse `{}` as {}: {error}",
                display_path(&plan.source),
                plan.format.name()
            )
        })?;
        rewrite_includes(&mut document, &base, plan.format, &plans).map_err(|error| {
            format!(
                "rewrite includes in `{}`: {error}",
                display_path(&plan.source)
            )
        })?;
        let spec: TreeSpec = serde_json::from_value(document)
            .map_err(|error| format!("convert `{}`: {error}", display_path(&plan.source)))?;
        let bytes = target.serialize_spec(&spec).map_err(|error| {
            format!(
                "serialize `{}` as {}: {error}",
                display_path(&plan.source),
                target.name()
            )
        })?;
        pending.push((plan.relative.clone(), bytes));
    }

    let created = prepare_output(output_dir)?;
    if created && let Err(error) = fs::create_dir_all(output_dir) {
        let _ = fs::remove_dir_all(output_dir);
        return Err(format!(
            "create output directory `{}`: {error}",
            display_path(output_dir)
        ));
    }

    let mut written = Vec::with_capacity(pending.len());
    for (relative, bytes) in pending {
        let path = output_dir.join(&relative);
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

    Ok(ConversionOutcome {
        files: written.len(),
        root: output_dir.join(root_relative),
    })
}

fn root_relative(
    root_source: &Path,
    input: &Path,
    base: &Path,
    target: Format,
) -> Result<PathBuf, String> {
    let relative = match root_source.strip_prefix(base) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.to_path_buf(),
        _ => {
            let name = input
                .file_name()
                .ok_or_else(|| String::from("input path has no file name"))?;
            PathBuf::from(name)
        }
    };
    Ok(with_extension(relative, target))
}

fn add_plan(
    plans: &mut BTreeMap<PathBuf, PlannedFile>,
    destinations: &mut BTreeMap<String, PathBuf>,
    source: PathBuf,
    relative: PathBuf,
    target: Format,
) -> Result<(), String> {
    let relative = with_extension(relative, target);
    let key = destination_key(&relative);
    if let Some(existing) = destinations.get(&key) {
        if existing != &source {
            return Err(format!(
                "conversion maps `{}` and `{}` to `{}`",
                display_path(existing),
                display_path(&source),
                include_path(&relative)
            ));
        }
    } else {
        destinations.insert(key, source.clone());
    }
    if let Some(existing) = plans.get(&source) {
        if existing.relative != relative {
            return Err(format!(
                "`{}` would be converted to both `{}` and `{}`",
                display_path(&source),
                include_path(&existing.relative),
                include_path(&relative)
            ));
        }
        return Ok(());
    }
    let format = Format::from_path(&source).map_err(|error| {
        format!(
            "cannot convert `{}` as a source format: {error}",
            display_path(&source)
        )
    })?;
    plans.insert(
        source.clone(),
        PlannedFile {
            source,
            relative,
            format,
        },
    );
    Ok(())
}

fn with_extension(mut path: PathBuf, target: Format) -> PathBuf {
    path.set_extension(target.extension());
    path
}

fn destination_key(path: &Path) -> String {
    let key = include_path(path);
    if cfg!(any(windows, target_os = "macos")) {
        key.to_lowercase()
    } else {
        key
    }
}

fn validate_destinations(plans: &BTreeMap<PathBuf, PlannedFile>) -> Result<(), String> {
    let destinations: BTreeSet<String> = plans
        .values()
        .map(|plan| destination_key(&plan.relative))
        .collect();
    for plan in plans.values() {
        let mut ancestor = plan.relative.parent();
        while let Some(path) = ancestor {
            if !path.as_os_str().is_empty() && destinations.contains(&destination_key(path)) {
                return Err(format!(
                    "conversion needs `{}` as both a file and a directory",
                    include_path(path)
                ));
            }
            ancestor = path.parent();
        }
    }
    Ok(())
}

fn include_path(path: &Path) -> String {
    path.iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn rewrite_includes(
    value: &mut Value,
    base: &Path,
    source_format: Format,
    plans: &BTreeMap<PathBuf, PlannedFile>,
) -> Result<(), String> {
    match value {
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("include") {
                let path = object
                    .get_mut("path")
                    .ok_or_else(|| String::from("include node has no path"))?;
                let include = path
                    .as_str()
                    .ok_or_else(|| String::from("include path must be a string"))?
                    .to_owned();
                let (source, _) = resolve_include(base, &include, source_format)?;
                let plan = plans.get(&source).ok_or_else(|| {
                    format!("include `{include}` is not part of the conversion plan")
                })?;
                *path = Value::String(include_path(&plan.relative));
            }
            for child in object.values_mut() {
                rewrite_includes(child, base, source_format, plans)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                rewrite_includes(child, base, source_format, plans)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn prepare_output(output_dir: &Path) -> Result<bool, String> {
    if output_dir.exists() {
        if !output_dir.is_dir() {
            return Err(format!(
                "output path `{}` is not a directory",
                display_path(output_dir)
            ));
        }
        if fs::read_dir(output_dir)
            .map_err(|error| {
                format!(
                    "read output directory `{}`: {error}",
                    display_path(output_dir)
                )
            })?
            .next()
            .is_some()
        {
            return Err(format!(
                "output directory `{}` is not empty",
                display_path(output_dir)
            ));
        }
        Ok(false)
    } else {
        Ok(true)
    }
}

pub(crate) fn cleanup_output(output_dir: &Path, written: &[PathBuf], created: bool) {
    for path in written.iter().rev() {
        let _ = fs::remove_file(path);
    }
    if created {
        let _ = fs::remove_dir_all(output_dir);
        return;
    }
    let mut directories: Vec<PathBuf> = written
        .iter()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect();
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    directories.dedup();
    for directory in directories {
        if directory != output_dir {
            let _ = fs::remove_dir(&directory);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{cleanup_output, include_path};
    use std::fs;
    use std::path::Path;

    #[test]
    fn include_paths_use_forward_slashes() {
        assert_eq!(
            include_path(&Path::new("shared").join("child.yaml")),
            "shared/child.yaml"
        );
    }

    #[test]
    fn cleanup_restores_the_output_directory_state() {
        let root = std::env::temp_dir().join(format!(
            "playact-cli-convert-cleanup-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let output = root.join("out");
        fs::create_dir_all(output.join("shared")).expect("create output directory");
        let nested = output.join("shared").join("child.yaml");
        let flat = output.join("root.yaml");
        fs::write(&nested, "child").expect("write nested file");
        fs::write(&flat, "root").expect("write flat file");

        cleanup_output(&output, &[nested.clone(), flat.clone()], false);
        assert!(!nested.exists());
        assert!(!flat.exists());
        assert!(output.is_dir());
        assert!(!output.join("shared").exists());

        cleanup_output(&output, &[], true);
        assert!(!output.exists());
        fs::remove_dir_all(&root).expect("remove test directory");
    }
}
