//! Phase 1.6: multiple `rule_kernel!` invocations now coexist in
//! one module.  This used to require wrapping each invocation in
//! its own submodule because the auto-derived `Q` and `D` types
//! collided.  Switching to the prefixed `<Name>Q` / `<Name>D`
//! form removes the collision.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel;

rule_kernel! {
    pub struct WidgetA {
        val: dff::DFF<Bits<8>>,
    }

    impl WidgetA {
        #[rule]
        fn bump(ctx: &mut RuleCtx<Self>, _enable: bool) {
            set!(ctx.val, *ctx.val + bits::<8>(1));
        }

        #[output]
        fn output(self_q: &Self, _enable: bool) -> Bits<8> {
            *self_q.val
        }
    }
}

rule_kernel! {
    pub struct WidgetB {
        val: dff::DFF<Bits<16>>,
    }

    impl WidgetB {
        #[rule]
        fn bump(ctx: &mut RuleCtx<Self>, _enable: bool) {
            set!(ctx.val, *ctx.val + bits::<16>(2));
        }

        #[output]
        fn output(self_q: &Self, _enable: bool) -> Bits<16> {
            *self_q.val
        }
    }
}

#[test]
fn multi_widget_module_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: WidgetA = WidgetA::default();
    let stream = std::iter::repeat_n(true, 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}

#[test]
fn two_widgets_in_one_module_compile() {
    let _a: WidgetA = WidgetA::default();
    let _b: WidgetB = WidgetB::default();
}

#[test]
fn both_widgets_run() {
    let a: WidgetA = WidgetA::default();
    let stream_a = std::iter::repeat_n(true, 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let last_a = a
        .run(stream_a)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0xff);
    assert!(last_a >= 3 && last_a <= 4);

    let b: WidgetB = WidgetB::default();
    let stream_b = std::iter::repeat_n(true, 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let last_b = b
        .run(stream_b)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0xffff);
    assert!(last_b >= 6 && last_b <= 8);
}
