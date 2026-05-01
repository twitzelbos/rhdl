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
    parse2, Block, Expr, ExprMacro, FnArg, Generics, Ident, ImplItem, ImplItemFn, Item, ItemImpl,
    ItemStruct, Macro, Pat, Path, ReturnType, Type, TypeReference,
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
    /// All scheduled writes, in source order.  Each entry came from
    /// either a `set!(ctx.field, value)` macro call or a
    /// `ctx.field = value;` direct-assignment statement (both forms
    /// have identical semantics; the macro is the legacy spelling
    /// kept for backward compatibility).
    actions: Vec<Action>,
    /// Every register field read by this rule (in guards, in action
    /// values, and in the preamble).  Used to build the conflict
    /// matrix per `rule-architecture.md` §6.
    read_set: std::collections::BTreeSet<String>,
    /// Statements from the rule body that are not guards and not
    /// writes — typically `let` bindings that compute intermediate
    /// values used by multiple action expressions.  Hoisted into
    /// the lowered kernel inside a per-rule block scope so that all
    /// of the rule's actions see the same precomputed values.
    /// `*ctx.field` reads inside these statements have already been
    /// rewritten to `q.field`.
    preamble: Vec<syn::Stmt>,
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
    /// `#[rule(trace)]` (or `#[rule(trace = true)]`) — opt-in: when
    /// set, the macro emits an additional `let fire_<rule>: bool =
    /// _fire_<rule>;` binding (and likewise for `can_fire_<rule>`)
    /// in the kernel body.  Both bindings are visible (no
    /// underscore prefix), so RHDL's trace infrastructure surfaces
    /// them in VCDs.  Off by default to keep generated kernels lean
    /// and VCDs uncluttered for the common case where the user
    /// doesn't need to inspect rule firing.
    trace: bool,
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
    /// Every register field referenced by the output body
    /// (via `*self_q.field` or `self_q.field`).  Used to identify
    /// register fields that participate in the kernel even though
    /// no rule reads or writes them — the attribute form
    /// (which can't see the struct) needs this to construct the
    /// `D` value with all field positions populated.
    field_reads: std::collections::BTreeSet<String>,
}

/// Per-field classification used by [`lower_rule_kernel`] to decide
/// the right auto-hold lowering for fields no rule writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// `dff::DFF<T>` — auto-hold via `let _next_<field> = q.<field>;`.
    /// q and d both contain `T`, so the assignment type-checks.
    Dff,
    /// A composed sub-widget field (e.g. `MyWidget`) — auto-hold via
    /// `let _next_<field> = Default::default();`.  q contains the
    /// sub-widget's `Out` struct; d expects the sub-widget's `In`
    /// struct.  The two types differ, so we can't use `q.field` as
    /// the auto-hold default — we drive `In::default()` instead.
    SubWidget,
}

/// Field name + classification.  Threaded through `lower_rule_kernel`
/// so the auto-hold path emits the right default per field kind.
#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: Ident,
    pub kind: FieldKind,
}

/// Classify a struct field by inspecting its type token-syntactically.
/// Returns `FieldKind::Dff` if the type's last segment matches a
/// known DFF wrapper:
///
/// - `DFF` — the canonical name (`rhdl_fpga::core::dff::DFF<T>`).
/// - `Reg` — the user-facing alias (`rhdl_rule_rt::Reg<T>` →
///   `dff::DFF<T>`).
///
/// Anything else is `FieldKind::SubWidget`.
///
/// Used by the function-like form which sees the struct definition.
/// The attribute form uses an explicit `subwidgets = "..."` list
/// instead.
///
/// **Limitation**: this is a syntactic check, not a type check.
/// Custom DFF wrappers (anything not literally named `DFF` or `Reg`)
/// will be misclassified as sub-widgets.  If a user introduces such
/// a wrapper, they can use the function-like form (which sees their
/// struct) and add their wrapper's last-segment name here, OR they
/// can switch to the attribute form and not list it under
/// `subwidgets = "..."` to keep the DFF default.
fn classify_field(ty: &Type) -> FieldKind {
    if let Type::Path(p) = ty {
        if let Some(seg) = p.path.segments.last() {
            let name = seg.ident.to_string();
            if name == "DFF" || name == "Reg" {
                return FieldKind::Dff;
            }
        }
    }
    FieldKind::SubWidget
}

/// Public entry point — function-like form: `rule_kernel! { struct + impl }`.
///
/// Receives both items in a single token stream, validates that the
/// impl's self-type names the struct, augments the struct with the
/// standard derives (`Synchronous`, `SynchronousDQ`, `Default`),
/// and emits the struct + the lowered kernel together.
///
/// The actual lowering work is shared with [`expand_rule_kernel_attr`]
/// via [`lower_rule_kernel`].
pub fn expand_rule_kernel(input: TokenStream) -> syn::Result<TokenStream> {
    let RuleKernelInput {
        item_struct,
        item_impl,
    } = parse2(input)?;

    // Validate: the impl's self-type matches the struct's name.
    let struct_name = item_struct.ident.clone();
    let impl_name = match &*item_impl.self_ty {
        Type::Path(p) => p.path.segments.last().map(|s| &s.ident),
        _ => None,
    };
    match impl_name {
        Some(name) if *name == struct_name => {}
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

    let struct_generics = item_struct.generics.clone();

    // Extract the struct's field names + types so the lowering can
    // (a) auto-hold any field that no rule touches, and (b) classify
    // each field as DFF or sub-widget so the auto-hold default is
    // type-correct.  Only the function-like form can do (b) — the
    // attribute form doesn't see the struct.
    let expected_fields: Vec<FieldInfo> = item_struct
        .fields
        .iter()
        .filter_map(|f| {
            f.ident.as_ref().map(|name| FieldInfo {
                name: name.clone(),
                kind: classify_field(&f.ty),
            })
        })
        .collect();

    let mut struct_emit = item_struct.clone();
    inject_derives(&mut struct_emit);

    let body = lower_rule_kernel(
        &struct_name,
        &struct_generics,
        item_impl,
        Some(expected_fields),
    )?;
    Ok(quote! {
        #struct_emit
        #body
    })
}

/// Public entry point — attribute-on-impl form: `#[rule_kernel] impl Foo { ... }`.
///
/// Receives only the impl block.  The struct must be defined elsewhere
/// (typically immediately above the impl) with the standard derives
/// the user wants — `#[rule_kernel]` does not inject any.  The struct's
/// generics are inferred from the impl block's own generics
/// (which by Rust convention will mirror the struct's).
///
/// This is the attribute-form companion to the function-like
/// [`expand_rule_kernel`].  Both share [`lower_rule_kernel`] —
/// behavioural parity is enforced by sharing the lowering code, not
/// by reimplementation.
///
/// The user-facing tradeoff between the two forms is documented in
/// `rule-architecture.md` §4.5.
pub fn expand_rule_kernel_attr(item: TokenStream) -> syn::Result<TokenStream> {
    expand_rule_kernel_attr_with_args(TokenStream::new(), item)
}

/// Like [`expand_rule_kernel_attr`] but also takes the attribute's
/// argument list, parsed for `subwidgets = "field1, field2"` to
/// classify those fields as sub-widgets in the auto-hold lowering.
///
/// Without an explicit list, the attribute form treats every field
/// as DFF (which matches the attribute form's prior behaviour
/// before sub-widget composition was supported).
///
/// Syntax: `#[rule_kernel_attr(subwidgets = "regs, sub")]`
pub fn expand_rule_kernel_attr_with_args(
    attr: TokenStream,
    item: TokenStream,
) -> syn::Result<TokenStream> {
    let item_impl: ItemImpl = parse2(item)?;

    let struct_ident = match &*item_impl.self_ty {
        Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident.clone())
            .ok_or_else(|| {
                syn::Error::new(
                    item_impl.self_ty.span(),
                    "#[rule_kernel] impl's self-type must name a struct",
                )
            })?,
        _ => {
            return Err(syn::Error::new(
                item_impl.self_ty.span(),
                "#[rule_kernel] impl's self-type must be a simple path (e.g. `impl MyWidget`)",
            ));
        }
    };

    let struct_generics = item_impl.generics.clone();

    // Parse the attribute's `subwidgets = "..."` arg, if any.
    let subwidget_names: std::collections::BTreeSet<String> = if attr.is_empty() {
        std::collections::BTreeSet::new()
    } else {
        parse_subwidgets_arg(attr)?
    };

    // Attribute form can't see the struct, so it can't enumerate
    // every field — but the explicit `subwidgets` list lets the
    // user mark which composed fields need the sub-widget auto-hold.
    // The fields the macro discovers (via rule reads/writes/output)
    // are classified using this list: in `subwidgets` → SubWidget;
    // not in `subwidgets` → Dff.
    let attr_field_classifier = if subwidget_names.is_empty() {
        None
    } else {
        Some(subwidget_names)
    };

    lower_rule_kernel_with_subwidget_marker(
        &struct_ident,
        &struct_generics,
        item_impl,
        None,
        attr_field_classifier,
    )
}

/// Parse `subwidgets = "field1, field2, ..."` from an attribute's
/// argument list.  Returns the set of field names.
///
/// Accepts either `subwidgets = "f1, f2"` (string-literal form,
/// most flexible) or `subwidgets(f1, f2)` (parenthesised form,
/// slightly nicer to read).  The string-literal form is the
/// canonical one.
fn parse_subwidgets_arg(
    attr: TokenStream,
) -> syn::Result<std::collections::BTreeSet<String>> {
    use syn::parse::{Parse, ParseStream};
    use syn::punctuated::Punctuated;

    struct AttrArgs {
        subwidgets: std::collections::BTreeSet<String>,
    }

    impl Parse for AttrArgs {
        fn parse(input: ParseStream) -> syn::Result<Self> {
            let mut subwidgets = std::collections::BTreeSet::new();
            // `subwidgets = "f1, f2"` form.
            let key: Ident = input.parse()?;
            if key != "subwidgets" {
                return Err(syn::Error::new(
                    key.span(),
                    "expected `subwidgets = \"...\"` argument",
                ));
            }
            let _eq: syn::Token![=] = input.parse()?;
            let lit: syn::LitStr = input.parse()?;
            for name in lit.value().split(',') {
                let n = name.trim();
                if !n.is_empty() {
                    subwidgets.insert(n.to_string());
                }
            }
            // Allow trailing comma; ignore subsequent args for now.
            if input.peek(syn::Token![,]) {
                let _: syn::Token![,] = input.parse()?;
            }
            Ok(Self { subwidgets })
        }
    }
    let _ = Punctuated::<Ident, syn::Token![,]>::new(); // silence unused import

    let parsed: AttrArgs = parse2(attr)?;
    Ok(parsed.subwidgets)
}

/// Shared lowering — the heart of the macro.  Both the function-like
/// `rule_kernel!` and the `#[rule_kernel]` attribute call this.
///
/// Emits:
/// - the `SynchronousIO` impl,
/// - the `#[kernel]` function with the synthesized scheduler,
/// - any non-rule, non-output `impl` items the user wrote
///   (preserved verbatim under their own `impl` block).
///
/// Does NOT emit the struct itself — that is the caller's
/// responsibility (the function-like form augments it with derives;
/// the attribute form leaves it to the user).
fn lower_rule_kernel(
    struct_name: &Ident,
    struct_generics: &Generics,
    item_impl: ItemImpl,
    expected_fields: Option<Vec<FieldInfo>>,
) -> syn::Result<TokenStream> {
    lower_rule_kernel_with_subwidget_marker(
        struct_name,
        struct_generics,
        item_impl,
        expected_fields,
        None,
    )
}

/// Version of [`lower_rule_kernel`] that also accepts an explicit
/// sub-widget name set (used by the attribute form, where the
/// struct isn't visible and per-field type classification has to
/// be supplied as an explicit list via the attribute argument).
fn lower_rule_kernel_with_subwidget_marker(
    struct_name: &Ident,
    struct_generics: &Generics,
    item_impl: ItemImpl,
    expected_fields: Option<Vec<FieldInfo>>,
    attr_subwidget_marker: Option<std::collections::BTreeSet<String>>,
) -> syn::Result<TokenStream> {
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
    let (impl_generics, ty_generics, where_clause) = struct_generics.split_for_impl();
    // For expression position we use the turbofish form so const-generic
    // values flow into the D constructor without Rust having to infer
    // them from field types (inference can fail on const generics).
    let ty_generics_turbofish = ty_generics.as_turbofish();

    // Collect register-field names from the union of every rule's
    // read-set, every rule's write-set, and the output method's
    // field reads.  Order of iteration is stable (BTreeSet) so the
    // emitted code is deterministic across compilations.
    //
    // The function-like form ALSO supplies the struct's actual field
    // list (`expected_field_names`); any field that exists in the
    // struct but isn't touched by any rule or output gets **auto-hold
    // semantics** in the lowered kernel — `_next_<field> = q.<field>`
    // with no rule ever overwriting it, so the field stays at its
    // current value forever.  This means users can declare DFF fields
    // in the struct without being forced to add `let _ = *self_q.x;`
    // workarounds in the output method just to satisfy the macro.
    //
    // The attribute form can't see the struct, so it skips this and
    // relies on the user to either touch every field in some rule or
    // accept Rust's "missing field" error.  Documented in §4.5.
    let mut field_name_set: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    for rule in &rules {
        for action in &rule.actions {
            field_name_set.insert(action.field.to_string());
        }
        for r in &rule.read_set {
            field_name_set.insert(r.clone());
        }
    }
    for f in &output.field_reads {
        field_name_set.insert(f.clone());
    }
    if let Some(expected) = expected_fields.as_ref() {
        for f in expected {
            field_name_set.insert(f.name.to_string());
        }
    }
    let field_names: Vec<Ident> = field_name_set
        .iter()
        .map(|s| Ident::new(s, Span::call_site()))
        .collect();

    // Per-field kind map.  Function-like form supplies it via
    // expected_fields (each field already classified).  Attribute
    // form falls back to the explicit `subwidgets="..."` marker
    // (everything in the marker → SubWidget; everything else → Dff).
    // If neither is available, default everything to Dff (the
    // pre-sub-widget-composition behaviour).
    let field_kind_for = |name: &str| -> FieldKind {
        if let Some(expected) = expected_fields.as_ref() {
            for f in expected {
                if f.name == name {
                    return f.kind;
                }
            }
        }
        if let Some(marker) = attr_subwidget_marker.as_ref() {
            if marker.contains(name) {
                return FieldKind::SubWidget;
            }
        }
        FieldKind::Dff
    };

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

        // Per-rule trace exposure (`#[rule(trace)]`).  When opted-in,
        // emit `let can_fire_<rule>` and `let fire_<rule>` aliases —
        // visible names (no underscore prefix) that RHDL's trace
        // infrastructure surfaces in VCDs.  Off by default: the
        // common case shouldn't pay the kernel-emission and VCD-
        // clutter cost for debug-only signals.
        //
        // The trailing `let _trace_<rule> = (...)` consumes the
        // visible names so the kernel's `unused_variables` lint
        // (deny-by-default inside `#[kernel]`) doesn't fire.
        if rule.trace {
            let trace_can_fire = format_ident!("can_fire_{}", rule.name);
            let trace_fire = format_ident!("fire_{}", rule.name);
            let trace_anchor = format_ident!("_trace_{}", rule.name);
            scheduler_decls.push(quote! {
                let #trace_can_fire: bool = #can_fire_ident;
                let #trace_fire: bool = #fire_ident;
                let #trace_anchor: (bool, bool) = (#trace_can_fire, #trace_fire);
            });
        }
    }

    // For each register field, emit a chain of let-rebindings:
    //   let _next_<field> = q.<field>;
    //   <per-rule preamble + value pre-computation block>
    //   let _next_<field> = if _fire_<rule> { _rule_<rule>_w<i> } else { _next_<field> };
    // The priority chain ensures that for any field, at most one
    // `_fire_<rule>` is true among rules that write the field —
    // so last-write-wins still produces the correct result.
    //
    // The per-rule preamble block is the new piece: any `let`
    // bindings the user wrote in the rule body live here, in scope
    // for every action expression.  This means multiple `set!`s (or
    // direct `ctx.field = expr;` assignments) can share intermediate
    // values without duplicating computation.  When a rule has no
    // preamble and exactly one action, the lowering collapses to
    // the original simpler form.
    let mut next_decls: Vec<TokenStream> = Vec::new();
    for field in &field_names {
        let next_ident = format_ident!("_next_{field}");
        let initial = match field_kind_for(&field.to_string()) {
            FieldKind::Dff => {
                // DFF auto-hold: q.<field> is the current value;
                // also assignable to d.<field> (same type).
                quote! { q.#field }
            }
            FieldKind::SubWidget => {
                // Sub-widget auto-hold: q.<field> is the sub-widget's
                // Out struct; d.<field> expects the sub-widget's In.
                // We can't use `q.<field>` (different type from
                // d.<field>); we use `D::dont_care().<field>` instead,
                // which projects the In type from D and gives a
                // stable zero-valued initial input.
                //
                // (Sub-widget input semantics: when no rule drives
                // the sub-widget, the parent harness's "any" value
                // is the contract.  RHDL's `dont_care` compiles to
                // a stable zero, so the sub-widget receives a
                // quiescent input each cycle — equivalent to
                // `Default::default()` for typical In structs.)
                quote! {
                    <#d_ident #ty_generics as ::rhdl::prelude::Digital>::dont_care().#field
                }
            }
        };
        next_decls.push(quote! { let #next_ident = #initial; });
    }
    let _ = &q_ident;
    let _ = &d_ident;
    for rule in &rules_sorted {
        let fire_ident = format_ident!("_fire_{}", rule.name);
        if rule.actions.is_empty() {
            continue; // guards-only rule has no next-state effect
        }

        // Per-rule pre-computation.  We emit a single block that
        // (a) runs the preamble once — `let` bindings inside the
        // block are in scope for every action value computed below,
        // and (b) destructures into per-action variables via tuple
        // pattern.  The user's intermediate computations are
        // therefore evaluated once per cycle, not once per action.
        let preamble = &rule.preamble;
        let action_value_idents: Vec<Ident> = (0..rule.actions.len())
            .map(|i| format_ident!("_rule_{}_w{}", rule.name, i))
            .collect();
        let action_values: Vec<&Expr> = rule.actions.iter().map(|a| &a.value).collect();

        if rule.actions.len() == 1 && rule.preamble.is_empty() {
            // Fast-path: no preamble, one action — fold the value
            // expression directly into the conditional update.
            let field = &rule.actions[0].field;
            let value = &rule.actions[0].value;
            let next_ident = format_ident!("_next_{field}");
            next_decls.push(quote! {
                let #next_ident = if #fire_ident { #value } else { #next_ident };
            });
            continue;
        }

        if rule.actions.len() == 1 {
            // Single action with preamble: emit one let with the
            // preamble inside, no tuple destructure.
            let v_ident = &action_value_idents[0];
            let value = action_values[0];
            next_decls.push(quote! {
                let #v_ident = {
                    #(#preamble)*
                    #value
                };
            });
        } else {
            // Multi-action: tuple destructure with the preamble
            // bound once at the top of the shared block.  Trailing
            // commas keep the tuple pattern correct for the
            // (uncommon) two-action case.
            next_decls.push(quote! {
                let ( #(#action_value_idents,)* ) = {
                    #(#preamble)*
                    ( #(#action_values,)* )
                };
            });
        }

        // Conditional updates: one `let _next_<field> = if _fire_<rule>
        // { _rule_<rule>_w<i> } else { _next_<field> };` per action.
        for (i, action) in rule.actions.iter().enumerate() {
            let field = &action.field;
            let next_ident = format_ident!("_next_{field}");
            let v_ident = &action_value_idents[i];
            next_decls.push(quote! {
                let #next_ident = if #fire_ident { #v_ident } else { #next_ident };
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

    // After the walker runs, `body.stmts` contains the surviving
    // statements: the rule's preamble — let bindings and helper
    // expressions that the macro hoists into a per-rule block scope
    // so all of the rule's actions can share them.  `*ctx.field`
    // reads inside those statements have already been rewritten to
    // `q.field` by the walker.
    let preamble = body.stmts;

    let RuleAnnotations {
        priority,
        conflict_free_with,
        mutually_exclusive_with,
        urgent_before,
        trace,
    } = parse_rule_annotations(method)?;
    Ok(Rule {
        name,
        input_name,
        input_type,
        guards: walker.guards,
        actions: walker.actions,
        read_set: walker.read_set,
        preamble,
        priority,
        conflict_free_with,
        mutually_exclusive_with,
        urgent_before,
        trace,
    })
}

#[derive(Default)]
struct RuleAnnotations {
    priority: Option<u32>,
    conflict_free_with: Vec<String>,
    mutually_exclusive_with: Vec<String>,
    urgent_before: Vec<String>,
    trace: bool,
}

/// Parse `#[rule(priority = N, conflict_free = "x", mutually_exclusive = "y", urgent_before = "z", trace)]`.
///
/// `trace` accepts both the bare `#[rule(trace)]` form and the
/// explicit `#[rule(trace = true)]` form; the latter also accepts
/// `false` to disable.
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
            } else if meta.path.is_ident("trace") {
                // Accept both `trace` (bare) and `trace = <bool>`.
                if let Ok(value) = meta.value() {
                    let v: syn::LitBool = value.parse()?;
                    anno.trace = v.value;
                } else {
                    anno.trace = true;
                }
                Ok(())
            } else {
                Err(meta.error(
                    "unknown #[rule(...)] argument; supported: \
                     priority, conflict_free, mutually_exclusive, \
                     urgent_before, trace",
                ))
            }
        })?;
    }
    Ok(anno)
}

fn parse_output(method: &ImplItemFn) -> syn::Result<OutputMethod> {
    // Two accepted signatures:
    //   1. `fn output(self_q: &Self, input: <Type>) -> <Out>` — the
    //      historical form; output reads state via `*self_q.field`.
    //   2. `fn output(input: <Type>) -> <Out>` — new shorthand for
    //      stateless outputs (output is purely a function of input).
    //      Useful when no field-read is needed, so the user doesn't
    //      have to declare and silence an unused `self_q` parameter.
    //
    // We distinguish purely by parameter count and first-parameter
    // shape: if the first parameter looks like a receiver
    // (`FnArg::Receiver` or `FnArg::Typed` whose type is a
    // reference), we treat it as the receiver.  Otherwise we treat
    // it as the input.  The macro requires at least one parameter
    // (the input) and at most two (receiver + input).
    let params: Vec<&FnArg> = method.sig.inputs.iter().collect();
    if params.is_empty() || params.len() > 2 {
        return Err(syn::Error::new(
            method.sig.span(),
            "#[output] takes either one parameter (`input: <Type>`) or two \
             (`self_q: &Self, input: <Type>`); got a different shape",
        ));
    }

    let first_is_receiver = match params[0] {
        FnArg::Receiver(_) => true,
        FnArg::Typed(pt) => matches!(&*pt.ty, Type::Reference(_)),
    };

    let (receiver_param, input_arg) = if params.len() == 2 {
        if !first_is_receiver {
            return Err(syn::Error::new(
                params[0].span(),
                "#[output]'s first parameter (when two are present) must be \
                 a receiver — `self_q: &Self` or `&self`",
            ));
        }
        (Some(params[0]), params[1])
    } else {
        // 1 parameter — it must be the input.
        if first_is_receiver {
            return Err(syn::Error::new(
                params[0].span(),
                "#[output] takes the input as its sole parameter when no \
                 receiver is declared; got something that looks like a \
                 receiver instead",
            ));
        }
        (None, params[0])
    };
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
                "#[output]'s input parameter must be a typed input, not `self`",
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

    // Determine the receiver name (`self_q` or `self`) so we can
    // rewrite its field-access references to `q.field`.  If there
    // is no receiver, use a sentinel name that matches nothing.
    let receiver_name: Ident = match receiver_param {
        None => Ident::new("__no_receiver__", method.sig.span()),
        Some(FnArg::Receiver(_)) => Ident::new("self", method.sig.span()),
        Some(FnArg::Typed(pt)) => match &*pt.pat {
            Pat::Ident(pi) => pi.ident.clone(),
            _ => Ident::new("self_q", method.sig.span()),
        },
    };

    let mut body = method.block.clone();
    let mut rewriter = OutputBodyWalker {
        receiver_name,
        field_reads: std::collections::BTreeSet::new(),
    };
    rewriter.visit_block_mut(&mut body);

    Ok(OutputMethod {
        input_name,
        input_type,
        return_type,
        body,
        field_reads: rewriter.field_reads,
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
        // Walk every statement in source order.  Three statement
        // shapes get extracted (and dropped from the kept body):
        //
        // 1. `guard!(expr);` and `guard!(expr)` macro invocations.
        // 2. `set!(ctx.field, expr);` and `set!(ctx.field, expr)`.
        // 3. `ctx.field = expr;` direct assignments — equivalent
        //    to the `set!` macro, with no ceremony.
        //
        // Everything else (let bindings, helper expressions, etc.)
        // is visited to rewrite `*ctx.field` reads and KEPT.  The
        // kept statements become the rule's preamble — hoisted
        // into a per-rule block scope where all action expressions
        // can see the precomputed values.
        let mut keep: Vec<syn::Stmt> = Vec::with_capacity(block.stmts.len());
        for mut stmt in std::mem::take(&mut block.stmts) {
            // Macro statements: guard!/set! extraction.
            let extracted = match &stmt {
                syn::Stmt::Macro(stmt_macro) => self.try_handle_macro(&stmt_macro.mac),
                syn::Stmt::Expr(Expr::Macro(em), _semi) => self.try_handle_macro(&em.mac),
                _ => None,
            };
            if extracted.is_some() {
                continue; // statement was a guard!() / set!() — drop from body.
            }
            // Direct-assignment shape: `ctx.field = value;` or
            // `ctx.field = value` (statement form).  Both lower to
            // an `Action` exactly like `set!` does.
            if self.try_extract_direct_assignment(&stmt) {
                continue;
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
        // DFF read: `*ctx.field` → `q.field`.
        if let Some((rewritten, field)) = try_rewrite_ctx_read_with_field(expr) {
            self.read_set.insert(field);
            *expr = rewritten;
            return;
        }
        // Sub-widget read (also catches sub-field/method/index access on
        // DFF-stored values): `ctx.X.<rest>` → `q.X.<rest>`.  See
        // [`try_rewrite_ctx_subwidget_read`] for the full discussion.
        if let Some((rewritten, field)) = try_rewrite_ctx_subwidget_read(expr) {
            self.read_set.insert(field);
            *expr = rewritten;
            // Recurse into the rewritten expression so any nested
            // ctx accesses inside the args / index / inner field
            // get rewritten too.  (E.g. `ctx.regs[*ctx.idx]` —
            // outer rewrite gives `q.regs[*ctx.idx]`; we then need
            // to walk the rewritten expr to handle the inner
            // `*ctx.idx` DFF read.)
            visit_mut::visit_expr_mut(self, expr);
            return;
        }
        visit_mut::visit_expr_mut(self, expr);
    }
}

impl RuleBodyWalker {
    /// If `stmt` is `ctx.field = value;` (or the semicolon-less
    /// expression statement form), extract it as an [`Action`] and
    /// return `true`.  Otherwise leave the statement alone and
    /// return `false`.
    ///
    /// The LHS must be exactly `ctx.field` (no nested fields, no
    /// indexing).  The RHS may be any expression; `*ctx.field`
    /// reads inside it are rewritten to `q.field` and tracked in
    /// the rule's read-set.
    ///
    /// This is the syntactic alternative to `set!(ctx.field,
    /// value)`; the two forms produce identical lowered hardware.
    fn try_extract_direct_assignment(&mut self, stmt: &syn::Stmt) -> bool {
        // Match both `ctx.field = expr;` (Stmt::Expr with semicolon)
        // and the rare `ctx.field = expr` (no semicolon, terminal
        // expression of a block) — the second is unusual but valid
        // Rust syntax.
        let assign = match stmt {
            syn::Stmt::Expr(Expr::Assign(a), _) => a,
            _ => return false,
        };
        // LHS must be `ctx.field`.
        let field = match ctx_field_lhs(&assign.left) {
            Some(f) => f,
            None => return false,
        };
        // Clone the RHS, rewrite `*ctx.<f>` reads, track them.
        let mut value = (*assign.right).clone();
        let reads = rewrite_ctx_reads_in_expr(&mut value);
        self.read_set.extend(reads);
        self.actions.push(Action { field, value });
        true
    }

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

/// If the expression is `ctx.field` (no deref, no indexing,
/// no further field access), return the field's identifier.
/// Used to recognise the LHS of a `ctx.field = value;` direct-
/// assignment statement.
fn ctx_field_lhs(expr: &Expr) -> Option<Ident> {
    let field = match expr {
        Expr::Field(f) => f,
        _ => return None,
    };
    let path = match &*field.base {
        Expr::Path(p) => &p.path,
        _ => return None,
    };
    if !path.is_ident("ctx") {
        return None;
    }
    match &field.member {
        syn::Member::Named(name) => Some(name.clone()),
        _ => None,
    }
}

/// If the expression is `*ctx.field`, rewrite to `q.field` and
/// return the rewritten expression plus the field name read.
///
/// This is the **DFF read** pattern: a leading `*` deref tells the
/// walker that `ctx.field` is the DFF's stored value (read out of
/// the q struct).  See [`try_rewrite_ctx_subwidget_read`] for the
/// sister **sub-widget read** pattern (`ctx.subwidget.<inner>`,
/// no deref) added in PR #45's follow-up.
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

/// If `expr` is `ctx.field.<...rest>` (no leading `*`, with at
/// least one nested access on `ctx.field`), rewrite the `ctx`
/// prefix to `q` and return the rewritten expression plus the
/// outermost field name.
///
/// This is the **sub-widget read** pattern.  It covers two related
/// cases that share the same syntactic form:
///
/// - **Sub-widget output reads**: `ctx.regfile.rdata` →
///   `q.regfile.rdata`, where `regfile` is a sub-widget and `rdata`
///   is one of its `Out` struct's fields.
/// - **Sub-field / method access on DFF-stored values**:
///   `ctx.flags.bit(3)` → `q.flags.bit(3)`, where `flags` is a
///   `dff::DFF<Bits<8>>` and `.bit(3)` is a method on `Bits<8>`.
///
/// Both cases lower correctly because the auto-derived `Q` struct
/// exposes DFF-stored values as their inner type AND sub-widget
/// outputs as the sub-widget's `Out` struct — both of which have
/// the field/method shape the rewrite expects.
///
/// The walker can't statically distinguish DFF-vs-sub-widget at
/// proc-macro time without struct-type introspection (which only
/// the function-like form has, not the attribute form), so the
/// rewrite is uniform: `ctx.X.Y...` → `q.X.Y...`.  The Rust type
/// system then decides whether the rewritten expression is sound.
///
/// Returns the rewritten expression and the outermost field name
/// (`field` in `ctx.field.inner`) so the rule's read-set tracks
/// the outer access.  Sub-field/method accesses past the first hop
/// don't add extra read-set entries — a read of `ctx.regfile.rdata`
/// counts as a read of `regfile` for conflict-matrix purposes.
fn try_rewrite_ctx_subwidget_read(expr: &Expr) -> Option<(Expr, String)> {
    // Walk up the field-access chain to find the bottom (closest
    // to `ctx`).  Pattern: `ctx.<outer_field>.<inner_field>...` or
    // `ctx.<outer_field>.method(...)` etc.  We accept anything
    // shaped like `Expr::Field(ctx, outer_field).<rest>` where the
    // base of the outermost `.` is `ctx.<outer_field>`.
    //
    // Concretely, we recognise an expression that contains a sub-
    // expression of the form `ctx.<outer_field>` somewhere inside
    // a containing field-access, method-call, or index.  Rather
    // than walking the chain by hand, we look at just the
    // outermost layer: `Expr::Field(base, _)` where base is also
    // a Field on ctx, OR `Expr::MethodCall(receiver, ...)` where
    // receiver is a Field on ctx, OR `Expr::Index(base, _)` where
    // base is a Field on ctx.  Each rewrites to swap `ctx` for `q`.
    //
    // We don't recurse the walker into the rewritten expression
    // (the visitor handles that for nested expressions); this
    // function only handles a single rewrite per call.

    // Helper: given an expression that might be `ctx.<field>`,
    // return the field name and a fresh `q.<field>` to substitute.
    fn ctx_field_to_q_field(e: &Expr) -> Option<(Expr, String)> {
        if let Expr::Field(syn::ExprField { base, member, .. }) = e {
            if let Expr::Path(syn::ExprPath { path, .. }) = &**base {
                if path.is_ident("ctx") {
                    if let syn::Member::Named(field) = member {
                        let name = field.to_string();
                        return Some((syn::parse_quote! { q.#field }, name));
                    }
                }
            }
        }
        None
    }

    match expr {
        // ctx.outer.inner   →  q.outer.inner
        Expr::Field(syn::ExprField { base, member, .. }) => {
            if let Some((q_base, name)) = ctx_field_to_q_field(base) {
                let m = member.clone();
                return Some((syn::parse_quote! { #q_base.#m }, name));
            }
            None
        }
        // ctx.outer.method(...)   →  q.outer.method(...)
        Expr::MethodCall(syn::ExprMethodCall {
            receiver, method, args, turbofish, ..
        }) => {
            if let Some((q_recv, name)) = ctx_field_to_q_field(receiver) {
                let args = args.clone();
                let turbofish = turbofish.clone();
                return Some((syn::parse_quote! { #q_recv.#method #turbofish ( #args ) }, name));
            }
            None
        }
        // ctx.outer[idx]   →  q.outer[idx]
        Expr::Index(syn::ExprIndex { expr: base, index, .. }) => {
            if let Some((q_base, name)) = ctx_field_to_q_field(base) {
                let idx = (**index).clone();
                return Some((syn::parse_quote! { #q_base[#idx] }, name));
            }
            None
        }
        _ => None,
    }
}

/// Rewrite all `*ctx.field` reads AND `ctx.X.<rest>` sub-widget /
/// sub-field reads in `expr`.  Returns the set of fields that
/// were read.
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
            if let Some((replacement, field)) = try_rewrite_ctx_subwidget_read(expr) {
                self.reads.insert(field);
                *expr = replacement;
                visit_mut::visit_expr_mut(self, expr);
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
    field_reads: std::collections::BTreeSet<String>,
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
                            self.field_reads.insert(field.to_string());
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
                        self.field_reads.insert(field.to_string());
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
