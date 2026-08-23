use proc_macro2::TokenStream;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Attribute, DeriveInput, Expr};

pub(crate) fn get_fqdn(decl: &DeriveInput) -> TokenStream {
    let struct_name = &decl.ident;
    if decl.generics.type_params().count() > 0 {
        let mut generics_names = decl
            .generics
            .type_params()
            .map(|x| &x.ident)
            .flat_map(|x| {
                [
                    quote!(std::any::type_name::<#x>().to_string()),
                    quote!(",".to_string()),
                ]
            })
            .collect::<Punctuated<_, syn::Token![,]>>();
        if !generics_names.is_empty() {
            generics_names.pop(); // Remove last comma string
            generics_names.pop_punct(); // Remove last punctuation
        }
        let generics_names = quote!(#generics_names);
        quote!(&vec![module_path!().to_string(), "::".to_string(), stringify!(#struct_name).to_string(), "<".to_string(),  #generics_names, ">".to_string()].join(""))
    } else {
        quote!(concat!(module_path!(), "::", stringify! (#struct_name)))
    }
}

#[cfg(test)]
pub(crate) fn pretty_print(tokens: &proc_macro2::TokenStream) -> String {
    let tokens_str = tokens.to_string();
    prettyplease::unparse(
        &syn::parse_file(&tokens_str)
            .unwrap_or_else(|err| panic!("Tokens are not valid rust code: {tokens_str}  {err}")),
    )
}

pub(crate) fn evaluate_const_expression(expr: &syn::Expr) -> syn::Result<i64> {
    let expr_as_string = quote!(#expr).to_string();
    match evalexpr::eval_int(&expr_as_string) {
        Ok(x) => Ok(x),
        Err(err) => Err(syn::Error::new(
            expr.span(),
            format!("Failed to evaluate expression: {err}"),
        )),
    }
}

pub struct FieldSet<'a> {
    pub(crate) component_name: Vec<syn::Ident>,
    pub(crate) component_ty: Vec<&'a syn::Type>,
}

impl<'a> TryFrom<&'a syn::Fields> for FieldSet<'a> {
    type Error = syn::Error;

    fn try_from(fields: &'a syn::Fields) -> syn::Result<Self> {
        let mut component_name = Vec::new();
        let mut component_ty = Vec::new();
        for field in fields.iter() {
            if parse_rhdl_skip_attribute(&field.attrs) {
                continue;
            }
            if let Some(name) = &field.ident {
                component_name.push(name.clone());
                component_ty.push(&field.ty);
            }
        }
        Ok(FieldSet {
            component_name,
            component_ty,
        })
    }
}

pub(crate) fn parse_rhdl_skip_attribute(attrs: &[Attribute]) -> bool {
    for attr in attrs {
        if attr.path().is_ident("rhdl")
            && let Ok(Expr::Path(path)) = attr.parse_args()
            && path.path.is_ident("skip")
        {
            return true;
        }
    }
    false
}

pub(crate) fn parse_dq_no_prefix_attribute(attrs: &[Attribute]) -> bool {
    for attr in attrs {
        if attr.path().is_ident("rhdl")
            && let Ok(Expr::Path(path)) = attr.parse_args()
            && path.path.is_ident("dq_no_prefix")
        {
            return true;
        }
    }
    false
}

/// Emit `Clone`, `Copy` and `PartialEq` for a generated `Q`/`D` struct
/// without bounding the type parameters.
///
/// # Why this is not `#[derive(Clone, Copy, PartialEq)]`
///
/// `#[derive]` bounds the *type parameters*, not the *field types*.
/// That is usually harmless, because a parameter normally appears in a
/// field type unchanged. The `Q` and `D` structs break that assumption:
/// their fields are the associated-type projections
/// `<C as SynchronousIO>::O` and `<C as CircuitIO>::I`, so the
/// parameter `C` does not appear in any field type once the projection
/// is normalised.
///
/// A widget generic over a sub-circuit therefore produced
/// `impl<C: Copy> Copy for Q<C>` — demanding `C: Copy` of a *circuit*,
/// which is never `Copy`. The struct was fine; the bounds were not.
///
/// No where-clause needs adding here, and that is worth stating
/// explicitly: `SynchronousIO::I`/`O` and `CircuitIO::I`/`O` are all
/// bounded by `Digital`, and `Digital: Copy + Clone + PartialEq`. The
/// field types are unconditionally all three, so the struct's own
/// where-clause is sufficient and these impls are total.
///
/// The [`crate::timed`] derive already takes this shape, adding
/// predicates over field types rather than parameters. This makes the
/// DQ derives consistent with it.
pub(crate) fn perfect_derive_value_traits(
    name: &syn::Ident,
    generics: &syn::Generics,
    field_names: &[syn::Ident],
) -> proc_macro2::TokenStream {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    // Built conditionally rather than as `true #(&& ..)*`: a trailing
    // conjunction with a literal trips `clippy::nonminimal_bool`, and
    // this expands into user crates where their lint settings apply.
    let eq_body = match field_names.split_first() {
        None => quote::quote! { true },
        Some((first, rest)) => {
            quote::quote! { self.#first == other.#first #(&& self.#rest == other.#rest)* }
        }
    };
    quote::quote! {
        impl #impl_generics ::std::clone::Clone for #name #ty_generics #where_clause {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl #impl_generics ::std::marker::Copy for #name #ty_generics #where_clause {}

        impl #impl_generics ::std::cmp::PartialEq for #name #ty_generics #where_clause {
            fn eq(&self, other: &Self) -> bool {
                #eq_body
            }
        }
    }
}
