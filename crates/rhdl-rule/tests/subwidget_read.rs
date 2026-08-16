//! Sub-field and method access on DFF-stored values from rule
//! bodies.
//!
//! Companion to `tests/cross_kernel_call.rs` and the documented
//! limitation in `tests/subwidget_access_known_failing.rs`.  This
//! test file demonstrates the **DFF sub-field / method** path that
//! the walker change in PR #46 unlocks: a rule body can write
//! `ctx.<dff>.<inner_field>` or `ctx.<dff>.<method>(args)` and the
//! walker rewrites it to `q.<dff>.<inner_field>` /
//! `q.<dff>.<method>(args)`, tracking the DFF in the read-set.
//!
//! Before this change, such access required a clumsy two-step
//! pattern: read the DFF into a let-binding (`let v = *ctx.dff;`)
//! and then access the sub-field (`v.<inner>`).  Now the access
//! is direct.
//!
//! ## Why not full sub-widget composition?
//!
//! The walker change supports the read syntax for sub-widget
//! composition too — `ctx.<sub_widget>.<output_field>` lowers
//! correctly to `q.<sub_widget>.<output_field>`.  But the
//! auto-hold path in `lower_rule_kernel` emits
//! `let _next_<field> = q.<field>;` for every field that no rule
//! writes — which type-errors for sub-widget fields, where
//! `q.<field>` (the sub-widget's `Out` struct) and `d.<field>`
//! (its `In` struct) differ.  Fixing that needs struct-type
//! introspection (function-like form has it; attribute form
//! doesn't), which is the bigger follow-up.  See
//! `tests/subwidget_access_known_failing.rs` for the worked
//! limitation example and `tests/cross_kernel_call.rs` for the
//! current workaround pattern (compose at the parent layer).

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel_attr;

// =================================================================
// 1. Method call on a DFF-stored value.
//
// `Bits<N>` has methods like `.any()`, `.all()`, `.bit(...)` —
// before this change, calling them on a DFF required reading the
// DFF first into a let-binding.  Now the access is direct.
// =================================================================

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct MethodAccessWidget {
    flags: dff::DFF<Bits<8>>,
    seen_any: dff::DFF<bool>,
}

#[rule_kernel_attr]
impl MethodAccessWidget {
    /// Direct method call on a DFF: `ctx.flags.any()` is the new
    /// idiom; previously this required `let v = *ctx.flags;
    /// v.any()`.
    #[rule]
    fn step(ctx: &mut RuleCtx<Self>, new_flags: Bits<8>) {
        let any_before = ctx.flags.any();
        ctx.flags = new_flags;
        ctx.seen_any = *ctx.seen_any || any_before;
    }

    #[output]
    fn output(self_q: &Self, _new_flags: Bits<8>) -> (Bits<8>, bool) {
        (*self_q.flags, *self_q.seen_any)
    }
}

#[test]
fn method_on_dff_via_ctx() {
    let uut: MethodAccessWidget = MethodAccessWidget::default();
    // Cycle 0: drive non-zero flags so cycle 1 sees `any() == true`
    // on the previous-cycle flags.
    let stream = std::iter::once(bits::<8>(0xFF))
        .chain(std::iter::repeat_n(bits::<8>(0x00), 4))
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output)
        .unwrap();
    assert!(
        last.1,
        "should have seen any()=true on the previous cycle's flags"
    );
}

// =================================================================
// 2. Indexing into a DFF-stored array.
//
// `dff::DFF<[T; N]>` is a common pattern (e.g. our register file).
// Indexing `ctx.array[idx]` previously needed
// `let v = *ctx.array; v[idx]`.  Now: direct.
// =================================================================

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct ArrayAccessWidget {
    table: dff::DFF<[Bits<8>; 4]>,
    last_read: dff::DFF<Bits<8>>,
}

#[rule_kernel_attr]
impl ArrayAccessWidget {
    #[rule]
    fn step(ctx: &mut RuleCtx<Self>, idx: Bits<2>) {
        // Direct index into the DFF-stored array via ctx.
        ctx.last_read = ctx.table[idx];
    }

    #[output]
    fn output(self_q: &Self, _idx: Bits<2>) -> Bits<8> {
        *self_q.last_read
    }
}

#[test]
fn index_into_dff_array_via_ctx() {
    let uut: ArrayAccessWidget = ArrayAccessWidget::default();
    let stream = std::iter::repeat_n(bits::<2>(2), 3)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(99);
    // Initial array is all-zero, so reads return 0.
    assert_eq!(last, 0);
}

// =================================================================
// 3. Multiple sub-field accesses combined in one rule body.
// =================================================================

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct CombinedAccessWidget {
    a: dff::DFF<Bits<8>>,
    b: dff::DFF<Bits<8>>,
    out: dff::DFF<Bits<8>>,
}

#[rule_kernel_attr]
impl CombinedAccessWidget {
    /// Mix DFF reads (existing `*ctx` pattern) with method calls
    /// on DFFs (new `ctx.x.method()` pattern).
    #[rule]
    fn combine(ctx: &mut RuleCtx<Self>, _enable: bool) {
        // Combination of `*` deref read + method-call read.
        let any_a = ctx.a.any();
        let raw_b = *ctx.b;
        ctx.out = if any_a { raw_b } else { bits::<8>(0xFF) };
    }

    #[output]
    fn output(self_q: &Self, _enable: bool) -> Bits<8> {
        *self_q.out
    }
}

#[test]
fn combined_dff_access_patterns_work() {
    let uut: CombinedAccessWidget = CombinedAccessWidget::default();
    // a is 0 initially, so any() = false → out gets 0xFF.
    let stream = std::iter::repeat_n(true, 3)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    assert_eq!(last, 0xFF);
}

// =================================================================
// 4. iverilog round-trip — proves the rewrites produce real
//    synthesisable hardware.
// =================================================================

#[test]
fn dff_subfield_access_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: MethodAccessWidget = MethodAccessWidget::default();
    let stream = std::iter::repeat_n(bits::<8>(0xCD), 3)
        .with_reset(2)
        .clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = test_bench.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
