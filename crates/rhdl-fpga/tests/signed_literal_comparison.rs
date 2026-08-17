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
//! Two separate losses of signedness contribute, and either alone is
//! sufficient:
//!
//! 1. The literal becomes an unsigned `localparam`. `hdl/formatter.rs`
//!    emits `localparam {name} = {value};` with no `signed` qualifier.
//! 2. A field extracted from the `q` bundle is declared unsigned, even
//!    though its RHDL type is signed. A *direct* input is emitted
//!    correctly as `input reg signed [7:0]`, so the type information
//!    exists and is being dropped rather than never known.
//!
//! Verilog's self-determined type rules make a relational expression
//! unsigned if **either** operand is unsigned, so this silently inverts
//! the comparison for negative inputs.
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
