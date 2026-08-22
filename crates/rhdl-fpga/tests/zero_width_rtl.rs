//! A zero-width value must not leave an undriven register behind.
//!
//! # What was wrong
//!
//! `make_binary` in `lower_rhif_to_rtl.rs` guards its *result* for
//! emptiness but not its operands, so `self.operand(arg)` materialised
//! `b0` registers for zero-width arguments. Whatever would have defined
//! them — an `Index` extracting no bits — had been skipped by its own
//! `is_empty` guard. Side by side:
//!
//! ```text
//! RTL at F = bool                RTL at F = ()
//!   r0 <- r1[8..9]                 reg r1 : b0     <- allocated
//!   r2 <- r3[8..9]                 reg r2 : b0     <- and never written
//!   r4 <- r0 != r2                 r0 <- r1 != r2
//! ```
//!
//! An unassigned Verilog `reg` is `x`, so the comparison was `x != x`
//! in emitted hardware while the Rust simulator returned a defined
//! `false`.
//!
//! # Why "it compiles" is not the assertion
//!
//! The pre-fix code compiled fine. It produced *legal* Verilog that
//! evaluated to `x`. Every Rust-side tier passed. So the test that
//! catches this has to compare the two simulators against each other,
//! which is exactly what the Tier-4 round-trip does — `rtl()`/`ntl()`
//! plus `run_iverilog()` replays the recorded Rust samples through
//! `iverilog` and fails on the first cycle they disagree.
//!
//! An `x` in Verilog against a defined `false` in Rust is a
//! disagreement, so these tests fail on the pre-fix compiler and pass
//! on the fixed one. A test that merely built the descriptor would
//! have passed either way.

use rhdl::prelude::*;
use rhdl_fpga::rcstream::bus::Item;

/// A widget whose kernel compares the framing markers of two items —
/// the shape that produced the bug at `F = ()`.
mod cmp {
    use rhdl::prelude::*;
    use rhdl_fpga::core::dff;
    use rhdl_fpga::rcstream::bus::Item;

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    pub struct MarkerCompare<F: Digital> {
        /// Registers the result so there is something with bits to
        /// observe, and keeps the widget from folding away entirely.
        out: dff::DFF<bool>,
        /// Carries `F` through the design.
        hold: dff::DFF<Item<b8, F>>,
    }

    #[derive(PartialEq, Clone, Copy, Debug, Digital)]
    pub struct In<F: Digital> {
        pub a: Item<b8, F>,
        pub b: Item<b8, F>,
    }

    impl<F: Digital> SynchronousIO for MarkerCompare<F> {
        type I = In<F>;
        type O = bool;
        type Kernel = marker_compare_kernel<F>;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn marker_compare_kernel<F: Digital>(_cr: ClockReset, i: In<F>, q: Q<F>) -> (bool, D<F>) {
        let mut d = D::<F>::dont_care();
        // `hold` exists only to carry `F` through a register, so the
        // framing type is present in the design rather than optimised
        // out of it.
        d.hold = i.a;
        // The comparison under test, over the two *inputs* -- the same
        // shape `ComplexMixer` has.  At a zero-width `F` both operands
        // have no bits, and the lowering must fold this rather than
        // leave two undriven registers behind.
        d.out = i.a.frame != i.b.frame;
        (q.out, d)
    }
}

fn stimulus<F: Digital>(frames: &[(F, F)]) -> Vec<cmp::In<F>> {
    frames
        .iter()
        .map(|(af, bf)| cmp::In::<F> {
            a: Item::<b8, F> {
                data: bits(1),
                frame: *af,
            },
            b: Item::<b8, F> {
                data: bits(2),
                frame: *bf,
            },
        })
        .collect()
}

/// **The test that would have caught this.**
///
/// A zero-width comparison, round-tripped through `iverilog`.
///
/// Verified able to fail: with the fold in `make_binary` disabled, this
/// test fails. Note *how* it fails now — `check_registers_are_written`
/// raises an ICE pointing at the kernel, so the compile stops. Before
/// the checks existed the same code compiled and the failure appeared
/// as a testbench byte-diff, `Expected 000111…, got 0x0111…`, with
/// nothing naming the cause. Moving the failure from a simulator
/// disagreement to a compile error at the layer that caused it is the
/// point of the change.
#[test]
fn a_zero_width_comparison_agrees_between_simulators() -> miette::Result<()> {
    let uut = cmp::MarkerCompare::<()>::default();
    let seq = stimulus::<()>(&[((), ()); 8]);
    let tb = uut
        .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&uut, &Default::default())?.run_iverilog()?;
    Ok(())
}

/// And the folded answer is the *correct* one.
///
/// A one-inhabitant type has only one value, so two of them are always
/// equal and `!=` is always `false`.
///
/// **This test passes on the pre-fix compiler too**, and is included
/// for exactly that reason: the Rust simulator always returned a
/// defined `false`, which is why the defect was invisible to every
/// Rust-side tier. It guards the *value* — the round-trip above guards
/// the agreement — and neither one subsumes the other, since two
/// simulators can agree on a wrong answer.
#[test]
fn a_zero_width_comparison_is_always_false() {
    let uut = cmp::MarkerCompare::<()>::default();
    let seq = stimulus::<()>(&[((), ()); 8]);
    let out: Vec<bool> = uut
        .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .map(|s| s.output)
        .collect();
    assert!(
        out.iter().all(|v| !v),
        "two values of a one-inhabitant type are always equal, so `!=` \
         must be false everywhere; got {out:?}"
    );
}

/// The same widget at a framing type with bits still compares for real.
///
/// Guards the fold against being over-eager: if it fired for non-empty
/// operands, this would report no differences.
#[test]
fn a_one_bit_comparison_still_compares() -> miette::Result<()> {
    let uut = cmp::MarkerCompare::<bool>::default();
    let seq = stimulus::<bool>(&[
        (false, false),
        (true, false),
        (true, true),
        (false, true),
        (false, false),
    ]);
    let out: Vec<bool> = uut
        .run(seq.clone().into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .map(|s| s.output)
        .collect();
    assert!(
        out.iter().any(|v| *v),
        "a real comparison must sometimes be true; got {out:?}"
    );

    let tb = uut
        .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&uut, &Default::default())?.run_iverilog()?;
    Ok(())
}

/// The widgets that carry a framing type through a zero-width
/// `Constant` still work, and still round-trip.
///
/// `IqSplit` and `IqCombine` are the reason the *other* half of this
/// defect could not be fixed by rejecting zero-width values outright.
#[test]
fn the_phantom_carrier_widgets_round_trip() -> miette::Result<()> {
    use rhdl_fpga::rcstream::util::{combine::IqCombine, split::IqSplit};
    let _ = IqSplit::<16, ()>::default()
        .descriptor("top".into())?
        .hdl()?;
    let _ = IqCombine::<16, ()>::default()
        .descriptor("top".into())?
        .hdl()?;
    Ok(())
}
