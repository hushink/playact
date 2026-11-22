//! Build-time registry of node factories.

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

/// Factory that turns typed params into an independent tick closure per node.
///
/// Callers usually pass a closure directly and never implement this trait manually.
#[doc(hidden)]
pub trait ActionFactory<W, P>: Send + Sync + 'static {
    /// Creates an independent tick closure for one node.
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

/// Factory that turns typed params into an independent condition closure per node.
///
/// Callers usually pass a closure directly and never implement this trait manually.
#[doc(hidden)]
pub trait ConditionFactory<W, P>: Send + Sync + 'static {
    /// Creates an independent condition closure for one node.
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

/// Build-time registry that maps names to action / condition / custom node factories.
///
/// The registry is used only at build time and is not held by the tree; each
/// node is created separately by its factory, so closure state is naturally
/// isolated between instances.
pub struct NodeRegistry<W: 'static> {
    actions: HashMap<String, Factory<W>>,
    conditions: HashMap<String, Factory<W>>,
    custom: HashMap<String, Factory<W>>,
}

impl<W: 'static> NodeRegistry<W> {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            actions: HashMap::new(),
            conditions: HashMap::new(),
            custom: HashMap::new(),
        }
    }

    /// Registers a closure action.
    ///
    /// The outer factory `factory` is called once per node build and returns
    /// that node's own tick closure, so closure state is never shared between
    /// instances.
    ///
    /// When `params` is omitted from the configuration it is deserialized from
    /// `serde_json::Value::Null`. For parameterless actions use `P = ()` and
    /// omit `params`; for struct params with fields, write `"params": {}`
    /// explicitly.
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

    /// Registers a read-only condition.
    ///
    /// The missing-params rules match [`NodeRegistry::action`]: `P = ()` may
    /// omit `params`, while structs with fields must write `"params": {}`
    /// explicitly.
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

    /// Registers a custom node factory.
    ///
    /// The factory receives typed params and the already recursively built
    /// children, and may return any `BoxedNode<W>`; use it for stateful actions
    /// or custom composite nodes.
    ///
    /// The missing-params rules match [`NodeRegistry::action`].
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
