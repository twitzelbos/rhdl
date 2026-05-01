//! Phase-1 acceptance test: read-write conflict between two rules.
//!
//! Rule A reads `q.a` and writes `q.b`.
//! Rule B writes `q.a`.
//!
//! Per `rule-architecture.md` §6.1, this is a read-write conflict
//! (A reads what B writes).  The priority chain should suppress
//! rule B when rule A fires.
//!
//! Without conflict detection (Phase 0 last-write-wins), both
//! rules would fire and `a` would be reset to 0 every cycle.
//! With Phase 1's conflict matrix, rule A fires (priority 0); B is
//! suppressed; `a` remains untouched and grows monotonically (it's
//! incremented by an external mechanism — but in this test, since
//! nothing else writes `a`, it stays at its reset value of 0
//! and `b` is consistently set to `a + 1 = 1`).

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel;

rule_kernel! {
    pub struct Coupled {
        a: dff::DFF<Bits<8>>,
        b: dff::DFF<Bits<8>>,
    }

    impl Coupled {
        // Priority 0: read a, write b = a + 7.
        #[rule]
        fn read_a_write_b(ctx: &mut RuleCtx<Self>, _enable: bool) {
            set!(ctx.b, *ctx.a + bits::<8>(7));
        }

        // Priority 1: zero out a.  Conflicts with the rule above
        // (read-write on `a`).
        #[rule]
        fn zero_a(ctx: &mut RuleCtx<Self>, _enable: bool) {
            set!(ctx.a, bits::<8>(99));
        }

        #[output]
        fn output(self_q: &Self, _enable: bool) -> (Bits<8>, Bits<8>) {
            (*self_q.a, *self_q.b)
        }
    }
}

#[test]
fn read_write_conflict_suppresses_lower_priority_writer() {
    let uut: Coupled = Coupled::default();
    let stream = std::iter::repeat_n(true, 5)
        .with_reset(2)
        .clock_pos_edge(100);
    let final_state = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output)
        .unwrap_or((bits(0xff), bits(0xff)));
    let (a, b) = (final_state.0.raw(), final_state.1.raw());
    // Rule A always fires (no guards); the priority chain suppresses
    // rule B (which would set a = 99).  `a` therefore stays at its
    // post-reset value of 0.  `b` is set to `a + 7 = 7` every cycle.
    assert_eq!(
        a, 0,
        "expected `a` to stay at its reset value 0 (rule B suppressed by priority); \
         got {a}.  Phase-0 last-write-wins would give 99.",
    );
    assert_eq!(b, 7, "expected `b = a + 7 = 7`; got {b}");
}
