//! Phase-1 acceptance test: a 2-rule, 2-register kernel using a
//! compound `In` input type (mirrors the design plan §4.1 sketch
//! minus the `reset_on_max` rule, which the plan example
//! deliberately mis-prioritises for illustration).
//!
//! The kernel is a counter that only counts when a flag is raised.
//! A separate rule raises the flag on a `start` pulse if the flag
//! is currently low.
//!
//! Conflict matrix:
//!
//! - `count_up` reads `flag`, writes `counter`.
//! - `raise_flag` reads `flag`, writes `flag`.
//!
//! The two rules conflict (read-write on `flag`).  But their
//! guards are mutually exclusive (`count_up` requires
//! `flag == true`; `raise_flag` requires `flag == false`), so in
//! practice at most one is ever ready in a given cycle — the
//! priority chain doesn't suppress anything dynamically, but the
//! conflict matrix still flags the pair.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel;

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct CnfIn {
    pub start: bool,
    pub enable: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct CnfOut {
    pub counter: Bits<8>,
    pub flag: bool,
}

rule_kernel! {
    pub struct CounterAndFlag {
        counter: dff::DFF<Bits<8>>,
        flag: dff::DFF<bool>,
    }

    impl CounterAndFlag {
        // Priority 0: count up when the flag is raised and enable is high.
        #[rule]
        fn count_up(ctx: &mut RuleCtx<Self>, i: CnfIn) {
            guard!(*ctx.flag);
            guard!(i.enable);
            set!(ctx.counter, *ctx.counter + bits::<8>(1));
        }

        // Priority 1: raise the flag on `start` if it is currently low.
        #[rule]
        fn raise_flag(ctx: &mut RuleCtx<Self>, i: CnfIn) {
            guard!(i.start);
            guard!(!*ctx.flag);
            set!(ctx.flag, true);
        }

        #[output]
        fn output(self_q: &Self, _i: CnfIn) -> CnfOut {
            CnfOut {
                counter: *self_q.counter,
                flag: *self_q.flag,
            }
        }
    }
}

#[test]
fn counter_only_counts_after_flag_is_raised() {
    let uut: CounterAndFlag = CounterAndFlag::default();
    // Stream:
    //  - 1 cycle: start=true,  enable=false  → raise_flag fires; counter stays 0
    //  - 5 cycles: start=false, enable=true   → count_up fires every cycle
    let mut stream_in: Vec<CnfIn> = vec![CnfIn {
        start: true,
        enable: false,
    }];
    for _ in 0..5 {
        stream_in.push(CnfIn {
            start: false,
            enable: true,
        });
    }
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let outputs: Vec<_> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    // After the start pulse, flag should be raised and counter should
    // have started counting.  By the end of the 5 enabled cycles we
    // expect counter ≥ 4 (off-by-one tolerance for sample timing).
    let final_state = outputs.last().expect("no outputs");
    assert!(
        final_state.flag,
        "expected flag=true after the start pulse; got flag={}",
        final_state.flag,
    );
    let final_count = final_state.counter.raw();
    assert!(
        final_count >= 4 && final_count <= 5,
        "expected counter near 5 after 5 enabled cycles; got {final_count}",
    );
}

#[test]
fn counter_holds_when_flag_is_low() {
    let uut: CounterAndFlag = CounterAndFlag::default();
    // Stream: enable=true but never start → flag never raised → counter never increments.
    let stream_in: Vec<CnfIn> = std::iter::repeat_n(
        CnfIn {
            start: false,
            enable: true,
        },
        10,
    )
    .collect();
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let final_count = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.counter.raw())
        .unwrap_or(0xff);
    assert_eq!(
        final_count, 0,
        "counter should stay at 0 when flag never gets raised; got {final_count}",
    );
}
