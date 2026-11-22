#![cfg(feature = "config")]

//! 配置层公共 API 集成测试。

use playact::config::{ConfigBuildErrorKind, NodeRegistry, TreeSpec};
use playact::prelude::*;

struct World {
    hp: f64,
}

#[derive(serde::Deserialize)]
struct HpBelow {
    ratio: f64,
}

#[test]
fn builds_and_runs_from_json() {
    let mut registry = NodeRegistry::<World>::new();
    registry
        .condition::<HpBelow>("hp_below", |params: HpBelow| {
            move |ctx: &Context<'_, World>| ctx.world_ref().hp < params.ratio
        })
        .expect("register condition");
    registry
        .action::<()>("noop", |_params: ()| {
            move |_: &mut Context<'_, World>| Status::Success
        })
        .expect("register action");

    let spec = TreeSpec::from_json_str(
        r#"
        {
          "root": {
            "type": "sequence",
            "children": [
              { "type": "condition", "name": "hp_below", "params": { "ratio": 0.3 } },
              { "type": "action", "name": "noop" }
            ]
          }
        }
        "#,
    )
    .expect("parse spec");

    let mut tree = spec.build(&registry).expect("build tree");
    let mut world = World { hp: 0.2 };
    assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
}

#[test]
fn unknown_action_reports_path() {
    let registry = NodeRegistry::<World>::new();
    let spec = TreeSpec::from_json_str(
        r#"{"root":{"type":"sequence","children":[{"type":"action","name":"missing"}]}}"#,
    )
    .expect("parse spec");

    let error = spec.build(&registry).err().expect("must fail");
    assert!(matches!(
        error.kind(),
        ConfigBuildErrorKind::UnknownAction { name } if name == "missing"
    ));
    assert_eq!(error.path(), Some("root.children[0]"));
}
