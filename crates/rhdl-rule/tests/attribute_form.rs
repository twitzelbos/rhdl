//! Phase 2-followup — `#[rule_kernel_attr]` attribute-on-impl form.
//!
//! Mirrors the Phase-1 `simple_counter.rs` test set, but uses the
//! attribute form instead of the function-like `rule_kernel!`.
//! Demonstrates that:
//!
//! 1. The user writes the standard RHDL derives on the struct
//!    themselves (just like every other RHDL widget) and applies
//!    `#[rule_kernel_attr]` to the impl block.
//! 2. The two forms are behaviourally identical — the lowering code
//!    is shared in `rhdl-rule-core::lower_rule_kernel`.
//! 3. Generic structs work in the attribute form too.
//! 4. Multiple kernels can coexist in one module.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel_attr;

// ---- Single-rule kernel (parity with simple_counter.rs) ----

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct AttrCounter {
    counter: dff::DFF<Bits<8>>,
}

#[rule_kernel_attr]
impl AttrCounter {
    #[rule]
    fn increment(ctx: &mut RuleCtx<Self>, enable: bool) {
        guard!(enable);
        set!(ctx.counter, *ctx.counter + bits::<8>(1));
    }

    #[output]
    fn output(self_q: &Self, _enable: bool) -> Bits<8> {
        *self_q.counter
    }
}

#[test]
fn attr_counter_counts_when_enabled() {
    let uut: AttrCounter = AttrCounter::default();
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
    assert!(last >= 4 && last <= 5, "expected ~5 counts, got {last}");
}

#[test]
fn attr_counter_holds_when_disabled() {
    let uut: AttrCounter = AttrCounter::default();
    let stream = std::iter::repeat_n(false, 5)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(99);
    assert_eq!(last, 0, "expected counter to hold at 0; got {last}");
}

#[test]
fn attr_counter_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: AttrCounter = AttrCounter::default();
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

// ---- Multi-rule with priority + conflict_free (parity with priority_demo) ----

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct AttrPriority {
    val: dff::DFF<Bits<8>>,
}

#[rule_kernel_attr]
impl AttrPriority {
    #[rule(priority = 0)]
    fn winner(ctx: &mut RuleCtx<Self>, _flag: bool) {
        set!(ctx.val, bits::<8>(0xaa));
    }

    #[rule(priority = 1)]
    fn loser(ctx: &mut RuleCtx<Self>, _flag: bool) {
        set!(ctx.val, bits::<8>(0xbb));
    }

    #[output]
    fn output(self_q: &Self, _flag: bool) -> Bits<8> {
        *self_q.val
    }
}

#[test]
fn attr_priority_chain_picks_winner() {
    let uut: AttrPriority = AttrPriority::default();
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
    assert_eq!(last, 0xaa, "expected priority winner (0xaa); got {last:#x}");
}

// ---- Generic struct via attribute form ----

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct AttrGenericCounter<const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    count: dff::DFF<Bits<N>>,
}

#[rule_kernel_attr]
impl<const N: usize> AttrGenericCounter<N>
where
    rhdl::bits::W<N>: BitWidth,
{
    #[rule]
    fn bump(ctx: &mut RuleCtx<Self>, enable: bool) {
        guard!(enable);
        set!(ctx.count, *ctx.count + bits::<N>(1));
    }

    #[output]
    fn output(self_q: &Self, _enable: bool) -> Bits<N> {
        *self_q.count
    }
}

#[test]
fn attr_generic_counter_at_width_4_counts() {
    let uut: AttrGenericCounter<4> = AttrGenericCounter::default();
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
    assert!(
        last >= 3 && last <= 5,
        "expected ~5 bumps at N=4, got {last}"
    );
}

#[test]
fn attr_generic_counter_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: AttrGenericCounter<6> = AttrGenericCounter::default();
    let stream = std::iter::repeat_n(true, 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}

// ---- Two kernels in one module — both function-like AND attribute mix ----

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct AttrWidgetA {
    val: dff::DFF<Bits<8>>,
}

#[rule_kernel_attr]
impl AttrWidgetA {
    #[rule]
    fn bump(ctx: &mut RuleCtx<Self>, _enable: bool) {
        set!(ctx.val, *ctx.val + bits::<8>(1));
    }

    #[output]
    fn output(self_q: &Self, _enable: bool) -> Bits<8> {
        *self_q.val
    }
}

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct AttrWidgetB {
    val: dff::DFF<Bits<16>>,
}

#[rule_kernel_attr]
impl AttrWidgetB {
    #[rule]
    fn bump(ctx: &mut RuleCtx<Self>, _enable: bool) {
        set!(ctx.val, *ctx.val + bits::<16>(2));
    }

    #[output]
    fn output(self_q: &Self, _enable: bool) -> Bits<16> {
        *self_q.val
    }
}

#[test]
fn attr_two_widgets_in_one_module_compile() {
    let _a: AttrWidgetA = AttrWidgetA::default();
    let _b: AttrWidgetB = AttrWidgetB::default();
}

#[test]
fn attr_widget_a_runs() {
    let a: AttrWidgetA = AttrWidgetA::default();
    let stream = std::iter::repeat_n(true, 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = a
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0xff);
    assert!(last >= 3 && last <= 4);
}
