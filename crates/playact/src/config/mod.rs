//! Optional configuration layer: a pure-data `TreeSpec` plus a build-time `NodeRegistry`.
//!
//! The configuration only describes tree structure and leaf parameters; business
//! actions (including fallible and async actions), conditions, predicates,
//! utility scores, and typed key references are registered by name at build time
//! by the caller. Parameters are deserialized into concrete types at build time,
//! so ticks no longer involve `serde_json::Value` or `Any`.

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
#[non_exhaustive]
pub struct TreeSpec {
    /// Schema version; defaults to 1.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Optional tree name.
    #[serde(default)]
    pub name: Option<String>,
    /// Root node.
    pub root: SpecNode,
    /// Service nodes run before the root on every tick; defaults to empty.
    #[serde(default)]
    pub services: Vec<SpecNode>,
}

/// Node description within a configuration.
///
/// The outer tag is `type`, for example `{"type":"sequence","children":[...]}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
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
    /// Switches branches by a registered typed blackboard key.
    Switch {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Registered switch key name.
        key: String,
        /// Branch values paired one-to-one with `children`.
        #[serde(default)]
        values: Vec<Value>,
        /// Child nodes.
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// Utility selector driven by registered score functions.
    UtilitySelector {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Score advantage required to switch branches; defaults to 0.
        #[serde(default)]
        hysteresis: f32,
        /// Branches with a score name and child.
        #[serde(default)]
        branches: Vec<UtilityBranchSpec>,
    },
    /// Conditional branch selected by a registered predicate.
    IfThenElse {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Registered predicate name.
        condition: String,
        /// Predicate parameters.
        #[serde(default)]
        params: Value,
        /// Branch taken when the predicate is true.
        #[serde(rename = "then")]
        then_branch: Box<SpecNode>,
        /// Branch taken when the predicate is false.
        #[serde(rename = "else")]
        else_branch: Box<SpecNode>,
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
    /// Repeats the child while a registered predicate holds.
    While {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Registered predicate name.
        condition: String,
        /// Predicate parameters.
        #[serde(default)]
        params: Value,
        /// Child node.
        child: Box<SpecNode>,
    },
    /// Runs the child only while a registered predicate holds.
    Guard {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Registered predicate name.
        condition: String,
        /// Predicate parameters.
        #[serde(default)]
        params: Value,
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
    /// Waits until a registered predicate becomes true.
    WaitUntil {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Registered predicate name.
        condition: String,
        /// Predicate parameters.
        #[serde(default)]
        params: Value,
    },
    /// Waits for the next event from a registered event key.
    WaitForEvent {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Registered event key name.
        event: String,
    },
    /// Writes a fixed value to a registered typed key.
    SetValue {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Registered set-value key name.
        key: String,
        /// Value deserialized into the key's value type.
        value: Value,
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
    /// A registered fallible action.
    TryAction {
        /// Action name in the registry, also used as the display name.
        name: String,
        /// Parameters.
        #[serde(default)]
        params: Value,
    },
    /// A registered async/remote action.
    AsyncAction {
        /// Action name in the registry, also used as the display name.
        name: String,
        /// Parameters.
        #[serde(default)]
        params: Value,
    },
    /// A subtree with a private blackboard.
    ///
    /// Inputs are copied from the parent on activation; outputs are copied back
    /// on terminal status. Key names must be registered with
    /// [`NodeRegistry::scoped_key`].
    Scoped {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Keys copied from the parent blackboard.
        #[serde(default)]
        inputs: Vec<String>,
        /// Keys copied back to the parent blackboard.
        #[serde(default)]
        outputs: Vec<String>,
        /// Child node.
        child: Box<SpecNode>,
    },
    /// A subtree contract with required and provided keys.
    ///
    /// Required keys must exist on activation; provided keys are cleared on
    /// activation and abort. Key names must be registered with
    /// [`NodeRegistry::contract_key`].
    Contract {
        /// Display name.
        #[serde(default)]
        name: Option<String>,
        /// Keys that must exist before the child runs.
        #[serde(default)]
        required: Vec<String>,
        /// Keys cleared on activation and abort.
        #[serde(default)]
        provided: Vec<String>,
        /// Child node.
        child: Box<SpecNode>,
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

/// One branch of a configured `utility_selector`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct UtilityBranchSpec {
    /// Registered score name.
    pub score: String,
    /// Score parameters.
    #[serde(default)]
    pub params: Value,
    /// Child node.
    pub child: Box<SpecNode>,
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
    fn child_entries(&self) -> Vec<(String, &SpecNode)> {
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
            | Self::Switch { children, .. }
            | Self::Custom { children, .. } => children
                .iter()
                .enumerate()
                .map(|(index, child)| (format!("children[{index}]"), child))
                .collect(),
            Self::UtilitySelector { branches, .. } => branches
                .iter()
                .enumerate()
                .map(|(index, branch)| (format!("branches[{index}].child"), branch.child.as_ref()))
                .collect(),
            Self::IfThenElse {
                then_branch,
                else_branch,
                ..
            } => vec![
                (String::from("then"), then_branch.as_ref()),
                (String::from("else"), else_branch.as_ref()),
            ],
            Self::Inverter { child, .. }
            | Self::AlwaysSucceed { child, .. }
            | Self::AlwaysFail { child, .. }
            | Self::Repeat { child, .. }
            | Self::Retry { child, .. }
            | Self::UntilSuccess { child, .. }
            | Self::UntilFailure { child, .. }
            | Self::While { child, .. }
            | Self::Guard { child, .. }
            | Self::Cooldown { child, .. }
            | Self::Timeout { child, .. }
            | Self::Limiter { child, .. }
            | Self::Throttle { child, .. }
            | Self::Probability { child, .. }
            | Self::Identified { child, .. }
            | Self::Scoped { child, .. }
            | Self::Contract { child, .. } => vec![(String::from("child"), child.as_ref())],
            Self::Wait { .. }
            | Self::WaitUntil { .. }
            | Self::WaitForEvent { .. }
            | Self::SetValue { .. }
            | Self::Action { .. }
            | Self::Condition { .. }
            | Self::TryAction { .. }
            | Self::AsyncAction { .. }
            | Self::Include { .. } => Vec::new(),
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

    /// Returns the JSON Schema for the configuration format.
    ///
    /// The schema describes the structural `TreeSpec` shape and built-in node
    /// parameters. Registered action/condition/predicate/score/key/custom names
    /// and their parameter schemas are application-specific and are not included.
    #[cfg(feature = "schema")]
    #[must_use]
    pub fn json_schema() -> Value {
        serde_json::to_value(schemars::schema_for!(TreeSpec))
            .expect("TreeSpec schema serialization cannot fail")
    }

    /// Validates version, depth, and node-count limits without building a tree.
    ///
    /// The root and every service are validated; the node count is cumulative.
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
        validate_node(&self.root, "root", 1, limits, &mut nodes)?;
        for (index, service) in self.services.iter().enumerate() {
            validate_node(
                service,
                &format!("services[{index}]"),
                1,
                limits,
                &mut nodes,
            )?;
        }
        Ok(())
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

    for (segment, child) in node.child_entries() {
        let child_path = format!("{path}.{segment}");
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
    use crate::blackboard::Key;
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
    fn builds_services_from_json_and_runs_them_first() {
        static SENSED: crate::blackboard::Key<bool> =
            crate::blackboard::Key::new("test.config.sensed");

        let mut registry = NodeRegistry::<()>::new();
        registry
            .action::<()>("sense", |_| {
                move |ctx: &mut Context<'_, ()>| {
                    ctx.blackboard().set(&SENSED, true);
                    Status::Success
                }
            })
            .unwrap();
        registry
            .action::<()>("use_sense", |_| {
                move |ctx: &mut Context<'_, ()>| {
                    if ctx.blackboard_ref().get(&SENSED) == Some(&true) {
                        Status::Success
                    } else {
                        Status::Failure
                    }
                }
            })
            .unwrap();

        let spec = TreeSpec::from_json_str(
            r#"{
                "root": {"type":"action","name":"use_sense"},
                "services": [{"type":"action","name":"sense"}]
            }"#,
        )
        .expect("valid spec");
        let mut tree = spec.build(&registry).expect("build tree");

        assert_eq!(tree.service_count(), 1);
        assert_eq!(tree.describe_services()[0].kind, Kind::Action);
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn config_rejects_duplicate_node_ids() {
        let mut registry = NodeRegistry::<()>::new();
        registry
            .action::<()>("ok", |_| |_: &mut Context<'_, ()>| Status::Success)
            .unwrap();

        let spec = TreeSpec::from_json_str(
            r#"{
                "root": {"type":"sequence","children":[
                    {"type":"identified","id":"dup","child":{"type":"action","name":"ok"}},
                    {"type":"identified","id":"dup","child":{"type":"action","name":"ok"}}
                ]}
            }"#,
        )
        .expect("valid spec");
        let error = spec.build(&registry).err().expect("duplicate id");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::DuplicateNodeId { id, .. } if id == "dup"
        ));
        assert_eq!(error.path(), Some("root.children[1]"));
    }

    #[test]
    fn config_detects_duplicate_ids_from_custom_nodes() {
        let mut registry = NodeRegistry::<()>::new();
        registry
            .custom::<()>("dupe", |_params: (), _children: Vec<BoxedNode<()>>| {
                Ok(Box::new(crate::composite::Selector::new(vec![
                    Box::new(crate::node::IdentifiedNode::new(
                        "same",
                        Box::new(crate::leaf::Action::<()>::new("first", |_| Status::Success)),
                    )),
                    Box::new(crate::node::IdentifiedNode::new(
                        "same",
                        Box::new(crate::leaf::Action::<()>::new("second", |_| {
                            Status::Success
                        })),
                    )),
                ])))
            })
            .unwrap();

        let spec = TreeSpec::from_json_str(r#"{"root":{"type":"custom","kind":"dupe"}}"#)
            .expect("valid spec");
        let error = spec.build(&registry).err().expect("duplicate id");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::DuplicateNodeId { id, path }
                if id == "same" && path.as_str() == "root.children[1]"
        ));
        assert_eq!(error.path(), Some("root.children[1]"));
    }

    #[test]
    fn validate_counts_services() {
        let spec = TreeSpec::from_json_str(
            r#"{
                "root": {"type":"wait","duration_ms":1},
                "services": [
                    {"type":"wait","duration_ms":1},
                    {"type":"wait","duration_ms":1}
                ]
            }"#,
        )
        .expect("valid spec");

        let error = spec
            .validate(&ValidationLimits {
                max_depth: 2,
                max_nodes: 2,
            })
            .expect_err("node limit");
        assert_eq!(error.path(), Some("services[1]"));
    }

    #[test]
    fn builds_if_then_else_from_json() {
        let mut registry = NodeRegistry::<bool>::new();
        registry
            .condition::<()>("flag", |_| |ctx: &Context<'_, bool>| *ctx.world_ref())
            .unwrap();
        registry
            .action::<()>("then", |_| |_: &mut Context<'_, bool>| Status::Success)
            .unwrap();
        registry
            .action::<()>("else", |_| |_: &mut Context<'_, bool>| Status::Failure)
            .unwrap();

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"if_then_else","condition":"flag","then":{"type":"action","name":"then"},"else":{"type":"action","name":"else"}}}"#,
        )
        .expect("valid spec");
        let mut tree = spec.build(&registry).expect("build tree");
        assert_eq!(tree.describe().kind, Kind::IfThenElse);

        let mut world = true;
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        world = false;
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Failure);
    }

    #[test]
    fn builds_guard_from_json() {
        let mut registry = NodeRegistry::<bool>::new();
        registry
            .condition::<()>("flag", |_| |ctx: &Context<'_, bool>| *ctx.world_ref())
            .unwrap();
        registry
            .action::<()>("hit", |_| |_: &mut Context<'_, bool>| Status::Success)
            .unwrap();

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"guard","condition":"flag","child":{"type":"action","name":"hit"}}}"#,
        )
        .expect("valid spec");
        let mut tree = spec.build(&registry).expect("build tree");

        let mut world = false;
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Failure);
        world = true;
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn builds_contract_from_json() {
        static REQUIRED: Key<u32> = Key::new("test.config.contract.required");
        static OUTPUT: Key<u32> = Key::new("test.config.contract.output");

        let mut registry = NodeRegistry::<()>::new();
        registry.contract_key(&REQUIRED).expect("register required");
        registry.contract_key(&OUTPUT).expect("register output");
        registry
            .action::<()>("fill", |_| {
                |ctx: &mut Context<'_, ()>| {
                    ctx.blackboard().set(&OUTPUT, 7);
                    Status::Success
                }
            })
            .unwrap();

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"contract","name":"guarded",
                "required":["test.config.contract.required"],
                "provided":["test.config.contract.output"],
                "child":{"type":"action","name":"fill"}}}"#,
        )
        .expect("valid spec");
        let mut tree = spec.build(&registry).expect("build tree");
        assert_eq!(tree.describe().kind, Kind::Contract);

        let mut world = ();
        // A missing required key fails without ticking the child.
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Failure);
        assert!(tree.blackboard().get(&OUTPUT).is_none());

        tree.blackboard_mut().set(&REQUIRED, 1);
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        assert_eq!(tree.blackboard().get(&OUTPUT), Some(&7));
    }

    #[test]
    fn builds_scoped_from_json() {
        static INPUT: Key<u32> = Key::new("test.config.scope.input");
        static OUTPUT: Key<u32> = Key::new("test.config.scope.output");
        static PRIVATE: Key<u32> = Key::new("test.config.scope.private");

        let mut registry = NodeRegistry::<()>::new();
        registry.scoped_key(&INPUT).expect("register input");
        registry.scoped_key(&OUTPUT).expect("register output");
        registry
            .action::<()>("work", |_| {
                |ctx: &mut Context<'_, ()>| {
                    let seen = ctx.blackboard_ref().get(&INPUT).copied().unwrap_or(0);
                    ctx.blackboard().set(&PRIVATE, seen + 1);
                    ctx.blackboard().set(&OUTPUT, seen + 10);
                    Status::Success
                }
            })
            .unwrap();

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"scoped","name":"sandbox",
                "inputs":["test.config.scope.input"],
                "outputs":["test.config.scope.output"],
                "child":{"type":"action","name":"work"}}}"#,
        )
        .expect("valid spec");
        let mut tree = spec.build(&registry).expect("build tree");
        assert_eq!(tree.describe().kind, Kind::ScopedSubtree);

        let mut world = ();
        tree.blackboard_mut().set(&INPUT, 5);
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        assert_eq!(tree.blackboard().get(&INPUT), Some(&5));
        assert_eq!(tree.blackboard().get(&OUTPUT), Some(&15));
        assert!(tree.blackboard().get(&PRIVATE).is_none());
    }

    #[test]
    fn unknown_boundary_keys_report_paths() {
        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"contract","required":["missing"],
                "child":{"type":"action","name":"ok"}}}"#,
        )
        .expect("valid spec");
        let error = spec
            .build(&action_registry())
            .err()
            .expect("unknown contract key");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::UnknownContractKey { key } if key == "missing"
        ));
        assert_eq!(error.path(), Some("root.required[0]"));

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"scoped","outputs":["missing"],
                "child":{"type":"action","name":"ok"}}}"#,
        )
        .expect("valid spec");
        let error = spec
            .build(&action_registry())
            .err()
            .expect("unknown scoped key");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::UnknownScopedKey { key } if key == "missing"
        ));
        assert_eq!(error.path(), Some("root.outputs[0]"));
    }

    #[test]
    fn registry_rejects_duplicate_boundary_keys() {
        static SHARED: Key<u32> = Key::new("test.config.boundary.shared");

        let mut registry = NodeRegistry::<()>::new();
        registry.contract_key(&SHARED).expect("register contract");
        assert!(matches!(
            registry.contract_key(&SHARED),
            Err(RegistryError::Duplicate { category, .. }) if category == "contract key"
        ));

        registry.scoped_key(&SHARED).expect("register scoped");
        assert!(matches!(
            registry.scoped_key(&SHARED),
            Err(RegistryError::Duplicate { category, .. }) if category == "scoped key"
        ));
    }

    #[test]
    fn builds_while_from_json() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);
        let mut registry = NodeRegistry::<bool>::new();
        registry
            .condition::<()>("flag", |_| |ctx: &Context<'_, bool>| *ctx.world_ref())
            .unwrap();
        registry
            .action::<()>("hit", move |_| {
                let counter = Arc::clone(&counter);
                move |_: &mut Context<'_, bool>| {
                    counter.fetch_add(1, Ordering::Relaxed);
                    Status::Success
                }
            })
            .unwrap();

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"while","condition":"flag","child":{"type":"action","name":"hit"}}}"#,
        )
        .expect("valid spec");
        let mut tree = spec.build(&registry).expect("build tree");

        let mut world = true;
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Running);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        world = false;
        assert_eq!(
            tree.tick(&mut world, Time::from_millis(16, 16)),
            Status::Success
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn builds_switch_from_registered_key() {
        static PHASE: crate::blackboard::Key<u32> =
            crate::blackboard::Key::new("test.config.phase");

        let mut registry = NodeRegistry::<()>::new();
        registry.switch_key(PHASE).expect("register switch key");
        registry
            .action::<()>("ok", |_| |_: &mut Context<'_, ()>| Status::Success)
            .unwrap();
        registry
            .action::<()>("fail", |_| |_: &mut Context<'_, ()>| Status::Failure)
            .unwrap();

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"switch","key":"test.config.phase","values":[1,2],"children":[{"type":"action","name":"ok"},{"type":"action","name":"fail"}]}}"#,
        )
        .expect("valid spec");
        let mut tree = spec.build(&registry).expect("build tree");
        assert_eq!(tree.describe().kind, Kind::Switch);

        let mut world = ();
        tree.blackboard_mut().set(&PHASE, 1_u32);
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        tree.blackboard_mut().set(&PHASE, 2_u32);
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Failure);
        tree.blackboard_mut().clear_key(&PHASE);
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Failure);
    }

    #[test]
    fn switch_config_rejects_unknown_key_and_bad_values() {
        static PHASE: crate::blackboard::Key<u32> =
            crate::blackboard::Key::new("test.config.phase.bad");

        let mut registry = NodeRegistry::<()>::new();
        registry.switch_key(PHASE).expect("register switch key");
        registry
            .action::<()>("ok", |_| |_: &mut Context<'_, ()>| Status::Success)
            .unwrap();

        let unknown = TreeSpec::from_json_str(
            r#"{"root":{"type":"switch","key":"missing","values":[],"children":[]}}"#,
        )
        .expect("valid spec");
        let error = unknown.build(&registry).err().expect("unknown key");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::UnknownSwitchKey { key } if key == "missing"
        ));
        assert_eq!(error.path(), Some("root"));

        let mismatch = TreeSpec::from_json_str(
            r#"{"root":{"type":"switch","key":"test.config.phase.bad","values":[1],"children":[{"type":"action","name":"ok"},{"type":"action","name":"ok"}]}}"#,
        )
        .expect("valid spec");
        let error = mismatch.build(&registry).err().expect("length mismatch");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::InvalidTree { .. }
        ));

        let bad_value = TreeSpec::from_json_str(
            r#"{"root":{"type":"switch","key":"test.config.phase.bad","values":["x"],"children":[{"type":"action","name":"ok"}]}}"#,
        )
        .expect("valid spec");
        let error = bad_value.build(&registry).err().expect("bad value");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::InvalidParams { .. }
        ));
    }

    #[test]
    fn builds_utility_selector_from_json() {
        let mut registry = NodeRegistry::<f32>::new();
        registry
            .score::<()>("world", |_| |ctx: &Context<'_, f32>| *ctx.world_ref())
            .unwrap();
        registry
            .score::<()>("fixed", |_| |_: &Context<'_, f32>| 5.0)
            .unwrap();
        registry
            .action::<()>("high", |_| |_: &mut Context<'_, f32>| Status::Success)
            .unwrap();
        registry
            .action::<()>("low", |_| |_: &mut Context<'_, f32>| Status::Failure)
            .unwrap();

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"utility_selector","branches":[
                {"score":"world","child":{"type":"action","name":"high"}},
                {"score":"fixed","child":{"type":"action","name":"low"}}
            ]}}"#,
        )
        .expect("valid spec");
        let mut tree = spec.build(&registry).expect("build tree");
        assert_eq!(tree.describe().kind, Kind::UtilitySelector);

        let mut world = 10.0;
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        world = 1.0;
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Failure);
    }

    #[test]
    fn utility_config_rejects_unknown_score() {
        let mut registry = NodeRegistry::<()>::new();
        registry
            .action::<()>("ok", |_| |_: &mut Context<'_, ()>| Status::Success)
            .unwrap();

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"utility_selector","branches":[
                {"score":"missing","child":{"type":"action","name":"ok"}}
            ]}}"#,
        )
        .expect("valid spec");
        let error = spec.build(&registry).err().expect("unknown score");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::UnknownScore { name } if name == "missing"
        ));
        assert_eq!(error.path(), Some("root.branches[0].score"));
    }

    #[test]
    fn builds_set_value_from_json() {
        static HP: crate::blackboard::Key<u32> = crate::blackboard::Key::new("test.config.hp");

        let mut registry = NodeRegistry::<()>::new();
        registry.set_value_key(HP).expect("register set-value key");

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"set_value","key":"test.config.hp","value":42}}"#,
        )
        .expect("valid spec");
        let mut tree = spec.build(&registry).expect("build tree");
        assert_eq!(tree.describe().kind, Kind::SetValue);

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        assert_eq!(tree.blackboard().get(&HP), Some(&42));
        spec.validate(&ValidationLimits::default())
            .expect("valid spec");
    }

    #[test]
    fn set_value_config_rejects_unknown_key_and_bad_value() {
        static HP: crate::blackboard::Key<u32> = crate::blackboard::Key::new("test.config.hp.bad");

        let mut registry = NodeRegistry::<()>::new();
        registry.set_value_key(HP).expect("register set-value key");

        let unknown =
            TreeSpec::from_json_str(r#"{"root":{"type":"set_value","key":"missing","value":1}}"#)
                .expect("valid spec");
        let error = unknown.build(&registry).err().expect("unknown key");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::UnknownSetValueKey { key } if key == "missing"
        ));
        assert_eq!(error.path(), Some("root"));

        let bad_value = TreeSpec::from_json_str(
            r#"{"root":{"type":"set_value","key":"test.config.hp.bad","value":"x"}}"#,
        )
        .expect("valid spec");
        let error = bad_value.build(&registry).err().expect("bad value");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::InvalidParams { .. }
        ));
    }

    #[test]
    fn builds_wait_until_from_json() {
        let mut registry = NodeRegistry::<bool>::new();
        registry
            .condition::<()>("flag", |_| |ctx: &Context<'_, bool>| *ctx.world_ref())
            .unwrap();

        let spec = TreeSpec::from_json_str(r#"{"root":{"type":"wait_until","condition":"flag"}}"#)
            .expect("valid spec");
        let mut tree = spec.build(&registry).expect("build tree");

        let mut world = false;
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Running);
        world = true;
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn builds_wait_for_event_from_json() {
        static SIGNAL: crate::event::EventKey<u32> =
            crate::event::EventKey::new("test.config.signal");

        let mut registry = NodeRegistry::<()>::new();
        registry.event_key(SIGNAL).expect("register event key");
        registry
            .action::<()>("emit", |_| {
                move |ctx: &mut Context<'_, ()>| {
                    SIGNAL.push(ctx, 7);
                    Status::Success
                }
            })
            .unwrap();

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"sequence","children":[
                {"type":"action","name":"emit"},
                {"type":"wait_for_event","event":"test.config.signal"}
            ]}}"#,
        )
        .expect("valid spec");
        let mut tree = spec.build(&registry).expect("build tree");
        assert_eq!(tree.describe().kind, Kind::Sequence);

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);

        let wait_only = TreeSpec::from_json_str(
            r#"{"root":{"type":"wait_for_event","event":"test.config.signal"}}"#,
        )
        .expect("valid spec");
        let mut waiting = wait_only.build(&registry).expect("build tree");
        assert_eq!(waiting.tick(&mut world, Time::ZERO), Status::Running);
    }

    #[test]
    fn builds_try_action_from_json() {
        let mut registry = NodeRegistry::<()>::new();
        registry
            .try_action::<(), &'static str>("risky", |_| |_: &mut Context<'_, ()>| Err("boom"))
            .expect("register try action");

        let spec = TreeSpec::from_json_str(r#"{"root":{"type":"try_action","name":"risky"}}"#)
            .expect("valid spec");
        let mut tree = spec.build(&registry).expect("build tree");
        assert_eq!(tree.describe().kind, Kind::TryAction);

        let stats = crate::stats::StatsTracer::new();
        tree.set_tracer(Some(Box::new(stats.clone())));
        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Failure);
        assert_eq!(stats.node(&[]).expect("root stats").errors, 1);
    }

    #[test]
    fn try_action_config_rejects_unknown_name() {
        let spec = TreeSpec::from_json_str(r#"{"root":{"type":"try_action","name":"missing"}}"#)
            .expect("valid spec");
        let error = spec
            .build(&NodeRegistry::<()>::new())
            .err()
            .expect("unknown try action");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::UnknownTryAction { name } if name == "missing"
        ));
        assert_eq!(error.path(), Some("root"));
    }

    #[test]
    fn builds_async_action_from_json() {
        struct ImmediateTask;

        impl crate::async_action::AsyncTask for ImmediateTask {
            fn poll(&mut self) -> Status {
                Status::Success
            }

            fn cancel(&mut self) {}
        }

        let mut registry = NodeRegistry::<()>::new();
        registry
            .async_action::<()>("remote", |_| {
                |_: &mut Context<'_, ()>| -> Box<dyn crate::async_action::AsyncTask> {
                    Box::new(ImmediateTask)
                }
            })
            .expect("register async action");

        let spec = TreeSpec::from_json_str(r#"{"root":{"type":"async_action","name":"remote"}}"#)
            .expect("valid spec");
        let mut tree = spec.build(&registry).expect("build tree");
        assert_eq!(tree.describe().kind, Kind::AsyncAction);

        let mut world = ();
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
    }

    #[test]
    fn async_action_config_rejects_unknown_name() {
        let spec = TreeSpec::from_json_str(r#"{"root":{"type":"async_action","name":"missing"}}"#)
            .expect("valid spec");
        let error = spec
            .build(&NodeRegistry::<()>::new())
            .err()
            .expect("unknown async action");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::UnknownAsyncAction { name } if name == "missing"
        ));
        assert_eq!(error.path(), Some("root"));
    }

    #[test]
    fn wait_for_event_config_rejects_unknown_key() {
        let spec =
            TreeSpec::from_json_str(r#"{"root":{"type":"wait_for_event","event":"missing"}}"#)
                .expect("valid spec");
        let error = spec
            .build(&NodeRegistry::<()>::new())
            .err()
            .expect("unknown event key");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::UnknownEventKey { key } if key == "missing"
        ));
        assert_eq!(error.path(), Some("root"));
    }

    #[test]
    fn validate_uses_utility_branch_paths() {
        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"utility_selector","branches":[
                {"score":"unused","child":{"type":"wait","duration_ms":1}}
            ]}}"#,
        )
        .expect("valid spec");
        let error = spec
            .validate(&ValidationLimits {
                max_depth: 1,
                max_nodes: 10,
            })
            .expect_err("depth limit");
        assert_eq!(error.path(), Some("root.branches[0].child"));
    }

    #[test]
    fn predicate_registration_and_typed_params() {
        #[derive(serde::Deserialize)]
        struct Above {
            value: u32,
        }

        let mut registry = NodeRegistry::<u32>::new();
        registry
            .action::<()>("ok", |_| |_: &mut Context<'_, u32>| Status::Success)
            .unwrap();
        registry
            .predicate::<Above>("above", |params: Above| {
                move |ctx: &Context<'_, u32>| *ctx.world_ref() > params.value
            })
            .unwrap();

        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"guard","condition":"above","params":{"value":5},"child":{"type":"action","name":"ok"}}}"#,
        )
        .expect("valid spec");
        let mut tree = spec.build(&registry).expect("build tree");

        let mut world = 10;
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Success);
        world = 1;
        assert_eq!(tree.tick(&mut world, Time::ZERO), Status::Failure);

        let unknown = TreeSpec::from_json_str(
            r#"{"root":{"type":"guard","condition":"missing","child":{"type":"action","name":"ok"}}}"#,
        )
        .expect("valid spec");
        let error = unknown.build(&registry).err().expect("unknown predicate");
        assert!(matches!(
            error.kind(),
            ConfigBuildErrorKind::UnknownPredicate { name } if name == "missing"
        ));
        assert_eq!(error.path(), Some("root"));
    }

    #[test]
    fn validate_uses_named_branch_paths() {
        let spec = TreeSpec::from_json_str(
            r#"{"root":{"type":"if_then_else","condition":"unused","then":{"type":"wait","duration_ms":1},"else":{"type":"wait","duration_ms":1}}}"#,
        )
        .expect("valid spec");

        let error = spec
            .validate(&ValidationLimits {
                max_depth: 1,
                max_nodes: 10,
            })
            .expect_err("depth limit");
        assert_eq!(error.path(), Some("root.then"));
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
        assert!(text.contains("\"scoped\""));
        assert!(text.contains("\"contract\""));
        assert!(text.contains("\"if_then_else\""));
        assert!(text.contains("\"then\""));
        assert!(text.contains("\"else\""));
        assert!(text.contains("\"guard\""));
        assert!(text.contains("\"while\""));
        assert!(text.contains("\"switch\""));
        assert!(text.contains("\"utility_selector\""));
        assert!(text.contains("\"set_value\""));
        assert!(text.contains("\"wait_until\""));
        assert!(text.contains("\"wait_for_event\""));
        assert!(text.contains("\"try_action\""));
        assert!(text.contains("\"async_action\""));
        assert!(text.contains("\"services\""));
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

        registry
            .predicate::<()>("yes", |_| |_: &Context<'_, ()>| true)
            .expect("register predicate");
        match registry.predicate::<()>("yes", |_| |_: &Context<'_, ()>| true) {
            Err(RegistryError::Duplicate { category, name }) => {
                assert_eq!(category, "predicate");
                assert_eq!(name, "yes");
            }
            other => panic!("unexpected predicate duplicate result: {other:?}"),
        }
        match registry.condition::<()>("yes", |_| |_: &Context<'_, ()>| true) {
            Err(RegistryError::Duplicate { category, name }) => {
                assert_eq!(category, "predicate");
                assert_eq!(name, "yes");
            }
            other => panic!("unexpected condition/predicate conflict: {other:?}"),
        }
        match registry.predicate::<()>("  ", |_| |_: &Context<'_, ()>| true) {
            Err(RegistryError::EmptyName { category }) => {
                assert_eq!(category, "predicate");
            }
            other => panic!("unexpected predicate empty-name result: {other:?}"),
        }

        static SWITCH: crate::blackboard::Key<u32> =
            crate::blackboard::Key::new("test.registry.switch");
        registry.switch_key(SWITCH).expect("register switch key");
        match registry.switch_key(SWITCH) {
            Err(RegistryError::Duplicate { category, name }) => {
                assert_eq!(category, "switch key");
                assert_eq!(name, "test.registry.switch");
            }
            other => panic!("unexpected switch-key duplicate result: {other:?}"),
        }

        static SET_VALUE: crate::blackboard::Key<u32> =
            crate::blackboard::Key::new("test.registry.set_value");
        registry
            .set_value_key(SET_VALUE)
            .expect("register set-value key");
        match registry.set_value_key(SET_VALUE) {
            Err(RegistryError::Duplicate { category, name }) => {
                assert_eq!(category, "set value key");
                assert_eq!(name, "test.registry.set_value");
            }
            other => panic!("unexpected set-value-key duplicate result: {other:?}"),
        }

        static EVENT: crate::event::EventKey<u32> =
            crate::event::EventKey::new("test.registry.event");
        registry.event_key(EVENT).expect("register event key");
        match registry.event_key(EVENT) {
            Err(RegistryError::Duplicate { category, name }) => {
                assert_eq!(category, "event key");
                assert_eq!(name, "test.registry.event");
            }
            other => panic!("unexpected event-key duplicate result: {other:?}"),
        }

        registry
            .score::<()>("total", |_| |_: &Context<'_, ()>| 1.0)
            .expect("register score");
        match registry.score::<()>("total", |_| |_: &Context<'_, ()>| 1.0) {
            Err(RegistryError::Duplicate { category, name }) => {
                assert_eq!(category, "score");
                assert_eq!(name, "total");
            }
            other => panic!("unexpected score duplicate result: {other:?}"),
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
