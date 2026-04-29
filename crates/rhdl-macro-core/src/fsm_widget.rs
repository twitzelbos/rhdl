//! `#[derive(FsmWidget)]` — the widget-side companion to `#[derive(Fsm)]`.
//!
//! Tags a `Synchronous`-derived widget struct as the host of an
//! FSM.  Reads the helper attribute `#[fsm(state_field = "...",
//! state_enum = "Path::To::Enum"[, strict])]` and emits an
//! `impl FsmWidget for Foo` that exposes the widget's
//! [`FsmDescriptor`] for the static-analysis pass and the diagram
//! generator to consume.
//!
//! The `state_enum` argument names the type that implements
//! [`FsmState`] (typically the same enum the `#[derive(Fsm)]`
//! macro was applied to).  Both `state_field` and `state_enum` are
//! required — making them explicit beats best-effort parsing of
//! the field's `dff::DFF<E>` type because (a) it works regardless
//! of how the user wraps the DFF, and (b) the error message when
//! something's missing is precise.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DeriveInput, Expr, ExprLit, Lit, Meta, Path, Token,
    punctuated::Punctuated, spanned::Spanned,
};

#[derive(Debug, Default, Clone)]
struct FsmWidgetAttr {
    state_field: Option<String>,
    state_enum: Option<Path>,
    strict: bool,
    /// Set by `#[fsm(allow_implicit)]`.  When true, the extractor
    /// includes implicit self-loops (the canonical kernel-top
    /// default + selective override pattern) in the transition
    /// graph.  When false (default), implicit self-loops are
    /// excluded, which makes the Layer 2 `DeadlockCandidate`
    /// diagnostic fire for states whose only would-be outgoing
    /// edges are implicit holds — closing the deadlock-masking
    /// gap (`fsm-architecture.md` §5.4.1).
    allow_implicit: bool,
}

fn parse_fsm_widget_attr(attrs: &[Attribute]) -> syn::Result<FsmWidgetAttr> {
    let mut out = FsmWidgetAttr::default();
    for attr in attrs {
        if !attr.path().is_ident("fsm") {
            continue;
        }
        let nested = attr
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
        for meta in nested {
            match meta {
                Meta::Path(p) if p.is_ident("strict") => {
                    out.strict = true;
                }
                Meta::Path(p) if p.is_ident("allow_implicit") => {
                    out.allow_implicit = true;
                }
                Meta::NameValue(nv) if nv.path.is_ident("state_field") => {
                    if let Expr::Lit(ExprLit {
                        lit: Lit::Str(s), ..
                    }) = nv.value
                    {
                        out.state_field = Some(s.value());
                    } else {
                        return Err(syn::Error::new(
                            nv.path.span(),
                            "fsm(state_field = ...) expects a string literal",
                        ));
                    }
                }
                Meta::NameValue(nv) if nv.path.is_ident("state_enum") => {
                    // state_enum can be supplied as either a string literal
                    // ("Path::To::Enum") or a path expression
                    // (Path::To::Enum).  Accept both and parse uniformly.
                    let parsed = match nv.value {
                        Expr::Lit(ExprLit {
                            lit: Lit::Str(s), ..
                        }) => syn::parse_str::<Path>(&s.value()).map_err(|e| {
                            syn::Error::new(
                                s.span(),
                                format!(
                                    "fsm(state_enum = \"...\") could not be parsed as a path: {e}",
                                ),
                            )
                        })?,
                        Expr::Path(ep) => ep.path,
                        other => {
                            return Err(syn::Error::new(
                                other.span(),
                                "fsm(state_enum = ...) expects a path or a string literal naming a path",
                            ));
                        }
                    };
                    out.state_enum = Some(parsed);
                }
                // Pass through `initial = ...` since it's used by
                // the enum-side derive — the same `#[fsm(...)]`
                // attribute might appear on both the enum and the
                // widget when they share a module, and we should
                // not error on attributes meant for the enum.
                Meta::NameValue(nv) if nv.path.is_ident("initial") => {}
                other => {
                    return Err(syn::Error::new(
                        other.span(),
                        "unrecognised fsm attribute (allowed: `state_field = \"...\"`, `state_enum = ...`, `strict`, `allow_implicit`)",
                    ));
                }
            }
        }
    }
    Ok(out)
}

pub fn derive_fsm_widget(input: TokenStream) -> syn::Result<TokenStream> {
    let decl = syn::parse2::<DeriveInput>(input)?;
    let widget_name = &decl.ident;
    let (impl_generics, ty_generics, where_clause) = decl.generics.split_for_impl();

    let Data::Struct(_) = &decl.data else {
        return Err(syn::Error::new(
            decl.span(),
            "#[derive(FsmWidget)] only supports structs",
        ));
    };

    let attr = parse_fsm_widget_attr(&decl.attrs)?;
    let state_field = attr.state_field.ok_or_else(|| {
        syn::Error::new(
            decl.ident.span(),
            "#[derive(FsmWidget)] requires `#[fsm(state_field = \"...\")]`",
        )
    })?;
    let state_enum = attr.state_enum.ok_or_else(|| {
        syn::Error::new(
            decl.ident.span(),
            "#[derive(FsmWidget)] requires `#[fsm(state_enum = ...)]` (path or string)",
        )
    })?;
    let strict = attr.strict;
    let allow_implicit = attr.allow_implicit;

    // Verify the named field actually exists on the struct, so
    // the user gets an error at #[derive(FsmWidget)] expansion
    // rather than a confusing trait-bound error later.
    let fields = match &decl.data {
        Data::Struct(s) => &s.fields,
        _ => unreachable!(), // ruled out above
    };
    let has_field = fields
        .iter()
        .any(|f| f.ident.as_ref().map(|i| *i == state_field).unwrap_or(false));
    if !has_field {
        return Err(syn::Error::new(
            Span::call_site(),
            format!(
                "field `{state_field}` named in #[fsm(state_field = ...)] does not exist on `{widget_name}`",
            ),
        ));
    }

    // The widget name we report through the descriptor uses the
    // bare ident — fully-qualified module paths aren't available
    // at macro time without nightly's `module_path!()` runtime
    // call, and the bare name is what users see in diagnostics.
    let widget_name_str = widget_name.to_string();

    // The default state_var binding is `q.<state_field>` — keeps
    // the widget-side ergonomics single-attribute (the kernel
    // doesn't need to repeat the binding).  v2 will add a
    // `#[fsm_kernel(state_var = "...")]` attribute macro for the
    // non-canonical case.
    let state_var = format!("q.{state_field}");

    // A trampoline trait that ties widget → state-enum.  The
    // analysis pass and the diagram generator look up
    // `<W as FsmWidget>::fsm_descriptor()` to get all the
    // metadata they need.
    let table_ident = format_ident!("__RHDL_FSM_DESCRIPTOR_{}", widget_name);

    Ok(quote! {
        #[doc(hidden)]
        #[automatically_derived]
        impl #impl_generics #widget_name #ty_generics #where_clause {
            #[allow(non_snake_case, dead_code)]
            fn #table_ident() -> rhdl::core::fsm::FsmDescriptor {
                rhdl::core::fsm::FsmDescriptor {
                    widget_name: #widget_name_str,
                    widget: rhdl::core::fsm::FsmWidgetTag {
                        state_field: #state_field,
                        strict: #strict,
                        allow_implicit: #allow_implicit,
                    },
                    kernel: rhdl::core::fsm::FsmKernelTag {
                        state_var: #state_var,
                    },
                    variants_fn: <#state_enum as rhdl::core::fsm::FsmState>::fsm_variants,
                    initial_fn: <#state_enum as rhdl::core::fsm::FsmState>::fsm_initial_index,
                }
            }
        }

        #[automatically_derived]
        impl #impl_generics rhdl::core::fsm::FsmWidget for #widget_name #ty_generics #where_clause {
            type StateEnum = #state_enum;
            fn fsm_descriptor() -> rhdl::core::fsm::FsmDescriptor {
                Self::#table_ident()
            }
        }
    })
}

#[cfg(test)]
mod tests;
