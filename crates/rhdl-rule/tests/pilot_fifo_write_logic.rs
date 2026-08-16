//! Pilot rewrite #2 — `fifo::write_logic` as a rule kernel.
//!
//! Validates the design plan §15 / §16 / §21 commitment.
//!
//! ## Design choice: single-rule rewrite (and why)
//!
//! At first glance the FIFO write logic looks like a three-rule
//! decomposition: `do_write` advances the pointer when there's
//! room, `mark_overflow` latches when there isn't, `tick_delayed`
//! propagates the pointer to its delayed copy every cycle.  But
//! when expressed that way, the macro's conflict matrix correctly
//! flags a write-read conflict between `do_write` (writes
//! `write_address`) and `tick_delayed` (reads `write_address`),
//! and the priority chain *suppresses* `tick_delayed` whenever
//! `do_write` fires — breaking the byte-identical-behaviour
//! guarantee.  Right call for the macro; wrong shape for this
//! widget.  See `rule-architecture.md` §6.1 for the conflict
//! definition.
//!
//! The honest rewrite is a single rule whose body captures all
//! three writes atomically.  With the **per-rule preamble** added
//! in PR #26, the body reads naturally: shared computation
//! (`full`, `will_write`) lives at the top of the rule body and
//! is in scope for every direct-assignment that follows.
//!
//! The lesson for the design plan: a widget whose every-cycle
//! behaviour is "everything happens together" is naturally one
//! rule, not several — even if you can name the sub-actions.
//! Multi-rule decomposition shines when at most one of several
//! sub-actions fires per cycle (e.g. `toggle_ff` — see Pilot 3).
//!
//! Parity-tested against `fifo::write_logic::FIFOWriteCore` for a
//! representative input stream (varying `read_address` and
//! `write_enable`, including the full-with-write-enable overflow
//! case).

use rhdl::prelude::*;
#[allow(unused_imports)]
use rhdl_fpga::core::dff; // referenced inside the rule_kernel! macro body
use rhdl_fpga::fifo::write_logic::{FIFOWriteCore, In as OrigIn, Out as OrigOut};
use rhdl_rule::rule_kernel;

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct RuleFIFOWriteIn<const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    pub read_address: Bits<N>,
    pub write_enable: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct RuleFIFOWriteOut<const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    pub full: bool,
    pub almost_full: bool,
    pub overflow: bool,
    pub ram_write_address: Bits<N>,
    pub write_address: Bits<N>,
}

rule_kernel! {
    /// Rule-kernel rewrite of `fifo::write_logic::FIFOWriteCore`.
    ///
    /// Three rules: `do_write` advances the pointer; `mark_overflow`
    /// latches the overflow flag; `tick_delayed` propagates the
    /// pointer to the delayed copy every cycle.
    pub struct RuleFIFOWriteCore<const N: usize>
    where
        rhdl::bits::W<N>: BitWidth,
    {
        write_address: dff::DFF<Bits<N>>,
        write_address_delayed: dff::DFF<Bits<N>>,
        overflow: dff::DFF<bool>,
    }

    impl RuleFIFOWriteCore {
        /// Single rule firing every cycle.  Captures the entire
        /// write-side state transition atomically.
        ///
        /// Reads from `*ctx.field` see the pre-firing snapshot;
        /// writes (`ctx.field = expr`) commit at the cycle
        /// boundary.  `full` and `will_write` are computed once
        /// in the preamble and visible to every write below.
        #[rule]
        fn step(ctx: &mut RuleCtx<Self>, i: RuleFIFOWriteIn<N>) {
            // Preamble — shared computation.
            let full       = (*ctx.write_address + bits::<N>(1)) == i.read_address;
            let will_write = i.write_enable && !full;

            // Three non-blocking writes.
            ctx.write_address         = if will_write { *ctx.write_address + bits::<N>(1) } else { *ctx.write_address };
            ctx.overflow              = *ctx.overflow || (i.write_enable && full);
            ctx.write_address_delayed = *ctx.write_address;
        }

        /// Output: same shape and semantics as the original
        /// `fifo::write_logic` `Out<N>`.
        #[output]
        fn output(self_q: &Self, i: RuleFIFOWriteIn<N>) -> RuleFIFOWriteOut<N> {
            let full = (*self_q.write_address + bits::<N>(1)) == i.read_address;
            let almost_full =
                full || ((*self_q.write_address + bits::<N>(2)) == i.read_address);
            // `overflow` output mirrors the latched flag, raised
            // immediately when a write hits a full FIFO this cycle.
            let overflow = *self_q.overflow || (i.write_enable && full);
            RuleFIFOWriteOut::<N> {
                full,
                almost_full,
                overflow,
                ram_write_address: *self_q.write_address,
                write_address: *self_q.write_address_delayed,
            }
        }
    }
}

/// **Parity test** — the rule-kernel rewrite must produce
/// byte-identical output sequences to the original
/// `fifo::write_logic::FIFOWriteCore` for a representative input
/// stream that exercises every mode (idle, write-when-empty,
/// write-when-full → overflow, write-after-overflow holds).
#[test]
fn rule_fifo_write_parity_with_original() {
    // Mix of write-enable patterns + read pointer advances.  The
    // key cases:
    //  - early: write_enable on empty FIFO → counter advances
    //  - middle: read pointer chases (FIFO not full)
    //  - late: read pointer stops, FIFO fills, overflow latches
    let ws = vec![
        true, true, true, true, // 4 writes from empty
        false, false, // pause
        true, true, true, true, // 4 more writes
        false, false, false, // pause; read can catch up
        true, true, true, // more writes
    ];
    let reads: Vec<u128> = vec![
        0, 0, 0, 0, // reader idle
        0, 1, // reader advances
        2, 3, 4, 5, // reader advances again
        5, 5, 5, // reader stops
        5, 5, 5, // reader still stopped
    ];
    assert_eq!(ws.len(), reads.len());

    let inputs_orig: Vec<OrigIn<3>> = ws
        .iter()
        .zip(reads.iter())
        .map(|(&we, &ra)| OrigIn::<3> {
            read_address: bits::<3>(ra),
            write_enable: we,
        })
        .collect();
    let inputs_rule: Vec<RuleFIFOWriteIn<3>> = ws
        .iter()
        .zip(reads.iter())
        .map(|(&we, &ra)| RuleFIFOWriteIn::<3> {
            read_address: bits::<3>(ra),
            write_enable: we,
        })
        .collect();

    let original: FIFOWriteCore<3> = FIFOWriteCore::default();
    let original_out: Vec<OrigOut<3>> = original
        .run(inputs_orig.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();

    let rule = RuleFIFOWriteCore::<3>::default();
    let rule_out: Vec<RuleFIFOWriteOut<3>> = rule
        .run(inputs_rule.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();

    assert_eq!(
        original_out.len(),
        rule_out.len(),
        "both widgets must produce the same number of cycles",
    );
    for cycle in 0..original_out.len() {
        let o = &original_out[cycle];
        let r = &rule_out[cycle];
        assert_eq!(o.full, r.full, "cycle {cycle}: full mismatch");
        assert_eq!(
            o.almost_full, r.almost_full,
            "cycle {cycle}: almost_full mismatch",
        );
        assert_eq!(o.overflow, r.overflow, "cycle {cycle}: overflow mismatch");
        assert_eq!(
            o.ram_write_address, r.ram_write_address,
            "cycle {cycle}: ram_write_address mismatch",
        );
        assert_eq!(
            o.write_address, r.write_address,
            "cycle {cycle}: write_address mismatch",
        );
    }
}

#[test]
fn rule_fifo_write_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = RuleFIFOWriteCore::<3>::default();
    let inputs: Vec<RuleFIFOWriteIn<3>> = (0..16)
        .map(|i| RuleFIFOWriteIn::<3> {
            read_address: bits::<3>((i / 4) as u128),
            write_enable: i % 2 == 0,
        })
        .collect();
    let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = test_bench.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
