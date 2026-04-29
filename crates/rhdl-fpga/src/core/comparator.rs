//! Wide-bus comparator
//!
//! Pure combinational unsigned comparator that emits all five
//! standard comparison flags (`eq`, `lt`, `le`, `gt`, `ge`) for an
//! `N`-bit pair `(a, b)`.  The built-in `Bits<N>::==` and
//! `Bits<N>::<` operators already cover the bit-level work — this
//! widget exists to (a) provide a single named unit that emits the
//! whole flag set in one call (handy as an arbiter or scheduler
//! sub-block), and (b) be a clear reference for callers building
//! wider or signed variants.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +---+Comparator+---+
     |                  | bool
B<N> |              eq  +--->
+--->| a            lt  +--->
B<N> |              le  +--->
+--->| b            gt  +--->
     |              ge  +--->
     +------------------+
")]
//!
//!# Internals
//!
//! Five combinational outputs derived directly from `Bits<N>`'s
//! built-in equality and unsigned-less-than operators.  No state.
//! For `N` larger than the FPGA's native LUT carry-chain width,
//! the synthesizer will tile the comparison automatically — there
//! is no benefit to manually splitting the comparator widget.
//!
//!# Signed comparison
//!
//! This widget is **unsigned**.  For signed comparison, either
//! convert operands to `SignedBits<N>` before calling, or wrap this
//! widget with sign-bit XOR-and-flip logic.  A signed variant is
//! recorded as a follow-up.
//!
//!# Parameters
//!
//! - `N` — width of the operands
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/comparator.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/comparator.md")]

use rhdl::prelude::*;

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Combined comparison flags for an `(a, b)` pair.
pub struct Flags {
    /// `a == b`
    pub eq: bool,
    /// `a < b` (unsigned)
    pub lt: bool,
    /// `a <= b` (unsigned)
    pub le: bool,
    /// `a > b` (unsigned)
    pub gt: bool,
    /// `a >= b` (unsigned)
    pub ge: bool,
}

#[kernel]
/// Unsigned comparator: emits all five comparison flags at once.
pub fn comparator<const N: usize>(a: Bits<N>, b: Bits<N>) -> Flags
where
    rhdl::bits::W<N>: BitWidth,
{
    let lt = a < b;
    let eq = a == b;
    let mut o = Flags::dont_care();
    o.eq = eq;
    o.lt = lt;
    o.le = lt || eq;
    o.gt = !lt && !eq;
    o.ge = !lt;
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;
    use std::path::PathBuf;

    // Tier 1 — direct kernel unit tests

    #[test]
    fn test_equal_values() {
        let f = comparator::<8>(bits(42), bits(42));
        assert!(f.eq);
        assert!(!f.lt);
        assert!(f.le);
        assert!(!f.gt);
        assert!(f.ge);
    }

    #[test]
    fn test_a_less_than_b() {
        let f = comparator::<8>(bits(7), bits(42));
        assert!(!f.eq);
        assert!(f.lt);
        assert!(f.le);
        assert!(!f.gt);
        assert!(!f.ge);
    }

    #[test]
    fn test_a_greater_than_b() {
        let f = comparator::<8>(bits(200), bits(42));
        assert!(!f.eq);
        assert!(!f.lt);
        assert!(!f.le);
        assert!(f.gt);
        assert!(f.ge);
    }

    #[test]
    fn test_zero_vs_max() {
        let f = comparator::<8>(bits(0), bits(255));
        assert!(f.lt);
        assert!(!f.gt);
        assert!(!f.eq);
    }

    #[test]
    fn test_max_vs_zero() {
        let f = comparator::<8>(bits(255), bits(0));
        assert!(f.gt);
        assert!(!f.lt);
        assert!(!f.eq);
    }

    /// Exhaustive 4-bit sweep, comparing widget output against
    /// straightforward Rust comparisons.
    #[test]
    fn test_exhaustive_4bit_matches_rust() {
        for a in 0u128..16 {
            for b in 0u128..16 {
                let f = comparator::<4>(bits(a), bits(b));
                assert_eq!(f.eq, a == b, "a={a} b={b}");
                assert_eq!(f.lt, a < b, "a={a} b={b}");
                assert_eq!(f.le, a <= b, "a={a} b={b}");
                assert_eq!(f.gt, a > b, "a={a} b={b}");
                assert_eq!(f.ge, a >= b, "a={a} b={b}");
            }
        }
    }

    // Tier 3+4 — kernel VM + Verilog cross-validation
    #[test]
    fn test_comparator_kernel_vm_and_verilog() -> miette::Result<()> {
        // 4-bit exhaustive: 16 × 16 = 256 input pairs.
        let mut inputs: Vec<(Bits<4>, Bits<4>)> = Vec::new();
        for a in 0u128..16 {
            for b in 0u128..16 {
                inputs.push((bits(a), bits(b)));
            }
        }
        test_kernel_vm_and_verilog_synchronous::<comparator<4>, _, _, _>(
            comparator::<4>,
            inputs.into_iter(),
        )?;
        Ok(())
    }

    // Tier 5 — VCD digest via Func wrapper
    #[test]
    fn test_comparator_trace() -> miette::Result<()> {
        #[derive(PartialEq, Debug, Digital, Clone, Copy)]
        struct In {
            a: Bits<8>,
            b: Bits<8>,
        }
        #[kernel]
        fn wrap(_cr: ClockReset, i: In) -> Flags {
            comparator::<8>(i.a, i.b)
        }
        let uut: Func<In, Flags> = Func::try_new::<wrap>()?;
        let inputs = [
            (0u128, 0u128),
            (1, 2),
            (2, 1),
            (42, 42),
            (255, 0),
            (0, 255),
            (100, 100),
            (200, 100),
        ]
        .into_iter()
        .map(|(a, b)| In {
            a: bits(a),
            b: bits(b),
        })
        .collect::<Vec<_>>()
        .into_iter()
        .with_reset(1)
        .clock_pos_edge(100);
        let vcd = uut.run(inputs).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("comparator");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["54378d304682363e9bac26c0542ec4aef34e87a84ae10c4f98bcf580981c626b"];
        let digest = vcd.dump_to_file(root.join("comparator.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
