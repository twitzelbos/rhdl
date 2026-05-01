//! Phase-1 acceptance test: two rules both write the same register
//! with always-true guards.  The priority scheduler should let the
//! first rule (source order = priority 0) win.
//!
//! Without conflict detection, Phase 0's last-write-wins would
//! produce the second rule's value (99).  With Phase 1's conflict
//! matrix + priority chain, the first rule wins (7).

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel;

rule_kernel! {
    pub struct PriorityDemo {
        val: dff::DFF<Bits<8>>,
    }

    impl PriorityDemo {
        // Priority 0 — always fires.  Writes `val = 7`.
        #[rule]
        fn set_low(ctx: &mut RuleCtx<Self>, _flag: bool) {
            set!(ctx.val, bits::<8>(7));
        }

        // Priority 1 — also always fires.  Writes `val = 99`.
        // Conflicts with `set_low` (write-write on `val`); the
        // scheduler should suppress this rule whenever `set_low`
        // fires.
        #[rule]
        fn set_high(ctx: &mut RuleCtx<Self>, _flag: bool) {
            set!(ctx.val, bits::<8>(99));
        }

        #[output]
        fn output(self_q: &Self, _flag: bool) -> Bits<8> {
            *self_q.val
        }
    }
}

#[test]
fn priority_chain_picks_the_first_writer() {
    let uut: PriorityDemo = PriorityDemo::default();
    let stream = std::iter::repeat_n(true, 5)
        .with_reset(2)
        .clock_pos_edge(100);
    let final_value = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    assert_eq!(
        final_value, 7,
        "expected priority-0 rule (set_low) to win the write-write conflict; \
         got {final_value} (Phase-0 last-write-wins would give 99)",
    );
}
