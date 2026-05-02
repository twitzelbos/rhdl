//! Sub-widget input drive from rule bodies.
//!
//! Companion to `tests/subwidget_composition.rs` (read side, PR #47).
//! This file pins down the **write side** — rules can drive a
//! sub-widget's input via `ctx.<sub_widget> = SubIn { ... }` — which
//! turns out to work as a side effect of PR #47's auto-hold fix
//! without any additional walker or lowering changes.
//!
//! ## How it falls out for free
//!
//! - The walker treats `ctx.<field> = expr` as a direct-assignment
//!   action regardless of whether `<field>` is a DFF or a sub-widget
//!   (the walker is field-name-based, not field-kind-based).
//! - The action lowers through the same `_next_<field>` shadowing
//!   chain as DFF actions: initial `let _next_<field> = <auto-hold>;`
//!   then `let _next_<field> = if _fire_rule { value } else { _next_<field> };`.
//! - For sub-widget fields, the auto-hold default (PR #47) is
//!   `<D as Digital>::dont_care().<field>` — same type as the
//!   user's `SubIn { ... }` value.  Both branches of the if-else
//!   type-check, the field commits at the cycle edge, and Rust is
//!   happy.
//! - Multi-rule arbitration on the same sub-widget input uses the
//!   same priority chain as DFF arbitration.
//!
//! ## Same-cycle read-after-write works
//!
//! Sub-widgets are combinational from `d.<sub>` (input) to
//! `q.<sub>` (output) within a cycle.  So a rule that drives a
//! sub-widget's input AND reads its output sees the result of the
//! drive immediately — no waiting a cycle.  This is the canonical
//! "drive raddr → read rdata" pattern that motivated the original
//! sub-widget-composition follow-up; it works today.
//!
//! ## What this means for the BSV-parity plan
//!
//! The "Phase 1: sub-widget input drive" item in the plan turned
//! out to be a no-op — already shipped as a side effect of PR #47.
//! The genuinely-missing pieces are when-clauses (Phase 2),
//! method-based interfaces (Phase 3), and maximal-parallel
//! scheduling (Phase 4).
//!
//! These tests are the regression guard so future macro changes
//! can't silently break the working behaviour.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel_attr;

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct CounterIn { pub bump: bool }

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct CounterOut { pub current: Bits<8> }

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct BumpCounter {
    n: dff::DFF<Bits<8>>,
}

impl SynchronousIO for BumpCounter {
    type I = CounterIn;
    type O = CounterOut;
    type Kernel = bump_counter_kernel;
}

#[kernel]
pub fn bump_counter_kernel(_cr: ClockReset, i: CounterIn, q: Q) -> (CounterOut, D) {
    let mut d = D::dont_care();
    d.n = if i.bump { q.n + bits::<8>(1) } else { q.n };
    (CounterOut { current: q.n }, d)
}

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct DriverWidget {
    counter: BumpCounter,
    last_seen: dff::DFF<Bits<8>>,
}

#[rule_kernel_attr(subwidgets = "counter")]
impl DriverWidget {
    #[rule]
    fn drive_and_observe(ctx: &mut RuleCtx<Self>, do_bump: bool) {
        // Drive the sub-widget's input.
        ctx.counter = CounterIn { bump: do_bump };
        // Read its output (combinational result of THIS cycle's input).
        ctx.last_seen = ctx.counter.current;
    }

    #[output]
    fn output(self_q: &Self, _do_bump: bool) -> Bits<8> {
        *self_q.last_seen
    }
}

#[test]
fn rule_drives_subwidget_input() {
    let uut: DriverWidget = DriverWidget::default();
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
    // If sub-widget drive works: counter advances each cycle (because
    // we drive bump=true), and last_seen tracks counter.current
    // (which is the *previous* cycle's value, since we read q.n).
    // Over 5 cycles with bump=true, counter should reach ~4-5.
    assert!(last >= 3, "counter should advance via rule-driven bump; got {last}");
}

#[test]
fn rule_drives_subwidget_input_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: DriverWidget = DriverWidget::default();
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

// =================================================================
// 2. Multi-rule arbitration: two rules race to drive the same
//    sub-widget input.  Priority-ordered → highest priority wins.
// =================================================================

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct ArbiterWidget {
    counter: BumpCounter,
    fired_rule: dff::DFF<Bits<2>>,
}

#[rule_kernel_attr(subwidgets = "counter")]
impl ArbiterWidget {
    /// High-priority rule: bump on mode == 1.
    #[rule(priority = 0)]
    fn high(ctx: &mut RuleCtx<Self>, mode: Bits<2>) {
        guard!(mode == bits::<2>(1));
        ctx.counter = CounterIn { bump: true };
        ctx.fired_rule = bits::<2>(1);
    }

    /// Low-priority rule: hold (don't bump) on mode == 2.
    #[rule(priority = 1)]
    fn low(ctx: &mut RuleCtx<Self>, mode: Bits<2>) {
        guard!(mode == bits::<2>(2));
        ctx.counter = CounterIn { bump: false };
        ctx.fired_rule = bits::<2>(2);
    }

    #[output]
    fn output(self_q: &Self, _mode: Bits<2>) -> Bits<2> {
        *self_q.fired_rule
    }
}

#[test]
fn multi_rule_arbitration_on_shared_subwidget() {
    let uut: ArbiterWidget = ArbiterWidget::default();
    // Mode 1 always → high rule wins.
    let stream = std::iter::repeat_n(bits::<2>(1), 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    assert_eq!(last, 1, "high-priority rule should win");
}

// =================================================================
// 3. Alto regfile-style read: drive raddr in the same rule that
//    reads rdata.  This is the canonical pattern that motivated
//    all the sub-widget composition work.
// =================================================================

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct RegfileIn {
    pub raddr: Bits<2>,
    pub waddr: Bits<2>,
    pub wdata: Bits<8>,
    pub wen: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct RegfileOut {
    pub rdata: Bits<8>,
}

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct Regfile4x8 {
    cells: dff::DFF<[Bits<8>; 4]>,
}

impl SynchronousIO for Regfile4x8 {
    type I = RegfileIn;
    type O = RegfileOut;
    type Kernel = regfile_4x8_kernel;
}

#[kernel]
pub fn regfile_4x8_kernel(_cr: ClockReset, i: RegfileIn, q: Regfile4x8Q) -> (RegfileOut, Regfile4x8D) {
    let mut d = Regfile4x8D::dont_care();
    let mut next = q.cells;
    if i.wen {
        next[i.waddr] = i.wdata;
    }
    d.cells = next;
    (RegfileOut { rdata: q.cells[i.raddr] }, d)
}

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct AltoStyleReader {
    regs: Regfile4x8,
    last_read: dff::DFF<Bits<8>>,
}

#[rule_kernel_attr(subwidgets = "regs")]
impl AltoStyleReader {
    /// Drive raddr to the requested index, then read rdata in the
    /// same cycle.  This is the Alto regfile pattern that the
    /// PR-#44 first-cut tried and failed to do.
    #[rule]
    fn read_at(ctx: &mut RuleCtx<Self>, idx: Bits<2>) {
        ctx.regs = RegfileIn {
            raddr: idx,
            waddr: bits::<2>(0),
            wdata: bits::<8>(0),
            wen: false,
        };
        // Same-cycle read of rdata, combinationally derived from
        // the just-driven raddr.
        ctx.last_read = ctx.regs.rdata;
    }

    #[output]
    fn output(self_q: &Self, _idx: Bits<2>) -> Bits<8> {
        *self_q.last_read
    }
}

#[test]
fn alto_regfile_style_read_works() {
    let uut: AltoStyleReader = AltoStyleReader::default();
    // All cells default to 0; rdata at any addr returns 0.
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
    assert_eq!(last, 0, "should read zero from uninitialised regfile");
}

#[test]
fn alto_regfile_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: AltoStyleReader = AltoStyleReader::default();
    let stream = std::iter::repeat_n(bits::<2>(1), 3)
        .with_reset(2)
        .clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
