//! Documented limitation — sub-widget access through `ctx`.
//!
//! Companion to `tests/cross_kernel_call.rs`.  Cross-kernel calls
//! work; sub-widget state access through `ctx` does not.  This
//! test would compile if sub-widget access were supported, but
//! today it produces "cannot find value `ctx`" errors because the
//! rule-body walker only recognises `*ctx.<dff_field>` reads, not
//! `ctx.<sub_widget>.<field>` paths.
//!
//! The failure is gated behind `#[cfg(any())]` so the rest of the
//! test suite still compiles cleanly; flip the cfg to manually
//! verify the failure when investigating.
//!
//! ## Why this matters
//!
//! In Bluespec System Verilog, a rule body can freely call methods
//! on submodules:
//!
//! ```bsv
//! rule do_step;
//!   let x = regfile.read(rsel);    // submodule method call
//!   alu_unit.compute(x, t);        // ditto
//! endrule
//! ```
//!
//! `rhdl-rule`'s analogue would be:
//!
//! ```rust,ignore
//! #[rule]
//! fn do_step(ctx: &mut RuleCtx<Self>, ...) {
//!     let x = ctx.regfile.cells[rsel];   // <-- doesn't lower
//!     // ...
//! }
//! ```
//!
//! This is the **actual** limitation the Alto task system (PR #44)
//! hit — not the cross-kernel-call thing the original CHANGELOG
//! attributed it to.  Sub-widget access requires the rule walker
//! to know which fields are DFFs vs. sub-widgets and emit
//! different lowering for each.
//!
//! ## Workaround until this lands
//!
//! Compose: keep the `rule_kernel_attr` widget pure-DFF, and have
//! a separate regular `Synchronous` widget that consumes the
//! rule-kernel's output and drives the sub-widgets.  This is the
//! pattern PR #44's `AltoTaskSystem` + `Microengine` use.

#![cfg(any())] // not compiled normally; flip to investigate.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel_attr;

/// A standalone Synchronous widget — used as a sub-widget below.
#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct SubwidgetCounter {
    n: dff::DFF<Bits<8>>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct SubwidgetIn { pub bump: bool }

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct SubwidgetOut { pub current: Bits<8> }

impl SynchronousIO for SubwidgetCounter {
    type I = SubwidgetIn;
    type O = SubwidgetOut;
    type Kernel = subwidget_kernel;
}

#[kernel]
pub fn subwidget_kernel(_cr: ClockReset, i: SubwidgetIn, q: Q) -> (SubwidgetOut, D) {
    let mut d = D::dont_care();
    d.n = if i.bump { *q.n + bits::<8>(1) } else { *q.n };
    (SubwidgetOut { current: *q.n }, d)
}

/// Try to compose a rule kernel that reads a sub-widget's state via ctx.
/// **This currently fails to compile** with "cannot find value `ctx`"
/// because the rule walker doesn't recognise `ctx.<sub_widget>.<field>`.
#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct RuleWithSubwidget {
    sub: SubwidgetCounter,
    last_seen: dff::DFF<Bits<8>>,
}

#[rule_kernel_attr]
impl RuleWithSubwidget {
    #[rule]
    fn observe(ctx: &mut RuleCtx<Self>, _i: bool) {
        // The line below is what we'd LIKE to write — read the
        // sub-widget's output through ctx.  Today the macro doesn't
        // recognise `ctx.<sub>.<field>` and the lowered kernel ends
        // up with a stray `ctx` reference that the compiler can't
        // resolve.
        let observed = ctx.sub.current;
        ctx.last_seen = observed;
    }

    #[output]
    fn output(self_q: &Self, _i: bool) -> Bits<8> {
        *self_q.last_seen
    }
}
