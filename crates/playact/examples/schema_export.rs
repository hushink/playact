//! Export a registry-aware configuration schema for editor completion.
//!
//! Run:
//!
//! ```text
//! cargo run --example schema_export --features config,schema
//! cargo run --example schema_export --features config,schema -- config/schema.json
//! ```
//!
//! The optional argument is the output path; the default is
//! `target/playact-schema.json`. Point your editor at the generated file so
//! that action, condition, predicate, score, key, custom, and subtree fields
//! complete from this registry instead of from free-form strings.

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

use playact::config::NodeRegistry;
use playact::prelude::*;

static MODE: Key<String> = Key::new("demo.mode");
static MORALE: Key<u32> = Key::new("demo.morale");
static ALARM: EventKey<()> = EventKey::new("demo.alarm");

fn main() -> Result<(), Box<dyn Error>> {
    let mut registry = NodeRegistry::<()>::new();

    registry.action::<()>("patrol", |_| |_: &mut Context<'_, ()>| Status::Success)?;
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

    let output = env::args_os().nth(1).map_or_else(
        || PathBuf::from("target/playact-schema.json"),
        PathBuf::from,
    );
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }

    let schema = registry.json_schema();
    fs::write(&output, serde_json::to_vec_pretty(&schema)?)?;

    println!("wrote {}", output.display());
    println!("registered names are now available as enum values for editors");
    Ok(())
}
