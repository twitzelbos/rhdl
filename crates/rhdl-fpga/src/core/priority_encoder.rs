//! Priority encoder
//!
//! Pure combinational priority encoders.  Given an `N`-bit input
//! vector, return the index of the lowest- (or highest-) set bit, or
//! `None` if no bits are set.  These functions are the foundation of
//! arbiters, interrupt controllers, leading-zero counters, and one-hot
//! address decoders.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +--+PriorityEncoder+--+
     |                     |
B<N> |                     | Option<B<W>>
+--->| input         index +--->
     |                     |
     +---------------------+
")]
//!
//!# Internals
//!
//! Each encoder unrolls a constant-bound `for` loop.  Two `mut`
//! locals — a `found` flag and an `idx` accumulator — track the
//! priority decision through the unrolled iterations.  The compiler
//! synthesizes this into a chain of priority muxes.  For wide inputs,
//! consider pipelining downstream consumers since the carry chain is
//! `O(N)` in depth.
//!
//!# Parameters
//!
//! - `N` — width of the input bit vector
//! - `W` — width of the output index, which must satisfy
//!   `2^W >= N` so every valid index is representable
//!
//! The `W` parameter is independent so users can pick whatever width
//! their downstream consumers need.  The smallest legal value is
//! `ceil(log2(N))`.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/priority_encoder.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/priority_encoder.md")]

use rhdl::prelude::*;

#[kernel]
/// Priority encoder — index of the **least-significant** set bit.
///
/// Returns `None` if `input == 0`, otherwise `Some(idx)` where `idx`
/// is the index of the lowest-numbered bit that is set.
pub fn priority_encoder_lsb<const N: usize, const W: usize>(input: Bits<N>) -> Option<Bits<W>>
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth,
{
    let mut idx: Bits<W> = bits(0);
    let mut found = false;
    for i in 0..N {
        let bit_i = (input >> (i as u128)) & bits(1);
        if bit_i != bits(0) && !found {
            idx = bits(i as u128);
            found = true;
        }
    }
    if found {
        Some(idx)
    } else {
        None
    }
}

#[kernel]
/// Priority encoder — index of the **most-significant** set bit.
///
/// Returns `None` if `input == 0`, otherwise `Some(idx)` where `idx`
/// is the index of the highest-numbered bit that is set.  Useful as a
/// leading-bit-position primitive (e.g. for fixed-point normalization
/// or wide-comparator early-out).
pub fn priority_encoder_msb<const N: usize, const W: usize>(input: Bits<N>) -> Option<Bits<W>>
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth,
{
    let mut idx: Bits<W> = bits(0);
    let mut found = false;
    // Iterate LSB→MSB; later (higher) matches overwrite earlier ones.
    for i in 0..N {
        let bit_i = (input >> (i as u128)) & bits(1);
        if bit_i != bits(0) {
            idx = bits(i as u128);
            found = true;
        }
    }
    if found {
        Some(idx)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;
    use std::path::PathBuf;

    // Tier 1 — direct kernel unit tests for LSB

    #[test]
    fn test_lsb_zero_input_is_none() {
        assert_eq!(priority_encoder_lsb::<8, 3>(bits(0)), None);
    }

    #[test]
    fn test_lsb_single_bit_returns_its_index() {
        for i in 0..8u128 {
            assert_eq!(
                priority_encoder_lsb::<8, 3>(bits(1 << i)),
                Some(bits(i)),
                "bit {i}"
            );
        }
    }

    #[test]
    fn test_lsb_picks_lowest_when_multiple_set() {
        // 0b1010 -> lowest set is bit 1
        assert_eq!(priority_encoder_lsb::<8, 3>(bits(0b1010)), Some(bits(1)));
        // 0b1100 -> lowest set is bit 2
        assert_eq!(priority_encoder_lsb::<8, 3>(bits(0b1100)), Some(bits(2)));
        // 0b1111_0000 -> lowest set is bit 4
        assert_eq!(
            priority_encoder_lsb::<8, 3>(bits(0b1111_0000)),
            Some(bits(4))
        );
    }

    #[test]
    fn test_lsb_all_ones_picks_zero() {
        assert_eq!(priority_encoder_lsb::<8, 3>(bits(0xFF)), Some(bits(0)));
    }

    // Tier 1 — direct kernel unit tests for MSB

    #[test]
    fn test_msb_zero_input_is_none() {
        assert_eq!(priority_encoder_msb::<8, 3>(bits(0)), None);
    }

    #[test]
    fn test_msb_single_bit_returns_its_index() {
        for i in 0..8u128 {
            assert_eq!(
                priority_encoder_msb::<8, 3>(bits(1 << i)),
                Some(bits(i)),
                "bit {i}"
            );
        }
    }

    #[test]
    fn test_msb_picks_highest_when_multiple_set() {
        assert_eq!(priority_encoder_msb::<8, 3>(bits(0b1010)), Some(bits(3)));
        assert_eq!(priority_encoder_msb::<8, 3>(bits(0b0011)), Some(bits(1)));
        assert_eq!(
            priority_encoder_msb::<8, 3>(bits(0b0011_1100)),
            Some(bits(5))
        );
    }

    #[test]
    fn test_msb_all_ones_picks_top() {
        assert_eq!(priority_encoder_msb::<8, 3>(bits(0xFF)), Some(bits(7)));
    }

    // Exhaustive Tier 1 sweep for an 8-bit input.
    #[test]
    fn test_lsb_exhaustive_8bit_matches_reference() {
        for v in 0u128..256 {
            let expected = if v == 0 {
                None
            } else {
                Some(v.trailing_zeros() as u128)
            };
            assert_eq!(
                priority_encoder_lsb::<8, 3>(bits(v)).map(|b| b.raw()),
                expected,
                "input {v:#010b}"
            );
        }
    }

    #[test]
    fn test_msb_exhaustive_8bit_matches_reference() {
        for v in 0u128..256 {
            let expected = if v == 0 {
                None
            } else {
                Some(127 - v.leading_zeros() as u128)
            };
            assert_eq!(
                priority_encoder_msb::<8, 3>(bits(v)).map(|b| b.raw()),
                expected,
                "input {v:#010b}"
            );
        }
    }

    // Tier 3+4 — kernel VM and Verilog cross-validation
    // (test_kernel_vm_and_verilog_synchronous compiles the kernel to
    // Verilog and runs both sims, comparing outputs at every input).

    #[test]
    fn test_lsb_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = (0u128..256).map(|v| (bits::<8>(v),));
        test_kernel_vm_and_verilog_synchronous::<priority_encoder_lsb<8, 3>, _, _, _>(
            priority_encoder_lsb::<8, 3>,
            inputs,
        )?;
        Ok(())
    }

    #[test]
    fn test_msb_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = (0u128..256).map(|v| (bits::<8>(v),));
        test_kernel_vm_and_verilog_synchronous::<priority_encoder_msb<8, 3>, _, _, _>(
            priority_encoder_msb::<8, 3>,
            inputs,
        )?;
        Ok(())
    }

    // Tier 5 — VCD digest from the Func-wrapped variant used in the example.
    // See examples/priority_encoder.rs for the wrapper definition.
    #[test]
    fn test_priority_encoder_trace() -> miette::Result<()> {
        #[kernel]
        fn wrap_pe(_cr: ClockReset, input: Bits<8>) -> Option<Bits<3>> {
            priority_encoder_lsb::<8, 3>(input)
        }
        let uut: Func<Bits<8>, Option<Bits<3>>> = Func::try_new::<wrap_pe>()?;
        let inputs = [
            bits(0b0000_0000),
            bits(0b0000_0001),
            bits(0b0000_0010),
            bits(0b0000_1000),
            bits(0b1000_0000),
            bits(0b1010_1010),
            bits(0b0101_0101),
            bits(0b1111_0000),
        ]
        .into_iter()
        .with_reset(1)
        .clock_pos_edge(100);
        let vcd = uut.run(inputs).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("priority_encoder");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["c4f2ed7bda598958daa3b2da62b0b35927f83549b8917896f25cf085cf2e017f"];
        let digest = vcd.dump_to_file(root.join("priority_encoder.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
