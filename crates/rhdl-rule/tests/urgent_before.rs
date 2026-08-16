//! Phase 2 — `urgent_before` annotation.
//!
//! `#[rule(urgent_before = "other")]` declares a partial-order edge
//! in the schedule: when both rules are ready and they conflict, the
//! annotated rule fires.  The macro topologically sorts the rules
//! over the `urgent_before` DAG, breaking ties with explicit
//! `priority` then source order.
//!
//! The annotation is only meaningful for rules that conflict;
//! declaring `urgent_before` between non-conflicting rules is a
//! compile error (no schedule choice to influence).
//!
//! These tests rely on the same shape as `priority_demo.rs`: two
//! rules write the same register; whichever fires "wins".

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel;

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct ContestInput {
    pub want_lo: bool,
    pub want_hi: bool,
}

// `prefer_lo` is urgent_before `prefer_hi`, so when both fire the
// register lands at 0x42 (the `lo` value), independent of priority.
rule_kernel! {
    pub struct UrgentLoWins {
        out: dff::DFF<Bits<8>>,
    }

    impl UrgentLoWins {
        #[rule(urgent_before = "prefer_hi")]
        fn prefer_lo(ctx: &mut RuleCtx<Self>, i: ContestInput) {
            guard!(i.want_lo);
            set!(ctx.out, bits::<8>(0x42));
        }

        #[rule]
        fn prefer_hi(ctx: &mut RuleCtx<Self>, i: ContestInput) {
            guard!(i.want_hi);
            set!(ctx.out, bits::<8>(0xff));
        }

        #[output]
        fn output(self_q: &Self, _i: ContestInput) -> Bits<8> {
            *self_q.out
        }
    }
}

#[test]
fn urgent_before_makes_lo_win_when_both_active() {
    let uut: UrgentLoWins = UrgentLoWins::default();
    let stream_in = vec![
        ContestInput {
            want_lo: true,
            want_hi: true,
        },
        ContestInput {
            want_lo: true,
            want_hi: true,
        },
        ContestInput {
            want_lo: true,
            want_hi: true,
        },
    ];
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    assert_eq!(
        last, 0x42,
        "expected prefer_lo (urgent_before prefer_hi) to win; got {last:#x}",
    );
}

#[test]
fn urgent_before_only_lo_writes_when_only_lo_active() {
    let uut: UrgentLoWins = UrgentLoWins::default();
    let stream_in = vec![
        ContestInput {
            want_lo: true,
            want_hi: false,
        },
        ContestInput {
            want_lo: true,
            want_hi: false,
        },
    ];
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    assert_eq!(
        last, 0x42,
        "expected lo-only path to write 0x42; got {last:#x}"
    );
}

#[test]
fn urgent_before_hi_still_fires_when_lo_inactive() {
    let uut: UrgentLoWins = UrgentLoWins::default();
    let stream_in = vec![
        ContestInput {
            want_lo: false,
            want_hi: true,
        },
        ContestInput {
            want_lo: false,
            want_hi: true,
        },
    ];
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    assert_eq!(
        last, 0xff,
        "expected hi-only path to write 0xff; got {last:#x}"
    );
}

#[test]
fn urgent_before_hdl_round_trip() -> Result<(), RHDLError> {
    let uut: UrgentLoWins = UrgentLoWins::default();
    let stream = std::iter::repeat_n(
        ContestInput {
            want_lo: true,
            want_hi: true,
        },
        3,
    )
    .with_reset(2)
    .clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}

// urgent_before *overrides* numeric priority — even when the
// "other" rule is annotated higher-priority numerically, the
// urgent_before-tagged rule still wins.
rule_kernel! {
    pub struct UrgentOverridesPriority {
        out: dff::DFF<Bits<8>>,
    }

    impl UrgentOverridesPriority {
        #[rule(urgent_before = "high_priority_writer")]
        fn urgent_writer(ctx: &mut RuleCtx<Self>, _i: bool) {
            set!(ctx.out, bits::<8>(0x11));
        }

        #[rule(priority = 0)]
        fn high_priority_writer(ctx: &mut RuleCtx<Self>, _i: bool) {
            set!(ctx.out, bits::<8>(0x22));
        }

        #[output]
        fn output(self_q: &Self, _i: bool) -> Bits<8> {
            *self_q.out
        }
    }
}

#[test]
fn urgent_before_beats_explicit_priority() {
    let uut: UrgentOverridesPriority = UrgentOverridesPriority::default();
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
    assert_eq!(
        last, 0x11,
        "urgent_before should beat numeric priority; expected 0x11, got {last:#x}",
    );
}
