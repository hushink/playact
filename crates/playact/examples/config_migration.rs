//! Version migration example: upgrade legacy documents before parsing.
//!
//! Run:
//!
//! ```text
//! cargo run --example config_migration --features config
//! ```
//!
//! The checked-in v0 configurations store their tree under `tree`. The
//! application-supplied migration renames that field to `root`; the pipeline
//! stamps the resulting version, and the migrated root and include are then
//! parsed and built as ordinary v1 specs.

use std::path::Path;

use playact::config::{ConfigError, ConfigMigrator, FsIncludeResolver, NodeRegistry};
use playact::prelude::*;
use serde_json::Value;

fn legacy_migrator() -> ConfigMigrator {
    ConfigMigrator::new()
        .with_migration(0, |mut document: Value| {
            let object = document
                .as_object_mut()
                .ok_or_else(|| ConfigError::new("legacy document must be an object"))?;
            let tree = object
                .remove("tree")
                .ok_or_else(|| ConfigError::with_path("tree", "legacy document has no tree"))?;
            object.insert(String::from("root"), tree);
            Ok(document)
        })
        .expect("migration registration")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let migrator = legacy_migrator();
    let spec = migrator.parse_json_str(include_str!("config_migration/root.json"))?;
    println!("Upgraded the root document to version {}.", spec.version);

    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/config_migration");
    let resolver = FsIncludeResolver::auto_with_migrator(&base, migrator)?;
    let registry = NodeRegistry::<()>::new();
    let mut tree = spec.build_with_resolver(&registry, &resolver)?;
    println!("Migrated tree:\n{}\n", tree.describe());

    for (now_ms, delta_ms) in [(0, 0), (250, 250), (1000, 750)] {
        let status = tree.tick(&mut (), Time::from_millis(now_ms, delta_ms));
        println!("t={now_ms}ms -> {status:?}");
    }
    Ok(())
}
