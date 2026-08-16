//! `#[fsm_properties(...)]` — the SVA-property attribute macro.
//!
//! Wraps a kernel function and emits a static slice of
//! [`FsmProperty`](rhdl_core::fsm::FsmProperty) declarations
//! plus an `impl FsmKernelProperties` for a marker type tied to
//! the kernel.  Layer 4 of `fsm-architecture.md`.
//!
//! Syntax:
//!
//! ```ignore
//! #[fsm_properties(
//!     invariant("state != State::Error", name = "no_error"),
//!     cover("state == State::Done"),
//!     liveness("state == State::Done", bound = 1024),
//!     assume("input.valid"),
//! )]
//! #[kernel]
//! pub fn my_machine(...) -> (Out, D) { ... }
//! ```
//!
//! Each declaration takes a string literal as its primary
//! argument (the property's expression body) plus optional
//! `name = "..."` and `bound = N` named arguments.  Liveness
//! properties accept `bound` for the bounded form; everything
//! else ignores it.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{
    Expr, ExprCall, ExprLit, ItemFn, Lit, Token, parse2, punctuated::Punctuated, spanned::Spanned,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PropKind {
    Invariant,
    Liveness,
    Cover,
    Assume,
}

impl PropKind {
    fn parse(s: &str, span: Span) -> syn::Result<Self> {
        match s {
            "invariant" => Ok(Self::Invariant),
            "liveness" => Ok(Self::Liveness),
            "cover" => Ok(Self::Cover),
            "assume" => Ok(Self::Assume),
            _ => Err(syn::Error::new(
                span,
                "unknown fsm property kind (allowed: invariant, liveness, cover, assume)",
            )),
        }
    }

    fn rust_variant(self) -> TokenStream {
        match self {
            Self::Invariant => quote! { rhdl::core::fsm::FsmPropertyKind::Invariant },
            Self::Liveness => quote! { rhdl::core::fsm::FsmPropertyKind::Liveness },
            Self::Cover => quote! { rhdl::core::fsm::FsmPropertyKind::Cover },
            Self::Assume => quote! { rhdl::core::fsm::FsmPropertyKind::Assume },
        }
    }
}

#[derive(Debug)]
struct ParsedProperty {
    kind: PropKind,
    expression: String,
    name: Option<String>,
    bound: Option<u64>,
}

fn parse_one_call(call: &ExprCall) -> syn::Result<ParsedProperty> {
    // First positional arg is required and must be a string literal.
    let kind_ident = match call.func.as_ref() {
        Expr::Path(p) if p.path.segments.len() == 1 => {
            let ident = &p.path.segments[0].ident;
            PropKind::parse(&ident.to_string(), ident.span())?
        }
        other => {
            return Err(syn::Error::new(
                other.span(),
                "fsm_properties entry must be a call expression like `invariant(\"...\", ...)`",
            ));
        }
    };

    let mut args_iter = call.args.iter();
    let expr_arg = args_iter.next().ok_or_else(|| {
        syn::Error::new(
            call.span(),
            "fsm_properties entry requires at least one positional argument (the property expression as a string literal)",
        )
    })?;
    let expression = match expr_arg {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => s.value(),
        other => {
            return Err(syn::Error::new(
                other.span(),
                "fsm_properties expression argument must be a string literal",
            ));
        }
    };

    let mut name = None;
    let mut bound = None;
    for extra in args_iter {
        let assign = match extra {
            Expr::Assign(a) => a,
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "fsm_properties extra arguments must be name = value pairs",
                ));
            }
        };
        let key = match assign.left.as_ref() {
            Expr::Path(p) if p.path.segments.len() == 1 => p.path.segments[0].ident.to_string(),
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "fsm_properties argument key must be a bare identifier",
                ));
            }
        };
        match key.as_str() {
            "name" => match assign.right.as_ref() {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(s), ..
                }) => {
                    name = Some(s.value());
                }
                other => {
                    return Err(syn::Error::new(
                        other.span(),
                        "fsm_properties name = ... expects a string literal",
                    ));
                }
            },
            "bound" => match assign.right.as_ref() {
                Expr::Lit(ExprLit {
                    lit: Lit::Int(i), ..
                }) => {
                    bound = Some(i.base10_parse::<u64>()?);
                }
                other => {
                    return Err(syn::Error::new(
                        other.span(),
                        "fsm_properties bound = ... expects an integer literal",
                    ));
                }
            },
            _ => {
                return Err(syn::Error::new(
                    assign.left.span(),
                    "fsm_properties argument key must be `name` or `bound`",
                ));
            }
        }
    }

    Ok(ParsedProperty {
        kind: kind_ident,
        expression,
        name,
        bound,
    })
}

pub fn fsm_properties(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    // Parse the attribute body as a comma-separated list of call
    // expressions.
    let entries = if attr.is_empty() {
        Vec::new()
    } else {
        let parser = Punctuated::<Expr, Token![,]>::parse_terminated;
        let raw = syn::parse::Parser::parse2(parser, attr)?;
        let mut out = Vec::new();
        for expr in raw {
            match expr {
                Expr::Call(call) => out.push(parse_one_call(&call)?),
                other => {
                    return Err(syn::Error::new(
                        other.span(),
                        "fsm_properties entries must be call expressions like `invariant(\"...\", ...)`",
                    ));
                }
            }
        }
        out
    };

    // Parse the wrapped function and stitch the metadata in next
    // to it.  We pass the function body through unchanged — this
    // attribute macro is metadata-only.
    let func: ItemFn = parse2(item)?;
    let fn_name = &func.sig.ident;

    // Marker type for the FsmKernelProperties impl.  Naming it
    // FsmProps_<kernel> mirrors the pattern used elsewhere in the
    // tree for kernel-attached marker types.
    let marker_ident = format_ident!("FsmProps_{}", fn_name);

    let table_ident = format_ident!("__RHDL_FSM_PROPERTIES_{}", fn_name);
    let n = entries.len();

    let rows = entries.iter().enumerate().map(|(idx, p)| {
        let name = p
            .name
            .clone()
            .unwrap_or_else(|| format!("{fn_name}_prop_{idx}"));
        let kind_tokens = p.kind.rust_variant();
        let expression = &p.expression;
        let bound_tokens = match p.bound {
            Some(n) => quote! { Some(#n) },
            None => quote! { None },
        };
        quote! {
            rhdl::core::fsm::FsmProperty {
                kind: #kind_tokens,
                name: #name,
                expression: #expression,
                bound: #bound_tokens,
            }
        }
    });

    Ok(quote! {
        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        const #table_ident: [rhdl::core::fsm::FsmProperty; #n] = [
            #( #rows ),*
        ];

        /// Marker type — auto-generated by `#[fsm_properties(...)]`.
        /// Look up properties via
        /// `<FsmProps_<kernel> as FsmKernelProperties>::fsm_properties()`.
        #[allow(non_camel_case_types, dead_code)]
        pub struct #marker_ident;

        #[automatically_derived]
        impl rhdl::core::fsm::FsmKernelProperties for #marker_ident {
            fn fsm_properties() -> &'static [rhdl::core::fsm::FsmProperty] {
                &#table_ident
            }
        }

        #func
    })
}

#[cfg(test)]
mod tests;
