//! `rhdl-rule` — Bluespec-style guarded atomic rules for RHDL.
//!
//! Proc-macro entry points.  Two surface spellings (see
//! `rule-architecture.md` §4.5):
//!
//! - [`rule_kernel!`] — function-like form, takes a struct + impl
//!   in one invocation; auto-injects the standard derives.
//! - [`macro@rule_kernel_attr`] — attribute form on an impl block;
//!   user writes `#[derive(Synchronous, SynchronousDQ)]` on the
//!   struct themselves.
//!
//! Both share the same lowering in [`rhdl-rule-core`](../rhdl_rule_core/index.html);
//! a token-level parity test enforces byte-identical output for
//! the same impl block.  Pick whichever spelling reads more
//! naturally for the widget at hand.
//!
//! # Phase 0 — minimal viable
//!
//! This is the *Phase 0* slice of the broader `rule-architecture.md`
//! plan.  It ships:
//!
//! - The [`rule_kernel!`] function-like macro that parses a struct
//!   + impl block containing `#[rule]` and `#[output]` methods, and
//!   emits a regular RHDL `Synchronous` widget + `#[kernel]`
//!   function.
//! - A simplified scheduler — rules fire in source-code (= priority)
//!   order; later rules' writes overwrite earlier rules' writes
//!   (last-write-wins), with no formal conflict analysis.  This is
//!   sufficient for non-conflicting rule sets and is the right
//!   semantics for many widgets.  Phase 1 adds the conflict-matrix
//!   + priority-arbitrated scheduler.
//! - The `guard!` and `set!` macros, recognised inside rule bodies.
//!
//! # What rule bodies can contain
//!
//! Rule bodies are lowered into the surrounding kernel function;
//! they accept the same Rust subset as any RHDL `#[kernel]` plus
//! the rule-specific extras.  Concretely:
//!
//! - **Guards**: `guard!(expr)` — rule's firing predicate.
//! - **Direct assignments to DFF fields**: `ctx.field = expr;` and
//!   the equivalent `set!(ctx.field, expr)` — recognised as atomic
//!   non-blocking commits at the cycle boundary.
//! - **DFF reads**: `*ctx.field` — rewritten to `q.field` and
//!   tracked in the rule's read-set (drives the conflict matrix).
//! - **DFF sub-field / method access**: `ctx.field.<inner>` (no
//!   leading `*`) — rewritten to `q.field.<inner>`.  Lets a rule
//!   directly access a method or inner field on a DFF-stored
//!   value without first reading the whole DFF into a let-binding:
//!   `ctx.flags.any()`, `ctx.table[idx]`, etc.  Same syntax also
//!   works for sub-widget output reads (`ctx.subwidget.out_field`),
//!   though full sub-widget composition has a separate next-decls
//!   issue tracked as a follow-up — see below.
//! - **Let-binding preambles**: `let x = expr;` — kept verbatim
//!   in the lowered kernel; in scope for every direct-assignment
//!   that follows.  Right tool for shared computation across
//!   multiple writes.
//! - **Cross-kernel calls**: rule bodies can freely call other
//!   `#[kernel]`-marked functions defined at module scope, the
//!   same way regular RHDL kernels can call other kernels.  This
//!   is the rhdl-rule analogue of BSV's "rule bodies can call any
//!   pure `function`."  See `tests/cross_kernel_call.rs` for
//!   worked examples.
//!
//! # What rule bodies can NOT contain (yet)
//!
//! - **Full sub-widget composition** — composing a sub-widget as
//!   a struct field of the rule kernel breaks the "auto-hold of
//!   unwritten fields" path: the next-decls emit
//!   `let _next_<field> = q.<field>;` for every field that no
//!   rule writes, but `q.<field>` is the sub-widget's `Out`
//!   struct while `d.<field>` is its `In` struct — the two have
//!   different types and the assignment fails to type-check.
//!   The walker rewrite (`ctx.X.Y` → `q.X.Y`) lowers the *read*
//!   half of sub-widget access correctly; the missing piece is
//!   the auto-hold strategy when the field is a sub-widget.
//!   Fixing that needs struct-type introspection (which the
//!   function-like form has but the attribute form doesn't) and
//!   a different lowering for sub-widget vs DFF auto-holds.
//!   Workaround: keep the rule kernel pure-DFF, and compose with
//!   sub-widgets at the parent layer via a regular `Synchronous`
//!   widget.  See `tests/subwidget_access_known_failing.rs` for
//!   the documented limitation and `crates/rhdl-alto/src/
//!   task_system.rs` for the real-world workaround pattern.
//!
//! - **Driving sub-widget inputs from a rule body** — same root
//!   cause as the above; needs the per-field type classification
//!   to know whether `ctx.field = expr` writes a DFF (existing
//!   path) or drives a sub-widget input (would need a different
//!   action lowering).
//!
//! # Example
//!
//! ```ignore
//! use rhdl::prelude::*;
//! use rhdl_fpga::core::dff;
//! use rhdl_rule::rule_kernel;
//!
//! rule_kernel! {
//!     #[derive(Clone, Debug, Default)]
//!     pub struct SimpleCounter {
//!         counter: dff::DFF<Bits<8>>,
//!     }
//!
//!     impl SimpleCounter {
//!         #[rule]
//!         fn increment(ctx: &mut RuleCtx<Self>, enable: bool) {
//!             guard!(enable);
//!             set!(ctx.counter, *ctx.counter + bits::<8>(1));
//!         }
//!
//!         #[output]
//!         fn output(self_q: &Self, _enable: bool) -> Bits<8> {
//!             *self_q.counter
//!         }
//!     }
//! }
//! ```

use proc_macro::TokenStream;

/// Function-like proc-macro: takes a struct definition + `impl`
/// block with `#[rule]` and `#[output]` methods, and emits a
/// regular RHDL `Synchronous` widget + `#[kernel]` function.
///
/// See `rule-architecture.md` §4.5 for the comparison with the
/// attribute form [`macro@rule_kernel_attr`] and the crate-level
/// docs for an example.
#[proc_macro]
pub fn rule_kernel(input: TokenStream) -> TokenStream {
    match rhdl_rule_core::expand_rule_kernel(input.into()) {
        Ok(output) => output.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Attribute proc-macro applied to the `impl` block of an existing
/// rule-kernel struct.  The user writes the standard RHDL derives
/// on the struct (`#[derive(Clone, Debug, Default, Synchronous,
/// SynchronousDQ)]`); this attribute walks the impl's `#[rule]`
/// and `#[output]` methods and synthesizes the scheduler + kernel
/// function.
///
/// Behaviourally identical to the function-like [`rule_kernel!`] —
/// the two share the same lowering code (`lower_rule_kernel` in
/// `rhdl-rule-core`).  Pick whichever spelling reads more
/// naturally; `rule-architecture.md` §4.5 walks through the
/// trade-off.
///
/// # Example
///
/// ```ignore
/// use rhdl::prelude::*;
/// use rhdl_fpga::core::dff;
/// use rhdl_rule::rule_kernel_attr as rule_kernel;
///
/// #[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
/// pub struct SimpleCounter {
///     counter: dff::DFF<Bits<8>>,
/// }
///
/// #[rule_kernel]
/// impl SimpleCounter {
///     #[rule]
///     fn increment(ctx: &mut RuleCtx<Self>, enable: bool) {
///         guard!(enable);
///         set!(ctx.counter, *ctx.counter + bits::<8>(1));
///     }
///
///     #[output]
///     fn output(self_q: &Self, _enable: bool) -> Bits<8> {
///         *self_q.counter
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn rule_kernel_attr(_attr: TokenStream, item: TokenStream) -> TokenStream {
    match rhdl_rule_core::expand_rule_kernel_attr(item.into()) {
        Ok(output) => output.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
