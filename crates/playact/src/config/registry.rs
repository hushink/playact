//! Build-time registry of node factories, predicates, and switch keys.

use std::collections::HashMap;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;
#[cfg(feature = "schema")]
use serde_json::json;

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
use crate::subtree::SubtreeFactory;

#[cfg(feature = "schema")]
use super::catalog::{self, CatalogEntry};
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
type SubtreeBuilder<W> =
    Box<dyn Fn(&Value) -> Result<SubtreeFactory<W>, ConfigBuildError> + Send + Sync>;

#[cfg(feature = "schema")]
type NodeDocs = HashMap<NodeCategory, HashMap<String, NodeDoc>>;

/// Registry category addressed by [`NodeRegistry::describe`].
///
/// Available with the `schema` feature. `Condition` and `Predicate` are
/// separate registrations even though both can be used by `guard`, `while`,
/// `if_then_else`, and `wait_until`; descriptions from either category are
/// merged into those fields.
#[cfg(feature = "schema")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NodeCategory {
    /// Closure action registered with [`NodeRegistry::action`].
    Action,
    /// Fallible action registered with [`NodeRegistry::try_action`].
    TryAction,
    /// Async action registered with [`NodeRegistry::async_action`].
    AsyncAction,
    /// Condition leaf registered with [`NodeRegistry::condition`].
    Condition,
    /// Predicate registered with [`NodeRegistry::predicate`].
    Predicate,
    /// Utility score registered with [`NodeRegistry::score`].
    Score,
    /// Custom node factory registered with [`NodeRegistry::custom`].
    Custom,
    /// Code-built subtree template registered with [`NodeRegistry::subtree`]
    /// or [`NodeRegistry::subtree_with_params`].
    Subtree,
    /// Key registered with [`NodeRegistry::switch_key`].
    SwitchKey,
    /// Key registered with [`NodeRegistry::set_value_key`].
    SetValueKey,
    /// Key registered with [`NodeRegistry::event_key`].
    EventKey,
    /// Key registered with [`NodeRegistry::scoped_key`].
    ScopedKey,
    /// Key registered with [`NodeRegistry::contract_key`].
    ContractKey,
}

#[cfg(feature = "schema")]
impl NodeCategory {
    const fn registry_name(self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::TryAction => "try action",
            Self::AsyncAction => "async action",
            Self::Condition => "condition",
            Self::Predicate => "predicate",
            Self::Score => "score",
            Self::Custom => "custom",
            Self::Subtree => "subtree",
            Self::SwitchKey => "switch key",
            Self::SetValueKey => "set value key",
            Self::EventKey => "event key",
            Self::ScopedKey => "scoped key",
            Self::ContractKey => "contract key",
        }
    }

    const fn supports_params(self) -> bool {
        matches!(
            self,
            Self::Action
                | Self::TryAction
                | Self::AsyncAction
                | Self::Condition
                | Self::Predicate
                | Self::Score
                | Self::Custom
                | Self::Subtree
        )
    }
}

/// Schema-only documentation attached to one registered name.
#[cfg(feature = "schema")]
#[derive(Debug, Clone, Default)]
struct NodeDoc {
    description: Option<String>,
    params: Option<Value>,
    value: Option<Value>,
}

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
    subtrees: HashMap<String, SubtreeBuilder<W>>,
    #[cfg(feature = "schema")]
    docs: NodeDocs,
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
            subtrees: HashMap::new(),
            #[cfg(feature = "schema")]
            docs: HashMap::new(),
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

    /// Registers a code-built subtree template.
    ///
    /// A `subtree` configuration node instantiates a fresh subtree from this
    /// factory through its `template` name, so complex subtrees can stay in
    /// typed Rust code while configuration reuses them in multiple places.
    pub fn subtree(
        &mut self,
        name: impl Into<String>,
        factory: SubtreeFactory<W>,
    ) -> Result<(), RegistryError> {
        let name = name.into();
        ensure_name_available(&self.subtrees, "subtree", &name)?;
        let erased: SubtreeBuilder<W> = Box::new(move |_params| Ok(factory.clone()));
        self.subtrees.insert(name, erased);
        Ok(())
    }

    /// Returns the `TreeSpec` JSON Schema with registered names injected.
    ///
    /// Requires the `schema` feature. Action, condition, predicate, score, key,
    /// custom, and subtree names become `enum` constraints on the matching
    /// fields, so editors complete the names this registry actually accepts.
    /// Descriptions and parameter schemas added with [`NodeRegistry::describe`]
    /// and [`NodeRegistry::describe_params`] are injected as well, together
    /// with the key value schemas added by
    /// [`NodeRegistry::describe_switch_key`] and
    /// [`NodeRegistry::describe_set_value_key`].
    #[cfg(feature = "schema")]
    #[must_use]
    pub fn json_schema(&self) -> Value {
        let mut schema = crate::config::TreeSpec::json_schema();
        let actions = sorted_names(&self.actions);
        let try_actions = sorted_names(&self.try_actions);
        let async_actions = sorted_names(&self.async_actions);
        let conditions = sorted_names(&self.conditions);
        let mut predicates: Vec<String> = self
            .conditions
            .keys()
            .chain(self.predicates.keys())
            .cloned()
            .collect();
        predicates.sort();
        predicates.dedup();
        let scores = sorted_names(&self.scores);
        let switch_keys = sorted_names(&self.switch_keys);
        let set_value_keys = sorted_names(&self.set_value_keys);
        let event_keys = sorted_names(&self.event_keys);
        let scoped_keys = sorted_names(&self.scoped_keys);
        let contract_keys = sorted_names(&self.contract_keys);
        let custom = sorted_names(&self.custom);
        let subtrees = sorted_names(&self.subtrees);

        let mut injection = SchemaInjection::new(&self.docs);
        injection.variant(
            &mut schema,
            "action",
            "name",
            &actions,
            &[NodeCategory::Action],
        );
        injection.variant(
            &mut schema,
            "try_action",
            "name",
            &try_actions,
            &[NodeCategory::TryAction],
        );
        injection.variant(
            &mut schema,
            "async_action",
            "name",
            &async_actions,
            &[NodeCategory::AsyncAction],
        );
        injection.variant(
            &mut schema,
            "condition",
            "name",
            &conditions,
            &[NodeCategory::Condition],
        );
        for kind in ["guard", "while", "if_then_else", "wait_until"] {
            injection.variant(
                &mut schema,
                kind,
                "condition",
                &predicates,
                &[NodeCategory::Condition, NodeCategory::Predicate],
            );
        }
        if let Some(variant) = variant_mut(&mut schema, "switch") {
            injection.field(variant, "key", &switch_keys, &[NodeCategory::SwitchKey]);
            let clauses = injection.value_clauses(
                "key",
                &switch_keys,
                &[NodeCategory::SwitchKey],
                |value| json!({ "properties": { "values": { "items": value } } }),
            );
            append_all_of(variant, clauses);
        }
        if let Some(variant) = variant_mut(&mut schema, "set_value") {
            injection.field(
                variant,
                "key",
                &set_value_keys,
                &[NodeCategory::SetValueKey],
            );
            let clauses = injection.value_clauses(
                "key",
                &set_value_keys,
                &[NodeCategory::SetValueKey],
                |value| json!({ "properties": { "value": value } }),
            );
            append_all_of(variant, clauses);
        }
        injection.variant(
            &mut schema,
            "wait_for_event",
            "event",
            &event_keys,
            &[NodeCategory::EventKey],
        );
        if let Some(variant) = variant_mut(&mut schema, "scoped") {
            injection.items(variant, "inputs", &scoped_keys, &[NodeCategory::ScopedKey]);
            injection.items(variant, "outputs", &scoped_keys, &[NodeCategory::ScopedKey]);
        }
        if let Some(variant) = variant_mut(&mut schema, "contract") {
            injection.items(
                variant,
                "required",
                &contract_keys,
                &[NodeCategory::ContractKey],
            );
            injection.items(
                variant,
                "provided",
                &contract_keys,
                &[NodeCategory::ContractKey],
            );
        }
        injection.variant(
            &mut schema,
            "subtree",
            "template",
            &subtrees,
            &[NodeCategory::Subtree],
        );
        injection.variant(
            &mut schema,
            "custom",
            "kind",
            &custom,
            &[NodeCategory::Custom],
        );
        if let Some(branch) = schema.pointer_mut("/$defs/UtilityBranchSpec") {
            injection.field(branch, "score", &scores, &[NodeCategory::Score]);
        }
        injection.finish(&mut schema);
        schema
    }

    /// Renders registered names and metadata as a Markdown node catalog.
    ///
    /// Requires the `schema` feature. Entries are grouped by category and sorted
    /// by name, so the output is deterministic. The parameter column summarizes
    /// the schemas attached with [`NodeRegistry::describe_params`],
    /// [`NodeRegistry::describe_switch_key`], and
    /// [`NodeRegistry::describe_set_value_key`].
    #[cfg(feature = "schema")]
    #[must_use]
    pub fn markdown_catalog(&self) -> String {
        const SECTIONS: &[(&str, &str, NodeCategory)] = &[
            ("Actions", "Parameters", NodeCategory::Action),
            ("Fallible actions", "Parameters", NodeCategory::TryAction),
            ("Async actions", "Parameters", NodeCategory::AsyncAction),
            ("Conditions", "Parameters", NodeCategory::Condition),
            ("Predicates", "Parameters", NodeCategory::Predicate),
            ("Utility scores", "Parameters", NodeCategory::Score),
            ("Custom nodes", "Parameters", NodeCategory::Custom),
            ("Subtree templates", "Parameters", NodeCategory::Subtree),
            ("Switch keys", "Value", NodeCategory::SwitchKey),
            ("Set-value keys", "Value", NodeCategory::SetValueKey),
            ("Event keys", "Details", NodeCategory::EventKey),
            ("Scoped keys", "Details", NodeCategory::ScopedKey),
            ("Contract keys", "Details", NodeCategory::ContractKey),
        ];

        let mut sections = Vec::new();
        for &(title, column, category) in SECTIONS {
            let entries: Vec<CatalogEntry> = self
                .names_for(category)
                .into_iter()
                .map(|name| {
                    let doc = self
                        .docs
                        .get(&category)
                        .and_then(|entries| entries.get(&name));
                    CatalogEntry {
                        description: doc.and_then(|doc| doc.description.clone()),
                        schema: doc
                            .and_then(|doc| doc.params.clone().or_else(|| doc.value.clone())),
                        name,
                    }
                })
                .collect();
            if !entries.is_empty() {
                sections.push(catalog::CatalogSection {
                    title,
                    column,
                    entries,
                });
            }
        }
        catalog::render(&sections)
    }

    /// Registers a parameterized code-built subtree template.
    ///
    /// The configured `params` value is deserialized into `P` once per
    /// `subtree` node, and the factory returns the template for that instance.
    /// When `params` is omitted it is deserialized from `null`; use `P = ()`
    /// for parameterless templates.
    pub fn subtree_with_params<P>(
        &mut self,
        name: impl Into<String>,
        factory: impl Fn(P) -> SubtreeFactory<W> + Send + Sync + 'static,
    ) -> Result<(), RegistryError>
    where
        P: DeserializeOwned + Send + 'static,
    {
        let name = name.into();
        ensure_name_available(&self.subtrees, "subtree", &name)?;
        let erased: SubtreeBuilder<W> = Box::new(move |params| {
            let params: P = parse_params(params)?;
            Ok(factory(params))
        });
        self.subtrees.insert(name, erased);
        Ok(())
    }

    /// Attaches an editor description to an already-registered name.
    ///
    /// Requires the `schema` feature. The description appears in
    /// [`NodeRegistry::json_schema`] completions for the matching field,
    /// including key and event fields. Calling this again replaces the previous
    /// description; an empty description is ignored.
    ///
    /// Returns [`RegistryError::UnknownName`] when `name` is not registered in
    /// `category`.
    #[cfg(feature = "schema")]
    pub fn describe(
        &mut self,
        category: NodeCategory,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<(), RegistryError> {
        let name = name.into();
        self.ensure_describable(category, &name)?;
        let doc = self
            .docs
            .entry(category)
            .or_default()
            .entry(name)
            .or_default();
        set_doc_description(doc, description.into());
        Ok(())
    }

    /// Attaches a description and the JSON Schema of the registered params type.
    ///
    /// Requires the `schema` feature. `P` must match the params type used when
    /// registering the factory; derive [`schemars::JsonSchema`] on that type
    /// (or implement it manually). [`NodeRegistry::json_schema`] then places
    /// the generated schema under `params` for this name, including field
    /// descriptions and validation keywords. Recursive types are supported:
    /// their definitions are moved into the root schema and references are
    /// rewritten.
    ///
    /// An empty description is ignored, so this can also be used to attach only
    /// the params schema. Returns [`RegistryError::UnknownName`] when `name` is
    /// not registered in `category`, and [`RegistryError::UnsupportedSchema`]
    /// for key categories, which do not take params.
    #[cfg(feature = "schema")]
    pub fn describe_params<P>(
        &mut self,
        category: NodeCategory,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<(), RegistryError>
    where
        P: schemars::JsonSchema,
    {
        if !category.supports_params() {
            return Err(RegistryError::UnsupportedSchema {
                category: category.registry_name(),
                target: "params",
            });
        }
        let name = name.into();
        self.ensure_describable(category, &name)?;
        let params = params_schema::<P>();
        let doc = self
            .docs
            .entry(category)
            .or_default()
            .entry(name)
            .or_default();
        set_doc_description(doc, description.into());
        doc.params = Some(params);
        Ok(())
    }

    /// Attaches a description and value schema to a registered switch key.
    ///
    /// Requires the `schema` feature. `K` must match the key's value type and
    /// implement [`schemars::JsonSchema`]. [`NodeRegistry::json_schema`] then
    /// types the corresponding `values` array items, so branch values validate
    /// against the key type in the editor.
    #[cfg(feature = "schema")]
    pub fn describe_switch_key<K>(
        &mut self,
        key: Key<K>,
        description: impl Into<String>,
    ) -> Result<(), RegistryError>
    where
        K: schemars::JsonSchema + Send + 'static,
    {
        self.describe_key_value(NodeCategory::SwitchKey, key, description)
    }

    /// Attaches a description and value schema to a registered set-value key.
    ///
    /// Requires the `schema` feature. `K` must match the key's value type and
    /// implement [`schemars::JsonSchema`]. [`NodeRegistry::json_schema`] then
    /// types the `value` field of the matching `set_value` node.
    #[cfg(feature = "schema")]
    pub fn describe_set_value_key<K>(
        &mut self,
        key: Key<K>,
        description: impl Into<String>,
    ) -> Result<(), RegistryError>
    where
        K: schemars::JsonSchema + Send + 'static,
    {
        self.describe_key_value(NodeCategory::SetValueKey, key, description)
    }

    #[cfg(feature = "schema")]
    fn describe_key_value<K>(
        &mut self,
        category: NodeCategory,
        key: Key<K>,
        description: impl Into<String>,
    ) -> Result<(), RegistryError>
    where
        K: schemars::JsonSchema + Send + 'static,
    {
        let name = String::from(key.name());
        self.ensure_describable(category, &name)?;
        let value = params_schema::<K>();
        let doc = self
            .docs
            .entry(category)
            .or_default()
            .entry(name)
            .or_default();
        set_doc_description(doc, description.into());
        doc.value = Some(value);
        Ok(())
    }

    #[cfg(feature = "schema")]
    fn ensure_describable(&self, category: NodeCategory, name: &str) -> Result<(), RegistryError> {
        if self.contains_name(category, name) {
            Ok(())
        } else {
            Err(RegistryError::UnknownName {
                category: category.registry_name(),
                name: name.to_string(),
            })
        }
    }

    #[cfg(feature = "schema")]
    fn contains_name(&self, category: NodeCategory, name: &str) -> bool {
        match category {
            NodeCategory::Action => self.actions.contains_key(name),
            NodeCategory::TryAction => self.try_actions.contains_key(name),
            NodeCategory::AsyncAction => self.async_actions.contains_key(name),
            NodeCategory::Condition => self.conditions.contains_key(name),
            NodeCategory::Predicate => self.predicates.contains_key(name),
            NodeCategory::Score => self.scores.contains_key(name),
            NodeCategory::Custom => self.custom.contains_key(name),
            NodeCategory::Subtree => self.subtrees.contains_key(name),
            NodeCategory::SwitchKey => self.switch_keys.contains_key(name),
            NodeCategory::SetValueKey => self.set_value_keys.contains_key(name),
            NodeCategory::EventKey => self.event_keys.contains_key(name),
            NodeCategory::ScopedKey => self.scoped_keys.contains_key(name),
            NodeCategory::ContractKey => self.contract_keys.contains_key(name),
        }
    }

    #[cfg(feature = "schema")]
    fn names_for(&self, category: NodeCategory) -> Vec<String> {
        match category {
            NodeCategory::Action => sorted_names(&self.actions),
            NodeCategory::TryAction => sorted_names(&self.try_actions),
            NodeCategory::AsyncAction => sorted_names(&self.async_actions),
            NodeCategory::Condition => sorted_names(&self.conditions),
            NodeCategory::Predicate => {
                let mut names: Vec<String> = self
                    .predicates
                    .keys()
                    .filter(|name| !self.conditions.contains_key(*name))
                    .cloned()
                    .collect();
                names.sort();
                names
            }
            NodeCategory::Score => sorted_names(&self.scores),
            NodeCategory::Custom => sorted_names(&self.custom),
            NodeCategory::Subtree => sorted_names(&self.subtrees),
            NodeCategory::SwitchKey => sorted_names(&self.switch_keys),
            NodeCategory::SetValueKey => sorted_names(&self.set_value_keys),
            NodeCategory::EventKey => sorted_names(&self.event_keys),
            NodeCategory::ScopedKey => sorted_names(&self.scoped_keys),
            NodeCategory::ContractKey => sorted_names(&self.contract_keys),
        }
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

    pub(crate) fn build_subtree(
        &self,
        template: &str,
        params: &Value,
    ) -> Result<BoxedNode<W>, ConfigBuildError> {
        match self.subtrees.get(template) {
            Some(builder) => Ok(builder(params)?.instantiate()),
            None => Err(ConfigBuildError::new(
                ConfigBuildErrorKind::UnknownSubtreeTemplate {
                    template: template.to_string(),
                },
            )),
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

#[cfg(feature = "schema")]
fn sorted_names<V>(map: &HashMap<String, V>) -> Vec<String> {
    let mut names: Vec<String> = map.keys().cloned().collect();
    names.sort();
    names
}

#[cfg(feature = "schema")]
fn variant_mut<'a>(schema: &'a mut Value, kind: &str) -> Option<&'a mut Value> {
    schema
        .pointer_mut("/$defs/SpecNode/oneOf")?
        .as_array_mut()?
        .iter_mut()
        .find(|variant| {
            variant
                .pointer("/properties/type/const")
                .and_then(Value::as_str)
                == Some(kind)
        })
}

#[cfg(feature = "schema")]
fn set_string_enum(variant: &mut Value, field: &str, names: &[String]) {
    if let Some(property) = variant.pointer_mut(&format!("/properties/{field}")) {
        set_enum(property, names);
    }
}

#[cfg(feature = "schema")]
fn set_array_item_enum(variant: &mut Value, field: &str, names: &[String]) {
    if let Some(items) = variant.pointer_mut(&format!("/properties/{field}/items")) {
        set_enum(items, names);
    }
}

#[cfg(feature = "schema")]
fn set_enum(value: &mut Value, names: &[String]) {
    if names.is_empty() {
        return;
    }
    if let Some(object) = value.as_object_mut() {
        object.insert(
            String::from("enum"),
            Value::Array(names.iter().cloned().map(Value::String).collect()),
        );
    }
}

#[cfg(feature = "schema")]
fn set_doc_description(doc: &mut NodeDoc, description: String) {
    if !description.trim().is_empty() {
        doc.description = Some(description);
    }
}

#[cfg(feature = "schema")]
fn doc_description(docs: &NodeDocs, categories: &[NodeCategory], name: &str) -> Option<String> {
    categories.iter().find_map(|category| {
        docs.get(category)
            .and_then(|entries| entries.get(name))
            .and_then(|doc| doc.description.clone())
    })
}

#[cfg(feature = "schema")]
fn doc_params(docs: &NodeDocs, categories: &[NodeCategory], name: &str) -> Option<Value> {
    categories.iter().find_map(|category| {
        docs.get(category)
            .and_then(|entries| entries.get(name))
            .and_then(|doc| doc.params.clone())
    })
}

#[cfg(feature = "schema")]
fn doc_value(docs: &NodeDocs, categories: &[NodeCategory], name: &str) -> Option<Value> {
    categories.iter().find_map(|category| {
        docs.get(category)
            .and_then(|entries| entries.get(name))
            .and_then(|doc| doc.value.clone())
    })
}

#[cfg(feature = "schema")]
fn append_all_of(node: &mut Value, clauses: Vec<Value>) {
    if clauses.is_empty() {
        return;
    }
    let object = node.as_object_mut().expect("schema node object");
    match object
        .entry(String::from("allOf"))
        .or_insert_with(|| Value::Array(Vec::new()))
    {
        Value::Array(all_of) => all_of.extend(clauses),
        entry => *entry = Value::Array(clauses),
    }
}

#[cfg(feature = "schema")]
fn params_schema<P>() -> Value
where
    P: schemars::JsonSchema,
{
    let schema = schemars::generate::SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<P>();
    let mut schema = Value::from(schema);
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
    }
    schema
}

#[cfg(feature = "schema")]
struct SchemaInjection<'a> {
    docs: &'a NodeDocs,
    defs: HashMap<String, Value>,
    counter: usize,
}

#[cfg(feature = "schema")]
impl<'a> SchemaInjection<'a> {
    fn new(docs: &'a NodeDocs) -> Self {
        Self {
            docs,
            defs: HashMap::new(),
            counter: 0,
        }
    }

    fn variant(
        &mut self,
        schema: &mut Value,
        kind: &str,
        field: &str,
        names: &[String],
        categories: &[NodeCategory],
    ) {
        if let Some(variant) = variant_mut(schema, kind) {
            self.field(variant, field, names, categories);
        }
    }

    fn field(
        &mut self,
        node: &mut Value,
        field: &str,
        names: &[String],
        categories: &[NodeCategory],
    ) {
        set_string_enum(node, field, names);
        if names.is_empty() {
            return;
        }
        self.descriptions(node, &format!("/properties/{field}"), names, categories);

        let docs = self.docs;
        let defs = &mut self.defs;
        let counter = &mut self.counter;
        let clauses: Vec<Value> = names
            .iter()
            .filter_map(|name| {
                let params = doc_params(docs, categories, name)?;
                *counter += 1;
                let params = relocate_defs(params, &format!("playact_params_{}", *counter), defs);
                Some(json!({
                    "if": {
                        "properties": { (field): { "const": name } },
                        "required": [field]
                    },
                    "then": { "properties": { "params": params } }
                }))
            })
            .collect();
        append_all_of(node, clauses);
    }

    fn items(
        &mut self,
        node: &mut Value,
        field: &str,
        names: &[String],
        categories: &[NodeCategory],
    ) {
        set_array_item_enum(node, field, names);
        if names.is_empty() {
            return;
        }
        self.descriptions(
            node,
            &format!("/properties/{field}/items"),
            names,
            categories,
        );
    }

    fn descriptions(
        &self,
        node: &mut Value,
        pointer: &str,
        names: &[String],
        categories: &[NodeCategory],
    ) {
        let descriptions: Vec<Option<String>> = names
            .iter()
            .map(|name| doc_description(self.docs, categories, name))
            .collect();
        if !descriptions.iter().any(Option::is_some) {
            return;
        }
        let entries: Vec<Value> = names
            .iter()
            .zip(descriptions)
            .map(|(name, description)| {
                let mut entry = serde_json::Map::new();
                entry.insert(String::from("const"), Value::String(name.clone()));
                if let Some(description) = description {
                    entry.insert(String::from("description"), Value::String(description));
                }
                Value::Object(entry)
            })
            .collect();
        if let Some(target) = node.pointer_mut(pointer).and_then(Value::as_object_mut) {
            target.insert(String::from("oneOf"), Value::Array(entries));
        }
    }

    fn value_clauses(
        &mut self,
        key_field: &str,
        names: &[String],
        categories: &[NodeCategory],
        build: impl Fn(Value) -> Value,
    ) -> Vec<Value> {
        let docs = self.docs;
        let defs = &mut self.defs;
        let counter = &mut self.counter;
        names
            .iter()
            .filter_map(|name| {
                let value = doc_value(docs, categories, name)?;
                *counter += 1;
                let value = relocate_defs(value, &format!("playact_value_{}", *counter), defs);
                Some(json!({
                    "if": {
                        "properties": { (key_field): { "const": name } },
                        "required": [key_field]
                    },
                    "then": build(value)
                }))
            })
            .collect()
    }

    fn finish(self, schema: &mut Value) {
        if self.defs.is_empty() {
            return;
        }
        if let Some(defs) = schema.pointer_mut("/$defs").and_then(Value::as_object_mut) {
            for (name, definition) in self.defs {
                defs.insert(name, definition);
            }
        }
    }
}

/// Moves `$defs` from a generated params schema into the root schema.
///
/// Schemars inlines the root type and uses `$defs` for nested types. Recursion
/// back to the root is represented as `$ref: "#"`, which would resolve to the
/// whole `TreeSpec` schema after embedding. Such roots are moved into `$defs`
/// as well and their self-references are rewritten.
#[cfg(feature = "schema")]
fn relocate_defs(mut params: Value, prefix: &str, root_defs: &mut HashMap<String, Value>) -> Value {
    let root_name = has_root_ref(&params).then(|| unique_def_name(root_defs, prefix, "root"));
    if let Some(root_name) = &root_name {
        // Reserve the root name so a nested definition cannot claim it.
        root_defs.insert(root_name.clone(), Value::Null);
    }

    let defs = params
        .as_object_mut()
        .and_then(|object| object.remove("$defs"))
        .and_then(|defs| defs.as_object().cloned());
    if let Some(defs) = defs {
        let mut mapping = HashMap::with_capacity(defs.len());
        for key in defs.keys() {
            mapping.insert(key.clone(), unique_def_name(root_defs, prefix, key));
        }
        for (key, mut definition) in defs {
            let Some(unique) = mapping.get(&key).cloned() else {
                continue;
            };
            rewrite_refs(&mut definition, &mapping, root_name.as_deref());
            root_defs.insert(unique, definition);
        }
        rewrite_refs(&mut params, &mapping, root_name.as_deref());
    } else if root_name.is_some() {
        rewrite_refs(&mut params, &HashMap::new(), root_name.as_deref());
    }

    if let Some(root_name) = root_name {
        let reference = format!("#/$defs/{}", escape_pointer_token(&root_name));
        root_defs.insert(root_name, params);
        return json!({ "$ref": reference });
    }
    params
}

#[cfg(feature = "schema")]
fn has_root_ref(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object.get("$ref").and_then(Value::as_str) == Some("#")
                || object.values().any(has_root_ref)
        }
        Value::Array(array) => array.iter().any(has_root_ref),
        _ => false,
    }
}

#[cfg(feature = "schema")]
fn unique_def_name(root_defs: &HashMap<String, Value>, prefix: &str, key: &str) -> String {
    let base = format!("{prefix}_{key}");
    let mut candidate = base.clone();
    let mut suffix = 1;
    while root_defs.contains_key(&candidate) {
        candidate = format!("{base}_{suffix}");
        suffix += 1;
    }
    candidate
}

#[cfg(feature = "schema")]
fn rewrite_refs(value: &mut Value, mapping: &HashMap<String, String>, root_name: Option<&str>) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get_mut("$ref") {
                if reference == "#" {
                    if let Some(root_name) = root_name {
                        *reference = format!("#/$defs/{}", escape_pointer_token(root_name));
                    }
                } else if let Some(rest) = reference.strip_prefix("#/$defs/") {
                    let (token, suffix) = rest
                        .find('/')
                        .map_or((rest, ""), |index| (&rest[..index], &rest[index..]));
                    let key = token.replace("~1", "/").replace("~0", "~");
                    if let Some(unique) = mapping.get(&key) {
                        *reference = format!("#/$defs/{}{}", escape_pointer_token(unique), suffix);
                    }
                }
            }
            for child in object.values_mut() {
                rewrite_refs(child, mapping, root_name);
            }
        }
        Value::Array(array) => {
            for child in array {
                rewrite_refs(child, mapping, root_name);
            }
        }
        _ => {}
    }
}

#[cfg(feature = "schema")]
fn escape_pointer_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}
