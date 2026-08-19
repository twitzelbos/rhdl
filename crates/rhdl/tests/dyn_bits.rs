use rhdl::prelude::*;

#[cfg(test)]
mod common;

#[cfg(test)]
use common::*;
use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog;

// A macro to deduplicate the test code for the bN x bN case for
// bits
macro_rules! test_op_b4xb4 {
    ($op: tt) => {
        {
        #[kernel]
        fn do_stuff(a1: Signal<b4, Red>, a2: Signal<b4, Red>) -> Signal<(b4, b4, b4, b4, b4), Red> {
            let b1 = a1.val();
            let b2 = a2.val();
            let a1 = b1.dyn_bits();
            let a2 = b2.dyn_bits();
            let c = a1 $op a2;
            let d = c $op 1;
            let e = 1 $op d;
            let f = a1 $op b2;
            let g = b1 $op a2;
            signal((c.as_bits(), d.as_bits(), e.as_bits(), f, g))
        }
        let args = exhaustive::<4>().into_iter().flat_map(|a1| {
            exhaustive::<4>()
                .into_iter()
                .map(move |a2| (red(a1), red(a2)))
        });
        test_kernel_vm_and_verilog::<do_stuff, _, _, _>(do_stuff, args)?;
    }
    }
}

macro_rules! test_op_s4xs4 {
    ($op: tt) => {
        {
        #[kernel]
        fn do_stuff(a1: Signal<s4, Red>, a2: Signal<s4, Red>) -> Signal<(s4, s4, s4), Red> {
            let a1 = a1.val().dyn_bits();
            let a2 = a2.val().dyn_bits();
            let c = a1 $op a2;
            let d = c + 1;
            let e = 1 + d;
            signal((c.as_signed_bits(), d.as_signed_bits(), e.as_signed_bits()))
        }
        let args = exhaustive_signed::<4>().into_iter().flat_map(|a1| {
            exhaustive_signed::<4>()
                .into_iter()
                .map(move |a2| (red(a1), red(a2)))
        });
        test_kernel_vm_and_verilog::<do_stuff, _, _, _>(do_stuff, args)?;
    }
    }
}

#[test]
fn test_add_via_dyn_bits() -> miette::Result<()> {
    test_op_b4xb4!(+);
    test_op_s4xs4!(+);
    Ok(())
}

#[test]
fn test_sub_via_dyn_bits() -> miette::Result<()> {
    test_op_b4xb4!(-);
    test_op_s4xs4!(-);
    Ok(())
}

macro_rules! shift_test_bits {
    ($op: tt) => {
    {
        #[kernel]
        fn do_stuff(a1: Signal<b8, Red>, a2: Signal<b3, Red>) -> Signal<(b8, b8, b8), Red> {
            let b1 = a1.val();
            let b2 = a2.val();
            let a1 = b1.dyn_bits();
            let a2 = b2.dyn_bits();
            let c = a1 $op a2;
            let d = c $op 1;
            let e = a1 $op b2;
            signal((c.as_bits(), d.as_bits(), e.as_bits()))
        }
        let args = exhaustive::<8>().into_iter().flat_map(|a1| {
            exhaustive::<3>()
                .into_iter()
                .map(move |a2| (red(a1), red(a2)))
        });
        test_kernel_vm_and_verilog::<do_stuff, _, _, _>(do_stuff, args)?;
    }
    };
}

#[test]
fn test_shift_via_dyn_bits() -> miette::Result<()> {
    shift_test_bits!(>>);
    shift_test_bits!(<<);
    Ok(())
}

macro_rules! shift_test_signed_bits {
    ($op: tt) => {
    {
        #[kernel]
        fn do_stuff(a1: Signal<s8, Red>, a2: Signal<b3, Red>) -> Signal<(s8, s8, s8), Red> {
            let b1 = a1.val();
            let b2 = a2.val();
            let a1 = b1.dyn_bits();
            let a2 = b2.dyn_bits();
            let c = a1 $op a2;
            let d = c $op 1;
            let e = a1 $op b2;
            signal((c.as_signed_bits(), d.as_signed_bits(), e.as_signed_bits()))
        }
        let args = exhaustive_signed::<8>().into_iter().flat_map(|a1| {
            exhaustive::<3>()
                .into_iter()
                .map(move |a2| (red(a1), red(a2)))
        });
        test_kernel_vm_and_verilog::<do_stuff, _, _, _>(do_stuff, args)?;
    }
    };
}

#[test]
fn test_shift_via_signed_dyn_bits() -> miette::Result<()> {
    shift_test_signed_bits!(>>);
    shift_test_signed_bits!(<<);
    Ok(())
}

#[test]
fn test_shl_signed_via_dyn_bits() -> miette::Result<()> {
    #[kernel]
    fn do_stuff(a1: Signal<s8, Red>, a2: Signal<b3, Red>) -> Signal<(s8, s8), Red> {
        let a1 = a1.val().dyn_bits();
        let a2 = a2.val().dyn_bits();
        let c = a1 << a2;
        let d = c << 1;
        signal((c.as_signed_bits(), d.as_signed_bits()))
    }
    let args = exhaustive_signed::<8>().into_iter().flat_map(|a1| {
        exhaustive::<3>()
            .into_iter()
            .map(move |a2| (red(a1), red(a2)))
    });
    test_kernel_vm_and_verilog::<do_stuff, _, _, _>(do_stuff, args)?;
    Ok(())
}

#[test]
fn test_or_via_dyn_bits() -> miette::Result<()> {
    test_op_b4xb4!(|);
    Ok(())
}

#[test]
fn test_and_via_dyn_bits() -> miette::Result<()> {
    test_op_b4xb4!(&);
    Ok(())
}

#[test]
fn test_xor_via_dyn_bits() -> miette::Result<()> {
    test_op_b4xb4!(^);
    Ok(())
}

#[test]
fn test_mul_via_dyn_bits() -> miette::Result<()> {
    test_op_b4xb4!(*);
    test_op_s4xs4!(*);
    Ok(())
}

#[test]
fn test_add_via_dyn_bits_fails_compile_with_mismatched() -> miette::Result<()> {
    #[kernel]
    fn do_stuff(a1: Signal<b4, Red>, a2: Signal<b4, Red>) -> Signal<b5, Red> {
        let a1 = a1.val().dyn_bits();
        let a2 = a2.val().dyn_bits();
        let c = a1.xadd(a2);
        let d = c.xadd(b4(1));
        let e: b5 = d.as_bits();
        signal(e)
    }
    let err = compile_design::<do_stuff>(CompilationMode::Asynchronous)
        .expect_err("Should have failed to compile");
    let report = miette_report(err);
    expect_test::expect_file!["expect/add_via_dyn_bits_fails_compile_with_mismatched.expect"]
        .assert_eq(&report);
    Ok(())
}

macro_rules! check_for_op_that_causes_overflow {
    ($op: ident) => {{
        #[kernel]
        fn do_stuff(a1: Signal<b128, Red>, a2: Signal<b128, Red>) -> Signal<b128, Red> {
            let a1 = a1.val().dyn_bits();
            let a2 = a2.val().dyn_bits();
            let c = a1.$op(a2);
            let c: b128 = c.as_bits();
            signal(c)
        }
        // Should cause a TypeError with bit overflow
        match compile_design::<do_stuff>(CompilationMode::Asynchronous) {
            Ok(_) => panic!("Should have failed to compile"),
            Err(RHDLError::RHDLTypeError(..)) => (),
            Err(_) => panic!("Should have failed to compile with a type error"),
        }
    }};
}

macro_rules! check_for_signed_op_that_causes_overflow {
    ($op: ident) => {{
        #[kernel]
        fn do_stuff(a1: Signal<s128, Red>, a2: Signal<s128, Red>) -> Signal<s128, Red> {
            let a1 = a1.val().dyn_bits();
            let a2 = a2.val().dyn_bits();
            let c = a1.$op(a2);
            let c: s128 = c.as_signed_bits();
            signal(c)
        }
        // Should cause a TypeError with bit overflow
        match compile_design::<do_stuff>(CompilationMode::Asynchronous) {
            Ok(_) => panic!("Should have failed to compile"),
            Err(RHDLError::RHDLTypeError(..)) => (),
            Err(_) => panic!("Should have failed to compile with a type error"),
        }
    }};
}

#[test]
fn test_xops_overflow() -> miette::Result<()> {
    check_for_op_that_causes_overflow!(xadd);
    check_for_op_that_causes_overflow!(xmul);
    check_for_signed_op_that_causes_overflow!(xadd);
    check_for_signed_op_that_causes_overflow!(xmul);
    Ok(())
}

#[test]
fn test_xadd_causes_overflow_warning_at_rhdl_compile_time() -> miette::Result<()> {
    #[kernel]
    fn do_stuff(a1: Signal<b128, Red>, a2: Signal<b128, Red>) -> Signal<b128, Red> {
        let a1 = a1.val().dyn_bits();
        let a2 = a2.val().dyn_bits();
        let c = a1.xadd(a2);
        let c: b128 = c.as_bits();
        signal(c)
    }
    // Should cause a TypeError with bit overflow
    let err = compile_design::<do_stuff>(CompilationMode::Asynchronous)
        .expect_err("Should have failed to compile");
    let report = miette_report(err);
    expect_test::expect_file!["expect/xadd_causes_overflow_warning_at_rhdl_compile_time.expect"]
        .assert_eq(&report);
    Ok(())
}

#[test]
fn test_xsgn_is_trapped_as_signed() -> miette::Result<()> {
    #[kernel]
    fn do_stuff(a: Signal<b4, Red>, b: Signal<b4, Red>) -> Signal<s4, Red> {
        let a = a.val().dyn_bits();
        let b = b.val().dyn_bits();
        let c = a.xsub(b);
        let c: s4 = c.as_signed_bits();
        signal(c)
    }
    // This should cause a type check error because the output xsub is 5 bits, not 4.
    let err = compile_design::<do_stuff>(CompilationMode::Asynchronous)
        .expect_err("Should have failed to compile");
    let report = miette_report(err);
    expect_test::expect_file!["expect/xsgn_is_trapped_as_signed.expect"].assert_eq(&report);
    Ok(())
}

// ---------------------------------------------------------------------
// `xmul` keeps its operands at their declared widths.
//
// `XMul` used to lower with two explicit `Cast{Resize}` ops widening both
// operands to the result width, so an 18x14 product emitted as a 32x32
// Verilog multiply. Operand widths are what decide a multiply's DSP cost
// -- a DSP48E1 is 18x25 -- so the pre-widening asked the synthesiser to
// recover the operand widths by bit-range analysis before it could map to
// a single slice.
//
// These tests pin the semantics rather than the emitted text.
// `test_kernel_vm_and_verilog` runs the RHIF VM, the RTL VM *and*
// `iverilog`, and requires all three to agree -- which is exactly the set
// of consumers that had to learn about narrow operands. The RTL VM in
// particular would compute the product at the *first* operand's width
// without `rtl::runtime_ops::binary_at_result_width`, so a regression
// there shows up here rather than as a silently wrong instrument.
// ---------------------------------------------------------------------

/// Unsigned `xmul` across mismatched operand widths, exhaustively.
///
/// `b4 x b3` has a 7-bit result, so the operands are narrower than the
/// destination in both positions and neither is a power-of-two special
/// case throughout.
#[test]
fn test_xmul_unsigned_mixed_widths() -> miette::Result<()> {
    #[kernel]
    fn do_stuff(a1: Signal<b4, Red>, a2: Signal<b3, Red>) -> Signal<b7, Red> {
        let p = a1.val().dyn_bits().xmul(a2.val().dyn_bits());
        signal(p.resize::<7>().as_bits())
    }
    let args = exhaustive::<4>().into_iter().flat_map(|a1| {
        exhaustive::<3>()
            .into_iter()
            .map(move |a2| (red(a1), red(a2)))
    });
    test_kernel_vm_and_verilog::<do_stuff, _, _, _>(do_stuff, args)?;
    Ok(())
}

/// Signed `xmul` across mismatched operand widths, exhaustively.
///
/// The signed case is the one that would break first if the operand
/// extension were dropped without the emitted `*` being
/// context-determined: a narrow negative operand read at the destination
/// width without sign extension is a large positive number. Sweeping both
/// operands over their full signed range covers every sign combination
/// including the maximum-negative values.
#[test]
fn test_xmul_signed_mixed_widths() -> miette::Result<()> {
    #[kernel]
    fn do_stuff(a1: Signal<s4, Red>, a2: Signal<s3, Red>) -> Signal<s7, Red> {
        let p = a1.val().dyn_bits().xmul(a2.val().dyn_bits());
        signal(p.resize::<7>().as_signed_bits())
    }
    let args = exhaustive_signed::<4>().into_iter().flat_map(|a1| {
        exhaustive_signed::<3>()
            .into_iter()
            .map(move |a2| (red(a1), red(a2)))
    });
    test_kernel_vm_and_verilog::<do_stuff, _, _, _>(do_stuff, args)?;
    Ok(())
}

/// The emitted Verilog really does carry the *declared* operand widths.
///
/// The tests above would still pass if the operands were pre-widened --
/// they check semantics, and the pre-widened form was also correct. This
/// one checks the thing the change exists for, in the spirit of
/// `dsp::mixer`'s `multiplier_count_is_as_claimed`: a resource claim that
/// cannot be tested is not a resource claim.
/// Operands for [`test_xmul_emits_narrow_operands`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct MulIn {
    pub a: SignedBits<18>,
    pub b: SignedBits<14>,
}

#[kernel]
fn mul_18x14(_cr: ClockReset, i: MulIn) -> SignedBits<32> {
    let p = i.a.dyn_bits().xmul(i.b.dyn_bits());
    p.resize::<32>().as_signed_bits()
}

#[test]
fn test_xmul_emits_narrow_operands() -> miette::Result<()> {
    let uut: Func<MulIn, SignedBits<32>> = Func::try_new::<mul_18x14>()?;
    let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();

    // Width of every declared signed register, so the multiply's operands
    // can be looked up by name.
    let mut width = std::collections::HashMap::new();
    for line in hdl.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("reg signed [") {
            if let Some((range, name)) = rest.split_once("] ") {
                if let Ok(hi) = range.split(':').next().unwrap_or("").parse::<usize>() {
                    width.insert(name.trim_end_matches(';').to_string(), hi + 1);
                }
            }
        }
    }
    let mut mults = Vec::new();
    for line in hdl.lines() {
        let t = line.trim().trim_end_matches(';');
        if let Some((lhs, rhs)) = t.split_once(" * ") {
            let a = lhs.split('=').nth(1).map(str::trim).unwrap_or("");
            if let (Some(wa), Some(wb)) = (width.get(a), width.get(rhs.trim())) {
                mults.push((*wa, *wb));
            }
        }
    }
    assert!(
        mults.contains(&(18, 14)),
        "expected an 18x14 multiply -- the declared operand widths, and a \
         single DSP48E1 port pair -- but found {mults:?}.  32x32 means XMul \
         is pre-widening its operands to the result width again.\n\n{hdl}"
    );
    Ok(())
}

/// **Loophole check:** `xmul` by a power-of-two literal.
///
/// `rtl_passes::lower_multiply_to_shift` rewrites a `Mul` whose second
/// operand is a one-bit literal into a `Shl`. Before `XMul` stopped
/// pre-widening, that pass only ever saw operands as wide as the result;
/// now it can see a narrow one. Shifts are deliberately excluded from
/// `binary_at_result_width` -- Verilog does not context-extend a shift
/// count -- so this exercises the interaction rather than assuming it is
/// benign.
#[test]
fn test_xmul_by_power_of_two_literal() -> miette::Result<()> {
    #[kernel]
    fn do_stuff(a1: Signal<s6, Red>) -> Signal<s10, Red> {
        // 4 is a single-bit literal, so the multiply is a shift candidate.
        let p = a1.val().dyn_bits().xmul(s4(4).dyn_bits());
        signal(p.resize::<10>().as_signed_bits())
    }
    let args = exhaustive_signed::<6>().into_iter().map(|a| (red(a),));
    test_kernel_vm_and_verilog::<do_stuff, _, _, _>(do_stuff, args)?;
    Ok(())
}

/// The same, unsigned.
#[test]
fn test_xmul_unsigned_by_power_of_two_literal() -> miette::Result<()> {
    #[kernel]
    fn do_stuff(a1: Signal<b6, Red>) -> Signal<b10, Red> {
        let p = a1.val().dyn_bits().xmul(b4(8).dyn_bits());
        signal(p.resize::<10>().as_bits())
    }
    let args = exhaustive::<6>().into_iter().map(|a| (red(a),));
    test_kernel_vm_and_verilog::<do_stuff, _, _, _>(do_stuff, args)?;
    Ok(())
}
