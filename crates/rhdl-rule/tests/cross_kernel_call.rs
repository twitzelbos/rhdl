//! Cross-kernel calls inside rule bodies.
//!
//! Demonstrates that a `#[rule]` body in a `rule_kernel_attr` impl
//! can freely call other `#[kernel]`-marked functions defined at
//! module scope, the same way regular RHDL kernels can call other
//! kernels.  This is the rhdl-rule analogue of BSV's "rule bodies
//! can call any pure `function`".
//!
//! ## Background
//!
//! While building the Alto task system (Tier C core 2, PR #44), I
//! initially thought cross-kernel calls were unsupported — my
//! first-cut Phase 2 had each task rule call shared `compute_cycle`
//! and `unpack_microinstruction` helpers, and the build failed with
//! "Unsupported statement type" + "cannot find value `ctx`" errors.
//!
//! The PR #44 CHANGELOG documented this as a rhdl-rule limitation
//! to follow up on.  When I started writing the fix, I built this
//! minimal repro first to confirm the failure mode — and discovered
//! the cross-kernel call *already worked*.  The actual failure was
//! sub-widget access through `ctx` (`ctx.regs.cells[mi.rsel]`),
//! not the function call itself.
//!
//! These tests freeze the cross-kernel-call behaviour as a
//! regression guard, document the actual constraint surface, and
//! point future contributors at the *real* limitation
//! (sub-widget access via ctx), which is the bigger fix.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel_attr;

// =================================================================
// 1. Simplest case — single-arg kernel called from the preamble.
// =================================================================

#[kernel]
pub fn helper_increment(x: Bits<8>) -> Bits<8> {
    x + bits::<8>(1)
}

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct IncrementerWidget {
    v: dff::DFF<Bits<8>>,
}

#[rule_kernel_attr]
impl IncrementerWidget {
    #[rule]
    fn step(ctx: &mut RuleCtx<Self>, enable: bool) {
        guard!(enable);
        // Cross-kernel call in the preamble.
        let next = helper_increment(*ctx.v);
        ctx.v = next;
    }

    #[output]
    fn output(self_q: &Self, _enable: bool) -> Bits<8> {
        *self_q.v
    }
}

#[test]
fn increment_via_helper_kernel() {
    let uut: IncrementerWidget = IncrementerWidget::default();
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
        last >= 4 && last <= 5,
        "expected ~5 increments via helper kernel; got {last}"
    );
}

// =================================================================
// 2. Multi-arg kernel + struct-typed return value — the canonical
//    "factor a complex per-cycle computation into a helper" pattern.
//    This was exactly what the Alto task system wanted to do.
// =================================================================

/// A 3-field result, the kind of thing factored helpers commonly return.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub struct Step {
    pub next: Bits<8>,
    pub overflow: bool,
    pub doubled: Bits<8>,
}

#[kernel]
pub fn helper_compute(value: Bits<8>, step: Bits<8>) -> Step {
    let next = value + step;
    let overflow = next < value; // wraparound detection
    let doubled = next + next;
    Step {
        next,
        overflow,
        doubled,
    }
}

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct ComputeWidget {
    value: dff::DFF<Bits<8>>,
    overflow_seen: dff::DFF<bool>,
    last_doubled: dff::DFF<Bits<8>>,
}

#[rule_kernel_attr]
impl ComputeWidget {
    #[rule]
    fn advance(ctx: &mut RuleCtx<Self>, step: Bits<8>) {
        // ONE call into the helper kernel; multiple ctx writes
        // reference its result.  This is the pattern the Alto
        // task system wanted: factor shared per-cycle math into a
        // helper, then commit the result to multiple DFFs.
        let r = helper_compute(*ctx.value, step);
        ctx.value = r.next;
        ctx.overflow_seen = *ctx.overflow_seen || r.overflow;
        ctx.last_doubled = r.doubled;
    }

    #[output]
    fn output(self_q: &Self, _step: Bits<8>) -> (Bits<8>, bool, Bits<8>) {
        (*self_q.value, *self_q.overflow_seen, *self_q.last_doubled)
    }
}

#[test]
fn multi_arg_helper_struct_return() {
    let uut: ComputeWidget = ComputeWidget::default();
    // Step by 5 each cycle, four cycles → value should be 20.
    let stream = std::iter::repeat_n(bits::<8>(5), 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output)
        .unwrap();
    // 3 commits visible × step 5 = 15.  (The 4th input is the
    // input being driven on the cycle the trace ends.)
    assert_eq!(last.0.raw(), 15, "value after the last visible commit");
    assert!(!last.1, "no overflow at value=15");
    assert_eq!(last.2.raw(), 30, "doubled = 2 × 15");
}

// =================================================================
// 3. Multiple helper kernels called from one rule body —
//    composition through a chain of helpers.
// =================================================================

#[kernel]
pub fn helper_mask_low_4(x: Bits<8>) -> Bits<8> {
    x & bits::<8>(0x0F)
}
#[kernel]
pub fn helper_shift_up_4(x: Bits<8>) -> Bits<8> {
    x << 4
}
#[kernel]
pub fn helper_combine(low: Bits<8>, high: Bits<8>) -> Bits<8> {
    low | high
}

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct ChainWidget {
    out: dff::DFF<Bits<8>>,
}

#[rule_kernel_attr]
impl ChainWidget {
    #[rule]
    fn step(ctx: &mut RuleCtx<Self>, input: Bits<8>) {
        // Chain of three helper-kernel calls.
        let low = helper_mask_low_4(input);
        let shifted = helper_shift_up_4(input);
        let combined = helper_combine(low, shifted);
        ctx.out = combined;
    }

    #[output]
    fn output(self_q: &Self, _input: Bits<8>) -> Bits<8> {
        *self_q.out
    }
}

#[test]
fn chain_of_helpers_composes() {
    let uut: ChainWidget = ChainWidget::default();
    // Input 0xA5 → low_nibble = 0x05; shifted = 0x50.
    // combined = 0x55.
    let stream = std::iter::repeat_n(bits::<8>(0xA5), 3)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    assert_eq!(last, 0x55);
}

// =================================================================
// 4. Cross-kernel call in a multi-rule kernel — to confirm the
//    pattern composes with rhdl-rule's scheduler / priority logic.
// =================================================================

#[kernel]
pub fn helper_next_value(current: Bits<8>) -> Bits<8> {
    current + bits::<8>(2)
}

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct MultiRuleHelperWidget {
    counter: dff::DFF<Bits<8>>,
    last_rule: dff::DFF<Bits<2>>,
}

#[rule_kernel_attr]
impl MultiRuleHelperWidget {
    /// High-priority rule; calls helper_next_value.
    #[rule(priority = 0)]
    fn high_priority(ctx: &mut RuleCtx<Self>, mode: Bits<2>) {
        guard!(mode == bits::<2>(0b01));
        ctx.counter = helper_next_value(*ctx.counter);
        ctx.last_rule = bits::<2>(1);
    }

    /// Low-priority rule; also calls helper_next_value.
    #[rule(priority = 1)]
    fn low_priority(ctx: &mut RuleCtx<Self>, mode: Bits<2>) {
        guard!(mode == bits::<2>(0b10));
        ctx.counter = helper_next_value(*ctx.counter);
        ctx.last_rule = bits::<2>(2);
    }

    #[output]
    fn output(self_q: &Self, _mode: Bits<2>) -> (Bits<8>, Bits<2>) {
        (*self_q.counter, *self_q.last_rule)
    }
}

#[test]
fn helper_works_inside_multi_rule_priority_kernel() {
    let uut: MultiRuleHelperWidget = MultiRuleHelperWidget::default();
    // Mode 0b01 always → high-priority rule fires every cycle.
    let stream = std::iter::repeat_n(bits::<2>(0b01), 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output)
        .unwrap();
    // 4 increments by 2 each = ~6-8.
    assert!(
        last.0.raw() >= 6,
        "counter should advance by 2 each cycle; got {}",
        last.0.raw()
    );
    assert_eq!(last.1.raw(), 1, "high_priority rule should win");
}

// =================================================================
// 5. iverilog round-trip — proves the cross-kernel-call lowering
//    produces real synthesisable hardware, not just a Rust-only
//    simulation artifact.
// =================================================================

#[test]
fn cross_kernel_call_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: ComputeWidget = ComputeWidget::default();
    let stream = std::iter::repeat_n(bits::<8>(3), 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = test_bench.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
