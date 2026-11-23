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

/// Structural limits applied by [`TreeSpec::validate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationLimits {
    /// Maximum root-to-leaf depth. The root is depth 1.
    pub max_depth: usize,
    /// Maximum number of nodes in the specification. Includes are counted as
    /// one node and are not expanded.
    pub max_nodes: usize,
}

impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_nodes: 10_000,
        }
    }
}

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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// Probability-gated child execution.
    Probability {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Probability in `[0, 1]`.
        chance: f64,
        /// Deterministic seed.
        seed: u64,
        /// Child node.
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
    /// Assigns a stable identity to a child node.
    Identified {
        /// Stable node identity.
        id: String,
        /// Child node.
        child: Box<SpecNode>,
    },
}

/// Parallel threshold in a configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

impl SpecNode {
    fn child_specs(&self) -> Vec<&SpecNode> {
        match self {
            Self::Sequence { children, .. }
            | Self::ReactiveSequence { children, .. }
            | Self::Selector { children, .. }
            | Self::ReactiveSelector { children, .. }
            | Self::RandomSelector { children, .. }
            | Self::WeightedRandomSelector { children, .. }
            | Self::Parallel { children, .. }
            | Self::Race { children, .. }
            | Self::MemParallel { children, .. }
            | Self::Custom { children, .. } => children.iter().collect(),
            Self::Inverter { child, .. }
            | Self::AlwaysSucceed { child, .. }
            | Self::AlwaysFail { child, .. }
            | Self::Repeat { child, .. }
            | Self::Retry { child, .. }
            | Self::UntilSuccess { child, .. }
            | Self::UntilFailure { child, .. }
            | Self::Cooldown { child, .. }
            | Self::Timeout { child, .. }
            | Self::Limiter { child, .. }
            | Self::Throttle { child, .. }
            | Self::Probability { child, .. }
            | Self::Identified { child, .. } => vec![child.as_ref()],
            Self::Wait { .. }
            | Self::Action { .. }
            | Self::Condition { .. }
            | Self::Include { .. } => Vec::new(),
        }
    }

    fn has_many_children(&self) -> bool {
        matches!(
            self,
            Self::Sequence { .. }
                | Self::ReactiveSequence { .. }
                | Self::Selector { .. }
                | Self::ReactiveSelector { .. }
                | Self::RandomSelector { .. }
                | Self::WeightedRandomSelector { .. }
                | Self::Parallel { .. }
                | Self::Race { .. }
                | Self::MemParallel { .. }
                | Self::Custom { .. }
        )
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

    /// Returns the JSON Schema for the configuration format.
    ///
    /// The schema describes the structural `TreeSpec` shape and built-in node
    /// parameters. Registered action/condition/custom names and their parameter
    /// schemas are application-specific and are not included.
    #[cfg(feature = "schema")]
    #[must_use]
    pub fn json_schema() -> Value {
        serde_json::to_value(schemars::schema_for!(TreeSpec))
            .expect("TreeSpec schema serialization cannot fail")
    }

    /// Validates version, depth, and node-count limits without building a tree.
    ///
    /// This is useful before accepting remote configuration or before handing a
    /// spec to a registry that may be expensive to invoke. Include nodes are
    /// counted as one node and are not expanded; the resolver must validate the
    /// included specs separately.
    pub fn validate(&self, limits: &ValidationLimits) -> Result<(), ConfigError> {
        if self.version != CURRENT_VERSION {
            return Err(ConfigError::with_path(
                "version",
                format!(
                    "unsupported version {}; supported is {CURRENT_VERSION}",
                    self.version
                ),
            ));
        }

        let mut nodes = 0_usize;
        validate_node(&self.root, "root", 1, limits, &mut nodes)
    }
}

fn validate_node(
    node: &SpecNode,
    path: &str,
    depth: usize,
    limits: &ValidationLimits,
    nodes: &mut usize,
) -> Result<(), ConfigError> {
    if depth > limits.max_depth {
        return Err(ConfigError::with_path(
            path,
            format!("tree depth exceeds limit {}", limits.max_depth),
        ));
    }

    *nodes += 1;
    if *nodes > limits.max_nodes {
        return Err(ConfigError::with_path(
            path,
            format!("node count exceeds limit {}", limits.max_nodes),
        ));
    }

    let children = node.child_specs();
    for (index, child) in children.iter().enumerate() {
        let child_path = if children.len() == 1 && !node.has_many_children() {
            format!("{path}.child")
        } else {
            format!("{path}.children[{index}]")
        };
        validate_node(child, &child_path, depth + 1, limits, nodes)?;
    }

    Ok(())
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
    fn builds_probability_from_json_and_rejects_invalid_chance() {
        let registry = action_registry();
        let spec = TreeSpec::from_json_str(
            r#"{
                "root": {
                    "type": "probability",
                    "chance": 1.0,
                    "seed": 7,
                    "child": {"type": "action", "name": "ok"}
                }
            }"#,
        )
        .unwrap();
        let mut tree = spec.build(&registry).expect("tree");
        let mut world = ();
        assert_eq!(
            tree.tick(&mut world, crate::time::Time::ZERO),
            Status::Success
        );

        let invalid = TreeSpec::from_json_str(
            r#"{
                "root": {
                    "type": "probability",
                    "chance": 2.0,
                    "seed": 7,
                    "child": {"type": "action", "name": "ok"}
                }
            }"#,
        )
        .unwrap();
        assert!(invalid.build(&registry).is_err());
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
    fn validate_accepts_within_limits() {
        let spec = TreeSpec::from_json_str(
            r#"{
                "version": 1,
                "root": {
                    "type": "sequence",
                    "children": [
                        {"type": "wait", "duration_ms": 1},
                        {"type": "wait", "duration_ms": 2}
                    ]
                }
            }"#,
        )
        .unwrap();

        spec.validate(&ValidationLimits {
            max_depth: 3,
            max_nodes: 3,
        })
        .unwrap();
    }

    #[test]
    fn validate_rejects_depth_and_node_count() {
        let spec = TreeSpec::from_json_str(
            r#"{
                "version": 1,
                "root": {
                    "type": "inverter",
                    "child": {
                        "type": "inverter",
                        "child": {"type": "wait", "duration_ms": 1}
                    }
                }
            }"#,
        )
        .unwrap();

        let depth_error = spec
            .validate(&ValidationLimits {
                max_depth: 2,
                max_nodes: 10,
            })
            .expect_err("depth limit");
        assert!(depth_error.message().contains("depth"));
        assert_eq!(depth_error.path(), Some("root.child.child"));

        let count_error = spec
            .validate(&ValidationLimits {
                max_depth: 10,
                max_nodes: 2,
            })
            .expect_err("node limit");
        assert!(count_error.message().contains("node count"));
        assert_eq!(count_error.path(), Some("root.child.child"));
    }

    #[test]
    fn validate_rejects_unsupported_version() {
        let spec = TreeSpec::from_json_str(
            r#"{"version": 2, "root": {"type": "wait", "duration_ms": 0}}"#,
        )
        .unwrap();

        let error = spec
            .validate(&ValidationLimits::default())
            .expect_err("unsupported version");
        assert_eq!(error.path(), Some("version"));
    }

    #[cfg(feature = "schema")]
    #[test]
    fn json_schema_contains_built_in_node_types() {
        let schema = TreeSpec::json_schema();
        let text = schema.to_string();
        assert!(text.contains("\"root\""));
        assert!(text.contains("\"sequence\""));
        assert!(text.contains("\"wait\""));
        assert!(text.contains("\"identified\""));
    }

    #[test]
    fn identified_nodes_carry_stable_ids() {
        let spec = TreeSpec::from_json_str(
            r#"{
                "root": {
                    "type": "identified",
                    "id": "root_id",
                    "child": {"type": "action", "name": "ok"}
                }
            }"#,
        )
        .unwrap();
        let tree = spec.build(&action_registry()).expect("tree");

        assert_eq!(
            tree.describe().id.as_ref().map(|id| id.as_str()),
            Some("root_id")
        );
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
