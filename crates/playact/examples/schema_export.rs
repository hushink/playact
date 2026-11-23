//! Export a registry-aware configuration schema for editor completion.
//!
//! Run:
//!
//! ```text
//! cargo run --example schema_export --features config,schema
//! cargo run --example schema_export --features config,schema -- config/schema.json docs/catalog.md
//! ```
//!
//! The first argument is the schema path; the default is
//! `target/playact-schema.json`. The second argument is the Markdown catalog
//! path; the default is `target/playact-catalog.md`.
//!
//! Point your editor at the schema so action, condition, predicate, score, key,
//! custom, and subtree fields complete from this registry instead of from
//! free-form strings. Descriptions, parameter schemas, and key value schemas
//! added with `describe`, `describe_params`, and `describe_*_key` become hover
//! text and validation constraints. The catalog renders the same metadata for
//! team documentation.

use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use playact::config::{NodeCategory, NodeRegistry};
use playact::prelude::*;

static MODE: Key<String> = Key::new("demo.mode");
static MORALE: Key<u32> = Key::new("demo.morale");
static ALARM: EventKey<()> = EventKey::new("demo.alarm");

// The fields are read through the derived `JsonSchema` and `Deserialize` impls.
#[allow(dead_code)]
#[derive(Debug, Deserialize, playact::schemars::JsonSchema)]
struct PatrolParams {
    /// Waypoints visited before the loop restarts.
    waypoints: u32,
    /// Movement speed multiplier.
    speed: f32,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut registry = NodeRegistry::<()>::new();

    registry.action::<PatrolParams>("patrol", |_| |_: &mut Context<'_, ()>| Status::Success)?;
    registry.action::<()>("attack", |_| |_: &mut Context<'_, ()>| Status::Success)?;
    registry.condition::<()>("always", |_| |_: &Context<'_, ()>| true)?;
    registry.predicate::<()>("enemy_visible", |_| |_: &Context<'_, ()>| true)?;
    registry.score::<()>("low_morale", |_| |_: &Context<'_, ()>| 0.2)?;
    registry.try_action::<(), &'static str>("open_chest", |_| {
        |_: &mut Context<'_, ()>| Err("locked")
    })?;

    registry.switch_key(MODE)?;
    registry.set_value_key(MORALE)?;
    registry.event_key(ALARM)?;

    registry.custom::<()>(
        "first_success",
        |_params: (), children: Vec<BoxedNode<()>>| Ok(Box::new(Selector::new(children))),
    )?;
    registry.subtree(
        "patrol_loop",
        SubtreeFactory::new(|| {
            BehaviorTreeBuilder::<()>::new("patrol_loop")
                .action("patrol", |_| Status::Success)
                .build()
                .expect("valid patrol subtree")
                .into_root()
        }),
    )?;

    registry.describe_params::<PatrolParams>(
        NodeCategory::Action,
        "patrol",
        "Walk the patrol route and report completion.",
    )?;
    registry.describe(NodeCategory::Action, "attack", "Strike the current target.")?;
    registry.describe(
        NodeCategory::Condition,
        "always",
        "Always succeeds; useful as a fallback.",
    )?;
    registry.describe(
        NodeCategory::Predicate,
        "enemy_visible",
        "True while a target is in sight.",
    )?;
    registry.describe(
        NodeCategory::Score,
        "low_morale",
        "Prefers this branch when morale is low.",
    )?;
    registry.describe(
        NodeCategory::Custom,
        "first_success",
        "Tries children in order.",
    )?;
    registry.describe(
        NodeCategory::Subtree,
        "patrol_loop",
        "Reusable patrol loop.",
    )?;
    registry.describe_switch_key::<String>(MODE, "Current game mode.")?;
    registry.describe_set_value_key::<u32>(MORALE, "Morale written to the blackboard.")?;
    registry.describe(
        NodeCategory::EventKey,
        "demo.alarm",
        "Raised when the alarm fires.",
    )?;

    let output = env::args_os().nth(1).map_or_else(
        || PathBuf::from("target/playact-schema.json"),
        PathBuf::from,
    );
    let catalog_output = env::args_os()
        .nth(2)
        .map_or_else(|| PathBuf::from("target/playact-catalog.md"), PathBuf::from);

    let schema = registry.json_schema();
    write(&output, &serde_json::to_vec_pretty(&schema)?)?;
    write(&catalog_output, registry.markdown_catalog().as_bytes())?;

    println!("wrote {}", output.display());
    println!("wrote {}", catalog_output.display());
    println!("registered names, descriptions, params, and key values are available for editors");
    Ok(())
}

fn write(path: &Path, contents: &[u8]) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}
