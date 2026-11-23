//! Preflight a tree configuration with schema validation, then build it.
//!
//! Run:
//!
//! ```text
//! cargo run --example config_lint --features config,schema
//! cargo run --example config_lint --features config,schema -- config/tree.json
//! ```
//!
//! Without an argument the example checks `examples/battle_ai.json`. The demo
//! registry matches that file; replace [`build_registry`] with your own
//! registrations to lint other configurations.
//!
//! Schema validation runs first with instance paths; building then reports the
//! precise configuration path for failures such as unknown names, bad
//! parameters, or duplicate IDs. The process exits nonzero on any failure,
//! which makes it usable as a deployment preflight.

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use serde::Deserialize;
use serde_json::Value;

use playact::config::{NodeCategory, NodeRegistry, TreeSpec};
use playact::prelude::*;

// The fields are read through the derived `JsonSchema` and `Deserialize` impls.
#[allow(dead_code)]
#[derive(Debug, Deserialize, playact::schemars::JsonSchema)]
struct HealParams {
    /// Healing amount.
    amount: f32,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, playact::schemars::JsonSchema)]
struct HpBelowParams {
    /// Health ratio threshold.
    ratio: f32,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, playact::schemars::JsonSchema)]
struct MultiStrikeParams {
    /// Damage per hit.
    damage: f32,
    /// Number of hits.
    hits: u32,
}

fn build_registry() -> Result<NodeRegistry<()>, Box<dyn Error>> {
    let mut registry = NodeRegistry::<()>::new();

    registry.action::<HealParams>("heal", |_| |_: &mut Context<'_, ()>| Status::Success)?;
    registry.action::<()>("wait", |_| |_: &mut Context<'_, ()>| Status::Success)?;
    registry.condition::<HpBelowParams>("hp_below", |_| |_: &Context<'_, ()>| true)?;
    registry.condition::<()>("enemy_alive", |_| |_: &Context<'_, ()>| true)?;
    registry.custom::<MultiStrikeParams>(
        "multi_strike",
        |_params: MultiStrikeParams, children: Vec<BoxedNode<()>>| {
            Ok(Box::new(Selector::new(children)))
        },
    )?;

    registry.describe_params::<HealParams>(NodeCategory::Action, "heal", "Restore health.")?;
    registry.describe(NodeCategory::Action, "wait", "Wait for the next tick.")?;
    registry.describe_params::<HpBelowParams>(
        NodeCategory::Condition,
        "hp_below",
        "Check the health ratio.",
    )?;
    registry.describe(
        NodeCategory::Condition,
        "enemy_alive",
        "Check whether the enemy survives.",
    )?;
    registry.describe_params::<MultiStrikeParams>(
        NodeCategory::Custom,
        "multi_strike",
        "Strike several times.",
    )?;

    Ok(registry)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("config_lint: {error}");
            ExitCode::FAILURE
        }
    }
}

fn schema_message(error: &jsonschema::ValidationError<'_>) -> String {
    match error.kind() {
        jsonschema::error::ValidationErrorKind::OneOfNotValid { .. }
        | jsonschema::error::ValidationErrorKind::OneOfMultipleValid { .. } => {
            String::from("configuration does not match any known node shape")
        }
        _ => error.to_string(),
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let path = env::args_os().nth(1).map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("examples")
                .join("battle_ai.json")
        },
        PathBuf::from,
    );
    let text = fs::read_to_string(&path)?;
    let document: Value = serde_json::from_str(&text)?;

    let registry = build_registry()?;
    let schema = registry.json_schema();
    let validator = jsonschema::validator_for(&schema)?;
    let errors: Vec<_> = validator.iter_errors(&document).collect();
    let schema_failed = !errors.is_empty();
    for error in &errors {
        eprintln!(
            "schema error at `{}`: {}",
            error.instance_path(),
            schema_message(error)
        );
    }

    let spec = TreeSpec::from_json_str(&text)?;
    match spec.build(&registry) {
        Ok(tree) if !schema_failed => {
            println!("{}: valid `{}` tree", path.display(), tree.name());
            Ok(())
        }
        Ok(_) => Err(format!("{} does not match the registry schema", path.display()).into()),
        Err(error) => {
            eprintln!("build error: {error}");
            Err(format!("{} is not a valid tree", path.display()).into())
        }
    }
}
