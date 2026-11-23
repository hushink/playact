//! Proc-macro DSL and derives for `playact`.
//!
//! [`bt!`] builds a behavior tree from nested blocks and expands to
//! `BehaviorTreeBuilder` calls. [`BtNode`] derives the `Node` boilerplate for a
//! custom struct. The macros only generate code; they do not depend on an async
//! runtime or pull in domain types.
//!
//! ```ignore
//! use playact::prelude::*;
//!
//! let tree = bt!(World, "player", reactive_selector {
//!     sequence {
//!         condition("low_hp", |ctx| ctx.world_ref().hp < 0.3),
//!         action("drink", |_| Status::Success),
//!     },
//!     action("patrol", |_| Status::Running),
//! })?;
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{DeriveInput, Ident, LitStr, Token, Type, braced, parenthesized, parse_macro_input};

mod bt_node;

struct TreeInput {
    world: Type,
    name: LitStr,
    root: Node,
}

struct Node {
    kind: Ident,
    args: Option<TokenStream2>,
    children: Option<Vec<Node>>,
}

impl Parse for TreeInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let world: Type = input.parse()?;
        input.parse::<Token![,]>()?;
        let name: LitStr = input.parse()?;
        input.parse::<Token![,]>()?;
        let root: Node = input.parse()?;

        if !input.is_empty() {
            return Err(input.error("unexpected tokens after the root node"));
        }

        Ok(Self { world, name, root })
    }
}

impl Parse for Node {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let kind: Ident = input.parse()?;

        let args = if input.peek(syn::token::Paren) {
            let content;
            parenthesized!(content in input);
            Some(content.parse()?)
        } else {
            None
        };

        let children = if input.peek(syn::token::Brace) {
            let content;
            braced!(content in input);
            let nodes = Punctuated::<Node, Token![,]>::parse_terminated(&content)?;
            Some(nodes.into_iter().collect())
        } else {
            None
        };

        Ok(Self {
            kind,
            args,
            children,
        })
    }
}

/// Builds a `BehaviorTree` with the nested DSL.
///
/// Syntax:
///
/// ```text
/// bt!(WorldType, "tree name", <root node>)
/// ```
///
/// Composite nodes: `sequence`, `reactive_sequence`, `selector`,
/// `reactive_selector`, `random_selector(seed)`, `parallel(success, failure)`,
/// `mem_parallel(success, failure)`, `race`.
///
/// Decorator nodes: `inverter`, `always_succeed`, `always_fail`, `repeat(n)`,
/// `retry(n)`, `until_success`, `until_failure`, `cooldown(duration)`,
/// `timeout(duration)`, `limiter(n)`, `throttle(window, max)`,
/// `guard(condition)`; a decorator block must contain exactly one child.
///
/// Leaves: `action(name, closure)`, `condition(name, closure)`, `wait(duration)`,
/// `set_value(name, key, value)`, `raw(expression)`.
///
/// Expands to `Result<BehaviorTree<WorldType>, BuildError>`.
#[proc_macro]
pub fn bt(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as TreeInput);
    match expand_tree(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Derives the `playact::Node` implementation for a custom node.
///
/// The struct must provide an inherent method with the same signature as
/// `Node::tick`:
///
/// ```ignore
/// #[derive(playact::BtNode)]
/// #[bt(world = World, name = "my_node")]
/// struct MyNode {
///     #[bt(id)]
///     id: playact::NodeId,
///     #[bt(children)]
///     children: Vec<playact::BoxedNode<World>>,
/// }
///
/// impl MyNode {
///     fn tick(&mut self, ctx: &mut playact::Context<'_, World>) -> playact::Status {
///         playact::Status::Success
///     }
/// }
/// ```
///
/// Struct attributes:
/// - `world = Type` (required): the `W` in `Node<W>`
/// - `name = "..."`: display name; defaults to the struct name
/// - `kind = "..."`: custom kind label; defaults to the struct name
/// - `abort`, `reset`, `own_state`, `state`: generate delegation to inherent
///   `abort`, `reset`, `save_own_state`/`load_own_state`, or
///   `save_state`/`load_state` methods respectively
///
/// Field attributes:
/// - `#[bt(id)]`: field becomes `Node::id()`; accepts `NodeId` or `Option<NodeId>`
/// - `#[bt(children)]`: field becomes `Node::children()`/`children_mut()`
///
/// The world type may be a concrete type or a generic parameter of the struct.
#[proc_macro_derive(BtNode, attributes(bt))]
pub fn derive_bt_node(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match bt_node::expand(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_tree(input: TreeInput) -> syn::Result<TokenStream2> {
    let path = playact_path();
    let world = input.world;
    let name = input.name;
    let root_call = expand_node(&input.root, &path)?;

    Ok(quote! {
        #path::BehaviorTreeBuilder::<#world>::new(#name)
            #root_call
            .build()
    })
}

fn expand_node(node: &Node, path: &TokenStream2) -> syn::Result<TokenStream2> {
    let kind = node.kind.to_string();
    let node_kind = &node.kind;
    match kind.as_str() {
        "action" | "condition" | "wait" | "set_value" | "raw" => {
            let args = require_args(node)?;
            reject_children(node)?;
            Ok(quote!(.#node_kind(#args)))
        }
        "sequence" | "reactive_sequence" | "selector" | "reactive_selector" | "race" => {
            reject_args(node)?;
            let body = expand_children(require_children(node)?, path)?;
            Ok(quote!(.#node_kind(|__b| #body)))
        }
        "random_selector" => {
            let args = require_args(node)?;
            let body = expand_children(require_children(node)?, path)?;
            Ok(quote!(.random_selector(#args, |__b| #body)))
        }
        "parallel" | "mem_parallel" => {
            let args = require_args(node)?;
            let body = expand_children(require_children(node)?, path)?;
            Ok(quote!(.#node_kind(#args, |__b| #body)))
        }
        "inverter" | "always_succeed" | "always_fail" | "until_success" | "until_failure" => {
            reject_args(node)?;
            let child = require_single_child(node)?;
            let child_call = expand_node(child, path)?;
            Ok(quote!(.#node_kind(|__b| __b #child_call)))
        }
        "repeat" | "retry" | "cooldown" | "timeout" | "limiter" | "throttle" | "guard" => {
            let args = require_args(node)?;
            let child = require_single_child(node)?;
            let child_call = expand_node(child, path)?;
            Ok(quote!(.#node_kind(#args, |__b| __b #child_call)))
        }
        other => Err(syn::Error::new(
            node.kind.span(),
            format!("unknown behavior tree node `{other}`"),
        )),
    }
}

fn expand_children(children: &[Node], path: &TokenStream2) -> syn::Result<TokenStream2> {
    let mut expression = quote!(__b);
    for child in children {
        let call = expand_node(child, path)?;
        expression = quote!(#expression #call);
    }
    Ok(expression)
}

fn require_args(node: &Node) -> syn::Result<TokenStream2> {
    match &node.args {
        Some(args) if !args.is_empty() => Ok(args.clone()),
        _ => Err(syn::Error::new(
            node.kind.span(),
            format!("`{}` requires arguments in parentheses", node.kind),
        )),
    }
}

fn reject_args(node: &Node) -> syn::Result<()> {
    if node.args.as_ref().is_some_and(|args| !args.is_empty()) {
        return Err(syn::Error::new(
            node.kind.span(),
            format!("`{}` does not take arguments", node.kind),
        ));
    }
    Ok(())
}

fn require_children(node: &Node) -> syn::Result<&[Node]> {
    match &node.children {
        Some(children) => Ok(children),
        None => Err(syn::Error::new(
            node.kind.span(),
            format!("`{}` requires a `{{ ... }}` body", node.kind),
        )),
    }
}

fn reject_children(node: &Node) -> syn::Result<()> {
    if node.children.is_some() {
        return Err(syn::Error::new(
            node.kind.span(),
            format!("`{}` is a leaf and cannot have a body", node.kind),
        ));
    }
    Ok(())
}

fn require_single_child(node: &Node) -> syn::Result<&Node> {
    let children = require_children(node)?;
    if children.len() != 1 {
        return Err(syn::Error::new(
            node.kind.span(),
            format!("`{}` requires exactly one child", node.kind),
        ));
    }
    Ok(&children[0])
}

fn playact_path() -> TokenStream2 {
    match crate_name("playact") {
        Ok(FoundCrate::Itself) => quote!(crate),
        Ok(FoundCrate::Name(name)) => {
            let ident = Ident::new(&name, Span::call_site());
            quote!(::#ident)
        }
        Err(_) => quote!(::playact),
    }
}
