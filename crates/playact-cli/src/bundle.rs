//! Copies a configuration and its include graph into an output directory.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use playact::config::MAX_INCLUDE_DEPTH;

use crate::format::Format;
use crate::lint::{collect_includes, display_path, resolve_base, resolve_include};

/// Result of bundling a configuration.
pub(crate) struct BundleOutcome {
    pub files: usize,
    pub root: PathBuf,
}

/// Copies `input` and every included file into `output_dir`.
///
/// Include strings are preserved, so the bundle is resolved against
/// `output_dir` (or an explicit base pointing there). Include paths must be
/// relative and lexically stay inside the include base.
pub(crate) fn bundle(
    input: &Path,
    output_dir: &Path,
    include_base: Option<&Path>,
) -> Result<BundleOutcome, String> {
    let base = resolve_base(input, include_base)?;
    let output_absolute = canonicalize_future(output_dir)?;
    if output_absolute.starts_with(&base) {
        return Err(String::from(
            "output directory must be outside the include base",
        ));
    }

    let root_name = input
        .file_name()
        .ok_or_else(|| String::from("input path has no file name"))?
        .to_os_string();
    let created = !output_dir.exists();
    if output_dir.exists() {
        if !output_dir.is_dir() {
            return Err(format!(
                "output path `{}` is not a directory",
                output_dir.display()
            ));
        }
        if fs::read_dir(output_dir)
            .map_err(|error| format!("read output directory: {error}"))?
            .next()
            .is_some()
        {
            return Err(format!(
                "output directory `{}` is not empty",
                output_dir.display()
            ));
        }
    } else {
        fs::create_dir_all(output_dir)
            .map_err(|error| format!("create output directory: {error}"))?;
    }

    let result = bundle_into(input, output_dir, &base, root_name);
    if result.is_err() && created {
        let _ = fs::remove_dir_all(output_dir);
    }
    result
}

fn bundle_into(
    input: &Path,
    output_dir: &Path,
    base: &Path,
    root_name: std::ffi::OsString,
) -> Result<BundleOutcome, String> {
    let format = Format::from_path(input)?;
    let text = fs::read_to_string(input).map_err(|error| format!("read input: {error}"))?;
    let document = format
        .parse_document(&text)
        .map_err(|error| format!("parse input: {error}"))?;

    let root = output_dir.join(&root_name);
    fs::copy(input, &root).map_err(|error| format!("copy input: {error}"))?;

    let root_source = input
        .canonicalize()
        .map_err(|error| format!("canonicalize input `{}`: {error}", display_path(input)))?;
    let mut destinations = HashMap::new();
    destinations.insert(PathBuf::from(root_name), root_source.clone());
    let mut visited_sources = HashSet::new();
    visited_sources.insert(root_source.clone());

    let mut bundler = Bundler {
        base,
        output: output_dir,
        destinations,
        visited_sources,
        stack: vec![root_source],
        files: 1,
    };
    bundler.walk(&document, format, 0)?;

    Ok(BundleOutcome {
        files: bundler.files,
        root,
    })
}

struct Bundler<'a> {
    base: &'a Path,
    output: &'a Path,
    destinations: HashMap<PathBuf, PathBuf>,
    visited_sources: HashSet<PathBuf>,
    stack: Vec<PathBuf>,
    files: usize,
}

impl Bundler<'_> {
    fn walk(
        &mut self,
        document: &serde_json::Value,
        format: Format,
        depth: usize,
    ) -> Result<(), String> {
        for (location, include) in collect_includes(document) {
            if depth >= MAX_INCLUDE_DEPTH {
                return Err(format!(
                    "include at `{location}`: nesting exceeds {MAX_INCLUDE_DEPTH} levels"
                ));
            }
            let relative = relative_include_path(&include)?;
            let (source, format) = resolve_include(self.base, &include, format)?;
            if self.stack.contains(&source) {
                return Err(format!(
                    "include at `{location}`: cycle through `{}`",
                    display_path(&source)
                ));
            }

            if let Some(existing) = self.destinations.get(&relative) {
                if existing != &source {
                    return Err(format!(
                        "include at `{location}`: `{include}` conflicts with another bundled file"
                    ));
                }
                ensure_parents(self.output, &include)?;
                continue;
            }
            self.destinations.insert(relative, source.clone());
            ensure_parents(self.output, &include)?;
            let destination = self.output.join(&include);
            fs::copy(&source, &destination).map_err(|error| {
                format!(
                    "copy include `{}` to `{}`: {error}",
                    display_path(&source),
                    display_path(&destination)
                )
            })?;
            self.files += 1;

            if !self.visited_sources.insert(source.clone()) {
                continue;
            }
            let text = fs::read_to_string(&source)
                .map_err(|error| format!("read include `{}`: {error}", display_path(&source)))?;
            let included_document = format
                .parse_document(&text)
                .map_err(|error| format!("parse include `{}`: {error}", display_path(&source)))?;
            self.stack.push(source);
            self.walk(&included_document, format, depth + 1)?;
            self.stack.pop();
        }
        Ok(())
    }
}

pub(crate) fn relative_include_path(include: &str) -> Result<PathBuf, String> {
    if include.trim().is_empty() {
        return Err(String::from("include path is empty"));
    }
    let mut parts = Vec::new();
    for component in Path::new(include).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(format!("include `{include}` escapes the include base"));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "include `{include}` must be a relative path for bundling"
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(format!("include `{include}` has no file name"));
    }
    let mut normalized = PathBuf::new();
    for part in parts {
        normalized.push(part);
    }
    Ok(normalized)
}

fn ensure_parents(output: &Path, include: &str) -> Result<(), String> {
    let mut current = output.to_path_buf();
    let mut components = Path::new(include).components().peekable();
    while let Some(component) = components.next() {
        match component {
            Component::Normal(part) => {
                current.push(part);
                if components.peek().is_some() {
                    fs::create_dir_all(&current).map_err(|error| {
                        format!(
                            "create include directory `{}`: {error}",
                            display_path(&current)
                        )
                    })?;
                }
            }
            Component::CurDir => {}
            Component::ParentDir => {
                current.pop();
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "include `{include}` must be a relative path for bundling"
                ));
            }
        }
    }
    Ok(())
}

fn canonicalize_future(path: &Path) -> Result<PathBuf, String> {
    let mut suffix = Vec::new();
    let mut current = path;
    while !current.exists() {
        let name = current
            .file_name()
            .ok_or_else(|| format!("cannot resolve output path `{}`", path.display()))?;
        suffix.push(name.to_os_string());
        current = current
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
    }
    let mut resolved = current
        .canonicalize()
        .map_err(|error| format!("resolve output path `{}`: {error}", path.display()))?;
    for part in suffix.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}
