//! Regression test for a **signed-comparison codegen defect**.
//!
//! Comparing a `SignedBits<N>` against a literal emits an *unsigned*
//! Verilog comparison, so every negative value compares greater than a
//! positive bound. The Rust simulator is unaffected — it runs the kernel
//! as a Rust function, where the comparison is genuinely signed — so
//! Tiers 1 and 2 pass and only the Tier 4 `iverilog` round-trip sees it.
//!
//! # What is emitted
//!
//! For `q.reg > signed::<8>(10)`:
//!
//! ```verilog
//! reg [7:0] r2;                  // q.reg, a SignedBits<8> — not signed
//! localparam l1 = 8'b00001010;   // the literal — not signed
//! r3 = r2 > l1;                  // therefore an unsigned comparison
//! ```
//!
//! # Scope: the literal, and only the literal
//!
//! `hdl/builder.rs` declares registers with their signedness --
//! `reg_decls` computes a `vlog::SignedWidth` from the operand's `Kind`
//! and emits `reg signed [7:0] rN;`. The `lit_decls` block immediately
//! below it emits `localparam {name} = {value};` with no width and no
//! `signed` qualifier. That asymmetry is the whole defect.
//!
//! Everything else about signed comparison works, and is verified by
//! `signed_comparison_between_registers_is_correct` below:
//!
//! - two signed values compared with no literal involved -- correct;
//! - a signed field extracted from a multi-field `q` bundle -- correct,
//!   the extraction preserves signedness.
//!
//! An earlier version of this file claimed the `q` bundle *also* lost
//! signedness. That was wrong: it was an artefact of the single-field
//! bundle used in the minimal repro, where the struct and its only
//! field occupy the same bits, so the bundle's own unsigned struct kind
//! is what gets declared. With a genuine multi-field bundle the field
//! is declared `reg signed` and iverilog agrees.
//!
//! Verilog's self-determined type rules make a relational expression
//! unsigned if **either** operand is unsigned, so this silently inverts
//! the comparison for negative inputs.
//!
//! # This is an incomplete implementation, not a design decision
//!
//! `translate_binary` emits bare Verilog operators with no signedness
//! wrapping at all -- note that `Shr` becomes `>>>` unconditionally,
//! which is only correct because the *operand declarations* carry
//! signedness. So RHDL's principle is: declare operands with their
//! signedness and let Verilog's own type rules do the work. Registers
//! implement that principle; literals do not. Fixing this upholds the
//! principle rather than adding a special case to it.
//!
//! Only relational operators are affected in practice. Same-width
//! add/sub are bit-identical regardless of signedness, and RHDL emits
//! same-width operands.
//!
//! # Why it matters
//!
//! This is the clamp idiom — `if x > hi { hi } else if x < lo { lo }` —
//! which is how every saturating datapath is written. A clamp built this
//! way inverts its own sense in hardware while simulating correctly.
//! `dsp::nco::sin_cos_linear_interp` hit exactly this and now avoids
//! saturation entirely; it is the first widget in the tree to compare
//! signed values against literals, which is why the defect survived.
//!
//! The fix belongs in `rhdl-core`'s HDL emission, and per CLAUDE.md
//! §11.1 is a compiler-level change requiring its own PR. The
//! infrastructure is already present — `SignedWidth::Signed`,
//! `signed_width()`, and a `kind.is_signed()` check in
//! `hdl/builder.rs` — so this is a dropped case, not a missing feature.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
/// Minimal widget: register a signed value, compare it to a literal.
pub struct SignedCmp {
    reg: dff::DFF<SignedBits<8>>,
}

impl SynchronousIO for SignedCmp {
    type I = SignedBits<8>;
    type O = bool;
    type Kernel = signed_cmp_kernel;
}

#[kernel]
#[doc(hidden)]
pub fn signed_cmp_kernel(_cr: ClockReset, i: SignedBits<8>, q: Q) -> (bool, D) {
    let mut d = D::dont_care();
    d.reg = i;
    let o = q.reg > signed::<8>(10);
    (o, d)
}

/// Straddles zero. If the comparison is unsigned, every negative input
/// reports `true` against a bound of `+10`.
fn stimulus() -> Vec<SignedBits<8>> {
    vec![
        signed::<8>(0),
        signed::<8>(20),
        signed::<8>(-1),
        signed::<8>(-100),
        signed::<8>(5),
        signed::<8>(-5),
        signed::<8>(127),
        signed::<8>(-128),
    ]
}

/// The Rust simulator gets this right, which is precisely the problem:
/// it means simulation cannot detect the defect.
#[test]
fn rust_simulation_compares_signed() {
    let uut = SignedCmp::default();
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

/// The emitted Verilog must agree with the Rust simulation.
///
/// **Currently fails**, which is the defect this file documents:
/// `TESTBENCH FAILED: Expected 0, got 1` on the first negative input.
/// Remove the `#[ignore]` when the codegen fix lands — it is the
/// acceptance test for that change.
#[test]
#[ignore = "known defect: signed comparison against a literal lowers to unsigned Verilog"]
fn verilog_agrees_with_rust() -> miette::Result<()> {
    let uut = SignedCmp::default();
    let stream = stimulus().into_iter().with_reset(1).clock_pos_edge(100);
    let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = tb.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = tb.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}

/// Pins the defect as *emitted text*, so the codegen fix is visible as a
/// diff here even before the round-trip above is re-enabled.
#[test]
fn emitted_comparison_operands_are_currently_unsigned() -> miette::Result<()> {
    let uut = SignedCmp::default();
    let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();

    let cmp = hdl
        .lines()
        .find(|l| l.contains(" > "))
        .expect("the kernel contains a comparison")
        .trim()
        .to_string();
    assert_eq!(cmp, "r3 = r2 > l1;");

    // Neither operand carries signedness at the point of comparison.
    assert!(
        hdl.contains("reg [7:0] r2;"),
        "expected the q-bundle field to be declared unsigned; if this now \
         reads `reg signed [7:0] r2;` the codegen fix has landed — \
         re-enable `verilog_agrees_with_rust` and delete this test"
    );
    assert!(
        hdl.contains("localparam l1 = 8'b00001010;"),
        "expected an unsigned localparam; if this now carries `signed` the \
         codegen fix has landed — re-enable `verilog_agrees_with_rust` and \
         delete this test"
    );
    Ok(())
}

/// The control case: signed comparison that does **not** involve a
/// literal is correct today, in both shapes that matter.
///
/// This is what bounds the defect. Without it the natural reading of
/// this file is "RHDL cannot compare signed values", which is false and
/// would send someone rewriting working widgets.
mod registers {
    use super::*;

    #[derive(PartialEq, Clone, Copy, Debug, Digital)]
    pub struct TwoIn {
        pub a: SignedBits<8>,
        pub b: SignedBits<8>,
    }

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    /// Two signed inputs, compared directly. No literal anywhere.
    pub struct CmpTwoInputs {
        keep: dff::DFF<bool>,
    }

    impl SynchronousIO for CmpTwoInputs {
        type I = TwoIn;
        type O = bool;
        type Kernel = cmp_two_inputs_kernel;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn cmp_two_inputs_kernel(_cr: ClockReset, i: TwoIn, q: Q) -> (bool, D) {
        let mut d = D::dont_care();
        d.keep = q.keep;
        let o = i.a > i.b;
        (o, d)
    }
}

mod bundle {
    use super::*;

    #[derive(PartialEq, Clone, Copy, Debug, Digital, Default)]
    /// Multi-field on purpose: with a single field the struct and the
    /// field occupy the same bits, so the bundle's own unsigned struct
    /// kind is what gets declared and the test proves nothing.
    pub struct Bundle {
        pub val: SignedBits<8>,
        pub other: Bits<8>,
        pub flag: bool,
    }

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    /// A signed field extracted from a multi-field `q` bundle.
    pub struct CmpFromBundle {
        state: dff::DFF<Bundle>,
    }

    impl SynchronousIO for CmpFromBundle {
        type I = SignedBits<8>;
        type O = bool;
        type Kernel = cmp_from_bundle_kernel;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn cmp_from_bundle_kernel(_cr: ClockReset, i: SignedBits<8>, q: Q) -> (bool, D) {
        let mut d = D::dont_care();
        let mut next = q.state;
        next.val = i;
        d.state = next;
        let o = q.state.val > i;
        (o, d)
    }
}

const SPAN: [i128; 8] = [0, 20, -1, -100, 5, -5, 127, -128];

#[test]
fn signed_comparison_between_registers_is_correct() -> miette::Result<()> {
    // Shape 1 -- two signed values, no literal.
    let uut = registers::CmpTwoInputs::default();
    let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
    assert_eq!(
        hdl.matches("reg signed [7:0]").count(),
        2,
        "both comparison operands should be declared signed:\n{hdl}"
    );
    let mut cases = Vec::new();
    for a in SPAN {
        for b in SPAN {
            cases.push(registers::TwoIn {
                a: signed::<8>(a),
                b: signed::<8>(b),
            });
        }
    }
    let stream = cases.into_iter().with_reset(1).clock_pos_edge(100);
    let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&uut, &Default::default())?.run_iverilog()?;

    // Shape 2 -- a signed field out of a multi-field bundle.
    let uut = bundle::CmpFromBundle::default();
    let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
    assert!(
        hdl.contains("reg signed [7:0]"),
        "extracting a signed field from a bundle should preserve \
         signedness:\n{hdl}"
    );
    let stream = SPAN
        .iter()
        .map(|v| signed::<8>(*v))
        .collect::<Vec<_>>()
        .into_iter()
        .with_reset(1)
        .clock_pos_edge(100);
    let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&uut, &Default::default())?.run_iverilog()?;
    Ok(())
}
