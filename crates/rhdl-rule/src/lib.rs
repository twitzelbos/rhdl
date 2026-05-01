//! `rhdl-rule` — Bluespec-style guarded atomic rules for RHDL.
//!
//! Proc-macro entry points.  See [`rhdl-rule-core`](../rhdl_rule_core/index.html)
//! for the implementation, and `rule-architecture.md` in the repo
//! root for the design.
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
/// See `rule-architecture.md` for the design and the crate-level
/// docs for an example.
#[proc_macro]
pub fn rule_kernel(input: TokenStream) -> TokenStream {
    match rhdl_rule_core::expand_rule_kernel(input.into()) {
        Ok(output) => output.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
