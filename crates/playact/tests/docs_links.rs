//! Checks that relative Markdown links in the crate documentation resolve.
//!
//! External URLs and pure anchors are ignored. Fenced code blocks are skipped so
//! JSON or Rust examples cannot be mistaken for Markdown links.

use std::fs;
use std::path::{Path, PathBuf};

fn markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut files = vec![root.join("README.md"), root.join("README.zh-CN.md")];
    let docs = root.join("docs");
    for entry in fs::read_dir(&docs).expect("read docs directory") {
        let path = entry.expect("docs entry").path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
            files.push(path);
        }
    }
    files.sort();
    files
}

fn relative_links(line: &str) -> Vec<&str> {
    let mut links = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find("](") {
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else {
            break;
        };
        links.push(&after[..end]);
        rest = &after[end + 1..];
    }
    links
}

fn local_target(raw: &str) -> Option<&str> {
    let raw = raw.trim();
    let raw = raw
        .strip_prefix('<')
        .and_then(|value| value.split_once('>').map(|(path, _)| path))
        .unwrap_or(raw);
    let target = raw.split_whitespace().next()?;
    let target = target.split(['#', '?']).next().unwrap_or(target);
    if target.is_empty()
        || target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
    {
        return None;
    }
    Some(target)
}

#[test]
fn local_markdown_links_exist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut failures = Vec::new();

    for file in markdown_files(root) {
        let text = fs::read_to_string(&file).expect("read markdown");
        let mut fence: Option<&'static str> = None;

        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if let Some(marker) = fence {
                if trimmed.starts_with(marker) {
                    fence = None;
                }
                continue;
            }
            if trimmed.starts_with("```") {
                fence = Some("```");
                continue;
            }
            if trimmed.starts_with("~~~") {
                fence = Some("~~~");
                continue;
            }

            for raw in relative_links(line) {
                let Some(target) = local_target(raw) else {
                    continue;
                };
                let resolved = if let Some(stripped) = target.strip_prefix('/') {
                    root.join(stripped)
                } else {
                    file.parent().expect("file parent").join(target)
                };
                if !resolved.exists() {
                    failures.push(format!(
                        "{}:{}: missing `{}` (resolved to {})",
                        file.display(),
                        index + 1,
                        target,
                        resolved.display()
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "broken local documentation links:\n{}",
        failures.join("\n")
    );
}
