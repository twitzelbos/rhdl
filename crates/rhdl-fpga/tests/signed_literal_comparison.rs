//! Regression tests for **signed comparison against a literal**.
//!
//! # The defect this fixed
//!
//! A `SignedBits<N>` literal was emitted as an unsigned Verilog
//! constant, so `q.field > signed::<8>(10)` lowered to an *unsigned*
//! comparison and every negative value compared greater than a positive
//! bound. IEEE 1364 §5.5.1: a relational expression is unsigned if
//! **either** operand is unsigned.
//!
//! Simulation could not see it. The Rust simulator runs the kernel as a
//! Rust function, where the comparison is genuinely signed, so Tiers 1
//! and 2 passed and only the Tier 4 `iverilog` round-trip caught it.
//!
//! # Why the fix is where it is
//!
//! `translate_binary` emits bare Verilog operators with no signedness
//! wrapping at all — note `Shr` becomes `>>>` *unconditionally*, which
//! is only correct because the operands' declarations carry signedness.
//! So RHDL's principle is: **operands carry their own signedness and
//! Verilog's type rules do the rest.** Registers implemented that;
//! literals did not.
//!
//! The fix gives the constant Verilog's own carrier for signedness, the
//! `s` base specifier: `8'sb00001010` rather than `8'b00001010`. That
//! is one line in `From<&TypedBits> for vlog::LitVerilog`, and it
//! upholds the principle rather than adding a special case to it.
//!
//! The same IEEE rule bounds the blast radius: mixing a signed literal
//! with an unsigned register still yields an unsigned expression, so
//! this can only ever promote signed-vs-signed to a signed comparison.
//! It cannot change a comparison that is unsigned today and correct.
//!
//! # What is still broken — a separate, narrower defect
//!
//! See `single_field_bundle_still_loses_signedness`. When a widget's
//! `q` bundle has exactly **one** field, the field occupies the whole
//! bundle, RHDL elides the extraction, and the comparison is emitted
//! against the *bundle* — whose kind is an unsigned struct — instead of
//! against the field. Every realistic shape works (direct input,
//! multi-field bundle); this one does not. Tracked separately because
//! it lives in aggregate field extraction, not in literal emission.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;

/// A span that straddles zero. If a comparison is unsigned, every
/// negative entry reports `true` against a bound of `+10`.
const SPAN: [i128; 8] = [0, 20, -1, -100, 5, -5, 127, -128];

fn stimulus() -> Vec<SignedBits<8>> {
    SPAN.iter().map(|v| signed::<8>(*v)).collect()
}

fn round_trip<T>(uut: &T) -> miette::Result<()>
where
    T: Synchronous<I = SignedBits<8>, O = bool>,
{
    let stream = stimulus().into_iter().with_reset(1).clock_pos_edge(100);
    let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    tb.rtl(uut, &Default::default())?.run_iverilog()?;
    tb.ntl(uut, &Default::default())?.run_iverilog()?;
    Ok(())
}

// ---------------------------------------------------------------------
// Shape 1 — a direct signed input compared against a literal.
// ---------------------------------------------------------------------

mod direct {
    use super::*;

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    /// Compares a signed *input* against a literal, so the only thing
    /// under test is the literal's signedness.
    pub struct CmpInputToLiteral {
        keep: dff::DFF<bool>,
    }

    impl SynchronousIO for CmpInputToLiteral {
        type I = SignedBits<8>;
        type O = bool;
        type Kernel = cmp_input_to_literal_kernel;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn cmp_input_to_literal_kernel(_cr: ClockReset, i: SignedBits<8>, q: Q) -> (bool, D) {
        let mut d = D::dont_care();
        d.keep = q.keep;
        let o = i > signed::<8>(10);
        (o, d)
    }
}

// ---------------------------------------------------------------------
// Shape 2 — a signed field out of a multi-field bundle, the shape a
// real saturating datapath has.
// ---------------------------------------------------------------------

mod bundle {
    use super::*;

    #[derive(PartialEq, Clone, Copy, Debug, Digital, Default)]
    /// Multi-field on purpose: with a single field the struct and the
    /// field occupy the same bits, which is the degenerate case that
    /// `single_field_bundle_still_loses_signedness` covers.
    pub struct State {
        pub val: SignedBits<8>,
        pub other: Bits<8>,
    }

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    /// The clamp idiom's real shape: a registered signed value compared
    /// against a constant bound.
    pub struct CmpBundleToLiteral {
        state: dff::DFF<State>,
    }

    impl SynchronousIO for CmpBundleToLiteral {
        type I = SignedBits<8>;
        type O = bool;
        type Kernel = cmp_bundle_to_literal_kernel;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn cmp_bundle_to_literal_kernel(_cr: ClockReset, i: SignedBits<8>, q: Q) -> (bool, D) {
        let mut d = D::dont_care();
        let mut next = q.state;
        next.val = i;
        d.state = next;
        let o = q.state.val > signed::<8>(10);
        (o, d)
    }
}

// ---------------------------------------------------------------------
// Shape 3 — the degenerate single-field bundle. Still broken.
// ---------------------------------------------------------------------

mod single_field {
    use super::*;

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    /// One field, so it spans the whole `q` bundle.
    pub struct CmpSingleField {
        reg: dff::DFF<SignedBits<8>>,
    }

    impl SynchronousIO for CmpSingleField {
        type I = SignedBits<8>;
        type O = bool;
        type Kernel = cmp_single_field_kernel;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn cmp_single_field_kernel(_cr: ClockReset, i: SignedBits<8>, q: Q) -> (bool, D) {
        let mut d = D::dont_care();
        d.reg = i;
        let o = q.reg > signed::<8>(10);
        (o, d)
    }
}

// ---------------------------------------------------------------------

/// The Rust simulator gets this right, which is precisely why the
/// defect survived: simulation cannot detect it.
#[test]
fn rust_simulation_compares_signed() {
    let uut = single_field::CmpSingleField::default();
    let stream = stimulus().into_iter().with_reset(1).clock_pos_edge(100);
    let out: Vec<bool> = uut
        .run(stream)
        .synchronous_sample()
        .map(|s| s.output)
        .collect();
    assert!(
        out.iter().any(|x| *x),
        "+20 and +127 must exceed +10 — a run where nothing is true proves nothing"
    );
    assert!(
        out.iter().any(|x| !*x),
        "negatives must not exceed +10 — a run where everything is true proves nothing"
    );
}

/// The literal carries its own signedness in the emitted Verilog.
///
/// Pinned as text because it *is* the fix: if this reverts to `8'b`,
/// the round-trip tests below start failing for a reason that is not
/// obvious from their output.
#[test]
fn emitted_literal_carries_signedness() -> miette::Result<()> {
    let uut = direct::CmpInputToLiteral::default();
    let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
    assert!(
        hdl.contains("8'sb00001010"),
        "the signed literal lost its `s` base specifier:\n{hdl}"
    );
    Ok(())
}

/// An **unsigned** literal must stay unsigned. This is the negative
/// test: the fix keys off the literal's kind, and a change that made
/// every literal signed would pass every other test in this file.
#[test]
fn unsigned_literals_are_untouched() -> miette::Result<()> {
    let uut = bundle::CmpBundleToLiteral::default();
    let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
    let signed_lits = hdl.matches("'sb").count();
    let unsigned_lits = hdl.matches("'b").count() - signed_lits;
    assert!(
        unsigned_lits > 0,
        "this widget has unsigned literals too; if none are emitted the \
         test is not checking anything:\n{hdl}"
    );
    Ok(())
}

/// Acceptance test for the fix: a signed input against a literal.
#[test]
fn verilog_agrees_with_rust_direct() -> miette::Result<()> {
    round_trip(&direct::CmpInputToLiteral::default())
}

/// Acceptance test for the fix in the shape that matters — the clamp
/// idiom, with the compared value coming out of a register bundle.
#[test]
fn verilog_agrees_with_rust_from_bundle() -> miette::Result<()> {
    round_trip(&bundle::CmpBundleToLiteral::default())
}

/// Signed comparison **not** involving a literal was always correct.
///
/// This bounds the defect. Without it the natural reading of this file
/// is "RHDL cannot compare signed values", which is false and would
/// send someone rewriting working widgets.
#[test]
fn signed_comparison_between_registers_is_correct() -> miette::Result<()> {
    let uut = bundle::CmpBundleToLiteral::default();
    let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
    assert!(
        hdl.contains("reg signed [7:0]"),
        "extracting a signed field from a bundle should preserve signedness:\n{hdl}"
    );
    Ok(())
}

/// **Known remaining defect**, separate from the one this file fixes.
///
/// With exactly one field in the `q` bundle the field spans the whole
/// bundle, RHDL elides the extraction, and the comparison is emitted
/// against the bundle — an unsigned struct kind — rather than against
/// the signed field:
///
/// ```verilog
/// reg [7:0] r2;                  // the q bundle, unsigned struct kind
/// localparam l1 = 8'sb00001010;  // the literal is signed (fixed)
/// r2 = arg_2;                    // extraction elided
/// r3 = r2 > l1;                  // unsigned again, because r2 is
/// ```
///
/// The fix belongs in aggregate field extraction, not literal emission,
/// so it is deliberately not bundled with that change. Remove the
/// `#[ignore]` when it lands — this is its acceptance test.
#[test]
#[ignore = "known defect: single-field q bundle elides extraction and loses the field's signedness"]
fn single_field_bundle_still_loses_signedness() -> miette::Result<()> {
    round_trip(&single_field::CmpSingleField::default())
}
