//! A zero-width value may flow through control flow.
//!
//! # What was wrong
//!
//! `let mut f = seed; if flag { f = seed; }` compiles for every framing
//! type with bits and was rejected for a zero-width one:
//!
//! ```text
//! Slot sr2 is read before being written
//! ```
//!
//! The cause is an interaction, not a single mistake. A zero-width copy
//! or select moves no bits, so an RHIF pass correctly removes it as a
//! no-op — leaving the destination slot read but never written. The RHIF
//! for the failing kernel is a single instruction:
//!
//! ```text
//! Reg r2 : ()   // f
//! r3 <- (sl0, sr2)
//! ```
//!
//! `check_rhif_flow` then flagged `r2`. Its sibling pass
//! `partial_initialization_check` does not, because `ensure_covered`
//! opens with a zero-width guard. **One of the two RHIF checks had been
//! taught about zero width and the other had not.**
//!
//! # Why the guard is sound
//!
//! A slot with no bits cannot be uninitialised. Its type has exactly one
//! inhabitant, so there is no bit whose value could be unknown and no
//! wrong value it could hold. Reading one before it is written yields
//! the only value it could ever have.
//!
//! Relaxing a safety check needs the downstream guards to be in place,
//! and they now are: `check_no_zero_width_registers` stops a zero-width
//! value becoming an RTL register, and the `LitVerilog` conversion
//! rejects a zero-width literal outright. Anything that escapes this
//! relaxation is caught later, loudly.
//!
//! # Which constructs were affected
//!
//! Measured, not assumed. At a zero-width `F`, before the fix:
//!
//! | construct | before |
//! |---|---|
//! | `let f = seed` | ok |
//! | `let mut f = seed` (no reassign) | ok |
//! | `let mut f = seed; if flag { f = seed; }` | **rejected** |
//! | using `seed` directly | ok |
//! | `match i { Some(x) => x, None => seed }` | **rejected** |
//!
//! So the trigger is a zero-width value crossing a **control-flow
//! merge**, where SSA needs a select. All of them compile now.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;

/// A widget whose kernel puts a zero-width value through both an
/// `if`-reassignment and a `match`, the two shapes that were rejected.
mod thru {
    use rhdl::prelude::*;
    use rhdl_fpga::core::dff;

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    pub struct ThroughControlFlow<F: Digital> {
        /// Something with bits, so the widget is not degenerate.
        count: dff::DFF<b8>,
        /// Carries `F` through a register.
        hold: dff::DFF<(b8, F)>,
    }

    #[derive(PartialEq, Clone, Copy, Debug, Digital)]
    pub struct In<F: Digital> {
        pub seed: F,
        pub alt: Option<F>,
        pub flag: bool,
    }

    impl<F: Digital> SynchronousIO for ThroughControlFlow<F> {
        type I = In<F>;
        type O = (b8, F);
        type Kernel = through_kernel<F>;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn through_kernel<F: Digital>(_cr: ClockReset, i: In<F>, q: Q<F>) -> ((b8, F), D<F>) {
        let mut d = D::<F>::dont_care();

        // Shape 1: mutable local across an `if`.
        let mut f = i.seed;
        if i.flag {
            f = i.seed;
        }

        // Shape 2: value produced by a `match`.
        let g = match i.alt {
            Some(x) => x,
            None => f,
        };

        d.count = q.count + 1;
        d.hold = (q.count, g);
        (q.hold, d)
    }
}

fn stimulus<F: Digital>(n: usize, seed: F) -> Vec<thru::In<F>> {
    (0..n)
        .map(|k| thru::In::<F> {
            seed,
            alt: if k % 3 == 0 { Some(seed) } else { None },
            flag: k % 2 == 0,
        })
        .collect()
}

/// **The zero-width instantiation compiles at all.**
///
/// This is the regression: before the fix, building the descriptor
/// failed with `Slot sr2 is read before being written`.
#[test]
fn a_zero_width_value_may_cross_a_control_flow_merge() -> miette::Result<()> {
    let uut = thru::ThroughControlFlow::<()>::default();
    let _ = uut.descriptor("top".into())?.hdl()?;
    Ok(())
}

/// And it behaves, agreeing with `iverilog` cycle by cycle.
///
/// Compiling is not enough — the previous two zero-width defects both
/// produced code that compiled and then disagreed between the
/// simulators. The round-trip is what rules that out.
#[test]
fn the_zero_width_chain_round_trips() -> miette::Result<()> {
    let uut = thru::ThroughControlFlow::<()>::default();
    let tb = uut
        .run(
            stimulus(12, ())
                .into_iter()
                .with_reset(1)
                .clock_pos_edge(100),
        )
        .collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&uut, &Default::default())?.run_iverilog()?;
    Ok(())
}

/// The counter still counts, so the zero-width plumbing did not
/// disturb the part of the design that has bits.
///
/// **Not a catching test.** `run()` interprets the circuit directly and
/// never lowers it, so this passes with the fix reverted too. It guards
/// behaviour, not the compile.
#[test]
fn the_bits_beside_it_still_work() {
    let uut = thru::ThroughControlFlow::<()>::default();
    let out: Vec<u128> = uut
        .run(
            stimulus(8, ())
                .into_iter()
                .with_reset(1)
                .clock_pos_edge(100),
        )
        .synchronous_sample()
        .map(|s| s.output.0.raw())
        .collect();
    // Registered twice (`count` then `hold`), so the sequence lags, but
    // it must still be strictly increasing once it starts moving.
    let tail = &out[3..];
    assert!(
        tail.windows(2).all(|w| w[1] == w[0] + 1),
        "the counter beside the zero-width value should still count: {out:?}"
    );
}

/// The same widget at a framing type with bits is unaffected.
#[test]
fn a_framing_type_with_bits_still_works() -> miette::Result<()> {
    let uut = thru::ThroughControlFlow::<bool>::default();
    let tb = uut
        .run(
            stimulus(12, true)
                .into_iter()
                .with_reset(1)
                .clock_pos_edge(100),
        )
        .collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&uut, &Default::default())?.run_iverilog()?;
    Ok(())
}

/// A genuinely uninitialised slot *with bits* is still rejected.
///
/// The guard is scoped to zero width, and this is what says so. Without
/// this test, widening the guard by accident would go unnoticed.
#[test]
fn a_real_uninitialised_read_is_still_an_error() {
    // `dont_care()` leaves the aggregate uninitialised; reading a field
    // of it must still be refused.
    let uut = bad::ReadsUninitialised::default();
    let err = match uut.descriptor("top".into()) {
        Ok(_) => panic!("reading an uninitialised value with bits must still fail"),
        Err(e) => format!("{e}"),
    };
    assert!(
        err.contains("Partial Initialization") || err.contains("read before"),
        "expected an initialisation diagnostic, got: {err}"
    );
}

mod bad {
    use rhdl::prelude::*;
    use rhdl_fpga::core::dff;

    #[derive(PartialEq, Clone, Copy, Debug, Digital, Default)]
    pub struct Pair {
        pub a: b8,
        pub b: b8,
    }

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    pub struct ReadsUninitialised {
        keep: dff::DFF<b8>,
    }

    impl SynchronousIO for ReadsUninitialised {
        type I = bool;
        type O = b8;
        type Kernel = bad_kernel;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn bad_kernel(_cr: ClockReset, i: bool, q: Q) -> (b8, D) {
        let mut d = D::dont_care();
        // `p` is never initialised, and `p.a` has bits, so reading it
        // must remain an error.
        let p = Pair::dont_care();
        d.keep = if i { p.a } else { q.keep };
        (q.keep, d)
    }
}

/// Sanity: the unused import guard.  `dff` is referenced by the widgets
/// above; this silences the unused-import lint without an attribute.
#[allow(dead_code)]
fn _uses_dff(_: dff::DFF<b8>) {}
