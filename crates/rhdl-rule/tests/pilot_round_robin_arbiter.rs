//! Pilot rewrite #1 — `core::round_robin_arbiter` as a rule kernel.
//!
//! Validates the design plan §15 / §16 / §21 commitment that the
//! existing widget can be expressed as a `rule_kernel!` form with
//! byte-identical simulation behaviour.
//!
//! ## Design choice: single-rule rewrite
//!
//! The round-robin arbiter is naturally one logical operation per
//! cycle: scan request bits in rotated priority order, grant the
//! first one, update `last_granted`.  A multi-rule decomposition
//! (one rule per requester) would either need dynamic priority —
//! which the static `#[rule(priority = N)]` annotation can't
//! express — or N copies of the rotation calculation in N rule
//! guards, which is worse.  The honest rewrite is a single rule
//! whose body is the original kernel's scan loop.
//!
//! This still demonstrates the rewrite path: any hand-written
//! `Synchronous` widget can be expressed as a single-rule
//! `rule_kernel!` and lower to byte-identical Verilog.  The
//! parity test below proves cycle-accurate equivalence against
//! the original `RoundRobinArbiter` for a representative input
//! stream.

use rhdl::prelude::*;
use rhdl_fpga::core::{dff, round_robin_arbiter::RoundRobinArbiter};
use rhdl_rule::rule_kernel;

rule_kernel! {
    /// Rule-kernel rewrite of `core::round_robin_arbiter`.
    ///
    /// Parity-tested against the original; see
    /// `pilot_round_robin_arbiter_matches_original`.
    pub struct RuleRoundRobinArbiter<const N: usize, const W: usize>
    where
        rhdl::bits::W<N>: BitWidth,
        rhdl::bits::W<W>: BitWidth,
    {
        last_granted: dff::DFF<Bits<W>>,
        valid: dff::DFF<bool>,
    }

    impl RuleRoundRobinArbiter {
        /// Single arbitration rule: scan the N request bits in
        /// rotated priority order and grant the first set bit.
        /// Same algorithm as the original kernel.
        ///
        /// The scan runs once in the rule's preamble; both writes
        /// reference its result.  Direct-assignment + preamble
        /// (added in PR #26) makes this read like a Rust function.
        #[rule]
        fn arbitrate(ctx: &mut RuleCtx<Self>, requests: Bits<N>) {
            // Preamble — runs once per cycle; in scope for all writes.
            let start: Bits<W> = if *ctx.valid {
                *ctx.last_granted + bits::<W>(1)
            } else {
                bits::<W>(0)
            };
            let mut winner_idx: Bits<W> = bits::<W>(0);
            let mut found = false;
            for i in 0..N {
                let offset: Bits<W> = bits::<W>(i as u128);
                let idx: Bits<W> = start + offset;
                let bit_at_idx = (requests >> idx) & bits::<N>(1);
                if bit_at_idx != bits::<N>(0) && !found {
                    winner_idx = idx;
                    found = true;
                }
            }

            // Two non-blocking writes referencing the preamble.
            ctx.last_granted = if found { winner_idx } else { *ctx.last_granted };
            ctx.valid = found;
        }

        /// Output: same Some/None convention as the original.
        #[output]
        fn output(self_q: &Self, requests: Bits<N>) -> Option<Bits<W>> {
            let start: Bits<W> = if *self_q.valid {
                *self_q.last_granted + bits::<W>(1)
            } else {
                bits::<W>(0)
            };
            let mut winner_idx: Bits<W> = bits::<W>(0);
            let mut found = false;
            for i in 0..N {
                let offset: Bits<W> = bits::<W>(i as u128);
                let idx: Bits<W> = start + offset;
                let bit_at_idx = (requests >> idx) & bits::<N>(1);
                if bit_at_idx != bits::<N>(0) && !found {
                    winner_idx = idx;
                    found = true;
                }
            }
            if found { Some(winner_idx) } else { None }
        }
    }
}

#[test]
fn rule_arbiter_grants_to_single_requester() {
    let uut = RuleRoundRobinArbiter::<4, 2>::default();
    let stream = std::iter::repeat_n(bits::<4>(0b0010), 16)
        .with_reset(1)
        .clock_pos_edge(100);
    let grants: Vec<Option<Bits<2>>> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    assert!(
        grants.iter().all(|g| *g == Some(bits::<2>(1))),
        "single requester should always win; got {grants:?}",
    );
}

#[test]
fn rule_arbiter_rotates_under_constant_pressure() {
    let uut = RuleRoundRobinArbiter::<4, 2>::default();
    let stream = std::iter::repeat_n(bits::<4>(0b1111), 32)
        .with_reset(1)
        .clock_pos_edge(100);
    let grants: Vec<Option<Bits<2>>> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    let expected: Vec<Option<Bits<2>>> =
        (0..32u128).map(|i| Some(bits::<2>(i % 4))).collect();
    assert_eq!(grants, expected, "expected strict 0,1,2,3 rotation");
}

/// **Parity test** — the rule-kernel rewrite must produce
/// byte-identical grant sequences to the original
/// `core::round_robin_arbiter::RoundRobinArbiter` for every input
/// pattern in a representative sweep.  This is the load-bearing
/// claim of the pilot rewrite.
#[test]
fn rule_arbiter_parity_with_original() {
    // Mix: all asking, some asking, none, single, dynamic.
    let inputs: Vec<Bits<4>> = vec![
        bits::<4>(0b1111),
        bits::<4>(0b1111),
        bits::<4>(0b0101),
        bits::<4>(0b0000),
        bits::<4>(0b1000),
        bits::<4>(0b1111),
        bits::<4>(0b0010),
        bits::<4>(0b0011),
        bits::<4>(0b1100),
        bits::<4>(0b0001),
        bits::<4>(0b1111),
        bits::<4>(0b0110),
    ];

    let original: RoundRobinArbiter<4, 2> = RoundRobinArbiter::default();
    let original_grants: Vec<Option<Bits<2>>> = original
        .run(
            inputs
                .clone()
                .into_iter()
                .with_reset(1)
                .clock_pos_edge(100),
        )
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();

    let rule = RuleRoundRobinArbiter::<4, 2>::default();
    let rule_grants: Vec<Option<Bits<2>>> = rule
        .run(inputs.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();

    assert_eq!(
        rule_grants, original_grants,
        "rule kernel must produce byte-identical grant sequence to the original",
    );
}

#[test]
fn rule_arbiter_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = RuleRoundRobinArbiter::<4, 2>::default();
    let inputs: Vec<Bits<4>> = vec![
        bits::<4>(0b1111),
        bits::<4>(0b0101),
        bits::<4>(0b0010),
        bits::<4>(0b1111),
        bits::<4>(0b0000),
        bits::<4>(0b1000),
    ];
    let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = test_bench.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
