//! 构建期节点工厂注册表。

use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::context::Context;
use crate::leaf::{Action, Condition};
use crate::node::BoxedNode;
use crate::status::Status;

use super::error::{ConfigBuildError, ConfigBuildErrorKind, RegistryError};

type Factory<W> =
    Box<dyn Fn(&Value, Vec<BoxedNode<W>>) -> Result<BoxedNode<W>, ConfigBuildError> + Send + Sync>;
type BoxedTick<W> = Box<dyn for<'a> FnMut(&mut Context<'a, W>) -> Status + Send + 'static>;
type BoxedPredicate<W> = Box<dyn for<'a> Fn(&Context<'a, W>) -> bool + Send + 'static>;

/// 把 typed params 变成每个节点独立 tick 闭包的工厂。
///
/// 用户通常直接传闭包，不需要手动实现这个 trait。
#[doc(hidden)]
pub trait ActionFactory<W, P>: Send + Sync + 'static {
    /// 为一个节点创建独立的 tick 闭包。
    fn make(&self, params: P) -> BoxedTick<W>;
}

impl<W, P, F, A> ActionFactory<W, P> for F
where
    F: Fn(P) -> A + Send + Sync + 'static,
    A: for<'a> FnMut(&mut Context<'a, W>) -> Status + Send + 'static,
{
    fn make(&self, params: P) -> BoxedTick<W> {
        Box::new(self(params))
    }
}

/// 把 typed params 变成每个节点独立条件闭包的工厂。
///
/// 用户通常直接传闭包，不需要手动实现这个 trait。
#[doc(hidden)]
pub trait ConditionFactory<W, P>: Send + Sync + 'static {
    /// 为一个节点创建独立的条件闭包。
    fn make(&self, params: P) -> BoxedPredicate<W>;
}

impl<W, P, F, C> ConditionFactory<W, P> for F
where
    F: Fn(P) -> C + Send + Sync + 'static,
    C: for<'a> Fn(&Context<'a, W>) -> bool + Send + 'static,
{
    fn make(&self, params: P) -> BoxedPredicate<W> {
        Box::new(self(params))
    }
}

/// 按名字注册 action / condition / custom 节点的构建期注册表。
///
/// 注册表只在构建期使用，不会被树持有；每个节点由工厂单独创建，因此
/// 多实例之间的闭包状态天然隔离。
pub struct NodeRegistry<W: 'static> {
    actions: HashMap<String, Factory<W>>,
    conditions: HashMap<String, Factory<W>>,
    custom: HashMap<String, Factory<W>>,
}

impl<W: 'static> NodeRegistry<W> {
    /// 创建空注册表。
    #[must_use]
    pub fn new() -> Self {
        Self {
            actions: HashMap::new(),
            conditions: HashMap::new(),
            custom: HashMap::new(),
        }
    }

    /// 注册一个闭包动作。
    ///
    /// 外层工厂 `factory` 会在每个节点构建时被调用一次，返回该节点自己的
    /// tick 闭包；因此闭包内部状态不会在实例之间共享。
    ///
    /// 配置省略 `params` 时按 `serde_json::Value::Null` 反序列化。无参
    /// action 用 `P = ()` 并省略 `params`；带字段的 struct 参数请显式写
    /// `"params": {}`。
    pub fn action<P>(
        &mut self,
        name: impl Into<String>,
        factory: impl ActionFactory<W, P>,
    ) -> Result<(), RegistryError>
    where
        P: DeserializeOwned + Send + 'static,
    {
        let name = name.into();
        let display_name = name.clone();
        let erased: Factory<W> = Box::new(move |params, children| {
            if !children.is_empty() {
                return Err(ConfigBuildError::new(ConfigBuildErrorKind::InvalidTree {
                    message: String::from("action nodes cannot have children"),
                }));
            }
            let params: P = parse_params(params)?;
            let tick = factory.make(params);
            Ok(Box::new(Action::new(display_name.clone(), tick)))
        });
        insert_factory(&mut self.actions, "action", name, erased)
    }

    /// 注册一个只读条件。
    ///
    /// 参数缺失规则与 [`NodeRegistry::action`] 相同：`P = ()` 可省略
    /// `params`，带字段的 struct 请显式写 `"params": {}`。
    pub fn condition<P>(
        &mut self,
        name: impl Into<String>,
        factory: impl ConditionFactory<W, P>,
    ) -> Result<(), RegistryError>
    where
        P: DeserializeOwned + Send + 'static,
    {
        let name = name.into();
        let display_name = name.clone();
        let erased: Factory<W> = Box::new(move |params, children| {
            if !children.is_empty() {
                return Err(ConfigBuildError::new(ConfigBuildErrorKind::InvalidTree {
                    message: String::from("condition nodes cannot have children"),
                }));
            }
            let params: P = parse_params(params)?;
            let predicate = factory.make(params);
            Ok(Box::new(Condition::new(display_name.clone(), predicate)))
        });
        insert_factory(&mut self.conditions, "condition", name, erased)
    }

    /// 注册一个自定义节点工厂。
    ///
    /// 工厂拿到 typed params 与已经递归构建好的 children，可以返回任意
    /// `BoxedNode<W>`，用于带状态的动作或自定义组合节点。
    ///
    /// 参数缺失规则与 [`NodeRegistry::action`] 相同。
    pub fn custom<P>(
        &mut self,
        kind: impl Into<String>,
        factory: impl Fn(P, Vec<BoxedNode<W>>) -> Result<BoxedNode<W>, ConfigBuildError>
        + Send
        + Sync
        + 'static,
    ) -> Result<(), RegistryError>
    where
        P: DeserializeOwned + Send + 'static,
    {
        let kind = kind.into();
        let erased: Factory<W> = Box::new(move |params, children| {
            let params: P = parse_params(params)?;
            factory(params, children)
        });
        insert_factory(&mut self.custom, "custom", kind, erased)
    }

    pub(crate) fn build_action(
        &self,
        name: &str,
        params: &Value,
        children: Vec<BoxedNode<W>>,
    ) -> Result<BoxedNode<W>, ConfigBuildError> {
        match self.actions.get(name) {
            Some(factory) => factory(params, children),
            None => Err(ConfigBuildError::new(ConfigBuildErrorKind::UnknownAction {
                name: name.to_string(),
            })),
        }
    }

    pub(crate) fn build_condition(
        &self,
        name: &str,
        params: &Value,
        children: Vec<BoxedNode<W>>,
    ) -> Result<BoxedNode<W>, ConfigBuildError> {
        match self.conditions.get(name) {
            Some(factory) => factory(params, children),
            None => Err(ConfigBuildError::new(
                ConfigBuildErrorKind::UnknownCondition {
                    name: name.to_string(),
                },
            )),
        }
    }

    pub(crate) fn build_custom(
        &self,
        kind: &str,
        params: &Value,
        children: Vec<BoxedNode<W>>,
    ) -> Result<BoxedNode<W>, ConfigBuildError> {
        match self.custom.get(kind) {
            Some(factory) => factory(params, children),
            None => Err(ConfigBuildError::new(ConfigBuildErrorKind::UnknownCustom {
                kind: kind.to_string(),
            })),
        }
    }
}

impl<W: 'static> Default for NodeRegistry<W> {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_params<P: DeserializeOwned>(params: &Value) -> Result<P, ConfigBuildError> {
    serde_json::from_value(params.clone()).map_err(|error| {
        ConfigBuildError::new(ConfigBuildErrorKind::InvalidParams {
            message: error.to_string(),
        })
    })
}

fn insert_factory<W>(
    map: &mut HashMap<String, Factory<W>>,
    category: &'static str,
    name: String,
    factory: Factory<W>,
) -> Result<(), RegistryError> {
    if name.trim().is_empty() {
        return Err(RegistryError::EmptyName { category });
    }
    if map.contains_key(&name) {
        return Err(RegistryError::Duplicate { category, name });
    }
    map.insert(name, factory);
    Ok(())
}
