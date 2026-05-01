//! Sub-widget composition inside rule kernels — full version.
//!
//! Companion to `tests/subwidget_read.rs` (DFF sub-field/method
//! access) and the documented limitation in
//! `tests/subwidget_access_known_failing.rs` (now obsolete for the
//! function-like form).
//!
//! What this PR enables: a `rule_kernel!` struct can now compose
//! a sub-widget as one of its fields, and rules can read the
//! sub-widget's outputs via `ctx.subwidget.<output_field>`.  The
//! sub-widget's input is auto-driven to `Default::default()` each
//! cycle (since no rule writes it directly).
//!
//! For the **function-like form**, the macro auto-classifies struct
//! fields by inspecting their type tokens: `dff::DFF<T>` and
//! `Reg<T>` are classified as DFF; everything else as sub-widget.
//!
//! For the **attribute form** (where the macro doesn't see the
//! struct), the user lists sub-widget field names explicitly:
//!
//! ```rust,ignore
//! #[rule_kernel_attr(subwidgets = "regs, sub")]
//! impl Foo { ... }
//! ```
//!
//! ## What's still TODO
//!
//! Driving sub-widget inputs from a rule body (`ctx.subwidget = SubIn { ... }`)
//! is not yet supported — sub-widgets receive `Default::default()`
//! inputs unconditionally.  Useful for observation patterns;
//! patterns that need to drive the sub-widget per-cycle (like the
//! Alto regfile case where a rule wants to read R[idx] for some
//! runtime idx) still need the workaround pattern (compose the
//! sub-widget at the parent layer, drive its input externally).

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::{rule_kernel, rule_kernel_attr};

// =================================================================
// Test fixture: a free-running counter sub-widget.
// =================================================================

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct CounterIn {}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct CounterOut {
    pub current: Bits<8>,
    pub is_even: bool,
}

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct FreeCounter {
    n: dff::DFF<Bits<8>>,
}

impl SynchronousIO for FreeCounter {
    type I = CounterIn;
    type O = CounterOut;
    type Kernel = free_counter_kernel;
}

#[kernel]
pub fn free_counter_kernel(_cr: ClockReset, _i: CounterIn, q: Q) -> (CounterOut, D) {
    let mut d = D::dont_care();
    d.n = q.n + bits::<8>(1);
    let out = CounterOut {
        current: q.n,
        is_even: (q.n & bits::<8>(1)) == bits::<8>(0),
    };
    (out, d)
}

// =================================================================
// 1. Function-like form — auto-classification.
//
// The macro sees the struct definition and classifies `counter`
// as a sub-widget (its type isn't `dff::DFF<...>` or `Reg<...>`).
// =================================================================

rule_kernel! {
    pub struct ObserverFnLike {
        counter: FreeCounter,
        last_seen: dff::DFF<Bits<8>>,
        even_count: dff::DFF<Bits<8>>,
    }

    impl ObserverFnLike {
        #[rule]
        fn observe(ctx: &mut RuleCtx<Self>, _enable: bool) {
            ctx.last_seen = ctx.counter.current;
            // Bump even_count when the sub-widget reports an even value.
            ctx.even_count = if ctx.counter.is_even {
                *ctx.even_count + bits::<8>(1)
            } else {
                *ctx.even_count
            };
        }

        #[output]
        fn output(self_q: &Self, _enable: bool) -> (Bits<8>, Bits<8>) {
            (*self_q.last_seen, *self_q.even_count)
        }
    }
}

#[test]
fn fn_like_observer_reads_subwidget_output() {
    let uut: ObserverFnLike = ObserverFnLike::default();
    let stream = std::iter::repeat_n(true, 6)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output)
        .unwrap();
    // Counter starts at 0, increments each cycle.  After several
    // cycles, last_seen reflects the previous-cycle counter value.
    assert!(last.0.raw() >= 4, "last_seen should advance with the counter; got {}", last.0.raw());
    // Even count should be roughly half the cycles.
    assert!(last.1.raw() >= 2, "even_count should accumulate; got {}", last.1.raw());
}

// =================================================================
// 2. Attribute form — explicit `subwidgets = "..."` marker.
// =================================================================

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct ObserverAttr {
    counter: FreeCounter,
    last_seen: dff::DFF<Bits<8>>,
}

#[rule_kernel_attr(subwidgets = "counter")]
impl ObserverAttr {
    #[rule]
    fn observe(ctx: &mut RuleCtx<Self>, _enable: bool) {
        ctx.last_seen = ctx.counter.current;
    }

    #[output]
    fn output(self_q: &Self, _enable: bool) -> Bits<8> {
        *self_q.last_seen
    }
}

#[test]
fn attr_form_observer_reads_subwidget_output() {
    let uut: ObserverAttr = ObserverAttr::default();
    let stream = std::iter::repeat_n(true, 5)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    assert!(last >= 3, "last_seen should advance; got {last}");
}

// =================================================================
// 3. Sub-widget output read via method call (extends PR #45's DFF
//    sub-field access to sub-widget outputs).
// =================================================================

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct ObserverWithBoolFlag {
    counter: FreeCounter,
    saw_even: dff::DFF<bool>,
}

#[rule_kernel_attr(subwidgets = "counter")]
impl ObserverWithBoolFlag {
    #[rule]
    fn observe(ctx: &mut RuleCtx<Self>, _enable: bool) {
        // Direct read of the sub-widget's bool output field.
        ctx.saw_even = *ctx.saw_even || ctx.counter.is_even;
    }

    #[output]
    fn output(self_q: &Self, _enable: bool) -> bool {
        *self_q.saw_even
    }
}

#[test]
fn observe_subwidget_bool_field() {
    let uut: ObserverWithBoolFlag = ObserverWithBoolFlag::default();
    let stream = std::iter::repeat_n(true, 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output)
        .unwrap_or(false);
    // Counter starts at 0 (even) so saw_even becomes true on cycle 1.
    assert!(last, "should have observed an even counter value");
}

// =================================================================
// 4. iverilog round-trip — proves the lowering produces real
//    synthesisable hardware with composed sub-widgets.
// =================================================================

#[test]
fn fn_like_subwidget_composition_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: ObserverFnLike = ObserverFnLike::default();
    let stream = std::iter::repeat_n(true, 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = test_bench.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}

#[test]
fn attr_form_subwidget_composition_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: ObserverAttr = ObserverAttr::default();
    let stream = std::iter::repeat_n(true, 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = test_bench.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
