//! Build-time registry of node factories, predicates, and switch keys.

use std::collections::HashMap;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::async_action::{AsyncAction, AsyncTask};
use crate::blackboard::Key;
use crate::composite::Switch;
use crate::context::Context;
use crate::contract::KeySpec;
use crate::event::EventKey;
use crate::leaf::{Action, Condition, SetValue, TryAction, WaitForEvent};
use crate::node::BoxedNode;
use crate::scope::ScopedKeySpec;
use crate::status::Status;

use super::error::{ConfigBuildError, ConfigBuildErrorKind, RegistryError};

type Factory<W> =
    Box<dyn Fn(&Value, Vec<BoxedNode<W>>) -> Result<BoxedNode<W>, ConfigBuildError> + Send + Sync>;
type PredicateFactory<W> =
    Box<dyn Fn(&Value) -> Result<BoxedPredicate<W>, ConfigBuildError> + Send + Sync>;
type SwitchFactory<W> = Box<
    dyn Fn(Option<&str>, &[Value], Vec<BoxedNode<W>>) -> Result<BoxedNode<W>, ConfigBuildError>
        + Send
        + Sync,
>;
type SetValueFactory<W> =
    Box<dyn Fn(Option<&str>, &Value) -> Result<BoxedNode<W>, ConfigBuildError> + Send + Sync>;
type EventFactory<W> = Box<dyn Fn(Option<&str>) -> BoxedNode<W> + Send + Sync>;
type BoxedScore<W> = Box<dyn for<'a> Fn(&Context<'a, W>) -> f32 + Send + 'static>;
type ScoreFactory<W> = Box<dyn Fn(&Value) -> Result<BoxedScore<W>, ConfigBuildError> + Send + Sync>;
type ErasedTryActionFactory<W> =
    Box<dyn Fn(&Value) -> Result<BoxedNode<W>, ConfigBuildError> + Send + Sync>;
type ErasedAsyncActionFactory<W> =
    Box<dyn Fn(&Value) -> Result<BoxedNode<W>, ConfigBuildError> + Send + Sync>;
type BoxedTick<W> = Box<dyn for<'a> FnMut(&mut Context<'a, W>) -> Status + Send + 'static>;
type BoxedTryTick<W, E> =
    Box<dyn for<'a> FnMut(&mut Context<'a, W>) -> Result<Status, E> + Send + 'static>;
type BoxedPredicate<W> = Box<dyn for<'a> Fn(&Context<'a, W>) -> bool + Send + 'static>;
type BoxedStart<W> =
    Box<dyn for<'a> FnMut(&mut Context<'a, W>) -> Box<dyn AsyncTask> + Send + 'static>;

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

/// Factory that turns typed params into an independent utility score closure.
///
/// Callers usually pass a closure directly and never implement this trait manually.
#[doc(hidden)]
pub trait UtilityScoreFactory<W, P>: Send + Sync + 'static {
    /// Creates an independent score closure for one branch.
    fn make(&self, params: P) -> BoxedScore<W>;
}

impl<W, P, F, S> UtilityScoreFactory<W, P> for F
where
    F: Fn(P) -> S + Send + Sync + 'static,
    S: for<'a> Fn(&Context<'a, W>) -> f32 + Send + 'static,
{
    fn make(&self, params: P) -> BoxedScore<W> {
        Box::new(self(params))
    }
}

/// Factory that turns typed params into an independent fallible tick closure.
///
/// Callers usually pass a closure directly and never implement this trait manually.
#[doc(hidden)]
pub trait TryActionFactory<W, P, E>: Send + Sync + 'static {
    /// Creates an independent fallible tick closure for one node.
    fn make(&self, params: P) -> BoxedTryTick<W, E>;
}

impl<W, P, E, F, A> TryActionFactory<W, P, E> for F
where
    F: Fn(P) -> A + Send + Sync + 'static,
    A: for<'a> FnMut(&mut Context<'a, W>) -> Result<Status, E> + Send + 'static,
    E: core::fmt::Display + Send + 'static,
{
    fn make(&self, params: P) -> BoxedTryTick<W, E> {
        Box::new(self(params))
    }
}

/// Factory that turns typed params into an independent async task starter.
///
/// Callers usually pass a closure directly and never implement this trait manually.
#[doc(hidden)]
pub trait AsyncActionFactory<W, P>: Send + Sync + 'static {
    /// Creates an independent task starter for one node.
    fn make(&self, params: P) -> BoxedStart<W>;
}

impl<W, P, F, S> AsyncActionFactory<W, P> for F
where
    F: Fn(P) -> S + Send + Sync + 'static,
    S: for<'a> FnMut(&mut Context<'a, W>) -> Box<dyn AsyncTask> + Send + 'static,
{
    fn make(&self, params: P) -> BoxedStart<W> {
        Box::new(self(params))
    }
}

/// Build-time registry for actions, conditions, predicates, utility scores,
/// typed key references, and custom node factories.
///
/// The registry is used only at build time and is not held by the tree; each
/// node is created separately by its factory, so closure state is naturally
/// isolated between instances.
pub struct NodeRegistry<W: 'static> {
    actions: HashMap<String, Factory<W>>,
    conditions: HashMap<String, Factory<W>>,
    predicates: HashMap<String, PredicateFactory<W>>,
    switch_keys: HashMap<String, SwitchFactory<W>>,
    set_value_keys: HashMap<String, SetValueFactory<W>>,
    event_keys: HashMap<String, EventFactory<W>>,
    contract_keys: HashMap<String, &'static dyn KeySpec>,
    scoped_keys: HashMap<String, &'static dyn ScopedKeySpec>,
    scores: HashMap<String, ScoreFactory<W>>,
    try_actions: HashMap<String, ErasedTryActionFactory<W>>,
    async_actions: HashMap<String, ErasedAsyncActionFactory<W>>,
    custom: HashMap<String, Factory<W>>,
}

impl<W: 'static> NodeRegistry<W> {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            actions: HashMap::new(),
            conditions: HashMap::new(),
            predicates: HashMap::new(),
            switch_keys: HashMap::new(),
            set_value_keys: HashMap::new(),
            event_keys: HashMap::new(),
            contract_keys: HashMap::new(),
            scoped_keys: HashMap::new(),
            scores: HashMap::new(),
            try_actions: HashMap::new(),
            async_actions: HashMap::new(),
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
    /// The same name is also available as a predicate to `guard`, `while`, and
    /// `if_then_else` configurations. The missing-params rules match
    /// [`NodeRegistry::action`]: `P = ()` may omit `params`, while structs with
    /// fields must write `"params": {}` explicitly.
    pub fn condition<P>(
        &mut self,
        name: impl Into<String>,
        factory: impl ConditionFactory<W, P>,
    ) -> Result<(), RegistryError>
    where
        P: DeserializeOwned + Send + 'static,
    {
        let name = name.into();
        ensure_name_available(&self.conditions, "condition", &name)?;
        ensure_name_available(&self.predicates, "predicate", &name)?;

        let display_name = name.clone();
        let factory = Arc::new(factory);
        let node_factory = Arc::clone(&factory);
        let predicate_factory = Arc::clone(&factory);

        let erased: Factory<W> = Box::new(move |params, children| {
            if !children.is_empty() {
                return Err(ConfigBuildError::new(ConfigBuildErrorKind::InvalidTree {
                    message: String::from("condition nodes cannot have children"),
                }));
            }
            let params: P = parse_params(params)?;
            let predicate = node_factory.make(params);
            Ok(Box::new(Condition::new(display_name.clone(), predicate)))
        });
        let predicate: PredicateFactory<W> = Box::new(move |params| {
            let params: P = parse_params(params)?;
            Ok(predicate_factory.make(params))
        });

        self.conditions.insert(name.clone(), erased);
        self.predicates.insert(name, predicate);
        Ok(())
    }

    /// Registers a predicate for `guard`, `while`, and `if_then_else`.
    ///
    /// Unlike [`NodeRegistry::condition`], this does not create a condition
    /// leaf, so the name cannot be used by a `condition` configuration node.
    /// The missing-params rules match [`NodeRegistry::action`].
    pub fn predicate<P>(
        &mut self,
        name: impl Into<String>,
        factory: impl ConditionFactory<W, P>,
    ) -> Result<(), RegistryError>
    where
        P: DeserializeOwned + Send + 'static,
    {
        let name = name.into();
        ensure_name_available(&self.predicates, "predicate", &name)?;
        let erased: PredicateFactory<W> = Box::new(move |params| {
            let params: P = parse_params(params)?;
            Ok(factory.make(params))
        });
        self.predicates.insert(name, erased);
        Ok(())
    }

    /// Registers a typed blackboard key for `switch` configurations.
    ///
    /// The configuration refers to the key by its `Key::name()`; `values` are
    /// deserialized into `K` at build time, so a branch value of the wrong type
    /// is a build error.
    pub fn switch_key<K>(&mut self, key: Key<K>) -> Result<(), RegistryError>
    where
        K: DeserializeOwned + Clone + PartialEq + Send + 'static,
    {
        let name = String::from(key.name());
        ensure_name_available(&self.switch_keys, "switch key", &name)?;
        let factory: SwitchFactory<W> = Box::new(move |display_name, values, children| {
            if values.len() != children.len() {
                return Err(ConfigBuildError::new(ConfigBuildErrorKind::InvalidTree {
                    message: format!(
                        "switch values length {} does not match children length {}",
                        values.len(),
                        children.len()
                    ),
                }));
            }

            let mut branches = Vec::with_capacity(values.len());
            for (index, (value, child)) in values.iter().zip(children).enumerate() {
                let value: K = serde_json::from_value(value.clone()).map_err(|error| {
                    ConfigBuildError::new(ConfigBuildErrorKind::InvalidParams {
                        message: format!("switch branch {index} value: {error}"),
                    })
                })?;
                branches.push((value, child));
            }

            let name = display_name.unwrap_or("Switch");
            Ok(Box::new(Switch::new(name, &key, branches)))
        });
        self.switch_keys.insert(name, factory);
        Ok(())
    }

    /// Registers a typed blackboard key for `set_value` configurations.
    ///
    /// The configuration refers to the key by its `Key::name()`; the configured
    /// value is deserialized into `T` at build time.
    pub fn set_value_key<T>(&mut self, key: Key<T>) -> Result<(), RegistryError>
    where
        T: DeserializeOwned + Clone + Send + 'static,
    {
        let name = String::from(key.name());
        ensure_name_available(&self.set_value_keys, "set value key", &name)?;
        let factory: SetValueFactory<W> = Box::new(move |display_name, value| {
            let value: T = serde_json::from_value(value.clone()).map_err(|error| {
                ConfigBuildError::new(ConfigBuildErrorKind::InvalidParams {
                    message: format!("set_value value: {error}"),
                })
            })?;
            let node_name = display_name.unwrap_or("SetValue");
            Ok(Box::new(SetValue::<W, T>::new(node_name, &key, value)))
        });
        self.set_value_keys.insert(name, factory);
        Ok(())
    }

    /// Registers a typed event key for `wait_for_event` configurations.
    ///
    /// The configuration refers to the key by its `EventKey::name()`.
    pub fn event_key<T>(&mut self, key: EventKey<T>) -> Result<(), RegistryError>
    where
        T: Send + 'static,
    {
        let name = String::from(key.name());
        ensure_name_available(&self.event_keys, "event key", &name)?;
        let factory: EventFactory<W> = Box::new(move |display_name| {
            let node = WaitForEvent::<W, T>::new(key);
            let node = match display_name {
                Some(display_name) => node.with_name(String::from(display_name)),
                None => node,
            };
            Box::new(node)
        });
        self.event_keys.insert(name, factory);
        Ok(())
    }

    /// Registers a typed key for `contract` configurations.
    ///
    /// The configuration refers to the key by its `Key::name()` and lists it in
    /// `required` or `provided`. Pass a static key reference:
    /// `registry.contract_key(&TARGET)?`.
    pub fn contract_key(&mut self, key: &'static dyn KeySpec) -> Result<(), RegistryError> {
        let name = String::from(key.name());
        ensure_name_available(&self.contract_keys, "contract key", &name)?;
        self.contract_keys.insert(name, key);
        Ok(())
    }

    /// Registers a cloneable typed key for `scoped` configurations.
    ///
    /// The configuration refers to the key by its `Key::name()` and lists it in
    /// `inputs` or `outputs`. The value type must implement `Clone`. Pass a
    /// static key reference: `registry.scoped_key(&TARGET)?`.
    pub fn scoped_key(&mut self, key: &'static dyn ScopedKeySpec) -> Result<(), RegistryError> {
        let name = String::from(key.name());
        ensure_name_available(&self.scoped_keys, "scoped key", &name)?;
        self.scoped_keys.insert(name, key);
        Ok(())
    }

    /// Registers a utility scoring function.
    ///
    /// The outer factory receives typed parameters once per `utility_selector`
    /// branch and returns that branch's score closure.
    pub fn score<P>(
        &mut self,
        name: impl Into<String>,
        factory: impl UtilityScoreFactory<W, P>,
    ) -> Result<(), RegistryError>
    where
        P: DeserializeOwned + Send + 'static,
    {
        let name = name.into();
        ensure_name_available(&self.scores, "score", &name)?;
        let erased: ScoreFactory<W> = Box::new(move |params| {
            let params: P = parse_params(params)?;
            Ok(factory.make(params))
        });
        self.scores.insert(name, erased);
        Ok(())
    }

    /// Registers a fallible action.
    ///
    /// On `Err`, the node reports the error through the installed tracer and
    /// returns `Failure`, like [`TryAction`].
    pub fn try_action<P, E>(
        &mut self,
        name: impl Into<String>,
        factory: impl TryActionFactory<W, P, E>,
    ) -> Result<(), RegistryError>
    where
        P: DeserializeOwned + Send + 'static,
        E: core::fmt::Display + Send + 'static,
    {
        let name = name.into();
        ensure_name_available(&self.try_actions, "try action", &name)?;
        let display_name = name.clone();
        let erased: ErasedTryActionFactory<W> = Box::new(move |params| {
            let params: P = parse_params(params)?;
            let tick = factory.make(params);
            Ok(Box::new(TryAction::<W, E>::new(display_name.clone(), tick)))
        });
        self.try_actions.insert(name, erased);
        Ok(())
    }

    /// Registers an async/remote action.
    ///
    /// The factory receives typed params once per node and returns the task
    /// starter consumed by [`AsyncAction`].
    pub fn async_action<P>(
        &mut self,
        name: impl Into<String>,
        factory: impl AsyncActionFactory<W, P>,
    ) -> Result<(), RegistryError>
    where
        P: DeserializeOwned + Send + 'static,
    {
        let name = name.into();
        ensure_name_available(&self.async_actions, "async action", &name)?;
        let display_name = name.clone();
        let erased: ErasedAsyncActionFactory<W> = Box::new(move |params| {
            let params: P = parse_params(params)?;
            let start = factory.make(params);
            Ok(Box::new(AsyncAction::<W>::new(display_name.clone(), start)))
        });
        self.async_actions.insert(name, erased);
        Ok(())
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

    pub(crate) fn build_predicate(
        &self,
        name: &str,
        params: &Value,
    ) -> Result<BoxedPredicate<W>, ConfigBuildError> {
        match self.predicates.get(name) {
            Some(factory) => factory(params),
            None => Err(ConfigBuildError::new(
                ConfigBuildErrorKind::UnknownPredicate {
                    name: name.to_string(),
                },
            )),
        }
    }

    pub(crate) fn build_switch(
        &self,
        key: &str,
        name: Option<&str>,
        values: &[Value],
        children: Vec<BoxedNode<W>>,
    ) -> Result<BoxedNode<W>, ConfigBuildError> {
        match self.switch_keys.get(key) {
            Some(factory) => factory(name, values, children),
            None => Err(ConfigBuildError::new(
                ConfigBuildErrorKind::UnknownSwitchKey {
                    key: key.to_string(),
                },
            )),
        }
    }

    pub(crate) fn build_set_value(
        &self,
        key: &str,
        name: Option<&str>,
        value: &Value,
    ) -> Result<BoxedNode<W>, ConfigBuildError> {
        match self.set_value_keys.get(key) {
            Some(factory) => factory(name, value),
            None => Err(ConfigBuildError::new(
                ConfigBuildErrorKind::UnknownSetValueKey {
                    key: key.to_string(),
                },
            )),
        }
    }

    pub(crate) fn build_event(
        &self,
        key: &str,
        name: Option<&str>,
    ) -> Result<BoxedNode<W>, ConfigBuildError> {
        match self.event_keys.get(key) {
            Some(factory) => Ok(factory(name)),
            None => Err(ConfigBuildError::new(
                ConfigBuildErrorKind::UnknownEventKey {
                    key: key.to_string(),
                },
            )),
        }
    }

    pub(crate) fn resolve_contract_key(
        &self,
        name: &str,
    ) -> Result<&'static dyn KeySpec, ConfigBuildError> {
        self.contract_keys.get(name).copied().ok_or_else(|| {
            ConfigBuildError::new(ConfigBuildErrorKind::UnknownContractKey {
                key: name.to_string(),
            })
        })
    }

    pub(crate) fn resolve_scoped_key(
        &self,
        name: &str,
    ) -> Result<&'static dyn ScopedKeySpec, ConfigBuildError> {
        self.scoped_keys.get(name).copied().ok_or_else(|| {
            ConfigBuildError::new(ConfigBuildErrorKind::UnknownScopedKey {
                key: name.to_string(),
            })
        })
    }

    pub(crate) fn build_score(
        &self,
        name: &str,
        params: &Value,
    ) -> Result<BoxedScore<W>, ConfigBuildError> {
        match self.scores.get(name) {
            Some(factory) => factory(params),
            None => Err(ConfigBuildError::new(ConfigBuildErrorKind::UnknownScore {
                name: name.to_string(),
            })),
        }
    }

    pub(crate) fn build_try_action(
        &self,
        name: &str,
        params: &Value,
    ) -> Result<BoxedNode<W>, ConfigBuildError> {
        match self.try_actions.get(name) {
            Some(factory) => factory(params),
            None => Err(ConfigBuildError::new(
                ConfigBuildErrorKind::UnknownTryAction {
                    name: name.to_string(),
                },
            )),
        }
    }

    pub(crate) fn build_async_action(
        &self,
        name: &str,
        params: &Value,
    ) -> Result<BoxedNode<W>, ConfigBuildError> {
        match self.async_actions.get(name) {
            Some(factory) => factory(params),
            None => Err(ConfigBuildError::new(
                ConfigBuildErrorKind::UnknownAsyncAction {
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
    ensure_name_available(map, category, &name)?;
    map.insert(name, factory);
    Ok(())
}

fn ensure_name_available<V>(
    map: &HashMap<String, V>,
    category: &'static str,
    name: &str,
) -> Result<(), RegistryError> {
    if name.trim().is_empty() {
        return Err(RegistryError::EmptyName { category });
    }
    if map.contains_key(name) {
        return Err(RegistryError::Duplicate {
            category,
            name: name.to_string(),
        });
    }
    Ok(())
}
