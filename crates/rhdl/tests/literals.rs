use rhdl::prelude::*;
#[cfg(test)]
mod common;
#[cfg(test)]
use common::*;
use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog;

#[test]
fn test_const_match_finite_bits() -> miette::Result<()> {
    const ONE: b8 = bits(1);
    const TWO: b8 = bits(2);
    const THREE: b8 = bits(3);
    #[kernel]
    fn add<C: Domain>(a: Signal<b8, C>) -> Signal<b8, C> {
        signal(match a.val() {
            ONE => TWO,
            TWO => THREE,
            _ => ONE,
        })
    }
    test_kernel_vm_and_verilog::<add<Red>, _, _, _>(add::<Red>, tuple_b8())?;
    Ok(())
}

#[test]
fn test_const_literal_match_not_raw() {
    #[kernel]
    pub fn kernel(x: Signal<b8, Red>) -> Signal<b3, Red> {
        let x = x.val();
        let y = match x {
            Bits::<8>(0) => b3(0),
            Bits::<8>(1) => b3(1),
            Bits::<8>(2) => b3(1),
            Bits::<8>(3) => b3(2),
            _ => b3(4),
        };
        signal(y)
    }
    test_kernel_vm_and_verilog::<kernel, _, _, _>(kernel, tuple_b8()).unwrap();
}

#[test]
fn test_const_literal_match() {
    #[kernel]
    fn add<C: Domain>(a: Signal<b8, C>) -> Signal<b8, C> {
        signal(b8(match a.val().raw() {
            1 => 1,
            2 => 2,
            _ => 3,
        }))
    }
    test_kernel_vm_and_verilog::<add<Red>, _, _, _>(add::<Red>, tuple_b8()).unwrap();
}

#[test]
fn test_const_literal_captured_match() {
    const ZERO: b4 = bits(0);
    const ONE: b4 = bits(1);
    const TWO: b4 = bits(2);

    #[kernel]
    fn do_stuff(a: Signal<b4, Red>) -> Signal<b4, Red> {
        signal(match a.val() {
            ONE => TWO,
            TWO => ONE,
            _ => ZERO,
        })
    }

    test_kernel_vm_and_verilog::<do_stuff, _, _, _>(do_stuff, tuple_exhaustive_red()).unwrap();
}

// This test is disabled until we either adopt custom suffixes or do some other thing
// to re-enable the ability to use literals in match arms.
#[test]
fn test_struct_literal_match() -> miette::Result<()> {
    #[derive(PartialEq, Debug, Digital, Clone, Copy)]
    pub struct Foo {
        a: b8,
        b: b8,
    }

    const FOO1: Foo = Foo {
        a: bits(1),
        b: bits(2),
    };

    const FOO2: Foo = Foo {
        a: bits(3),
        b: bits(4),
    };

    #[kernel]
    fn add(a: Signal<Foo, Red>) -> Signal<b8, Red> {
        let res = match a.val() {
            FOO1 => 1,
            FOO2 => 2,
            _ => 3,
        };
        signal(bits(res))
    }

    let test_vec = (0..4)
        .map(b8)
        .flat_map(|a| (0..4).map(b8).map(move |b| (red(Foo { a, b }),)))
        .collect::<Vec<_>>();
    test_kernel_vm_and_verilog::<add, _, _, _>(add, test_vec.into_iter())?;
    Ok(())
}

#[test]
fn test_plain_literals() -> miette::Result<()> {
    #[kernel]
    fn foo(a: Signal<b6, Red>, b: Signal<b6, Red>) -> Signal<b6, Red> {
        signal((a.val() + 2 + b.val()).resize())
    }

    test_kernel_vm_and_verilog::<foo, _, _, _>(foo, tuple_pair_bn_red::<6>())?;
    Ok(())
}

#[test]
fn test_plain_literals_signed_context() {
    #[kernel]
    fn foo(a: Signal<s6, Red>, b: Signal<s6, Red>) -> Signal<s6, Red> {
        signal(a.val() + 2 + b.val())
    }

    test_kernel_vm_and_verilog::<foo, _, _, _>(foo, tuple_pair_sn_red::<6>()).unwrap();
}

/// Comparing a `SignedBits<N>` against a literal must be a **signed**
/// comparison in the emitted Verilog as well as in the VM.
///
/// `doc/book/src/bits/comparison.md` already states this: "RHDL will
/// generate hardware descriptions for the comparison operators that
/// includes the appropriate sign handling if the operands are signed",
/// and the note below it documents comparing a bitvector against a
/// literal as supported. The implementation did not deliver it: the
/// literal was emitted as an unsigned Verilog constant, and IEEE 1364
/// §5.5.1 makes a relational expression unsigned if *either* operand
/// is unsigned. Every negative value therefore compared greater than a
/// positive bound.
///
/// `test_kernel_vm_and_verilog` is the right harness precisely because
/// the defect was invisible to Rust-level simulation -- the kernel run
/// as a Rust function compares correctly. Only cross-checking the VM
/// against emitted Verilog catches it. Exhaustive over all 256 values
/// of `s8`, which spans both signs.
#[test]
fn test_signed_comparison_against_literal() -> miette::Result<()> {
    #[kernel]
    fn cmp<C: Domain>(a: Signal<s8, C>) -> Signal<(bool, bool, bool, bool), C> {
        let a = a.val();
        signal((
            a > signed::<8>(10),
            a < signed::<8>(10),
            a >= signed::<8>(-5),
            a <= signed::<8>(-5),
        ))
    }
    test_kernel_vm_and_verilog::<cmp<Red>, _, _, _>(cmp::<Red>, s8_red())?;
    Ok(())
}

/// The negative case: an **unsigned** comparison against a literal must
/// stay unsigned.
///
/// The fix keys off the literal's kind, so a change that made every
/// literal signed would pass the test above and silently break every
/// unsigned comparison in the tree -- of which there are over a hundred
/// in `rhdl-fpga` alone. Exhaustive over all 256 values of `b8`, where
/// the top half is exactly where signed and unsigned disagree.
#[test]
fn test_unsigned_comparison_against_literal_unaffected() -> miette::Result<()> {
    #[kernel]
    fn cmp<C: Domain>(a: Signal<b8, C>) -> Signal<(bool, bool), C> {
        let a = a.val();
        signal((a > bits::<8>(200), a < bits::<8>(200)))
    }
    test_kernel_vm_and_verilog::<cmp<Red>, _, _, _>(cmp::<Red>, tuple_b8())?;
    Ok(())
}
