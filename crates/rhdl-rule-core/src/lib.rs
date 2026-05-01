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
    /// `None` if the rule takes only `ctx` (no input parameter).
    input_name: Option<Ident>,
    /// Type of the input parameter.  `None` if no input parameter.
    input_type: Option<Type>,
    /// All `guard!(...)` expressions, in source order.
    guards: Vec<Expr>,
    /// All `set!(ctx.field, value)` actions, in source order.
    actions: Vec<Action>,
    /// Every register field read by this rule (in guards and in
    /// action values).  Used to build the conflict matrix per
    /// `rule-architecture.md` §6.
    read_set: std::collections::BTreeSet<String>,
    /// Explicit `#[rule(priority = N)]` annotation, if present.
    /// Lower N = higher priority (fires first).  None = source-order.
    priority: Option<u32>,
    /// Rule names asserted to be conflict-free with this rule
    /// (`#[rule(conflict_free = "other")]`).  Validated against the
    /// computed conflict matrix.
    conflict_free_with: Vec<String>,
    /// Rule names asserted to be mutually exclusive with this rule
    /// (`#[rule(mutually_exclusive = "other")]`).  Phase 2 trusts the
    /// assertion and uses it as a scheduler-optimisation hint
    /// (the priority chain skips the suppression term for the
    /// asserted pair).
    mutually_exclusive_with: Vec<String>,
    /// `#[rule(urgent_before = "other")]` — explicit ordering: this
    /// rule MUST be scheduled before "other".  If both are ready
    /// and conflict, this one fires.  Composes with `priority` (an
    /// urgent_before edge takes precedence over numeric priority).
    urgent_before: Vec<String>,
}

impl Rule {
    /// Set of fields written by this rule (action targets).
    fn write_set(&self) -> std::collections::BTreeSet<String> {
        self.actions.iter().map(|a| a.field.to_string()).collect()
    }

    /// True iff this rule conflicts with `other` per the
    /// rule-architecture.md §6.1 conflict definition.  Read-only
    /// overlap is *not* a conflict.
    fn conflicts_with(&self, other: &Rule) -> bool {
        let w_self = self.write_set();
        let w_other = other.write_set();
        // write-write
        if !w_self.is_disjoint(&w_other) {
            return true;
        }
        // write-read (other reads what self writes)
        if !w_self.is_disjoint(&other.read_set) {
            return true;
        }
        // read-write
        if !self.read_set.is_disjoint(&w_other) {
            return true;
        }
        false
    }
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

    // Validate: every rule that *has* an input parameter shares its
    // type with the #[output] method.  Rules without an input
    // parameter are unconstrained.
    for rule in &rules {
        if let Some(rule_input_type) = rule.input_type.as_ref() {
            if !types_equal(rule_input_type, &output.input_type) {
                return Err(syn::Error::new(
                    rule_input_type.span(),
                    format!(
                        "every #[rule] that takes an input must use the same type as #[output]; \
                         rule `{}`'s input differs from the #[output] method's input",
                        rule.name,
                    ),
                ));
            }
        }
    }

    // Validate `conflict_free = "other"` assertions: if rule X
    // claims to be conflict-free with Y, but the computed conflict
    // matrix says they DO conflict, that's a compile error per
    // `rule-architecture.md` §12.
    let rules_by_name: std::collections::BTreeMap<String, &Rule> =
        rules.iter().map(|r| (r.name.to_string(), r)).collect();
    for rule in &rules {
        for cf_name in &rule.conflict_free_with {
            let other = match rules_by_name.get(cf_name.as_str()) {
                Some(o) => *o,
                None => {
                    return Err(syn::Error::new(
                        rule.name.span(),
                        format!(
                            "#[rule(conflict_free = \"{cf_name}\")] on `{}` references unknown \
                             rule `{cf_name}`",
                            rule.name,
                        ),
                    ));
                }
            };
            if rule.conflicts_with(other) {
                return Err(syn::Error::new(
                    rule.name.span(),
                    format!(
                        "#[rule(conflict_free = \"{cf_name}\")] on `{}` violates the computed \
                         conflict matrix: `{}` and `{cf_name}` overlap on read/write sets.  \
                         Either drop the assertion or refactor the rules so they don't share state.",
                        rule.name, rule.name,
                    ),
                ));
            }
        }
        // mutually_exclusive: Phase 1 records but doesn't yet prove
        // mutual exclusion of guards.  Validate the named rule
        // exists; defer the proof to Phase 2.
        for me_name in &rule.mutually_exclusive_with {
            if !rules_by_name.contains_key(me_name.as_str()) {
                return Err(syn::Error::new(
                    rule.name.span(),
                    format!(
                        "#[rule(mutually_exclusive = \"{me_name}\")] on `{}` references unknown \
                         rule `{me_name}`",
                        rule.name,
                    ),
                ));
            }
        }
    }

    // Sort rules into schedule order.
    //
    // Phase 2 — `urgent_before` edges are honoured first by way of a
    // topological sort over the DAG induced by them.  Among nodes
    // that are simultaneously available (in-degree 0), the
    // tie-breaker is `(explicit priority or u32::MAX/2, source
    // index)`, so explicitly-prioritised rules still come first and
    // unannotated rules retain source order.
    let order = build_schedule_order(&rules)?;
    let rules_sorted: Vec<&Rule> = order.iter().map(|&i| &rules[i]).collect();

    // Validate `urgent_before` semantics: the edge is only meaningful
    // when the two rules conflict (per the conflict matrix).  If they
    // never conflict, the user has either misunderstood the
    // annotation or the edge is dead — warn at the call site.  Phase
    // 2 treats a non-conflicting urgent_before edge as a hard error
    // because there is no honest interpretation: the scheduler can't
    // do anything observable with it.
    for (i, rule) in rules.iter().enumerate() {
        for ub_name in &rule.urgent_before {
            let j = rules
                .iter()
                .position(|r| r.name == ub_name.as_str())
                .expect("name was validated during schedule-order build");
            if i == j {
                continue; // already errored in build_schedule_order
            }
            if !rule.conflicts_with(&rules[j]) {
                return Err(syn::Error::new(
                    rule.name.span(),
                    format!(
                        "#[rule(urgent_before = \"{ub_name}\")] on `{}` is meaningless: \
                         the two rules don't conflict (no shared read/write set), so \
                         there is no schedule choice to influence.",
                        rule.name,
                    ),
                ));
            }
        }
    }

    // Generate.
    let kernel_fn_name = lower_camel_to_snake(struct_name);
    let kernel_fn_ident = format_ident!("{kernel_fn_name}");
    let q_ident = format_ident!("{}Q", struct_name);
    let d_ident = format_ident!("{}D", struct_name);

    // Phase 2: thread the struct's generics through every emitted item.
    // `split_for_impl` gives us:
    //   - impl_generics : `<T: Digital, const N: usize>` (with bounds)
    //   - ty_generics   : `<T, N>` (no bounds, for type position)
    //   - where_clause  : `where rhdl::bits::W<N>: BitWidth`
    // For expression position (e.g. constructing the D struct
    // value) we can rely on field-driven inference, so no turbofish
    // is needed.
    let (impl_generics, ty_generics, where_clause) = item_struct.generics.split_for_impl();
    // For expression position we use the turbofish form so const-generic
    // values flow into the D constructor without Rust having to infer
    // them from field types (inference can fail on const generics).
    let ty_generics_turbofish = ty_generics.as_turbofish();

    // Augment the struct with the standard derives.
    let mut struct_emit = item_struct.clone();
    inject_derives(&mut struct_emit);

    // Collect field names for default-hold and reset.
    let field_names: Vec<&Ident> = item_struct
        .fields
        .iter()
        .filter_map(|f| f.ident.as_ref())
        .collect();

    // ---------------------------------------------------------------
    // Phase 1 scheduler synthesis.
    //
    // For N rules in source-code priority order:
    //   let _can_fire_<rule_i> = (guard_1) && (guard_2) && ...;
    //   let _fire_<rule_i> = _can_fire_<rule_i>
    //       && !(_fire_<rule_j>)         // for every j < i where j conflicts with i
    //       && !(_fire_<rule_j>)         // ...
    //       ;
    //
    // This is exactly the priority chain from rule-architecture.md §7.
    // ---------------------------------------------------------------
    let mut scheduler_decls: Vec<TokenStream> = Vec::new();
    for (i, rule) in rules_sorted.iter().enumerate() {
        let guard_expr: Expr = if rule.guards.is_empty() {
            syn::parse_quote! { true }
        } else {
            let mut iter = rule.guards.iter();
            let first = iter.next().unwrap();
            let rest = iter;
            syn::parse_quote! { (#first) #( && (#rest) )* }
        };
        let can_fire_ident = format_ident!("_can_fire_{}", rule.name);
        let fire_ident = format_ident!("_fire_{}", rule.name);

        // Conflicts with higher-priority rules (those earlier in
        // `rules_sorted`).
        //
        // Phase 2 — `mutually_exclusive` optimisation: when the user
        // has declared two rules pairwise mutually exclusive (their
        // guards can never both be true on the same cycle), the
        // suppressor `&& !(_fire_other)` is redundant and we can
        // drop it from the priority chain.  We trust the user's
        // assertion (no formal proof in Phase 2) — the assertion
        // composes with the conflict matrix only as an optimisation
        // hint; it never *introduces* permission to fire.
        let i_name = rule.name.to_string();
        let mut suppressors: Vec<TokenStream> = Vec::new();
        for prior in rules_sorted.iter().take(i) {
            if !rule.conflicts_with(prior) {
                continue;
            }
            let prior_name = prior.name.to_string();
            let mutually_exclusive = rule.mutually_exclusive_with.iter().any(|n| n == &prior_name)
                || prior
                    .mutually_exclusive_with
                    .iter()
                    .any(|n| n == &i_name);
            if mutually_exclusive {
                continue;
            }
            let prior_fire = format_ident!("_fire_{}", prior.name);
            suppressors.push(quote! { && !(#prior_fire) });
        }

        scheduler_decls.push(quote! {
            let #can_fire_ident: bool = (#guard_expr);
            let #fire_ident: bool = #can_fire_ident #(#suppressors)*;
        });
    }

    // For each register field, emit a chain of let-rebindings:
    //   let _next_<field> = q.<field>;
    //   let _next_<field> = if _fire_<rule> { value } else { _next_<field> };
    // The priority chain ensures that for any field, at most one
    // `_fire_<rule>` is true among rules that write the field —
    // so last-write-wins still produces the correct result.
    let mut next_decls: Vec<TokenStream> = Vec::new();
    for field in &field_names {
        let next_ident = format_ident!("_next_{field}");
        next_decls.push(quote! { let #next_ident = q.#field; });
    }
    let _ = &q_ident;
    let _ = &d_ident;
    for rule in &rules_sorted {
        let fire_ident = format_ident!("_fire_{}", rule.name);
        for action in &rule.actions {
            let field = &action.field;
            let value = &action.value;
            let next_ident = format_ident!("_next_{field}");
            next_decls.push(quote! {
                let #next_ident = if #fire_ident { #value } else { #next_ident };
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
    // The kernel function's input parameter must have a stable name
    // that any rule body referring to the input uses.  Find the
    // first rule that takes an input and use its name.  If no rule
    // takes an input, fall back to the output method's input name
    // (still needed because the kernel function's signature has an
    // input parameter).
    let in_param: Ident = rules
        .iter()
        .find_map(|r| r.input_name.clone())
        .unwrap_or_else(|| output.input_name.clone());
    // Validate: every rule that *has* an input parameter uses the
    // same name as `in_param`.  Rules without input are unaffected.
    for rule in &rules {
        if let Some(name) = rule.input_name.as_ref() {
            if name != &in_param {
                return Err(syn::Error::new(
                    name.span(),
                    format!(
                        "every #[rule] that takes an input parameter must use the same \
                         parameter name; rule `{}` uses `{}` but the canonical name is `{}`",
                        rule.name, name, in_param,
                    ),
                ));
            }
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
            impl #impl_generics #struct_name #ty_generics #where_clause {
                #(#other_items)*
            }
        }
    };

    let expanded = quote! {
        #struct_emit

        impl #impl_generics ::rhdl::core::circuit::synchronous::SynchronousIO
            for #struct_name #ty_generics
            #where_clause
        {
            type I = #input_type;
            type O = #output_type;
            type Kernel = #kernel_fn_ident #ty_generics;
        }

        #[::rhdl::prelude::kernel]
        pub fn #kernel_fn_ident #impl_generics (
            cr: ::rhdl::prelude::ClockReset,
            #in_param: #input_type,
            q: #q_ident #ty_generics,
        ) -> (#output_type, #d_ident #ty_generics)
        #where_clause
        {
            // Phase 1 scheduler: compute can_fire / fire for each rule.
            #(#scheduler_decls)*

            // Per-register next-value chains.  The fire signals
            // above ensure at most one rule's write fires per
            // register per cycle; last-write-wins is therefore safe.
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
            // explicit reset block in Phase 0.  Field-type inference
            // determines the generic parameters of D, so no
            // turbofish is needed here.
            let _ = cr;
            (o, #d_ident #ty_generics_turbofish { #(#d_field_inits),* })
        }

        #other_items_out
    };

    Ok(expanded)
}

// ---------------------------------------------------------------
// Schedule-order construction
// ---------------------------------------------------------------

/// Build a stable topological order over rules.
///
/// Edges come from `#[rule(urgent_before = "other")]` annotations:
/// `A.urgent_before.contains(B.name)` means A must precede B in the
/// schedule.  Among nodes simultaneously available (in-degree 0)
/// the tie-breaker is `(explicit priority, source index)`, so
/// explicitly-prioritised rules still come first and unannotated
/// rules retain source order.
///
/// Returns the rule indices in schedule order.  Errors on:
/// - an `urgent_before` name that doesn't refer to any defined rule,
/// - a self-loop (`A.urgent_before` contains `A`), or
/// - a cycle in the urgent_before graph.
fn build_schedule_order(rules: &[Rule]) -> syn::Result<Vec<usize>> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    let n = rules.len();
    let name_to_idx: std::collections::BTreeMap<String, usize> = rules
        .iter()
        .enumerate()
        .map(|(i, r)| (r.name.to_string(), i))
        .collect();

    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut in_deg: Vec<usize> = vec![0; n];
    for (i, rule) in rules.iter().enumerate() {
        for ub_name in &rule.urgent_before {
            let j = match name_to_idx.get(ub_name.as_str()) {
                Some(&j) => j,
                None => {
                    return Err(syn::Error::new(
                        rule.name.span(),
                        format!(
                            "#[rule(urgent_before = \"{ub_name}\")] on `{}` references \
                             unknown rule `{ub_name}`",
                            rule.name,
                        ),
                    ));
                }
            };
            if j == i {
                return Err(syn::Error::new(
                    rule.name.span(),
                    format!(
                        "#[rule(urgent_before = \"{ub_name}\")] on `{}` references itself",
                        rule.name,
                    ),
                ));
            }
            adj[i].push(j);
            in_deg[j] += 1;
        }
    }

    // Kahn's algorithm with a stable, priority-aware tiebreaker.
    let priority_key = |i: usize| (rules[i].priority.unwrap_or(u32::MAX / 2), i);

    let mut available: BinaryHeap<Reverse<((u32, usize), usize)>> = BinaryHeap::new();
    for (i, &deg) in in_deg.iter().enumerate() {
        if deg == 0 {
            available.push(Reverse((priority_key(i), i)));
        }
    }

    let mut order: Vec<usize> = Vec::with_capacity(n);
    while let Some(Reverse((_, i))) = available.pop() {
        order.push(i);
        for &j in &adj[i] {
            in_deg[j] -= 1;
            if in_deg[j] == 0 {
                available.push(Reverse((priority_key(j), j)));
            }
        }
    }

    if order.len() != n {
        // Cycle: at least one rule still has in-degree > 0.  Point at
        // any such rule; if the user has multiple, the diagnostic at
        // least names one concrete entry-point into the cycle.
        let cycle_rule = (0..n)
            .find(|&i| in_deg[i] > 0)
            .map(|i| &rules[i].name)
            .expect("non-empty cycle");
        return Err(syn::Error::new(
            cycle_rule.span(),
            format!(
                "#[rule(urgent_before = ...)] forms a cycle involving `{cycle_rule}`; \
                 cycles are not allowed in the urgent_before graph",
            ),
        ));
    }

    Ok(order)
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

    // Expect: fn <name>(ctx: &mut RuleCtx<Self>) { ... }
    //     or: fn <name>(ctx: &mut RuleCtx<Self>, <input>: <Type>) { ... }
    let mut iter = method.sig.inputs.iter();
    let _ctx = iter.next().ok_or_else(|| {
        syn::Error::new(
            method.sig.span(),
            "rule must take a `ctx: &mut RuleCtx<Self>` first parameter",
        )
    })?;
    let input_arg = iter.next();
    if iter.next().is_some() {
        return Err(syn::Error::new(
            method.sig.span(),
            "rule must take at most two parameters: `ctx` and an optional input",
        ));
    }
    let (input_name, input_type): (Option<Ident>, Option<Type>) = match input_arg {
        None => (None, None),
        Some(FnArg::Typed(pat_type)) => {
            let name = match &*pat_type.pat {
                Pat::Ident(pi) => pi.ident.clone(),
                _ => {
                    return Err(syn::Error::new(
                        pat_type.pat.span(),
                        "rule input parameter must be a simple identifier",
                    ));
                }
            };
            (Some(name), Some((*pat_type.ty).clone()))
        }
        Some(FnArg::Receiver(_)) => {
            return Err(syn::Error::new(
                input_arg.unwrap().span(),
                "rule's second parameter must be a typed input, not `self`",
            ));
        }
    };

    // Walk the body for guard!/set! macros.
    let mut walker = RuleBodyWalker {
        guards: Vec::new(),
        actions: Vec::new(),
        read_set: std::collections::BTreeSet::new(),
        errors: Vec::new(),
    };
    let mut body = method.block.clone();
    walker.visit_block_mut(&mut body);
    if let Some(err) = walker.errors.into_iter().next() {
        return Err(err);
    }

    let RuleAnnotations {
        priority,
        conflict_free_with,
        mutually_exclusive_with,
        urgent_before,
    } = parse_rule_annotations(method)?;
    Ok(Rule {
        name,
        input_name,
        input_type,
        guards: walker.guards,
        actions: walker.actions,
        read_set: walker.read_set,
        priority,
        conflict_free_with,
        mutually_exclusive_with,
        urgent_before,
    })
}

#[derive(Default)]
struct RuleAnnotations {
    priority: Option<u32>,
    conflict_free_with: Vec<String>,
    mutually_exclusive_with: Vec<String>,
    urgent_before: Vec<String>,
}

/// Parse `#[rule(priority = N, conflict_free = "x", mutually_exclusive = "y", urgent_before = "z")]`.
fn parse_rule_annotations(method: &ImplItemFn) -> syn::Result<RuleAnnotations> {
    let mut anno = RuleAnnotations::default();

    for attr in &method.attrs {
        if !attr.path().is_ident("rule") {
            continue;
        }
        // Empty `#[rule]` is fine — no nested args to parse.
        if matches!(attr.meta, syn::Meta::Path(_)) {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("priority") {
                let value: syn::LitInt = meta.value()?.parse()?;
                anno.priority = Some(value.base10_parse()?);
                Ok(())
            } else if meta.path.is_ident("conflict_free") {
                let value: syn::LitStr = meta.value()?.parse()?;
                anno.conflict_free_with.push(value.value());
                Ok(())
            } else if meta.path.is_ident("mutually_exclusive") {
                let value: syn::LitStr = meta.value()?.parse()?;
                anno.mutually_exclusive_with.push(value.value());
                Ok(())
            } else if meta.path.is_ident("urgent_before") {
                let value: syn::LitStr = meta.value()?.parse()?;
                anno.urgent_before.push(value.value());
                Ok(())
            } else {
                Err(meta.error(
                    "unknown #[rule(...)] argument; supported: \
                     priority, conflict_free, mutually_exclusive, urgent_before",
                ))
            }
        })?;
    }
    Ok(anno)
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
    read_set: std::collections::BTreeSet<String>,
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
        if let Some((rewritten, field)) = try_rewrite_ctx_read_with_field(expr) {
            self.read_set.insert(field);
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
                        // Rewrite `*ctx.field` reads inside the guard,
                        // tracking the fields read.
                        let reads = rewrite_ctx_reads_in_expr(&mut e);
                        self.read_set.extend(reads);
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
                        let reads = rewrite_ctx_reads_in_expr(&mut value);
                        self.read_set.extend(reads);
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

/// If the expression is `*ctx.field`, rewrite to `q.field` and
/// return the rewritten expression plus the field name read.
fn try_rewrite_ctx_read_with_field(expr: &Expr) -> Option<(Expr, String)> {
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
                        let name = field.to_string();
                        return Some((syn::parse_quote! { q.#field }, name));
                    }
                }
            }
        }
    }
    None
}

/// Rewrite all `*ctx.field` reads in `expr`.  Returns the set of
/// fields that were read.
fn rewrite_ctx_reads_in_expr(expr: &mut Expr) -> std::collections::BTreeSet<String> {
    struct Rewriter {
        reads: std::collections::BTreeSet<String>,
    }
    impl VisitMut for Rewriter {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            if let Some((replacement, field)) = try_rewrite_ctx_read_with_field(expr) {
                self.reads.insert(field);
                *expr = replacement;
                return;
            }
            visit_mut::visit_expr_mut(self, expr);
        }
    }
    let mut r = Rewriter {
        reads: std::collections::BTreeSet::new(),
    };
    r.visit_expr_mut(expr);
    r.reads
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

/// Legacy Phase-0 emission helper; superseded by the inline
/// scheduler-synthesis loop in `expand_rule_kernel`.  Kept for
/// reference and in case Phase 2 wants to re-enable per-rule
/// blocks for diagnostics.
#[allow(dead_code)]
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

/// Add `Synchronous`, `SynchronousDQ`, `Default` derives to the
/// user's struct (if not already present).
///
/// **Phase 1.6:** we no longer inject `#[rhdl(dq_no_prefix)]`.
/// The default SynchronousDQ behaviour generates `<Name>Q` and
/// `<Name>D` types, which lets multiple `rule_kernel!` invocations
/// coexist in the same module without `Q`/`D` name collisions.
/// The kernel function references `<Name>Q` / `<Name>D` explicitly.
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
}
