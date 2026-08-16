//! `#[derive(Fsm)]` and the related FSM attribute family.
//!
//! `derive_fsm` produces an `impl FsmState for E` for any enum the
//! macro is applied to.  The implementation reflects the source
//! enum's variant order, computes discriminants the same way
//! `#[derive(Digital)]` does (auto-numbered from 0 unless the
//! source pins them), and surfaces `#[fsm_state(label = "...",
//! terminal)]` per-variant decorations.
//!
//! `derive_fsm` does NOT emit a `Digital` impl — that is the
//! orthogonal `#[derive(Digital)]` macro's job.  The two derives
//! co-exist on the same enum:
//!
//! ```ignore
//! #[derive(Fsm, PartialEq, Digital, Copy, Clone, Debug, Default)]
//! pub enum State { ... }
//! ```
//!
//! See `fsm-architecture.md` §4.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{
    Attribute, Data, DeriveInput, Expr, ExprLit, Lit, Meta, Token, Variant, punctuated::Punctuated,
};

use crate::digital_enum::allocate_discriminants;
use crate::utils::evaluate_const_expression;

/// Per-variant decoration parsed from `#[fsm_state(...)]` attrs.
#[derive(Debug, Default, Clone)]
struct FsmStateAttr {
    /// Optional display label.
    label: Option<String>,
    /// Whether the variant was marked `terminal`.
    terminal: bool,
}

/// Enum-level decoration parsed from `#[fsm(...)]` attrs on the
/// state enum.
#[derive(Debug, Default, Clone)]
struct FsmEnumAttr {
    /// Optional override of the initial variant's name.  Defaults
    /// to the variant marked `#[default]`, or variant 0 if none.
    initial: Option<String>,
}

fn parse_fsm_state_attr(attrs: &[Attribute]) -> syn::Result<FsmStateAttr> {
    let mut out = FsmStateAttr::default();
    for attr in attrs {
        if !attr.path().is_ident("fsm_state") {
            continue;
        }
        let nested = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
        for meta in nested {
            match meta {
                Meta::Path(p) if p.is_ident("terminal") => {
                    out.terminal = true;
                }
                Meta::NameValue(nv) if nv.path.is_ident("label") => {
                    if let Expr::Lit(ExprLit {
                        lit: Lit::Str(s), ..
                    }) = nv.value
                    {
                        out.label = Some(s.value());
                    } else {
                        return Err(syn::Error::new(
                            nv.path.span(),
                            "fsm_state(label = ...) expects a string literal",
                        ));
                    }
                }
                other => {
                    return Err(syn::Error::new(
                        other.span(),
                        "unrecognised fsm_state attribute (allowed: `terminal`, `label = \"...\"`)",
                    ));
                }
            }
        }
    }
    Ok(out)
}

fn parse_fsm_enum_attr(attrs: &[Attribute]) -> syn::Result<FsmEnumAttr> {
    let mut out = FsmEnumAttr::default();
    for attr in attrs {
        if !attr.path().is_ident("fsm") {
            continue;
        }
        let nested = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
        for meta in nested {
            match meta {
                Meta::NameValue(nv) if nv.path.is_ident("initial") => {
                    if let Expr::Lit(ExprLit {
                        lit: Lit::Str(s), ..
                    }) = nv.value
                    {
                        out.initial = Some(s.value());
                    } else {
                        return Err(syn::Error::new(
                            nv.path.span(),
                            "fsm(initial = ...) expects a string literal naming a variant",
                        ));
                    }
                }
                other => {
                    return Err(syn::Error::new(
                        other.span(),
                        "unrecognised fsm attribute on enum (allowed: `initial = \"VariantName\"`)",
                    ));
                }
            }
        }
    }
    Ok(out)
}

fn variant_has_default(v: &Variant) -> bool {
    v.attrs.iter().any(|a| a.path().is_ident("default"))
}

fn variant_has_payload(v: &Variant) -> bool {
    !matches!(v.fields, syn::Fields::Unit)
}

/// Strip attributes the FSM-derive consumed.  Without this the
/// downstream `#[derive(Digital)]` and `rustc` itself would reject
/// the unknown attribute names.
///
/// We do NOT actually remove them from the source — the macro
/// can't mutate the user's tokens.  Instead we rely on the user
/// declaring `#[fsm_state(...)]` and `#[fsm(...)]` as inert
/// attributes via the proc-macro's `attributes(...)` clause in
/// `rhdl-macro/src/lib.rs`.
fn _attrs_are_inert() {}

pub fn derive_fsm(input: TokenStream) -> syn::Result<TokenStream> {
    let decl = syn::parse2::<DeriveInput>(input)?;
    let enum_name = &decl.ident;
    let (impl_generics, ty_generics, where_clause) = decl.generics.split_for_impl();

    let Data::Enum(e) = decl.data else {
        return Err(syn::Error::new(
            decl.span(),
            "#[derive(Fsm)] only supports enums",
        ));
    };

    let enum_attr = parse_fsm_enum_attr(&decl.attrs)?;

    // Validate per-variant decorations once before we begin
    // emitting tokens; produces a usable error if a user typoed
    // an attribute key.
    let per_variant: Vec<(FsmStateAttr, &Variant)> = e
        .variants
        .iter()
        .map(|v| Ok::<_, syn::Error>((parse_fsm_state_attr(&v.attrs)?, v)))
        .collect::<syn::Result<_>>()?;

    // Allocate discriminants, mirroring the digital_enum derive's
    // logic so the FsmVariantDescriptor's discriminant column
    // matches what Digital sees.
    let raw_discriminants: Vec<Option<i64>> = e
        .variants
        .iter()
        .map(|v| {
            v.discriminant
                .as_ref()
                .map(|x| &x.1)
                .map(evaluate_const_expression)
        })
        .map(|x| x.transpose())
        .collect::<Result<Vec<_>, _>>()?;
    let discriminants = allocate_discriminants(&raw_discriminants);

    // Sanity-check the optional `#[fsm(initial = "...")]` against
    // the actual variant list.  If the user pinned an initial
    // that isn't a real variant, refuse to compile rather than
    // silently fall through.
    let initial_index = if let Some(name) = &enum_attr.initial {
        match e.variants.iter().position(|v| v.ident == name.as_str()) {
            Some(i) => i,
            None => {
                return Err(syn::Error::new(
                    decl.ident.span(),
                    format!(
                        "fsm(initial = \"{name}\") names a variant that does not exist on enum `{enum_name}`",
                    ),
                ));
            }
        }
    } else {
        // Default to the variant marked `#[default]`, or 0 if none.
        e.variants.iter().position(variant_has_default).unwrap_or(0)
    };

    // Build the per-variant rows of the static metadata slice.
    let variant_rows = per_variant
        .iter()
        .zip(discriminants.iter())
        .map(|((attr, v), &disc)| {
            let name_str = v.ident.to_string();
            let payload = variant_has_payload(v);
            let terminal = attr.terminal;
            let label_tokens = match &attr.label {
                Some(s) => quote! { Some(#s) },
                None => quote! { None },
            };
            quote! {
                rhdl::core::fsm::FsmVariantDescriptor {
                    name: #name_str,
                    discriminant: #disc as i128,
                    has_payload: #payload,
                    terminal: #terminal,
                    label: #label_tokens,
                }
            }
        });

    // The variant index is computed by matching on `self`.
    let variant_index_arms = e.variants.iter().enumerate().map(|(idx, v)| {
        let ident = &v.ident;
        let pattern_ignore = match &v.fields {
            syn::Fields::Unit => quote! {},
            syn::Fields::Unnamed(_) => quote! { (..) },
            syn::Fields::Named(_) => quote! { { .. } },
        };
        quote! {
            #enum_name::#ident #pattern_ignore => #idx
        }
    });

    // Storage for the static slice.  We give it a name derived
    // from the enum so multiple FSM enums in the same module
    // don't collide.
    let table_ident = format_ident!("__RHDL_FSM_VARIANTS_{}", enum_name);

    let n_variants = e.variants.len();

    Ok(quote! {
        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        const #table_ident: [rhdl::core::fsm::FsmVariantDescriptor; #n_variants] = [
            #( #variant_rows ),*
        ];

        #[automatically_derived]
        impl #impl_generics rhdl::core::fsm::FsmState for #enum_name #ty_generics #where_clause {
            fn fsm_variants() -> &'static [rhdl::core::fsm::FsmVariantDescriptor] {
                &#table_ident
            }
            fn fsm_initial_index() -> usize {
                #initial_index
            }
            fn fsm_variant_index(&self) -> usize {
                match self {
                    #( #variant_index_arms ),*
                }
            }
        }
    })
}

#[cfg(test)]
mod tests;
