//! A zero-width value can no longer produce the illegal Verilog literal
//! `0'b`.
//!
//! # What was wrong
//!
//! `TypedBits -> LitVerilog` built the literal by writing the base
//! specifier and then one character per bit. At zero bits the per-bit
//! part contributes nothing, so the result was `0'b` — a sized literal
//! with **no digits**, which is a Verilog syntax error.
//!
//! `Constant<()>` produced
//!
//! ```verilog
//! module top(input wire [1:0] clock_reset, output wire [0:0] o);
//!    assign o = 0'b;
//! endmodule
//! ```
//!
//! and `DFF<()>` hit it twice, in the `initial` block and the `always`
//! block, surfacing as two bare "Malformed statement" errors from
//! `iverilog` with no indication of the cause.
//!
//! # Why the fix legalises rather than rejects
//!
//! The obvious fix is to make a zero-width value an error: a register
//! or constant holding a one-inhabitant type has nothing to store, so
//! it looks like a design mistake worth reporting.
//!
//! **It is not a mistake — it is a deliberate idiom in this tree**, and
//! rejecting it breaks working widgets. `rcstream::util::split` and
//! `combine` carry their framing type through a `Constant<F>` field
//! precisely because `PhantomData` has no HDL and would fail at
//! `descriptor()`. At `F = ()` that is a `Constant<()>`, and an
//! unframed `RCStream` is a documented, load-bearing case.
//! `iq_split_survives_a_zero_width_framing_type` below is the
//! regression test for exactly that; it is the test that caught the
//! first attempt at this fix.
//!
//! So the two halves are separated:
//!
//! - [`TryFrom<&TypedBits> for LitVerilog`] still **rejects** a
//!   zero-width value, so no caller can obtain an illegal literal by
//!   accident.
//! - `signal_literal` is the deliberate opt-in for driving a signal,
//!   substituting a one-bit zero. That matches the one-bit port the
//!   emitter already declares for a zero-bit type, so declaration and
//!   literal finally agree. A one-inhabitant type cannot lose
//!   information to the substitution: there is only one value it could
//!   have been.
//!
//! # Scope
//!
//! This covers the *literal* half of the zero-width problem only. The
//! other half is untouched and is more serious: a zero-width value gets
//! no **defining instruction** during RHIF→RTL lowering while its
//! register is still allocated, so a zero-width comparison reduces to
//! `x != x` in emitted Verilog while the Rust simulator returns a
//! defined `false`. That one is silent — it compiles and passes every
//! Rust tier. See `widget-roadmap.md`.

use rhdl::prelude::*;
use rhdl_fpga::core::{constant::Constant, dff, ram};

/// A `Digital` type with no bits that is not `()`, to show the rule is
/// about width rather than about one specific type.
#[derive(PartialEq, Clone, Copy, Debug, Digital, Default)]
pub struct Nothing {}

#[test]
fn zero_width_types_are_a_supported_thing() {
    assert_eq!(<() as Digital>::BITS, 0);
    assert_eq!(<Nothing as Digital>::BITS, 0);
    // The premise: this is supported, not an accident. `rcstream::bus`
    // documents `F = ()` as adding no wire bits, and relies on it.
    assert_eq!(<rhdl_fpga::rcstream::bus::Item<b8, ()> as Digital>::BITS, 8);
}

// ---- the conversion still refuses to produce an illegal literal --------

#[test]
fn the_conversion_rejects_a_zero_width_value() {
    let tb = ().typed_bits();
    let r: Result<rhdl::prelude::vlog::LitVerilog, _> = (&tb).try_into();
    assert!(
        matches!(r, Err(RHDLError::ZeroWidthVerilogLiteral)),
        "the fallible conversion must not hand back `0'b`"
    );
}

#[test]
fn the_placeholder_is_a_one_bit_zero() {
    use rhdl::core::hdl::builder::signal_literal;
    let zero_width = signal_literal(&().typed_bits());
    assert_eq!(quote::quote!(#zero_width).to_string(), "1 'b0");
    // And it is a placeholder only for zero width — real values pass
    // through untouched.
    let real = signal_literal(&bits::<8>(0xA5).typed_bits());
    assert_eq!(quote::quote!(#real).to_string(), "8 'b10100101");
}

// ---- the widget sites now emit legal Verilog ---------------------------

/// `Constant<()>` emits a module that parses.
///
/// `Descriptor::hdl()` runs the emitted text through `iverilog -t null`,
/// so reaching `Ok` here *is* the syntax check. Before the fix this
/// module contained `assign o = 0'b;`.
#[test]
fn a_constant_of_a_zero_width_type_emits_legal_verilog() -> miette::Result<()> {
    let desc = Constant::new(()).descriptor("top".into())?;
    let text = desc.hdl()?.modules.to_string();
    assert!(
        !text.contains("0'b"),
        "still emitting the illegal literal:\n{text}"
    );
    Ok(())
}

/// `DFF<()>` emits a module that parses.
///
/// Before the fix this failed with two "Malformed statement" errors,
/// from the `initial` block and the `always` block.
#[test]
fn a_register_of_a_zero_width_type_emits_legal_verilog() -> miette::Result<()> {
    let desc = dff::DFF::<()>::default().descriptor("top".into())?;
    let text = desc.hdl()?.modules.to_string();
    assert!(
        !text.contains("0'b"),
        "still emitting the illegal literal:\n{text}"
    );
    Ok(())
}

/// The rule is about width, not about `()` specifically.
#[test]
fn a_zero_width_struct_behaves_the_same() -> miette::Result<()> {
    let desc = dff::DFF::<Nothing>::default().descriptor("top".into())?;
    let _ = desc.hdl()?;
    Ok(())
}

// ---- the regression that caught the first attempt ----------------------

/// **`IqSplit` at `F = ()` must keep working.**
///
/// It carries its framing type through a `Constant<F>` field, so at
/// `F = ()` it contains a `Constant<()>`. The first version of this fix
/// made that an error, which broke this widget and `IqCombine` — an
/// unframed `RCStream` would have become unrepresentable. This test
/// exists so that cannot happen again.
#[test]
fn iq_split_survives_a_zero_width_framing_type() -> miette::Result<()> {
    use rhdl_fpga::rcstream::util::{combine::IqCombine, split::IqSplit};
    let _ = IqSplit::<16, ()>::default()
        .descriptor("top".into())?
        .hdl()?;
    let _ = IqCombine::<16, ()>::default()
        .descriptor("top".into())?
        .hdl()?;
    // And a non-trivial framing type still works alongside it.
    let _ = IqSplit::<16, bool>::default()
        .descriptor("top".into())?
        .hdl()?;
    Ok(())
}

// ---- everything with bits is untouched ---------------------------------

#[test]
fn non_zero_width_types_are_unaffected() -> miette::Result<()> {
    let _ = Constant::new(false).descriptor("top".into())?.hdl()?;
    let _ = Constant::new(bits::<8>(0xA5))
        .descriptor("top".into())?
        .hdl()?;
    let _ = dff::DFF::<b8>::default().descriptor("top".into())?.hdl()?;
    let _ = dff::DFF::new(bits::<16>(0xFFFF))
        .descriptor("top".into())?
        .hdl()?;
    let _ = ram::synchronous::SyncBRAM::<b8, 4>::new([(bits::<4>(0), bits::<8>(0x5A))])
        .descriptor("top".into())?
        .hdl()?;
    Ok(())
}

/// A signed constant still carries its `s` base specifier.
///
/// Guarding the edit against disturbing the signedness fix from #73,
/// whose whole point is that dropping the `s` silently flips relational
/// comparisons in Verilog while the Rust simulator stays correct.
#[test]
fn signed_literals_keep_their_base_specifier() -> miette::Result<()> {
    let desc = Constant::new(signed::<8>(-3)).descriptor("top".into())?;
    let text = desc.hdl()?.modules.to_string();
    assert!(text.contains("'sb"), "expected an sb literal, got:\n{text}");
    Ok(())
}

/// One bit is the smallest genuinely-representable literal.
#[test]
fn one_bit_is_the_boundary_and_still_works() -> miette::Result<()> {
    let desc = Constant::new(true).descriptor("top".into())?;
    let text = desc.hdl()?.modules.to_string();
    assert!(text.contains("1'b1"), "got:\n{text}");
    Ok(())
}
