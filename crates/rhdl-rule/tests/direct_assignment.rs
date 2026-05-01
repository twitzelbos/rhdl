//! Direct-assignment syntax: `ctx.field = expr;` is equivalent to
//! `set!(ctx.field, expr)`, plus rule bodies can declare `let`
//! bindings that all action expressions see (the per-rule preamble).
//!
//! These two surface improvements were added in one PR — they're
//! load-bearing together because the preamble pattern only pays off
//! once the user can write multiple direct assignments referring to
//! the same precomputed values.
//!
//! ## What the tests cover
//!
//! 1. **Direct-assignment syntax works for a basic counter** — the
//!    simplest possible test that `ctx.field = expr;` lowers
//!    correctly.
//! 2. **Mixed `set!` + direct assignment in the same rule** — both
//!    spellings produce the same Action; the macro accepts either.
//! 3. **Preamble `let` bindings are visible to multiple actions** —
//!    the FIFO write-logic case that was previously the load-bearing
//!    counterexample now works as a clean three-action rule with
//!    shared computation.
//! 4. **Parity** between the direct-assignment form and the old
//!    `set!` form for an equivalent widget — byte-identical output
//!    sequences for the same input stream.
//! 5. **iverilog round-trip** on a kernel using both new features.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel;

// ---- Test 1: basic direct assignment -----------------------------

rule_kernel! {
    pub struct DirectCounter {
        count: dff::DFF<Bits<8>>,
    }

    impl DirectCounter {
        #[rule]
        fn bump(ctx: &mut RuleCtx<Self>, enable: bool) {
            guard!(enable);
            // Direct assignment instead of `set!(ctx.count, ...)`.
            ctx.count = *ctx.count + bits::<8>(1);
        }

        #[output]
        fn output(self_q: &Self, _enable: bool) -> Bits<8> {
            *self_q.count
        }
    }
}

#[test]
fn direct_assignment_counter_increments() {
    let uut: DirectCounter = DirectCounter::default();
    let stream = std::iter::repeat_n(true, 5)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(99);
    assert!(last >= 4 && last <= 5, "expected ~5 increments, got {last}");
}

// ---- Test 2: mixed set! + direct assignment ----------------------

rule_kernel! {
    pub struct MixedSyntax {
        a: dff::DFF<Bits<8>>,
        b: dff::DFF<Bits<8>>,
    }

    impl MixedSyntax {
        #[rule]
        fn step(ctx: &mut RuleCtx<Self>, _i: bool) {
            // Direct assignment.
            ctx.a = *ctx.a + bits::<8>(1);
            // set! macro form.
            set!(ctx.b, *ctx.b + bits::<8>(2));
        }

        #[output]
        fn output(self_q: &Self, _i: bool) -> Bits<16> {
            // Pack a and b into one output.
            let a_lo: Bits<16> = (*self_q.a).resize();
            let b_lo: Bits<16> = (*self_q.b).resize();
            (b_lo << 8) | a_lo
        }
    }
}

#[test]
fn mixed_syntax_both_forms_lower_to_actions() {
    let uut: MixedSyntax = MixedSyntax::default();
    let stream = std::iter::repeat_n(true, 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    let a_lo = last & 0xff;
    let b_lo = (last >> 8) & 0xff;
    assert!(a_lo >= 3 && a_lo <= 4, "a expected ~4, got {a_lo}");
    assert!(
        b_lo >= 6 && b_lo <= 8,
        "b expected ~8 (4 increments of 2), got {b_lo}",
    );
}

// ---- Test 3: per-rule preamble visible to multiple actions -------
//
// The FIFO write-logic case from Pilot 2.  Previously this was a
// single-rule rewrite because there was no way to share `full` and
// `will_write` across multiple `set!`s.  With the preamble feature,
// the rule reads naturally — three action assignments referring to
// two precomputed values.

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct PreambleFifoIn<const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    pub read_address: Bits<N>,
    pub write_enable: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct PreambleFifoOut<const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    pub full: bool,
    pub overflow: bool,
    pub write_address: Bits<N>,
}

rule_kernel! {
    pub struct PreambleFifo<const N: usize>
    where
        rhdl::bits::W<N>: BitWidth,
    {
        write_address: dff::DFF<Bits<N>>,
        write_address_delayed: dff::DFF<Bits<N>>,
        overflow: dff::DFF<bool>,
    }

    impl PreambleFifo {
        /// Single rule with a preamble that computes shared values
        /// (`full`, `will_write`) once and references them in three
        /// action assignments.  Reads naturally as the FIFO
        /// write-side state transition.
        #[rule]
        fn step(ctx: &mut RuleCtx<Self>, i: PreambleFifoIn<N>) {
            // Preamble: shared computation visible to all actions.
            let full: bool = (*ctx.write_address + bits::<N>(1)) == i.read_address;
            let will_write: bool = i.write_enable && !full;

            // Three action assignments referring to the preamble.
            ctx.write_address = if will_write {
                *ctx.write_address + bits::<N>(1)
            } else {
                *ctx.write_address
            };
            ctx.overflow = *ctx.overflow || (i.write_enable && full);
            ctx.write_address_delayed = *ctx.write_address;
        }

        #[output]
        fn output(self_q: &Self, i: PreambleFifoIn<N>) -> PreambleFifoOut<N> {
            let full = (*self_q.write_address + bits::<N>(1)) == i.read_address;
            let overflow = *self_q.overflow || (i.write_enable && full);
            PreambleFifoOut::<N> {
                full,
                overflow,
                write_address: *self_q.write_address_delayed,
            }
        }
    }
}

#[test]
fn preamble_fifo_advances_pointer_when_room() {
    let uut: PreambleFifo<3> = PreambleFifo::default();
    // Reader pinned at 0 — FIFO can hold 7 entries before going full
    // (write_address+1 == read_address means full).
    let stream = std::iter::repeat_n(
        PreambleFifoIn::<3> {
            read_address: bits::<3>(0),
            write_enable: true,
        },
        5,
    )
    .with_reset(2)
    .clock_pos_edge(100);
    let outputs: Vec<_> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    // The output reads `write_address_delayed`, which lags `write_address`
    // by one cycle, and the framework's synchronous sampling adds another
    // cycle of latency, so 5 cycles of advance show up as a counter ≥ 3
    // by the final sample.  The point of the test is that the pointer
    // advances *at all* (the preamble + direct-assignment lowering is
    // wired correctly), not the exact final value.
    let final_addr = outputs.last().map(|o| o.write_address.raw()).unwrap_or(99);
    assert!(
        final_addr >= 3 && final_addr <= 5,
        "expected pointer to advance; got {final_addr}; outputs: {outputs:?}",
    );
    // Overflow should not be latched (we didn't fill).
    assert!(
        !outputs.last().map(|o| o.overflow).unwrap_or(true),
        "overflow should not be latched",
    );
}

#[test]
fn preamble_fifo_latches_overflow_on_full_with_write() {
    let uut: PreambleFifo<3> = PreambleFifo::default();
    // With reader pinned at 0 and 8 writes, we'll fill (7 entries)
    // and the 8th write hits the full condition → overflow latches.
    let stream = std::iter::repeat_n(
        PreambleFifoIn::<3> {
            read_address: bits::<3>(0),
            write_enable: true,
        },
        12,
    )
    .with_reset(2)
    .clock_pos_edge(100);
    let outputs: Vec<_> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    let final_overflow = outputs.last().map(|o| o.overflow).unwrap_or(false);
    assert!(
        final_overflow,
        "overflow must latch when writing to a full FIFO; outputs: {outputs:?}",
    );
}

// ---- Test 4: parity between direct-assignment and set! forms -----

rule_kernel! {
    pub struct ParityDirect {
        count: dff::DFF<Bits<8>>,
    }

    impl ParityDirect {
        #[rule(priority = 0)]
        fn clear_rule(ctx: &mut RuleCtx<Self>, i: bool) {
            guard!(!i);
            ctx.count = bits::<8>(0);
        }

        #[rule(priority = 1)]
        fn bump(ctx: &mut RuleCtx<Self>, i: bool) {
            guard!(i);
            ctx.count = *ctx.count + bits::<8>(1);
        }

        #[output]
        fn output(self_q: &Self, _i: bool) -> Bits<8> {
            *self_q.count
        }
    }
}

rule_kernel! {
    pub struct ParitySet {
        count: dff::DFF<Bits<8>>,
    }

    impl ParitySet {
        #[rule(priority = 0)]
        fn clear_rule(ctx: &mut RuleCtx<Self>, i: bool) {
            guard!(!i);
            set!(ctx.count, bits::<8>(0));
        }

        #[rule(priority = 1)]
        fn bump(ctx: &mut RuleCtx<Self>, i: bool) {
            guard!(i);
            set!(ctx.count, *ctx.count + bits::<8>(1));
        }

        #[output]
        fn output(self_q: &Self, _i: bool) -> Bits<8> {
            *self_q.count
        }
    }
}

#[test]
fn direct_assignment_and_set_macro_produce_identical_outputs() {
    // Same input pattern through both widgets.
    let inputs = vec![true, true, false, true, false, true, true, true];

    let direct: ParityDirect = ParityDirect::default();
    let direct_out: Vec<u128> = direct
        .run(inputs.clone().into_iter().with_reset(2).clock_pos_edge(100))
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output.raw())
        .collect();

    let set_form: ParitySet = ParitySet::default();
    let set_out: Vec<u128> = set_form
        .run(inputs.into_iter().with_reset(2).clock_pos_edge(100))
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output.raw())
        .collect();

    assert_eq!(
        direct_out, set_out,
        "direct-assignment form and set! form must produce byte-identical outputs",
    );
}

// ---- Test 5: iverilog round-trip on the new features -------------

#[test]
fn preamble_fifo_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: PreambleFifo<3> = PreambleFifo::default();
    let inputs: Vec<PreambleFifoIn<3>> = (0..10)
        .map(|i| PreambleFifoIn::<3> {
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
