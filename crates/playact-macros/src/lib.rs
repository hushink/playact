//! `playact` 的 proc-macro DSL。
//!
//! 主入口是 [`bt!`]：用嵌套块构造行为树，展开为 `BehaviorTreeBuilder`
//! 调用。宏只生成代码，不依赖 async runtime，也不引入业务类型。
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
use syn::{Ident, LitStr, Token, Type, braced, parenthesized, parse_macro_input};

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

/// 用嵌套 DSL 构建 `BehaviorTree`。
///
/// 语法：
///
/// ```text
/// bt!(WorldType, "tree name", <root node>)
/// ```
///
/// 组合节点：`sequence`、`reactive_sequence`、`selector`、
/// `reactive_selector`、`random_selector(seed)`、`parallel(success, failure)`、
/// `mem_parallel(success, failure)`。
///
/// 装饰节点：`inverter`、`always_succeed`、`always_fail`、`repeat(n)`、
/// `retry(n)`、`until_success`、`until_failure`、`cooldown(duration)`、
/// `timeout(duration)`、`limiter(n)`；装饰块必须恰好一个子节点。
///
/// 叶子：`action(name, closure)`、`condition(name, closure)`、`wait(duration)`、
/// `raw(expression)`。
///
/// 展开结果是 `Result<BehaviorTree<WorldType>, BuildError>`。
#[proc_macro]
pub fn bt(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as TreeInput);
    match expand_tree(input) {
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
        "action" | "condition" | "wait" | "raw" => {
            let args = require_args(node)?;
            reject_children(node)?;
            Ok(quote!(.#node_kind(#args)))
        }
        "sequence" | "reactive_sequence" | "selector" | "reactive_selector" => {
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
        "repeat" | "retry" | "cooldown" | "timeout" | "limiter" => {
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
