//! Implementation of the `rhdl-rule` proc-macros (Phase 0).
//!
//! See `rule-architecture.md` in the repo root for the design.
//! This crate is the implementation companion to the proc-macro
//! shim in `rhdl-rule`.
//!
//! # Phase 0 scope
//!
//! - **One macro:** [`expand_rule_kernel`] — function-like
//!   `rule_kernel! { struct + impl }` invocation.
//! - **Simplified scheduler:** rules fire in source-code order;
//!   later rules' writes overwrite earlier ones (last-write-wins).
//!   No formal conflict-matrix analysis; no priority annotations.
//!   Sufficient for non-conflicting rule sets and many real
//!   widgets.  Phase 1 of the plan adds the conflict matrix.
//! - **Recognised macro vocabulary** inside rule bodies:
//!   - `guard!(expr)` — append a guard expression.  All guards
//!     are conjoined; the rule fires only if every guard is true.
//!   - `set!(ctx.field, value)` — schedule a write to register
//!     `field` with the given value.
//!   - `*ctx.field` — read register `field` (rewritten to
//!     `q.field` in the generated kernel).
//! - **One `#[output]` method** per rule kernel.  Computes the
//!   widget's output combinationally from the post-input state
//!   (which in Phase 0 is `q`, the pre-firing snapshot).
//!
//! # What's NOT in Phase 0
//!
//! - Multiple `#[rule]` methods that write the same field cause
//!   "last in source order wins"; the conflict matrix and the
//!   priority-arbitrated scheduler from `rule-architecture.md`
//!   §6–§7 are Phase 1.
//! - The `Reg<T>` ergonomic alias is deferred (users use
//!   `dff::DFF<T>` directly) until the runtime-crate split is sorted.
//! - Annotations beyond presence/absence (`urgent_before`,
//!   `conflict_free`, `mutually_exclusive`) are Phase 2.
//! - Cross-domain rules; multi-clock; method system — non-goals
//!   per the plan.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, ToTokens};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::visit_mut::{self, VisitMut};
use syn::{
    parse2, Block, Expr, ExprMacro, FnArg, Ident, ImplItem, ImplItemFn, Item, ItemImpl, ItemStruct,
    Macro, Pat, Path, ReturnType, Type, TypeReference,
};

/// The macro's input: a struct followed by an impl block.
struct RuleKernelInput {
    item_struct: ItemStruct,
    item_impl: ItemImpl,
}

impl Parse for RuleKernelInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut items: Vec<Item> = Vec::new();
        while !input.is_empty() {
            items.push(input.parse()?);
        }
        if items.len() != 2 {
            return Err(syn::Error::new(
                Span::call_site(),
                format!(
                    "rule_kernel! expects exactly two items (a struct followed by an impl block); \
                     got {} items.",
                    items.len()
                ),
            ));
        }
        let mut iter = items.into_iter();
        let item_struct = match iter.next().unwrap() {
            Item::Struct(s) => s,
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "rule_kernel! expected the first item to be a struct definition",
                ));
            }
        };
        let item_impl = match iter.next().unwrap() {
            Item::Impl(i) => i,
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "rule_kernel! expected the second item to be an impl block",
                ));
            }
        };
        Ok(Self {
            item_struct,
            item_impl,
        })
    }
}

/// One rule, parsed from a `#[rule]` method.
struct Rule {
    name: Ident,
    /// Name of the input parameter (the second arg, after ctx).
    input_name: Ident,
    /// Type of the input parameter.
    input_type: Type,
    /// All `guard!(...)` expressions, in source order.
    guards: Vec<Expr>,
    /// All `set!(ctx.field, value)` actions, in source order.
    actions: Vec<Action>,
}

struct Action {
    /// The register field name (e.g. `counter`).
    field: Ident,
    /// The expression assigned to the register.
    value: Expr,
}

/// The `#[output]` method, parsed.
struct OutputMethod {
    /// The input parameter (second arg).
    input_name: Ident,
    input_type: Type,
    /// The return type.
    return_type: Type,
    /// The body, with `*self_q.field` rewritten to `q.field`.
    body: Block,
}

/// Public entry point used by the proc-macro shim.
pub fn expand_rule_kernel(input: TokenStream) -> syn::Result<TokenStream> {
    let RuleKernelInput {
        item_struct,
        item_impl,
    } = parse2(input)?;

    // Validate: the impl's self-type matches the struct's name.
    let struct_name = &item_struct.ident;
    let impl_name = match &*item_impl.self_ty {
        Type::Path(p) => p.path.segments.last().map(|s| &s.ident),
        _ => None,
    };
    match impl_name {
        Some(name) if name == struct_name => {}
        _ => {
            return Err(syn::Error::new(
                item_impl.self_ty.span(),
                format!(
                    "rule_kernel! expected the impl block to be `impl {struct_name}`; \
                     got something else",
                ),
            ));
        }
    }

    // Walk impl items, classify.
    let mut rules: Vec<Rule> = Vec::new();
    let mut output: Option<OutputMethod> = None;
    let mut other_items: Vec<ImplItem> = Vec::new();
    for item in &item_impl.items {
        if let ImplItem::Fn(method) = item {
            if has_attr(method, "rule") {
                rules.push(parse_rule(method)?);
                continue;
            }
            if has_attr(method, "output") {
                if output.is_some() {
                    return Err(syn::Error::new(
                        method.sig.ident.span(),
                        "rule_kernel! permits at most one #[output] method per rule kernel",
                    ));
                }
                output = Some(parse_output(method)?);
                continue;
            }
        }
        other_items.push(item.clone());
    }

    if rules.is_empty() {
        return Err(syn::Error::new(
            item_impl.span(),
            "rule_kernel! requires at least one #[rule] method in the impl block",
        ));
    }
    let output = match output {
        Some(o) => o,
        None => {
            return Err(syn::Error::new(
                item_impl.span(),
                "rule_kernel! requires exactly one #[output] method in the impl block",
            ));
        }
    };

    // Validate: every rule's input type matches the output method's
    // input type.  Phase 0: single In type for the whole widget.
    for rule in &rules {
        if !types_equal(&rule.input_type, &output.input_type) {
            return Err(syn::Error::new(
                rule.input_type.span(),
                format!(
                    "rule_kernel! Phase 0 requires every #[rule] and the #[output] to share \
                     the same input type; rule `{}` has a different input type than #[output]",
                    rule.name,
                ),
            ));
        }
    }

    // Generate.
    let kernel_fn_name = lower_camel_to_snake(struct_name);
    let kernel_fn_ident = format_ident!("{kernel_fn_name}");

    // Augment the struct with the standard derives.
    let mut struct_emit = item_struct.clone();
    inject_derives(&mut struct_emit);

    // Collect field names for default-hold and reset.
    let field_names: Vec<&Ident> = item_struct
        .fields
        .iter()
        .filter_map(|f| f.ident.as_ref())
        .collect();

    // For each register field, emit a chain of let-rebindings:
    //   let _next_<field> = q.<field>;                    (default hold)
    //   let _next_<field> = if rule_fires { value } else { _next_<field> }; (per rule that writes)
    // At the end, build D as a single struct expression from
    // _next_<field> bindings.  This matches the canonical RHDL
    // kernel pattern (no `let mut` rebinding of D).
    let mut next_decls: Vec<TokenStream> = Vec::new();
    for field in &field_names {
        let next_ident = format_ident!("_next_{field}");
        next_decls.push(quote! { let #next_ident = q.#field; });
    }
    // Emit each rule as a sequence of guarded conditional rebindings.
    for rule in &rules {
        let guard_expr: Expr = if rule.guards.is_empty() {
            syn::parse_quote! { true }
        } else {
            let mut iter = rule.guards.iter();
            let first = iter.next().unwrap();
            let rest = iter;
            syn::parse_quote! { (#first) #( && (#rest) )* }
        };
        for action in &rule.actions {
            let field = &action.field;
            let value = &action.value;
            let next_ident = format_ident!("_next_{field}");
            next_decls.push(quote! {
                let #next_ident = if (#guard_expr) { #value } else { #next_ident };
            });
        }
    }

    // Final D struct expression.
    let d_field_inits: Vec<TokenStream> = field_names
        .iter()
        .map(|f| {
            let next_ident = format_ident!("_next_{f}");
            quote! { #f: #next_ident }
        })
        .collect();

    // Output computation.
    let output_block = emit_output_block(&output);

    let input_type = &output.input_type;
    let output_type = &output.return_type;
    // The kernel function's input parameter must have the same
    // name that the rule bodies use to refer to it.  Phase 0
    // requires every rule to use the same input *name*; we use the
    // first rule's name as the canonical one and validate the rest.
    let in_param = &rules[0].input_name;
    for rule in rules.iter().skip(1) {
        if rule.input_name != *in_param {
            return Err(syn::Error::new(
                rule.input_name.span(),
                format!(
                    "rule_kernel! Phase 0 requires every #[rule] to use the same input \
                     parameter name; rule `{}` uses `{}` but the first rule uses `{}`",
                    rule.name, rule.input_name, in_param,
                ),
            ));
        }
    }
    // The output method's input name may differ; we shadow it
    // inside the output block so the body sees its declared name.
    let output_input_name = &output.input_name;

    // Other items (non-rule, non-output methods) are preserved.
    let other_items_out = if other_items.is_empty() {
        quote! {}
    } else {
        quote! {
            impl #struct_name {
                #(#other_items)*
            }
        }
    };

    let expanded = quote! {
        #struct_emit

        impl ::rhdl::core::circuit::synchronous::SynchronousIO for #struct_name {
            type I = #input_type;
            type O = #output_type;
            type Kernel = #kernel_fn_ident;
        }

        #[::rhdl::prelude::kernel]
        pub fn #kernel_fn_ident(
            cr: ::rhdl::prelude::ClockReset,
            #in_param: #input_type,
            q: Q,
        ) -> (#output_type, D) {
            // Per-register next-value chains (last-write-wins via
            // sequence of let-rebindings).
            #(#next_decls)*

            // Output kernel: shadow the kernel's input parameter
            // under the name the output method declared, so the
            // user's body sees its expected binding.
            let o = {
                let #output_input_name = #in_param;
                #output_block
            };

            // Build D as a single struct expression.  Reset
            // semantics live in the wrapping DFFs (each holds its
            // own reset value) so the kernel does not need an
            // explicit reset block in Phase 0.
            let _ = cr;
            (o, D { #(#d_field_inits),* })
        }

        #other_items_out
    };

    Ok(expanded)
}

// ---------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------

fn has_attr(method: &ImplItemFn, name: &str) -> bool {
    method
        .attrs
        .iter()
        .any(|a| a.path().is_ident(name))
}

fn parse_rule(method: &ImplItemFn) -> syn::Result<Rule> {
    let name = method.sig.ident.clone();

    // Expect: fn <name>(ctx: &mut RuleCtx<Self>, <input>: <Type>) { ... }
    let mut iter = method.sig.inputs.iter();
    let _ctx = iter.next().ok_or_else(|| {
        syn::Error::new(
            method.sig.span(),
            "rule must take a `ctx: &mut RuleCtx<Self>` first parameter",
        )
    })?;
    let input_arg = iter.next().ok_or_else(|| {
        syn::Error::new(
            method.sig.span(),
            "rule must take a second `input: <Type>` parameter",
        )
    })?;
    if iter.next().is_some() {
        return Err(syn::Error::new(
            method.sig.span(),
            "rule must take exactly two parameters: `ctx` and `input`",
        ));
    }
    let (input_name, input_type) = match input_arg {
        FnArg::Typed(pat_type) => {
            let name = match &*pat_type.pat {
                Pat::Ident(pi) => pi.ident.clone(),
                _ => {
                    return Err(syn::Error::new(
                        pat_type.pat.span(),
                        "rule input parameter must be a simple identifier",
                    ));
                }
            };
            (name, (*pat_type.ty).clone())
        }
        FnArg::Receiver(_) => {
            return Err(syn::Error::new(
                input_arg.span(),
                "rule's second parameter must be a typed input, not `self`",
            ));
        }
    };

    // Walk the body for guard!/set! macros.
    let mut walker = RuleBodyWalker {
        guards: Vec::new(),
        actions: Vec::new(),
        errors: Vec::new(),
    };
    let mut body = method.block.clone();
    walker.visit_block_mut(&mut body);
    if let Some(err) = walker.errors.into_iter().next() {
        return Err(err);
    }

    Ok(Rule {
        name,
        input_name,
        input_type,
        guards: walker.guards,
        actions: walker.actions,
    })
}

fn parse_output(method: &ImplItemFn) -> syn::Result<OutputMethod> {
    // Expect: fn output(self_q: &Self, <input>: <Type>) -> <Out> { ... }
    let mut iter = method.sig.inputs.iter();
    let first = iter.next().ok_or_else(|| {
        syn::Error::new(method.sig.span(), "#[output] must take two parameters")
    })?;
    // Validate the first argument structurally; we only use it to
    // confirm the user followed the convention.
    match first {
        FnArg::Typed(pt) => {
            if !matches!(&*pt.ty, Type::Reference(_)) {
                return Err(syn::Error::new(
                    pt.ty.span(),
                    "#[output]'s first parameter must be `self_q: &Self` (a typed reference)",
                ));
            }
        }
        FnArg::Receiver(_) => {
            // Allow `&self` too; we'll rewrite `self.field` to
            // `q.field` in the body.
        }
    }
    let input_arg = iter.next().ok_or_else(|| {
        syn::Error::new(
            method.sig.span(),
            "#[output] must take a second `input: <Type>` parameter",
        )
    })?;
    if iter.next().is_some() {
        return Err(syn::Error::new(
            method.sig.span(),
            "#[output] takes exactly two parameters: `self_q: &Self` and `input: <Type>`",
        ));
    }
    let (input_name, input_type) = match input_arg {
        FnArg::Typed(pat_type) => {
            let name = match &*pat_type.pat {
                Pat::Ident(pi) => pi.ident.clone(),
                _ => {
                    return Err(syn::Error::new(
                        pat_type.pat.span(),
                        "#[output] input parameter must be a simple identifier",
                    ));
                }
            };
            (name, (*pat_type.ty).clone())
        }
        FnArg::Receiver(_) => {
            return Err(syn::Error::new(
                input_arg.span(),
                "#[output]'s second parameter must be a typed input",
            ));
        }
    };

    let return_type = match &method.sig.output {
        ReturnType::Type(_, ty) => (**ty).clone(),
        ReturnType::Default => {
            return Err(syn::Error::new(
                method.sig.span(),
                "#[output] must declare a return type",
            ));
        }
    };

    // Determine the receiver name (`self_q` or `self`) so we
    // can rewrite its field-access references to `q.field`.
    let receiver_name: Ident = match first {
        FnArg::Receiver(_) => Ident::new("self", first.span()),
        FnArg::Typed(pt) => match &*pt.pat {
            Pat::Ident(pi) => pi.ident.clone(),
            _ => Ident::new("self_q", first.span()),
        },
    };

    let mut body = method.block.clone();
    let mut rewriter = OutputBodyWalker { receiver_name };
    rewriter.visit_block_mut(&mut body);

    Ok(OutputMethod {
        input_name,
        input_type,
        return_type,
        body,
    })
}

// ---------------------------------------------------------------
// Walkers / rewriters
// ---------------------------------------------------------------

struct RuleBodyWalker {
    guards: Vec<Expr>,
    actions: Vec<Action>,
    errors: Vec<syn::Error>,
}

impl VisitMut for RuleBodyWalker {
    fn visit_block_mut(&mut self, block: &mut Block) {
        // Filter out statements that are guard!()/set!() macro
        // invocations (we extract them); leave everything else.
        let mut keep: Vec<syn::Stmt> = Vec::with_capacity(block.stmts.len());
        for mut stmt in std::mem::take(&mut block.stmts) {
            // Look for `mac;` and `mac` statement-level macros.
            let extracted = match &stmt {
                syn::Stmt::Macro(stmt_macro) => self.try_handle_macro(&stmt_macro.mac),
                syn::Stmt::Expr(Expr::Macro(em), _semi) => self.try_handle_macro(&em.mac),
                _ => None,
            };
            if extracted.is_some() {
                continue; // statement was a guard!() / set!() — drop from body.
            }
            // Otherwise visit children to rewrite `*ctx.field` reads.
            self.visit_stmt_mut(&mut stmt);
            keep.push(stmt);
        }
        block.stmts = keep;
    }

    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        // Inline guard!/set! in expression position (rare but supported).
        if let Expr::Macro(ExprMacro { mac, .. }) = expr {
            if let Some(rewritten) = self.try_handle_macro(mac) {
                *expr = rewritten;
                return;
            }
        }
        if let Some(rewritten) = try_rewrite_ctx_read(expr) {
            *expr = rewritten;
            return;
        }
        visit_mut::visit_expr_mut(self, expr);
    }
}

impl RuleBodyWalker {
    /// If `mac` is `guard!(expr)` or `set!(ctx.field, value)`, push
    /// the appropriate entry into our state and return a unit-typed
    /// expression to take its place in the AST (rules' macro
    /// invocations are removed from the visible body — they're
    /// extracted, not executed).
    fn try_handle_macro(&mut self, mac: &Macro) -> Option<Expr> {
        let last_seg = mac.path.segments.last()?;
        let name = last_seg.ident.to_string();
        match name.as_str() {
            "guard" => {
                // Parse the macro's tokens as a single expression.
                match parse2::<Expr>(mac.tokens.clone()) {
                    Ok(mut e) => {
                        // Rewrite `*ctx.field` reads inside the guard.
                        rewrite_ctx_reads_in_expr(&mut e);
                        self.guards.push(e);
                        Some(unit_expr(mac.span()))
                    }
                    Err(err) => {
                        self.errors.push(err);
                        Some(unit_expr(mac.span()))
                    }
                }
            }
            "set" => {
                // Parse: `ctx.field, value`.
                match parse2::<SetMacroArgs>(mac.tokens.clone()) {
                    Ok(SetMacroArgs { field, mut value }) => {
                        rewrite_ctx_reads_in_expr(&mut value);
                        self.actions.push(Action { field, value });
                        Some(unit_expr(mac.span()))
                    }
                    Err(err) => {
                        self.errors.push(err);
                        Some(unit_expr(mac.span()))
                    }
                }
            }
            _ => None,
        }
    }
}

struct SetMacroArgs {
    field: Ident,
    value: Expr,
}

impl Parse for SetMacroArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // ctx.field, value
        let _ctx: Ident = input.parse()?;
        let _dot: syn::Token![.] = input.parse()?;
        let field: Ident = input.parse()?;
        let _comma: syn::Token![,] = input.parse()?;
        let value: Expr = input.parse()?;
        Ok(Self { field, value })
    }
}

/// If the expression is `*ctx.field`, rewrite to `q.field`.  Returns
/// `Some(rewritten)` if a rewrite happened, `None` otherwise.
fn try_rewrite_ctx_read(expr: &Expr) -> Option<Expr> {
    if let Expr::Unary(syn::ExprUnary {
        op: syn::UnOp::Deref(_),
        expr: inner,
        ..
    }) = expr
    {
        if let Expr::Field(syn::ExprField {
            base, member, ..
        }) = &**inner
        {
            if let Expr::Path(syn::ExprPath { path, .. }) = &**base {
                if path.is_ident("ctx") {
                    if let syn::Member::Named(field) = member {
                        return Some(syn::parse_quote! { q.#field });
                    }
                }
            }
        }
    }
    None
}

fn rewrite_ctx_reads_in_expr(expr: &mut Expr) {
    struct Rewriter;
    impl VisitMut for Rewriter {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            if let Some(replacement) = try_rewrite_ctx_read(expr) {
                *expr = replacement;
                return;
            }
            visit_mut::visit_expr_mut(self, expr);
        }
    }
    Rewriter.visit_expr_mut(expr);
}

struct OutputBodyWalker {
    receiver_name: Ident,
}

impl VisitMut for OutputBodyWalker {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        // Rewrite `*<receiver>.field` → `q.field` (matches the user
        // convention of writing `*self.field` or `*self_q.field`).
        if let Expr::Unary(syn::ExprUnary {
            op: syn::UnOp::Deref(_),
            expr: inner,
            ..
        }) = expr
        {
            if let Expr::Field(syn::ExprField {
                base, member, ..
            }) = &**inner
            {
                if let Expr::Path(syn::ExprPath { path, .. }) = &**base {
                    if path.is_ident(&self.receiver_name) {
                        if let syn::Member::Named(field) = member {
                            *expr = syn::parse_quote! { q.#field };
                            return;
                        }
                    }
                }
            }
        }
        // Also rewrite plain `<receiver>.field` (no deref).
        if let Expr::Field(syn::ExprField {
            base, member, ..
        }) = expr
        {
            if let Expr::Path(syn::ExprPath { path, .. }) = &**base {
                if path.is_ident(&self.receiver_name) {
                    if let syn::Member::Named(field) = member {
                        *expr = syn::parse_quote! { q.#field };
                        return;
                    }
                }
            }
        }
        visit_mut::visit_expr_mut(self, expr);
    }
}

// ---------------------------------------------------------------
// Code generation
// ---------------------------------------------------------------

fn emit_rule_block(rule: &Rule) -> TokenStream {
    // Combine all guards via &&, defaulting to `true`.
    let guard_expr: Expr = if rule.guards.is_empty() {
        syn::parse_quote! { true }
    } else {
        let mut iter = rule.guards.iter();
        let first = iter.next().unwrap();
        let rest = iter;
        syn::parse_quote! { (#first) #( && (#rest) )* }
    };

    // For each action, emit `if guard { d.field = value; }`.
    let actions: Vec<TokenStream> = rule
        .actions
        .iter()
        .map(|a| {
            let field = &a.field;
            let value = &a.value;
            quote! {
                if __rule_guard {
                    d.#field = #value;
                }
            }
        })
        .collect();

    quote! {
        {
            let __rule_guard: bool = (#guard_expr);
            #(#actions)*
        }
    }
}

fn emit_output_block(output: &OutputMethod) -> TokenStream {
    let body = &output.body;
    quote! { #body }
}

// ---------------------------------------------------------------
// Misc helpers
// ---------------------------------------------------------------

fn unit_expr(span: Span) -> Expr {
    let mut e: Expr = syn::parse_quote! { () };
    if let Expr::Tuple(t) = &mut e {
        t.paren_token = syn::token::Paren(span);
    }
    e
}

fn types_equal(a: &Type, b: &Type) -> bool {
    // Strip outer references so `&T` matches `&T`.
    let a = strip_ref(a);
    let b = strip_ref(b);
    a.to_token_stream().to_string() == b.to_token_stream().to_string()
}

fn strip_ref(ty: &Type) -> &Type {
    match ty {
        Type::Reference(TypeReference { elem, .. }) => elem,
        other => other,
    }
}

fn lower_camel_to_snake(ident: &Ident) -> String {
    let s = ident.to_string();
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// Add `Synchronous`, `SynchronousDQ`, `Default` and the
/// `#[rhdl(dq_no_prefix)]` attribute to the user's struct (if not
/// already present).
fn inject_derives(item_struct: &mut ItemStruct) {
    // Find existing #[derive(...)] attribute or append a new one.
    let target_derives = ["Synchronous", "SynchronousDQ"];
    let target_derives_idents: Vec<Ident> = target_derives
        .iter()
        .map(|s| Ident::new(s, Span::call_site()))
        .collect();

    let mut found_derive = false;
    for attr in item_struct.attrs.iter_mut() {
        if attr.path().is_ident("derive") {
            found_derive = true;
            let _ = attr.parse_nested_meta(|meta| {
                // Pre-existing derives; we don't remove them, just
                // need to know what's there to avoid duplicates.
                let _ = meta;
                Ok(())
            });
            // Append our derives unconditionally; duplicates are
            // acceptable in syn-emitted code (the syntax permits it
            // but rustc will warn).  For simplicity, just rewrite
            // by parsing into an Attribute with the merged set.
            let existing_tokens = attr.meta.require_list().map(|l| l.tokens.clone());
            if let Ok(toks) = existing_tokens {
                let mut new_path: Path = syn::parse_quote! { ::rhdl::prelude::Synchronous };
                let _ = &mut new_path; // silence unused-binding warn in case macro inferred different
                let merged: TokenStream = quote! {
                    #toks ,
                    ::rhdl::prelude::Synchronous,
                    ::rhdl::prelude::SynchronousDQ
                };
                let new_attr: syn::Attribute = syn::parse_quote! {
                    #[derive(#merged)]
                };
                *attr = new_attr;
            }
            break;
        }
    }
    if !found_derive {
        // Synthesize a fresh #[derive(...)] attribute.
        let new_attr: syn::Attribute = syn::parse_quote! {
            #[derive(
                ::std::clone::Clone,
                ::std::fmt::Debug,
                ::std::default::Default,
                ::rhdl::prelude::Synchronous,
                ::rhdl::prelude::SynchronousDQ
            )]
        };
        item_struct.attrs.insert(0, new_attr);
    }

    let _ = target_derives_idents;

    // Add #[rhdl(dq_no_prefix)] if not already present.
    let has_rhdl_attr = item_struct.attrs.iter().any(|a| a.path().is_ident("rhdl"));
    if !has_rhdl_attr {
        let new_attr: syn::Attribute = syn::parse_quote! {
            #[rhdl(dq_no_prefix)]
        };
        item_struct.attrs.push(new_attr);
    }
}
