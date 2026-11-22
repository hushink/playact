//! Optional configuration layer: a pure-data `TreeSpec` plus a build-time `NodeRegistry`.
//!
//! The configuration only describes tree structure and leaf parameters; business
//! actions/conditions are registered by name at build time by the caller. Leaf
//! parameters are deserialized into concrete types at build time, so ticks no
//! longer involve `serde_json::Value` or `Any`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::composite::Threshold;

mod build;
pub mod error;
mod include;
pub mod registry;

pub use error::{ConfigBuildError, ConfigBuildErrorKind, ConfigError, RegistryError};
pub use include::{FsIncludeResolver, IncludeResolver};
pub use registry::NodeRegistry;

/// Configuration text parser. Automatically provided by the JSON/TOML/YAML/RON constructors.
pub type TreeParser = fn(&str) -> Result<TreeSpec, ConfigError>;

/// The currently supported spec version.
pub const CURRENT_VERSION: u32 = 1;

/// Maximum include nesting depth, a fallback guard against resolvers returning
/// cyclic configurations.
pub const MAX_INCLUDE_DEPTH: usize = 32;

fn default_version() -> u32 {
    CURRENT_VERSION
}

fn default_success_threshold() -> ThresholdSpec {
    ThresholdSpec::All
}

fn default_failure_threshold() -> ThresholdSpec {
    ThresholdSpec::Any
}

/// Pure-data description of a complete behavior tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeSpec {
    /// Schema version; defaults to 1.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Optional tree name.
    #[serde(default)]
    pub name: Option<String>,
    /// Root node.
    pub root: SpecNode,
}

/// Node description within a configuration.
///
/// The outer tag is `type`, for example `{"type":"sequence","children":[...]}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpecNode {
    /// Memory sequence.
    Sequence {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Child nodes.
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// Reactive sequence.
    ReactiveSequence {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Child nodes.
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// Memory selector.
    Selector {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Child nodes.
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// Reactive selector.
    ReactiveSelector {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Child nodes.
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// Random selector with memory.
    RandomSelector {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Deterministic seed.
        seed: u64,
        /// Child nodes.
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// Weighted random selector.
    WeightedRandomSelector {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Deterministic seed.
        seed: u64,
        /// Weights corresponding one-to-one with `children`; when omitted all default to 1,
        /// and 0 means never selected.
        #[serde(default)]
        weights: Vec<u32>,
        /// Child nodes.
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// Parallel.
    Parallel {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Success threshold; defaults to `all`.
        #[serde(default = "default_success_threshold")]
        success: ThresholdSpec,
        /// Failure threshold; defaults to `any`.
        #[serde(default = "default_failure_threshold")]
        failure: ThresholdSpec,
        /// Child nodes.
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// Race node: the first terminal state wins.
    Race {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Child nodes.
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// Parallel that accumulates terminal states across ticks.
    MemParallel {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Success threshold; defaults to `all`.
        #[serde(default = "default_success_threshold")]
        success: ThresholdSpec,
        /// Failure threshold; defaults to `any`.
        #[serde(default = "default_failure_threshold")]
        failure: ThresholdSpec,
        /// Child nodes.
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// Inverts the result.
    Inverter {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Child nodes.
        child: Box<SpecNode>,
    },
    /// Terminal result is always success.
    AlwaysSucceed {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Child nodes.
        child: Box<SpecNode>,
    },
    /// Terminal result is always failure.
    AlwaysFail {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Child nodes.
        child: Box<SpecNode>,
    },
    /// Repeats execution.
    Repeat {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Number of repetitions.
        times: core::num::NonZeroUsize,
        /// Child nodes.
        child: Box<SpecNode>,
    },
    /// Retries on failure.
    Retry {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Maximum number of attempts.
        max_attempts: core::num::NonZeroUsize,
        /// Child nodes.
        child: Box<SpecNode>,
    },
    /// Until success.
    UntilSuccess {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Child nodes.
        child: Box<SpecNode>,
    },
    /// Until failure.
    UntilFailure {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Child nodes.
        child: Box<SpecNode>,
    },
    /// Cooldown after success.
    Cooldown {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Cooldown duration (milliseconds).
        duration_ms: u64,
        /// Child nodes.
        child: Box<SpecNode>,
    },
    /// Execution timeout.
    Timeout {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Timeout duration (milliseconds).
        duration_ms: u64,
        /// Child nodes.
        child: Box<SpecNode>,
    },
    /// Count limit.
    Limiter {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Maximum count.
        max: core::num::NonZeroUsize,
        /// Child nodes.
        child: Box<SpecNode>,
    },
    /// Time-window rate limiting.
    Throttle {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Window length (milliseconds).
        window_ms: u64,
        /// Maximum number of terminal states within the window.
        max: core::num::NonZeroUsize,
        /// Child nodes.
        child: Box<SpecNode>,
    },
    /// Wait.
    Wait {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Wait duration (milliseconds).
        duration_ms: u64,
    },
    /// A registered action.
    Action {
        /// Action name in the registry, also used as the display name.
        name: String,
        /// Parameters.
        #[serde(default)]
        params: Value,
    },
    /// A registered condition.
    Condition {
        /// Condition name in the registry, also used as the display name.
        name: String,
        /// Parameters.
        #[serde(default)]
        params: Value,
    },
    /// Includes another configuration; requires [`TreeSpec::build_with_resolver`].
    Include {
        /// Path interpreted by the resolver.
        path: String,
        /// Display name; defaults to `path`.
        #[serde(default)]
        name: Option<String>,
    },
    /// Custom node.
    Custom {
        /// Custom kind in the registry.
        kind: String,
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Parameters.
        #[serde(default)]
        params: Value,
        /// Child nodes.
        #[serde(default)]
        children: Vec<SpecNode>,
    },
}

/// Parallel threshold in a configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdSpec {
    /// All must be satisfied.
    All,
    /// Any one must be satisfied.
    Any,
    /// At least `n` must be satisfied.
    AtLeast(core::num::NonZeroUsize),
}

impl From<ThresholdSpec> for Threshold {
    fn from(value: ThresholdSpec) -> Self {
        match value {
            ThresholdSpec::All => Self::All,
            ThresholdSpec::Any => Self::Any,
            ThresholdSpec::AtLeast(n) => Self::AtLeast(n),
        }
    }
}

impl TreeSpec {
    /// Parses from a JSON string; errors carry a `serde_path_to_error` path.
    pub fn from_json_str(input: &str) -> Result<Self, ConfigError> {
        let mut deserializer = serde_json::Deserializer::from_str(input);
        parse_with_path(&mut deserializer)
    }

    /// Parses from JSON bytes.
    pub fn from_json_slice(input: &[u8]) -> Result<Self, ConfigError> {
        let mut deserializer = serde_json::Deserializer::from_slice(input);
        parse_with_path(&mut deserializer)
    }

    /// Parses from a TOML string.
    #[cfg(feature = "toml")]
    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        toml::from_str(input).map_err(|error| ConfigError::new(error.to_string()))
    }

    /// Parses from a YAML string.
    #[cfg(feature = "yaml")]
    pub fn from_yaml_str(input: &str) -> Result<Self, ConfigError> {
        serde_yaml_ng::from_str(input).map_err(|error| ConfigError::new(error.to_string()))
    }

    /// Parses from a RON string.
    #[cfg(feature = "ron")]
    pub fn from_ron_str(input: &str) -> Result<Self, ConfigError> {
        ron::from_str(input).map_err(|error| ConfigError::new(error.to_string()))
    }
}

fn parse_with_path<'de, D>(deserializer: D) -> Result<TreeSpec, ConfigError>
where
    D: serde::Deserializer<'de>,
{
    serde_path_to_error::deserialize(deserializer).map_err(|error| {
        let path = error.path().to_string();
        let message = error.into_inner().to_string();
        if path.is_empty() {
            ConfigError::new(message)
        } else {
            ConfigError::with_path(path, message)
        }
    })
}

#[cfg(test)]
mod tests {
    use crate::context::Context;
    use crate::node::{BoxedNode, Kind, Node};
    use crate::status::Status;
    use crate::time::Time;

    use super::*;

    fn action_registry() -> NodeRegistry<()> {
        let mut registry = NodeRegistry::<()>::new();
        registry
            .action::<()>("ok", |_| |_: &mut Context<'_, ()>| Status::Success)
            .unwrap();
        registry
            .condition::<()>("always", |_| |_: &Context<'_, ()>| true)
            .unwrap();
        registry
    }

    #[test]
    fn json_roundtrip_and_run() {
        let json = r#"
        {
          "version": 1,
          "name": "demo",
          "root": {
            "type": "sequence",
            "children": [
              { "type": "condition", "name": "always" },
              { "type": "action", "name": "ok" }
            ]
          }
        }
        "#;

        let registry = action_registry();
        let spec = TreeSpec::from_json_str(json).expect("valid spec");
        let mut tree = spec.build(&registry).expect("build tree");

        let mut world = ();
        assert_eq!(tree.name(), "demo");
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn builds_race_from_json() {
        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"race","children":[{"type":"action","name":"ok"},{"type":"action","name":"ok"}]}}"#,
        )
        .expect("valid spec");

        let mut tree = spec.build(&action_registry()).expect("build tree");
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn builds_mem_parallel_from_json() {
        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"mem_parallel","children":[{"type":"action","name":"ok"}]}}"#,
        )
        .expect("valid spec");

        let mut tree = spec.build(&action_registry()).expect("build tree");
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn builds_random_selector_from_json() {
        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"random_selector","seed":7,"children":[{"type":"action","name":"ok"}]}}"#,
        )
        .expect("valid spec");

        let mut tree = spec.build(&action_registry()).expect("build tree");
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn builds_weighted_random_selector_from_json() {
        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"weighted_random_selector","seed":7,"weights":[0,1],"children":[{"type":"action","name":"ok"},{"type":"action","name":"ok"}]}}"#,
        )
        .expect("valid spec");

        let mut tree = spec.build(&action_registry()).expect("build tree");
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn weighted_selector_rejects_weight_length_mismatch() {
        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"weighted_random_selector","seed":7,"weights":[1],"children":[{"type":"action","name":"ok"},{"type":"action","name":"ok"}]}}"#,
        )
        .expect("valid spec");

        let error = spec
            .build(&action_registry())
            .err()
            .expect("weight mismatch");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::InvalidTree { .. }
        ));
    }

    #[test]
    fn builds_throttle_from_json() {
        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"throttle","window_ms":100,"max":1,"child":{"type":"action","name":"ok"}}}"#,
        )
        .expect("valid spec");

        let mut tree = spec.build(&action_registry()).expect("build tree");
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        assert_eq!(
            tree.tick(&mut world, Time::from_millis(50, 50)),
            Status::Failure
        );
        assert_eq!(
            tree.tick(&mut world, Time::from_millis(100, 50)),
            Status::Success
        );
    }

    #[test]
    fn include_without_resolver_errors() {
        let spec = TreeSpec::from_json_str(r#"{"root":{"type":"include","path":"sub"}}"#)
            .expect("valid spec");

        let error = spec
            .build(&action_registry())
            .err()
            .expect("include requires resolver");
        assert!(matches!(error.kind(), ConfigBuildErrorKind::Include { .. }));
    }

    #[test]
    fn include_resolves_nested_spec() {
        let resolver = |path: &str| -> Result<TreeSpec, ConfigError> {
            match path {
                "sub" => TreeSpec::from_json_str(r#"{"root":{"type":"action","name":"ok"}}"#),
                other => Err(ConfigError::new(format!("unknown include `{other}`"))),
            }
        };

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"include","path":"sub","name":"included"}}"#,
        )
        .expect("valid spec");

        let mut tree = spec
            .build_with_resolver(&action_registry(), &resolver)
            .expect("build tree");
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        assert_eq!(tree.describe().name, "included");
    }

    #[test]
    fn include_cycle_is_rejected() {
        let resolver = |_path: &str| -> Result<TreeSpec, ConfigError> {
            TreeSpec::from_json_str(r#"{"root":{"type":"include","path":"loop"}}"#)
        };

        let spec = TreeSpec::from_json_str(r#"{"root":{"type":"include","path":"loop"}}"#)
            .expect("valid spec");

        let error = spec
            .build_with_resolver(&action_registry(), &resolver)
            .err()
            .expect("cycle error");
        assert!(error.to_string().contains("cycle"));
    }

    #[test]
    fn typed_params_are_deserialized_at_build_time() {
        #[derive(serde::Deserialize)]
        struct Hp {
            ratio: f64,
        }

        let mut registry = NodeRegistry::<()>::new();
        registry
            .condition::<Hp>("hp_below", |params: Hp| {
                move |_: &Context<'_, ()>| params.ratio < 0.3
            })
            .unwrap();

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"condition","name":"hp_below","params":{"ratio":0.2}}}"#,
        )
        .expect("valid spec");

        let mut tree = spec.build(&registry).expect("build tree");
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    struct CustomLeaf;

    impl Node<()> for CustomLeaf {
        fn tick(&mut self, _ctx: &mut Context<'_, ()>) -> Status {
            Status::Success
        }

        fn kind(&self) -> Kind {
            Kind::Custom("custom_leaf")
        }

        fn name(&self) -> &str {
            "custom_leaf"
        }
    }

    #[test]
    fn custom_factory_receives_children() {
        let mut registry = NodeRegistry::<()>::new();
        registry
            .custom::<()>("wrap", |_params: (), children: Vec<BoxedNode<()>>| {
                assert!(children.is_empty());
                Ok(Box::new(CustomLeaf))
            })
            .unwrap();

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"custom","kind":"wrap","name":"my_custom"}}"#,
        )
        .expect("valid spec");

        let tree = spec.build(&registry).expect("build tree");
        let description = tree.describe();
        assert_eq!(description.name, "my_custom");
        assert_eq!(description.kind, Kind::Custom("custom_leaf"));
    }

    #[test]
    fn unknown_action_reports_path() {
        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"sequence","children":[{"type":"action","name":"missing"}]}}"#,
        )
        .expect("valid spec");

        let error = spec
            .build(&NodeRegistry::<()>::new())
            .err()
            .expect("must fail");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::UnknownAction { name } if name == "missing"
        ));
        assert_eq!(error.path(), Some("root.children[0]"));
    }

    #[test]
    fn invalid_params_reports_path() {
        #[derive(serde::Deserialize)]
        struct Params {
            _ratio: f64,
        }

        let mut registry = NodeRegistry::<()>::new();
        registry
            .condition::<Params>("needs_object", |_| |_: &Context<'_, ()>| true)
            .unwrap();

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"condition","name":"needs_object","params":42}}"#,
        )
        .expect("valid spec");

        let error = spec.build(&registry).err().expect("must fail");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::InvalidParams { .. }
        ));
        assert_eq!(error.path(), Some("root"));
    }

    #[test]
    fn invalid_parallel_threshold_reports_path() {
        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"parallel","success":{"at_least":2},"children":[{"type":"action","name":"ok"}]}}"#,
        )
        .expect("valid spec");

        let error = spec.build(&action_registry()).err().expect("must fail");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::InvalidThreshold(_)
        ));
        assert_eq!(error.path(), Some("root"));
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let spec =
            TreeSpec::from_json_str(r#"{"version":99,"root":{"type":"action","name":"ok"}}"#)
                .expect("valid spec");

        let error = spec.build(&action_registry()).err().expect("must fail");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::UnsupportedVersion {
                found: 99,
                supported: 1
            }
        ));
        assert_eq!(error.path(), Some("version"));
    }

    #[test]
    fn version_defaults_to_one() {
        let spec = TreeSpec::from_json_str(r#"{"root":{"type":"action","name":"ok"}}"#)
            .expect("valid spec");
        assert_eq!(spec.version, CURRENT_VERSION);
    }

    #[test]
    fn registry_rejects_duplicate_and_empty_names() {
        let mut registry = NodeRegistry::<()>::new();
        registry
            .action::<()>("ok", |_| |_: &mut Context<'_, ()>| Status::Success)
            .expect("register action");

        match registry.action::<()>("ok", |_| |_: &mut Context<'_, ()>| Status::Success) {
            Err(RegistryError::Duplicate { category, name }) => {
                assert_eq!(category, "action");
                assert_eq!(name, "ok");
            }
            other => panic!("unexpected duplicate result: {other:?}"),
        }

        match registry.condition::<()>("  ", |_| |_: &Context<'_, ()>| true) {
            Err(RegistryError::EmptyName { category }) => {
                assert_eq!(category, "condition");
            }
            other => panic!("unexpected empty-name result: {other:?}"),
        }
    }

    #[test]
    fn misspelled_fields_are_rejected() {
        let error = TreeSpec::from_json_str(
            r#"{"root":{"type":"sequence","chidren":[{"type":"action","name":"ok"}]}}"#,
        )
        .expect_err("misspelled field must be rejected");
        assert!(
            error.to_string().contains("chidren"),
            "unexpected error: {error}"
        );

        let error = TreeSpec::from_json_str(
            r#"{"root":{"type":"parallel","success_threshold":"any","children":[]}}"#,
        )
        .expect_err("misspelled field must be rejected");
        assert!(
            error.to_string().contains("success_threshold"),
            "unexpected error: {error}"
        );
    }

    #[cfg(any(feature = "toml", feature = "yaml", feature = "ron"))]
    fn typed_params_registry() -> NodeRegistry<()> {
        #[derive(serde::Deserialize)]
        struct Params {
            value: u32,
        }

        let mut registry = NodeRegistry::<()>::new();
        registry
            .condition::<Params>("eq", |params: Params| {
                move |_: &Context<'_, ()>| params.value == 1
            })
            .expect("register condition");
        registry
    }

    #[cfg(feature = "toml")]
    #[test]
    fn parses_toml() {
        let spec = TreeSpec::from_toml_str(
            r#"
version = 1
[root]
type = "action"
name = "ok"
"#,
        )
        .expect("valid toml");
        assert!(matches!(spec.root, SpecNode::Action { .. }));
    }

    #[cfg(feature = "toml")]
    #[test]
    fn builds_toml_with_typed_params() {
        let spec = TreeSpec::from_toml_str(
            r#"
version = 1
[root]
type = "condition"
name = "eq"
params = { value = 1 }
"#,
        )
        .expect("valid toml");

        let mut tree = spec.build(&typed_params_registry()).expect("build");
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn parses_yaml() {
        let spec = TreeSpec::from_yaml_str(
            r#"
version: 1
root:
  type: action
  name: ok
"#,
        )
        .expect("valid yaml");
        assert!(matches!(spec.root, SpecNode::Action { .. }));
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn builds_yaml_with_typed_params() {
        let spec = TreeSpec::from_yaml_str(
            r#"
version: 1
root:
  type: condition
  name: eq
  params:
    value: 1
"#,
        )
        .expect("valid yaml");

        let mut tree = spec.build(&typed_params_registry()).expect("build");
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[cfg(feature = "ron")]
    #[test]
    fn parses_ron() {
        let spec = TreeSpec::from_ron_str(r#"(version: 1, root: (type: "action", name: "ok"))"#)
            .expect("valid ron");
        assert!(matches!(spec.root, SpecNode::Action { .. }));
    }

    #[cfg(feature = "ron")]
    #[test]
    fn builds_ron_with_typed_params() {
        let spec = TreeSpec::from_ron_str(
            r#"(version: 1, root: (type: "condition", name: "eq", params: (value: 1)))"#,
        )
        .expect("valid ron");

        let mut tree = spec.build(&typed_params_registry()).expect("build");
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }
}
