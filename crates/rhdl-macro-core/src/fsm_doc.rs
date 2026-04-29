//! `#[fsm_doc]` — opt-in attribute that auto-includes the FSM
//! diagram for a widget into its struct-level rustdoc.
//!
//! Convention: the included markdown file lives at
//! `<CARGO_MANIFEST_DIR>/doc/<WidgetName>_fsm.md` (the default).
//! The `#[fsm_doc(file = "...")]` form lets the author override
//! the filename for the rare case where the struct name and the
//! conventional filename diverge.
//!
//! The included file is materialised by calling
//! [`rhdl_fpga::doc::write_fsm_diagram::<W>(filename)`] from a
//! widget test or example.  The drift-check helper
//! [`rhdl_fpga::doc::assert_fsm_diagram_up_to_date::<W>(filename)`]
//! catches stale files at `cargo test` time.
//!
//! This is the Phase 3c piece of `fsm-architecture.md`: it removes
//! the per-widget `#![doc = include_str!("../../doc/<name>_fsm.md")]`
//! boilerplate by automating the `include_str!` line via the
//! attribute macro.  The file-materialisation step still happens
//! at example-run time (for now); a future Phase 3d would automate
//! that via build.rs.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    DeriveInput, Expr, ExprLit, Lit, Meta, Token,
    parse::Parser,
    punctuated::Punctuated,
    spanned::Spanned,
};

#[derive(Default)]
struct FsmDocArgs {
    file: Option<String>,
}

fn parse_args(args: TokenStream) -> syn::Result<FsmDocArgs> {
    let mut out = FsmDocArgs::default();
    if args.is_empty() {
        return Ok(out);
    }
    let parser = Punctuated::<Meta, Token![,]>::parse_terminated;
    let metas = parser.parse2(args)?;
    for meta in metas {
        match meta {
            Meta::NameValue(nv) if nv.path.is_ident("file") => {
                if let Expr::Lit(ExprLit {
                    lit: Lit::Str(s), ..
                }) = nv.value
                {
                    out.file = Some(s.value());
                } else {
                    return Err(syn::Error::new(
                        nv.path.span(),
                        "fsm_doc(file = ...) expects a string literal",
                    ));
                }
            }
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "unrecognised fsm_doc argument (allowed: `file = \"...\"`)",
                ));
            }
        }
    }
    Ok(out)
}

pub fn fsm_doc(args: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let args = parse_args(args)?;
    let decl = syn::parse2::<DeriveInput>(input)?;
    let widget_name = decl.ident.to_string();
    let filename = args.file.unwrap_or_else(|| format!("{widget_name}_fsm.md"));
    let include_path = format!("/doc/{filename}");
    Ok(quote! {
        #[doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), #include_path))]
        #decl
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_args_uses_struct_name_for_filename() {
        let attrs: TokenStream = TokenStream::new();
        let input: TokenStream = "pub struct MyWidget { state: u8 }".parse().unwrap();
        let out = fsm_doc(attrs, input).unwrap().to_string();
        assert!(out.contains("MyWidget_fsm.md"));
        assert!(out.contains("CARGO_MANIFEST_DIR"));
        assert!(out.contains("pub struct MyWidget"));
    }

    #[test]
    fn explicit_file_argument_overrides_default() {
        let attrs: TokenStream = "file = \"custom.md\"".parse().unwrap();
        let input: TokenStream = "pub struct Other {}".parse().unwrap();
        let out = fsm_doc(attrs, input).unwrap().to_string();
        assert!(out.contains("custom.md"));
        assert!(!out.contains("Other_fsm.md"));
    }

    #[test]
    fn unknown_argument_errors() {
        let attrs: TokenStream = "bogus = \"x\"".parse().unwrap();
        let input: TokenStream = "pub struct X {}".parse().unwrap();
        assert!(fsm_doc(attrs, input).is_err());
    }

    #[test]
    fn non_string_file_argument_errors() {
        let attrs: TokenStream = "file = 42".parse().unwrap();
        let input: TokenStream = "pub struct X {}".parse().unwrap();
        assert!(fsm_doc(attrs, input).is_err());
    }
}
