//! `TreeSpec` → `BehaviorTree` 的构建逻辑。

use core::time::Duration;

use crate::composite::{
    MemParallel, Parallel, RandomSelector, ReactiveSelector, ReactiveSequence, Selector, Sequence,
    Threshold,
};
use crate::decorator::{
    AlwaysFail, AlwaysSucceed, Cooldown, Inverter, Limiter, Repeat, Retry, Timeout, UntilFailure,
    UntilSuccess,
};
use crate::leaf::Wait;
use crate::node::BoxedNode;
use crate::tree::{BehaviorTree, Named};

use super::error::{ConfigBuildError, ConfigBuildErrorKind};
use super::registry::NodeRegistry;
use super::{CURRENT_VERSION, SpecNode, TreeSpec};

impl TreeSpec {
    /// 按注册表构建行为树。
    ///
    /// 所有叶子参数在构建期反序列化；未知 action/condition/kind、参数错误、
    /// 版本不支持或阈值非法都会返回带路径的 [`ConfigBuildError`]。
    pub fn build<W: 'static>(
        &self,
        registry: &NodeRegistry<W>,
    ) -> Result<BehaviorTree<W>, ConfigBuildError> {
        if self.version != CURRENT_VERSION {
            return Err(
                ConfigBuildError::new(ConfigBuildErrorKind::UnsupportedVersion {
                    found: self.version,
                    supported: CURRENT_VERSION,
                })
                .at("version"),
            );
        }

        let root = build_node(&self.root, registry, "root")?;
        let name = self
            .name
            .clone()
            .unwrap_or_else(|| String::from("BehaviorTree"));

        Ok(BehaviorTree::new(root).with_name(name))
    }
}

fn build_node<W: 'static>(
    spec: &SpecNode,
    registry: &NodeRegistry<W>,
    path: &str,
) -> Result<BoxedNode<W>, ConfigBuildError> {
    match spec {
        SpecNode::Sequence { name, children } => {
            let node = Sequence::new(build_children(children, registry, path)?);
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::ReactiveSequence { name, children } => {
            let node = ReactiveSequence::new(build_children(children, registry, path)?);
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::Selector { name, children } => {
            let node = Selector::new(build_children(children, registry, path)?);
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::ReactiveSelector { name, children } => {
            let node = ReactiveSelector::new(build_children(children, registry, path)?);
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::RandomSelector {
            name,
            seed,
            children,
        } => {
            let node = RandomSelector::new(build_children(children, registry, path)?, *seed);
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::Parallel {
            name,
            success,
            failure,
            children,
        } => {
            let children = build_children(children, registry, path)?;
            let success: Threshold = (*success).into();
            let failure: Threshold = (*failure).into();
            let node = Parallel::try_new(children, success, failure).map_err(|error| {
                ConfigBuildError::new(ConfigBuildErrorKind::InvalidThreshold(error)).at(path)
            })?;
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::MemParallel {
            name,
            success,
            failure,
            children,
        } => {
            let children = build_children(children, registry, path)?;
            let success: Threshold = (*success).into();
            let failure: Threshold = (*failure).into();
            let node = MemParallel::try_new(children, success, failure).map_err(|error| {
                ConfigBuildError::new(ConfigBuildErrorKind::InvalidThreshold(error)).at(path)
            })?;
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::Inverter { name, child } => {
            let child = build_node(child, registry, &child_path(path))?;
            Ok(Box::new(with_name(Inverter::new(child), name)))
        }
        SpecNode::AlwaysSucceed { name, child } => {
            let child = build_node(child, registry, &child_path(path))?;
            Ok(Box::new(with_name(AlwaysSucceed::new(child), name)))
        }
        SpecNode::AlwaysFail { name, child } => {
            let child = build_node(child, registry, &child_path(path))?;
            Ok(Box::new(with_name(AlwaysFail::new(child), name)))
        }
        SpecNode::Repeat { name, times, child } => {
            let child = build_node(child, registry, &child_path(path))?;
            Ok(Box::new(with_name(Repeat::new(*times, child), name)))
        }
        SpecNode::Retry {
            name,
            max_attempts,
            child,
        } => {
            let child = build_node(child, registry, &child_path(path))?;
            Ok(Box::new(with_name(Retry::new(*max_attempts, child), name)))
        }
        SpecNode::UntilSuccess { name, child } => {
            let child = build_node(child, registry, &child_path(path))?;
            Ok(Box::new(with_name(UntilSuccess::new(child), name)))
        }
        SpecNode::UntilFailure { name, child } => {
            let child = build_node(child, registry, &child_path(path))?;
            Ok(Box::new(with_name(UntilFailure::new(child), name)))
        }
        SpecNode::Cooldown {
            name,
            duration_ms,
            child,
        } => {
            let child = build_node(child, registry, &child_path(path))?;
            Ok(Box::new(with_name(
                Cooldown::new(Duration::from_millis(*duration_ms), child),
                name,
            )))
        }
        SpecNode::Timeout {
            name,
            duration_ms,
            child,
        } => {
            let child = build_node(child, registry, &child_path(path))?;
            Ok(Box::new(with_name(
                Timeout::new(Duration::from_millis(*duration_ms), child),
                name,
            )))
        }
        SpecNode::Limiter { name, max, child } => {
            let child = build_node(child, registry, &child_path(path))?;
            Ok(Box::new(with_name(Limiter::new(*max, child), name)))
        }
        SpecNode::Wait { name, duration_ms } => {
            let node = Wait::new(Duration::from_millis(*duration_ms));
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::Action { name, params } => registry
            .build_action(name, params, Vec::new())
            .map_err(|error| error.at(path)),
        SpecNode::Condition { name, params } => registry
            .build_condition(name, params, Vec::new())
            .map_err(|error| error.at(path)),
        SpecNode::Custom {
            kind,
            name,
            params,
            children,
        } => {
            let children = build_children(children, registry, path)?;
            let node = registry
                .build_custom(kind, params, children)
                .map_err(|error| error.at(path))?;
            match name {
                Some(name) => Ok(Box::new(Named::new(name.clone(), node))),
                None => Ok(node),
            }
        }
    }
}

fn build_children<W: 'static>(
    children: &[SpecNode],
    registry: &NodeRegistry<W>,
    path: &str,
) -> Result<Vec<BoxedNode<W>>, ConfigBuildError> {
    children
        .iter()
        .enumerate()
        .map(|(index, child)| build_node(child, registry, &format!("{path}.children[{index}]")))
        .collect()
}

fn child_path(path: &str) -> String {
    format!("{path}.child")
}

fn with_name<N>(node: N, name: &Option<String>) -> N
where
    N: NamedNode,
{
    match name {
        Some(name) => node.rename(name.clone()),
        None => node,
    }
}

trait NamedNode: Sized {
    fn rename(self, name: String) -> Self;
}

macro_rules! impl_named_node {
    ($($ty:ident),* $(,)?) => {
        $(
            impl<W> NamedNode for $ty<W> {
                fn rename(self, name: String) -> Self {
                    self.with_name(name)
                }
            }
        )*
    };
}

impl_named_node!(
    Sequence,
    ReactiveSequence,
    Selector,
    ReactiveSelector,
    RandomSelector,
    Parallel,
    MemParallel,
    Inverter,
    AlwaysSucceed,
    AlwaysFail,
    Repeat,
    Retry,
    UntilSuccess,
    UntilFailure,
    Cooldown,
    Timeout,
    Limiter,
    Wait,
);
