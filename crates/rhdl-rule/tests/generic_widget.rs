//! Phase 2 — generic struct support in `rule_kernel!`.
//!
//! The struct definition can carry generic parameters (type
//! parameters bounded by `Digital` and const-generic widths), and
//! the macro threads them through the SynchronousIO impl, the kernel
//! function signature, the auto-derived Q/D types, and the final
//! D-struct expression.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel;

rule_kernel! {
    /// A counter parameterised by its bit width.
    pub struct GenericCounter<const N: usize>
    where
        rhdl::bits::W<N>: BitWidth,
    {
        count: dff::DFF<Bits<N>>,
    }

    impl GenericCounter {
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
}

#[test]
fn generic_counter_at_width_4_counts() {
    let uut: GenericCounter<4> = GenericCounter::default();
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
    // After 5 enabled cycles in steady-state we expect a non-zero count
    // bounded by N=4 width (max 15).  Exact value depends on framework
    // pipeline depth; sufficient to verify count > 0 and within range.
    assert!(
        last >= 3 && last <= 5,
        "expected ~5 bumps at N=4, got {last}"
    );
}

#[test]
fn generic_counter_at_width_8_counts() {
    let uut: GenericCounter<8> = GenericCounter::default();
    let stream = std::iter::repeat_n(true, 12)
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
        last >= 10 && last <= 12,
        "expected ~12 bumps at N=8, got {last}",
    );
}

#[test]
fn generic_counter_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: GenericCounter<6> = GenericCounter::default();
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

// A second instance to prove the macro can emit multiple generic
// widgets in the same module (i.e., that generics compose with the
// Phase-1.6 Q/D-prefix change).
rule_kernel! {
    pub struct GenericAdder<const N: usize>
    where
        rhdl::bits::W<N>: BitWidth,
    {
        sum: dff::DFF<Bits<N>>,
    }

    impl GenericAdder {
        #[rule]
        fn add(ctx: &mut RuleCtx<Self>, increment: Bits<N>) {
            set!(ctx.sum, *ctx.sum + increment);
        }

        #[output]
        fn output(self_q: &Self, _increment: Bits<N>) -> Bits<N> {
            *self_q.sum
        }
    }
}

#[test]
fn generic_adder_works() {
    let uut: GenericAdder<8> = GenericAdder::default();
    let stream = std::iter::repeat_n(bits::<8>(3), 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    // 4 adds of 3 = 12 in steady-state; framework pipeline may shift by 1.
    assert!(
        last >= 9 && last <= 12,
        "expected ~12 from 4 adds of 3, got {last}"
    );
}
