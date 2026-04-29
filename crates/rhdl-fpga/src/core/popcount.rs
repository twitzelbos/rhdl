//! Population count (popcount)
//!
//! Pure combinational `popcount`: counts the number of set bits in
//! an `N`-bit input.  Foundation for ECC syndrome weighting,
//! hash-table sizing, normalization, ML inference (e.g. binary
//! neural-net activation counts), and as a primitive in
//! protocol-level CRC checks.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +--+Popcount+--+
     |              |
B<N> |              | B<W>
+--->| input  count +--->
     |              |
     +--------------+
")]
//!
//!# Internals
//!
//! Unrolls a constant-bound `for` loop over the `N` input bits and
//! accumulates a `Bits<W>` counter.  The synthesizer turns this into
//! a small adder tree.  For very wide inputs, the tree depth grows
//! as `O(log N)` — pipeline downstream consumers if you need high
//! throughput at large `N`.
//!
//!# Parameters
//!
//! - `N` — width of the input bit vector
//! - `W` — width of the output count, satisfying `2^W > N` (so the
//!   maximum count of `N` is representable)
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/popcount.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/popcount.md")]

use rhdl::prelude::*;

#[kernel]
/// Popcount kernel — number of set bits in `input`.
pub fn popcount<const N: usize, const W: usize>(input: Bits<N>) -> Bits<W>
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth,
{
    let mut count: Bits<W> = bits(0);
    for i in 0..N {
        let bit_i = (input >> (i as u128)) & bits(1);
        let one_w: Bits<W> = if bit_i != bits(0) { bits(1) } else { bits(0) };
        count += one_w;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;
    use std::path::PathBuf;

    // Tier 1 — direct kernel unit tests

    #[test]
    fn test_zero_input_is_zero() {
        assert_eq!(popcount::<8, 4>(bits(0)), bits(0));
    }

    #[test]
    fn test_all_ones_is_n() {
        assert_eq!(popcount::<8, 4>(bits(0xFF)), bits(8));
    }

    #[test]
    fn test_single_bit_is_one() {
        for i in 0u128..8 {
            assert_eq!(popcount::<8, 4>(bits(1 << i)), bits(1), "bit {i}");
        }
    }

    #[test]
    fn test_alternating_patterns() {
        assert_eq!(popcount::<8, 4>(bits(0b1010_1010)), bits(4));
        assert_eq!(popcount::<8, 4>(bits(0b0101_0101)), bits(4));
        assert_eq!(popcount::<8, 4>(bits(0b1111_0000)), bits(4));
    }

    #[test]
    fn test_exhaustive_8bit_matches_rust_count_ones() {
        for v in 0u128..256 {
            let expected = (v as u8).count_ones() as u128;
            assert_eq!(popcount::<8, 4>(bits(v)).raw(), expected, "input {v:#010b}");
        }
    }

    // Tier 3+4 — kernel VM + Verilog cross-validation
    #[test]
    fn test_popcount_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = (0u128..256).map(|v| (bits::<8>(v),));
        test_kernel_vm_and_verilog_synchronous::<popcount<8, 4>, _, _, _>(
            popcount::<8, 4>,
            inputs,
        )?;
        Ok(())
    }

    // Tier 5 — VCD digest via Func wrapper
    #[test]
    fn test_popcount_trace() -> miette::Result<()> {
        #[kernel]
        fn wrap(_cr: ClockReset, input: Bits<8>) -> Bits<4> {
            popcount::<8, 4>(input)
        }
        let uut: Func<Bits<8>, Bits<4>> = Func::try_new::<wrap>()?;
        let inputs = [
            bits(0b0000_0000),
            bits(0b0000_0001),
            bits(0b0000_0011),
            bits(0b0000_0111),
            bits(0b0000_1111),
            bits(0b0001_1111),
            bits(0b0011_1111),
            bits(0b0111_1111),
            bits(0b1111_1111),
            bits(0b1010_1010),
        ]
        .into_iter()
        .with_reset(1)
        .clock_pos_edge(100);
        let vcd = uut.run(inputs).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("popcount");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["cf8b46d51ab164de873e936a30250b0e6358132eb8a16c4dee4fd0eb4980c5d5"];
        let digest = vcd.dump_to_file(root.join("popcount.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
