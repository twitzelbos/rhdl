//! Leading-zero count
//!
//! Pure combinational `clz`: counts the number of leading zero bits
//! in an `N`-bit input, scanning from the most-significant bit.
//! Returns `0` when the MSB is set, `N` when the input is all
//! zeros, and the position of the highest set bit otherwise.
//!
//! Foundation for floating- and fixed-point normalization, dynamic
//! range estimation in DSP, integer-to-float conversion, and the
//! "find leading 1" operations that underlie variable-shift
//! pre-aligners.
//!
//! Closely related to [super::priority_encoder::priority_encoder_msb]
//! — `clz(x) = N - 1 - msb(x)` when `x != 0`, and `clz(0) = N`.  The
//! kernel is implemented inline rather than as a wrapper so the
//! all-zeros special case stays cheap and the synthesized adder tree
//! is bounded.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +--+LeadingZeros+--+
     |                  |
B<N> |                  | B<W>
+--->| input        clz +--->
     |                  |
     +------------------+
")]
//!
//!# Internals
//!
//! Unrolls a constant-bound `for` loop scanning from MSB to LSB and
//! tracks the position of the first set bit with a `mut` flag and
//! accumulator (same idiom as
//! [super::priority_encoder::priority_encoder_lsb]).
//!
//!# Parameters
//!
//! - `N` — width of the input bit vector
//! - `W` — width of the output count, satisfying `2^W > N` (so the
//!   maximum count of `N`, returned for the all-zeros input, is
//!   representable)
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/leading_zeros.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/leading_zeros.md")]

use rhdl::prelude::*;

#[kernel]
/// Leading-zero count kernel.
///
/// For a non-zero input, returns the number of zero bits before the
/// highest set bit.  For the all-zeros input, returns `N`.
pub fn leading_zeros<const N: usize, const W: usize>(input: Bits<N>) -> Bits<W>
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth,
{
    let mut clz: Bits<W> = bits(N as u128);
    let mut found = false;
    // Iterate i = 0..N representing scan position from the MSB.
    // i = 0 is the MSB; i = N-1 is the LSB.
    for i in 0..N {
        let bit_pos = (N - 1 - i) as u128;
        let bit_v = (input >> bit_pos) & bits(1);
        if bit_v != bits(0) && !found {
            clz = bits(i as u128);
            found = true;
        }
    }
    clz
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;
    use std::path::PathBuf;

    // Tier 1 — direct kernel unit tests

    #[test]
    fn test_zero_input_returns_n() {
        assert_eq!(leading_zeros::<8, 4>(bits(0)), bits(8));
    }

    #[test]
    fn test_msb_set_returns_zero() {
        assert_eq!(leading_zeros::<8, 4>(bits(0b1000_0000)), bits(0));
        assert_eq!(leading_zeros::<8, 4>(bits(0b1111_1111)), bits(0));
    }

    #[test]
    fn test_lsb_only_returns_n_minus_one() {
        assert_eq!(leading_zeros::<8, 4>(bits(0b0000_0001)), bits(7));
    }

    #[test]
    fn test_each_single_bit() {
        // Bit i set (0..=7) → clz = 7 - i.
        for i in 0u128..8 {
            let v = 1u128 << i;
            let expected = 7 - i;
            assert_eq!(
                leading_zeros::<8, 4>(bits(v)),
                bits(expected),
                "single bit at position {i}"
            );
        }
    }

    #[test]
    fn test_exhaustive_8bit_matches_rust_leading_zeros() {
        for v in 0u128..256 {
            let expected = if v == 0 {
                8u128
            } else {
                // u8::leading_zeros() returns leading zeros in a u8.
                (v as u8).leading_zeros() as u128
            };
            assert_eq!(
                leading_zeros::<8, 4>(bits(v)).raw(),
                expected,
                "input {v:#010b}"
            );
        }
    }

    // Tier 3+4 — kernel VM + Verilog cross-validation
    #[test]
    fn test_clz_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = (0u128..256).map(|v| (bits::<8>(v),));
        test_kernel_vm_and_verilog_synchronous::<leading_zeros<8, 4>, _, _, _>(
            leading_zeros::<8, 4>,
            inputs,
        )?;
        Ok(())
    }

    // Tier 5 — VCD digest via Func wrapper
    #[test]
    fn test_clz_trace() -> miette::Result<()> {
        #[kernel]
        fn wrap(_cr: ClockReset, input: Bits<8>) -> Bits<4> {
            leading_zeros::<8, 4>(input)
        }
        let uut: Func<Bits<8>, Bits<4>> = Func::try_new::<wrap>()?;
        let inputs = [
            bits(0b0000_0000),
            bits(0b0000_0001),
            bits(0b0000_0010),
            bits(0b0000_0100),
            bits(0b0001_0000),
            bits(0b0100_0000),
            bits(0b1000_0000),
            bits(0b1111_1111),
        ]
        .into_iter()
        .with_reset(1)
        .clock_pos_edge(100);
        let vcd = uut.run(inputs).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("leading_zeros");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["66dbb08b240a811247bd3d452fad86800e9d53bc03243718d6b6de800e95428b"];
        let digest = vcd.dump_to_file(root.join("leading_zeros.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
