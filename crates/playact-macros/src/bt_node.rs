use proc_macro2::TokenStream;
use quote::quote;
use syn::meta::ParseNestedMeta;
use syn::{Data, DeriveInput, Ident, LitStr, Type};

use crate::playact_path;

#[derive(Default)]
struct Attrs {
    world: Option<Type>,
    name: Option<LitStr>,
    kind: Option<LitStr>,
    abort: bool,
    reset: bool,
    own_state: bool,
    state: bool,
}

pub(crate) fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    let path = playact_path();
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "BtNode can only be derived for structs",
        ));
    };

    let mut attrs = Attrs::default();
    for attr in &input.attrs {
        if attr.path().is_ident("bt") {
            attr.parse_nested_meta(|meta| parse_struct_attr(&mut attrs, meta))?;
        }
    }

    let world = attrs.world.ok_or_else(|| {
        syn::Error::new_spanned(&input.ident, "BtNode requires #[bt(world = Type)]")
    })?;
    let name = attrs
        .name
        .unwrap_or_else(|| LitStr::new(&input.ident.to_string(), input.ident.span()));
    let kind = attrs
        .kind
        .unwrap_or_else(|| LitStr::new(&input.ident.to_string(), input.ident.span()));

    let mut id_field: Option<&Ident> = None;
    let mut children_field: Option<&Ident> = None;
    for field in &data.fields {
        for attr in &field.attrs {
            if !attr.path().is_ident("bt") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("id") {
                    let ident = field
                        .ident
                        .as_ref()
                        .ok_or_else(|| meta.error("`#[bt(id)]` requires a named field"))?;
                    if id_field.replace(ident).is_some() {
                        return Err(meta.error("duplicate `#[bt(id)]` field"));
                    }
                    Ok(())
                } else if meta.path.is_ident("children") {
                    let ident = field
                        .ident
                        .as_ref()
                        .ok_or_else(|| meta.error("`#[bt(children)]` requires a named field"))?;
                    if children_field.replace(ident).is_some() {
                        return Err(meta.error("duplicate `#[bt(children)]` field"));
                    }
                    Ok(())
                } else {
                    Err(meta.error("unknown field attribute; expected `id` or `children`"))
                }
            })?;
        }
    }

    let id_field = id_field.map(|field| {
        let ty = &data
            .fields
            .iter()
            .find(|candidate| candidate.ident.as_ref() == Some(field))
            .expect("id field exists")
            .ty;
        let value = if is_option(ty) {
            quote!(self.#field.as_ref())
        } else {
            quote!(Some(&self.#field))
        };
        quote! {
            fn id(&self) -> Option<&#path::NodeId> {
                #value
            }
        }
    });

    let children_methods = children_field.map(|field| {
        quote! {
            fn children(&self) -> &[#path::BoxedNode<#world>] {
                &self.#field
            }

            fn children_mut(&mut self) -> &mut [#path::BoxedNode<#world>] {
                &mut self.#field
            }
        }
    });

    let abort_method = attrs.abort.then(|| {
        quote! {
            fn abort(&mut self, ctx: &mut #path::Context<'_, #world>) {
                Self::abort(self, ctx);
            }
        }
    });
    let reset_method = attrs.reset.then(|| {
        quote! {
            fn reset(&mut self) {
                Self::reset(self);
            }
        }
    });
    let own_state_methods = attrs.own_state.then(|| {
        quote! {
            fn save_own_state(&self) -> Option<#path::__private::Vec<u8>> {
                Self::save_own_state(self)
            }

            fn load_own_state(&mut self, state: &[u8]) -> bool {
                Self::load_own_state(self, state)
            }
        }
    });
    let state_methods = attrs.state.then(|| {
        quote! {
            fn save_state(&self) -> Option<#path::__private::Vec<u8>> {
                Self::save_state(self)
            }

            fn load_state(&mut self, state: &[u8]) -> bool {
                Self::load_state(self, state)
            }
        }
    });

    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics #path::Node<#world> for #ident #ty_generics #where_clause {
            fn tick(&mut self, ctx: &mut #path::Context<'_, #world>) -> #path::Status {
                Self::tick(self, ctx)
            }

            #abort_method
            #reset_method

            fn name(&self) -> &str {
                #name
            }

            fn kind(&self) -> #path::Kind {
                #path::Kind::Custom(#kind)
            }

            #id_field
            #children_methods
            #own_state_methods
            #state_methods
        }
    })
}

fn parse_struct_attr(attrs: &mut Attrs, meta: ParseNestedMeta<'_>) -> syn::Result<()> {
    if meta.path.is_ident("world") {
        attrs.world = Some(meta.value()?.parse()?);
    } else if meta.path.is_ident("name") {
        attrs.name = Some(meta.value()?.parse()?);
    } else if meta.path.is_ident("kind") {
        attrs.kind = Some(meta.value()?.parse()?);
    } else if meta.path.is_ident("abort") {
        attrs.abort = true;
    } else if meta.path.is_ident("reset") {
        attrs.reset = true;
    } else if meta.path.is_ident("own_state") {
        attrs.own_state = true;
    } else if meta.path.is_ident("state") {
        attrs.state = true;
    } else {
        return Err(meta.error(
            "unknown #[bt(...)] option; expected world, name, kind, abort, reset, own_state, or state",
        ));
    }
    Ok(())
}

fn is_option(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Path(path)
            if path
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "Option")
    )
}
