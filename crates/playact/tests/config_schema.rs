#![cfg(all(feature = "config", feature = "schema"))]

//! Validates shipped example configurations against the registry schema.

use std::fs;
use std::path::Path;

use playact::config::{NodeCategory, NodeRegistry};
use playact::prelude::*;
use serde_json::{Value, json};

static MODE: Key<String> = Key::new("test.config_schema.mode");
static TARGET_VISIBLE: Key<bool> = Key::new("showcase.target_visible");
static PATROL_PLAN: Key<Vec<String>> = Key::new("showcase.patrol_plan");
static SCAN_PULSE: EventKey<()> = EventKey::new("showcase.scan_pulse");

#[allow(dead_code)]
#[derive(Debug, serde::Deserialize, playact::schemars::JsonSchema)]
struct FastParams {
    /// Speed multiplier.
    speed: f32,
}

fn registry() -> NodeRegistry<()> {
    let mut registry = NodeRegistry::<()>::new();

    for name in ["wait", "heal", "plan_patrol", "sense", "scan_extra"] {
        registry
            .action::<Value>(name, |_| |_: &mut Context<'_, ()>| Status::Success)
            .unwrap();
    }
    registry
        .action::<FastParams>("fast", |_| |_: &mut Context<'_, ()>| Status::Success)
        .unwrap();
    registry
        .describe_params::<FastParams>(NodeCategory::Action, "fast", "Fast action.")
        .unwrap();

    for name in ["heal", "attack"] {
        registry
            .try_action::<Value, &'static str>(name, |_| |_: &mut Context<'_, ()>| Err("failed"))
            .unwrap();
    }
    for name in ["hp_below", "enemy_alive"] {
        registry
            .condition::<Value>(name, |_| |_: &Context<'_, ()>| true)
            .unwrap();
    }
    registry
        .predicate::<Value>("target_visible", |_| |_: &Context<'_, ()>| true)
        .unwrap();
    for name in ["heal_urgency", "attack_value", "idle_value"] {
        registry
            .score::<Value>(name, |_| |_: &Context<'_, ()>| 1.0)
            .unwrap();
    }
    registry
        .custom::<Value>(
            "multi_strike",
            |_params: Value, children: Vec<BoxedNode<()>>| Ok(Box::new(Selector::new(children))),
        )
        .unwrap();

    registry.switch_key(MODE).unwrap();
    registry
        .describe_switch_key::<String>(MODE, "Mode.")
        .unwrap();
    registry.event_key(SCAN_PULSE).unwrap();
    registry.scoped_key(&TARGET_VISIBLE).unwrap();
    registry.scoped_key(&PATROL_PLAN).unwrap();
    registry.contract_key(&TARGET_VISIBLE).unwrap();
    registry.contract_key(&PATROL_PLAN).unwrap();
    registry
}

fn validator(registry: &NodeRegistry<()>) -> jsonschema::Validator {
    jsonschema::validator_for(&registry.json_schema()).expect("registry schema compiles")
}

#[test]
fn example_configs_match_registry_schema() {
    let registry = registry();
    let validator = validator(&registry);
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");

    for file in [
        "battle_ai.json",
        "battle_ai_showcase.json",
        "battle_ai_showcase_v2.json",
    ] {
        let text = fs::read_to_string(examples.join(file)).expect("read example");
        let instance: Value = serde_json::from_str(&text).expect("parse example");
        if let Err(error) = validator.validate(&instance) {
            panic!("`{file}` does not match the registry schema: {error}");
        }
    }
}

#[test]
fn schema_rejects_unknown_names_and_wrong_params() {
    let registry = registry();
    let validator = validator(&registry);

    let unknown = json!({"root": {"type": "action", "name": "missing"}});
    assert!(
        validator.validate(&unknown).is_err(),
        "unknown action must be rejected"
    );

    let valid = json!({
        "root": {"type": "action", "name": "fast", "params": {"speed": 1.5}}
    });
    validator.validate(&valid).expect("valid params");

    let invalid = json!({
        "root": {"type": "action", "name": "fast", "params": {"speed": "quick"}}
    });
    let error = validator
        .validate(&invalid)
        .expect_err("wrong params type must be rejected");
    assert!(error.to_string().contains("speed"), "{error}");
}

#[test]
fn schema_rejects_wrong_switch_values() {
    let registry = registry();
    let validator = validator(&registry);

    let valid = json!({
        "root": {
            "type": "switch",
            "key": "test.config_schema.mode",
            "values": ["patrol"],
            "children": [{"type": "action", "name": "wait"}]
        }
    });
    validator.validate(&valid).expect("valid switch values");

    let invalid = json!({
        "root": {
            "type": "switch",
            "key": "test.config_schema.mode",
            "values": [1],
            "children": [{"type": "action", "name": "wait"}]
        }
    });
    assert!(
        validator.validate(&invalid).is_err(),
        "non-string switch value must be rejected"
    );
}
