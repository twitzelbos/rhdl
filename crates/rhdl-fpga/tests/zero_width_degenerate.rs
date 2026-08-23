//! A circuit that collapses to nothing at a zero-width parameter is
//! rejected for the right reason.
//!
//! # What was wrong
//!
//! A widget whose output type has no bits produces nothing observable,
//! and `build_synchronous_netlist` has always refused it with
//! *"Circuits with no outputs are not synthesizable"*. But that check
//! ran **third**, after the kernel was compiled — and a circuit whose
//! output collapses usually has its input, state and `D`/`Q` collapse
//! with it, so kernel compilation hit a zero-width literal first and
//! reported:
//!
//! ```text
//! A zero-width value has no Verilog literal representation
//! ```
//!
//! True, and useless. It says nothing about what the user should change,
//! and its help text pointed at zero-width sub-circuits — which are
//! **fine**, and are used deliberately by `rcstream::util::split` and
//! `combine` to carry a framing type that costs no wires.
//!
//! Hoisting the output check above kernel compilation gets the apt
//! diagnostic out. Nothing newly fails: a zero-output circuit was
//! already rejected, just further along and less legibly.
//!
//! # This is the last of the zero-width defects
//!
//! The set, for the record:
//!
//! | defect | where |
//! |---|---|
//! | zero-bit value rendered as illegal literal `0'b` | `TypedBits -> LitVerilog` |
//! | zero-width value left an undriven RTL register | `make_binary` lowering |
//! | zero-width value could not cross a control-flow merge | `check_rhif_flow` |
//! | zero-output circuit reported the wrong error | descriptor ordering |

use rhdl::prelude::*;
use rhdl_fpga::core::dff;

/// Everything collapses at `F = ()`: input, output, `D` and `Q`.
mod degenerate {
    use rhdl::prelude::*;
    use rhdl_fpga::core::dff;

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    pub struct AllEmpty<F: Digital> {
        hold: dff::DFF<F>,
    }

    impl<F: Digital> SynchronousIO for AllEmpty<F> {
        type I = F;
        type O = F;
        type Kernel = all_empty_kernel<F>;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn all_empty_kernel<F: Digital>(_cr: ClockReset, i: F, q: Q<F>) -> (F, D<F>) {
        let mut d = D::<F>::dont_care();
        d.hold = i;
        (q.hold, d)
    }
}

/// A zero-width sub-circuit alongside one with bits — the shape that
/// must keep working.
mod mixed {
    use rhdl::prelude::*;
    use rhdl_fpga::core::dff;

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    pub struct Mixed<F: Digital> {
        hold: dff::DFF<F>,
        count: dff::DFF<b8>,
    }

    #[derive(PartialEq, Clone, Copy, Debug, Digital)]
    pub struct In<F: Digital> {
        pub f: F,
        pub step: bool,
    }

    impl<F: Digital> SynchronousIO for Mixed<F> {
        type I = In<F>;
        type O = (b8, F);
        type Kernel = mixed_kernel<F>;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn mixed_kernel<F: Digital>(_cr: ClockReset, i: In<F>, q: Q<F>) -> ((b8, F), D<F>) {
        let mut d = D::<F>::dont_care();
        d.hold = i.f;
        d.count = if i.step { q.count + 1 } else { q.count };
        ((q.count, q.hold), d)
    }
}

/// **The diagnostic names the actual problem.**
#[test]
fn a_circuit_with_no_outputs_says_so() {
    let err = match degenerate::AllEmpty::<()>::default().descriptor("top".into()) {
        Ok(_) => panic!("a circuit with no outputs must be rejected"),
        Err(e) => e,
    };
    assert!(
        matches!(err, RHDLError::NoOutputsError),
        "expected NoOutputsError, got: {err}"
    );
    // And specifically *not* the literal error, which is what it used to
    // report and which says nothing actionable.
    assert!(
        !matches!(err, RHDLError::ZeroWidthVerilogLiteral),
        "the literal error is the wrong diagnostic for this"
    );
}

/// The same widget at a framing type with bits is untouched.
#[test]
fn the_same_widget_with_bits_still_builds() -> miette::Result<()> {
    let _ = degenerate::AllEmpty::<bool>::default()
        .descriptor("top".into())?
        .hdl()?;
    Ok(())
}

/// **A zero-width sub-circuit is not the problem and must keep working.**
///
/// This is the shape `rcstream::util::split` and `combine` rely on to
/// carry a framing type that costs no wires. The old help text told
/// users to avoid exactly this; it was wrong, and this test is why the
/// correction cannot regress.
#[test]
fn a_zero_width_subcircuit_beside_real_bits_is_fine() -> miette::Result<()> {
    let uut = mixed::Mixed::<()>::default();
    let _ = uut.descriptor("top".into())?.hdl()?;

    let seq: Vec<mixed::In<()>> = (0..8)
        .map(|k| mixed::In::<()> {
            f: (),
            step: k % 2 == 0,
        })
        .collect();
    let tb = uut
        .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&uut, &Default::default())?.run_iverilog()?;
    Ok(())
}

/// And the widgets that actually depend on it still build.
#[test]
fn the_phantom_carrier_widgets_still_build() -> miette::Result<()> {
    use rhdl_fpga::rcstream::util::{combine::IqCombine, split::IqSplit};
    let _ = IqSplit::<16, ()>::default()
        .descriptor("top".into())?
        .hdl()?;
    let _ = IqCombine::<16, ()>::default()
        .descriptor("top".into())?
        .hdl()?;
    Ok(())
}

#[allow(dead_code)]
fn _uses_dff(_: dff::DFF<b8>) {}
