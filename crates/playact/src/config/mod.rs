//! 可选配置层：纯数据 `TreeSpec` + 构建期注册的 `NodeRegistry`。
//!
//! 配置只描述树结构和叶子参数；业务 action/condition 由使用方在构建期
//! 按名字注册。叶子参数在构建期反序列化成具体类型，tick 期不再有
//! `serde_json::Value` 或 `Any`。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::composite::Threshold;

mod build;
pub mod error;
pub mod registry;

pub use error::{ConfigBuildError, ConfigBuildErrorKind, ConfigError, RegistryError};
pub use registry::NodeRegistry;

/// 当前支持的 spec 版本。
pub const CURRENT_VERSION: u32 = 1;

fn default_version() -> u32 {
    CURRENT_VERSION
}

fn default_success_threshold() -> ThresholdSpec {
    ThresholdSpec::All
}

fn default_failure_threshold() -> ThresholdSpec {
    ThresholdSpec::Any
}

/// 一棵完整行为树的纯数据描述。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeSpec {
    /// schema 版本，默认 1。
    #[serde(default = "default_version")]
    pub version: u32,
    /// 树名，可选。
    #[serde(default)]
    pub name: Option<String>,
    /// 根节点。
    pub root: SpecNode,
}

/// 配置中的节点描述。
///
/// 外层 tag 是 `type`，例如 `{"type":"sequence","children":[...]}`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpecNode {
    /// 记忆式顺序。
    Sequence {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 子节点。
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// 反应式顺序。
    ReactiveSequence {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 子节点。
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// 记忆式选择。
    Selector {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 子节点。
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// 反应式选择。
    ReactiveSelector {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 子节点。
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// 带记忆的随机选择。
    RandomSelector {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 确定性 seed。
        seed: u64,
        /// 子节点。
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// 并行。
    Parallel {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 成功阈值，默认 `all`。
        #[serde(default = "default_success_threshold")]
        success: ThresholdSpec,
        /// 失败阈值，默认 `any`。
        #[serde(default = "default_failure_threshold")]
        failure: ThresholdSpec,
        /// 子节点。
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// 跨 tick 累计终态的并行。
    MemParallel {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 成功阈值，默认 `all`。
        #[serde(default = "default_success_threshold")]
        success: ThresholdSpec,
        /// 失败阈值，默认 `any`。
        #[serde(default = "default_failure_threshold")]
        failure: ThresholdSpec,
        /// 子节点。
        #[serde(default)]
        children: Vec<SpecNode>,
    },
    /// 反转结果。
    Inverter {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 子节点。
        child: Box<SpecNode>,
    },
    /// 终态恒成功。
    AlwaysSucceed {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 子节点。
        child: Box<SpecNode>,
    },
    /// 终态恒失败。
    AlwaysFail {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 子节点。
        child: Box<SpecNode>,
    },
    /// 重复执行。
    Repeat {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 重复次数。
        times: core::num::NonZeroUsize,
        /// 子节点。
        child: Box<SpecNode>,
    },
    /// 失败重试。
    Retry {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 最大尝试次数。
        max_attempts: core::num::NonZeroUsize,
        /// 子节点。
        child: Box<SpecNode>,
    },
    /// 直到成功。
    UntilSuccess {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 子节点。
        child: Box<SpecNode>,
    },
    /// 直到失败。
    UntilFailure {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 子节点。
        child: Box<SpecNode>,
    },
    /// 成功冷却。
    Cooldown {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 冷却时长（毫秒）。
        duration_ms: u64,
        /// 子节点。
        child: Box<SpecNode>,
    },
    /// 运行超时。
    Timeout {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 超时时长（毫秒）。
        duration_ms: u64,
        /// 子节点。
        child: Box<SpecNode>,
    },
    /// 次数限制。
    Limiter {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 最多次数。
        max: core::num::NonZeroUsize,
        /// 子节点。
        child: Box<SpecNode>,
    },
    /// 等待。
    Wait {
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 等待时长（毫秒）。
        duration_ms: u64,
    },
    /// 注册的 action。
    Action {
        /// registry 中的 action 名字，同时也是显示名。
        name: String,
        /// 参数。
        #[serde(default)]
        params: Value,
    },
    /// 注册的 condition。
    Condition {
        /// registry 中的 condition 名字，同时也是显示名。
        name: String,
        /// 参数。
        #[serde(default)]
        params: Value,
    },
    /// 自定义节点。
    Custom {
        /// registry 中的 custom kind。
        kind: String,
        /// 显示名。
        #[serde(default)]
        name: Option<String>,
        /// 参数。
        #[serde(default)]
        params: Value,
        /// 子节点。
        #[serde(default)]
        children: Vec<SpecNode>,
    },
}

/// 配置中的并行阈值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdSpec {
    /// 全部满足。
    All,
    /// 任一满足。
    Any,
    /// 至少 `n` 个满足。
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
    /// 从 JSON 字符串解析，错误带 `serde_path_to_error` 路径。
    pub fn from_json_str(input: &str) -> Result<Self, ConfigError> {
        let mut deserializer = serde_json::Deserializer::from_str(input);
        parse_with_path(&mut deserializer)
    }

    /// 从 JSON 字节解析。
    pub fn from_json_slice(input: &[u8]) -> Result<Self, ConfigError> {
        let mut deserializer = serde_json::Deserializer::from_slice(input);
        parse_with_path(&mut deserializer)
    }

    /// 从 TOML 字符串解析。
    #[cfg(feature = "toml")]
    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        toml::from_str(input).map_err(|error| ConfigError::new(error.to_string()))
    }

    /// 从 YAML 字符串解析。
    #[cfg(feature = "yaml")]
    pub fn from_yaml_str(input: &str) -> Result<Self, ConfigError> {
        serde_yaml_ng::from_str(input).map_err(|error| ConfigError::new(error.to_string()))
    }

    /// 从 RON 字符串解析。
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
