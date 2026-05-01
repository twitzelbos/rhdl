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
