//! `TreeSpec` → `BehaviorTree` 的构建逻辑。

use core::time::Duration;

use crate::composite::{
    MemParallel, Parallel, RandomSelector, ReactiveSelector, ReactiveSequence, Selector, Sequence,
    Threshold, WeightedChild, WeightedRandomSelector,
};
use crate::decorator::{
    AlwaysFail, AlwaysSucceed, Cooldown, Inverter, Limiter, Repeat, Retry, Throttle, Timeout,
    UntilFailure, UntilSuccess,
};
use crate::leaf::Wait;
use crate::node::BoxedNode;
use crate::tree::{BehaviorTree, Named};

use super::error::{ConfigBuildError, ConfigBuildErrorKind};
use super::registry::NodeRegistry;
use super::{CURRENT_VERSION, IncludeResolver, MAX_INCLUDE_DEPTH, SpecNode, TreeSpec};

impl TreeSpec {
    /// 按注册表构建行为树；配置中出现 `include` 时返回错误。
    ///
    /// 所有叶子参数在构建期反序列化；未知 action/condition/kind、参数错误、
    /// 版本不支持或阈值非法都会返回带路径的 [`ConfigBuildError`]。
    pub fn build<W: 'static>(
        &self,
        registry: &NodeRegistry<W>,
    ) -> Result<BehaviorTree<W>, ConfigBuildError> {
        self.build_inner(registry, None)
    }

    /// 用 include 解析器构建行为树。
    ///
    /// resolver 负责解释 `include` 路径；引擎负责版本校验、循环检测与最大
    /// 嵌套深度 [`MAX_INCLUDE_DEPTH`]。
    pub fn build_with_resolver<W: 'static>(
        &self,
        registry: &NodeRegistry<W>,
        resolver: &dyn IncludeResolver,
    ) -> Result<BehaviorTree<W>, ConfigBuildError> {
        self.build_inner(registry, Some(resolver))
    }

    fn build_inner<'a, W: 'static>(
        &self,
        registry: &'a NodeRegistry<W>,
        resolver: Option<&'a dyn IncludeResolver>,
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

        let ctx = BuildCtx {
            registry,
            resolver,
            depth: 0,
            stack: Vec::new(),
        };
        let root = build_node(&self.root, &ctx, "root")?;
        let name = self
            .name
            .clone()
            .unwrap_or_else(|| String::from("BehaviorTree"));

        Ok(BehaviorTree::new(root).with_name(name))
    }
}

struct BuildCtx<'a, W: 'static> {
    registry: &'a NodeRegistry<W>,
    resolver: Option<&'a dyn IncludeResolver>,
    depth: usize,
    stack: Vec<String>,
}

fn build_node<W: 'static>(
    spec: &SpecNode,
    ctx: &BuildCtx<'_, W>,
    path: &str,
) -> Result<BoxedNode<W>, ConfigBuildError> {
    match spec {
        SpecNode::Sequence { name, children } => {
            let node = Sequence::new(build_children(children, ctx, path)?);
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::ReactiveSequence { name, children } => {
            let node = ReactiveSequence::new(build_children(children, ctx, path)?);
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::Selector { name, children } => {
            let node = Selector::new(build_children(children, ctx, path)?);
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::ReactiveSelector { name, children } => {
            let node = ReactiveSelector::new(build_children(children, ctx, path)?);
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::RandomSelector {
            name,
            seed,
            children,
        } => {
            let node = RandomSelector::new(build_children(children, ctx, path)?, *seed);
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::WeightedRandomSelector {
            name,
            seed,
            weights,
            children,
        } => {
            let children = build_children(children, ctx, path)?;
            if !weights.is_empty() && weights.len() != children.len() {
                return Err(ConfigBuildError::new(ConfigBuildErrorKind::InvalidTree {
                    message: format!(
                        "weighted_random_selector weights length {} does not match children length {}",
                        weights.len(),
                        children.len()
                    ),
                })
                .at(path));
            }
            let weighted = children
                .into_iter()
                .enumerate()
                .map(|(index, child)| {
                    WeightedChild::new(weights.get(index).copied().unwrap_or(1), child)
                })
                .collect();
            let node = WeightedRandomSelector::new(weighted, *seed);
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::Parallel {
            name,
            success,
            failure,
            children,
        } => {
            let children = build_children(children, ctx, path)?;
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
            let children = build_children(children, ctx, path)?;
            let success: Threshold = (*success).into();
            let failure: Threshold = (*failure).into();
            let node = MemParallel::try_new(children, success, failure).map_err(|error| {
                ConfigBuildError::new(ConfigBuildErrorKind::InvalidThreshold(error)).at(path)
            })?;
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::Inverter { name, child } => {
            let child = build_node(child, ctx, &child_path(path))?;
            Ok(Box::new(with_name(Inverter::new(child), name)))
        }
        SpecNode::AlwaysSucceed { name, child } => {
            let child = build_node(child, ctx, &child_path(path))?;
            Ok(Box::new(with_name(AlwaysSucceed::new(child), name)))
        }
        SpecNode::AlwaysFail { name, child } => {
            let child = build_node(child, ctx, &child_path(path))?;
            Ok(Box::new(with_name(AlwaysFail::new(child), name)))
        }
        SpecNode::Repeat { name, times, child } => {
            let child = build_node(child, ctx, &child_path(path))?;
            Ok(Box::new(with_name(Repeat::new(*times, child), name)))
        }
        SpecNode::Retry {
            name,
            max_attempts,
            child,
        } => {
            let child = build_node(child, ctx, &child_path(path))?;
            Ok(Box::new(with_name(Retry::new(*max_attempts, child), name)))
        }
        SpecNode::UntilSuccess { name, child } => {
            let child = build_node(child, ctx, &child_path(path))?;
            Ok(Box::new(with_name(UntilSuccess::new(child), name)))
        }
        SpecNode::UntilFailure { name, child } => {
            let child = build_node(child, ctx, &child_path(path))?;
            Ok(Box::new(with_name(UntilFailure::new(child), name)))
        }
        SpecNode::Cooldown {
            name,
            duration_ms,
            child,
        } => {
            let child = build_node(child, ctx, &child_path(path))?;
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
            let child = build_node(child, ctx, &child_path(path))?;
            Ok(Box::new(with_name(
                Timeout::new(Duration::from_millis(*duration_ms), child),
                name,
            )))
        }
        SpecNode::Limiter { name, max, child } => {
            let child = build_node(child, ctx, &child_path(path))?;
            Ok(Box::new(with_name(Limiter::new(*max, child), name)))
        }
        SpecNode::Throttle {
            name,
            window_ms,
            max,
            child,
        } => {
            let child = build_node(child, ctx, &child_path(path))?;
            Ok(Box::new(with_name(
                Throttle::new(Duration::from_millis(*window_ms), *max, child),
                name,
            )))
        }
        SpecNode::Wait { name, duration_ms } => {
            let node = Wait::new(Duration::from_millis(*duration_ms));
            Ok(Box::new(with_name(node, name)))
        }
        SpecNode::Action { name, params } => ctx
            .registry
            .build_action(name, params, Vec::new())
            .map_err(|error| error.at(path)),
        SpecNode::Condition { name, params } => ctx
            .registry
            .build_condition(name, params, Vec::new())
            .map_err(|error| error.at(path)),
        SpecNode::Custom {
            kind,
            name,
            params,
            children,
        } => {
            let children = build_children(children, ctx, path)?;
            let node = ctx
                .registry
                .build_custom(kind, params, children)
                .map_err(|error| error.at(path))?;
            match name {
                Some(name) => Ok(Box::new(Named::new(name.clone(), node))),
                None => Ok(node),
            }
        }
        SpecNode::Include {
            path: include_path,
            name,
        } => {
            let Some(resolver) = ctx.resolver else {
                return Err(ConfigBuildError::new(ConfigBuildErrorKind::Include {
                    path: include_path.clone(),
                    message: String::from(
                        "no include resolver configured; use TreeSpec::build_with_resolver",
                    ),
                })
                .at(path));
            };

            if ctx.stack.iter().any(|entry| entry == include_path) {
                return Err(ConfigBuildError::new(ConfigBuildErrorKind::Include {
                    path: include_path.clone(),
                    message: format!(
                        "include cycle: {} -> {include_path}",
                        ctx.stack.join(" -> ")
                    ),
                })
                .at(path));
            }

            if ctx.depth >= MAX_INCLUDE_DEPTH {
                return Err(ConfigBuildError::new(ConfigBuildErrorKind::Include {
                    path: include_path.clone(),
                    message: format!("include nesting exceeds {MAX_INCLUDE_DEPTH}"),
                })
                .at(path));
            }

            let included = resolver.resolve(include_path).map_err(|error| {
                ConfigBuildError::new(ConfigBuildErrorKind::Include {
                    path: include_path.clone(),
                    message: error.to_string(),
                })
                .at(path)
            })?;

            if included.version != CURRENT_VERSION {
                return Err(
                    ConfigBuildError::new(ConfigBuildErrorKind::UnsupportedVersion {
                        found: included.version,
                        supported: CURRENT_VERSION,
                    })
                    .at(include_path.clone()),
                );
            }

            let mut stack = ctx.stack.clone();
            stack.push(include_path.clone());
            let nested = BuildCtx {
                registry: ctx.registry,
                resolver: ctx.resolver,
                depth: ctx.depth + 1,
                stack,
            };
            let root = build_node(&included.root, &nested, include_path)?;
            let node_name = name.clone().unwrap_or_else(|| include_path.clone());
            Ok(Box::new(Named::new(node_name, root)))
        }
    }
}

fn build_children<W: 'static>(
    children: &[SpecNode],
    ctx: &BuildCtx<'_, W>,
    path: &str,
) -> Result<Vec<BoxedNode<W>>, ConfigBuildError> {
    children
        .iter()
        .enumerate()
        .map(|(index, child)| build_node(child, ctx, &format!("{path}.children[{index}]")))
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
    WeightedRandomSelector,
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
    Throttle,
    Wait,
);
