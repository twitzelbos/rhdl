//! One-hot encoders and decoders
//!
//! Pure combinational converters between an `N`-bit one-hot vector
//! and a `W`-bit binary index (`N = 2^W` for a dense decoder; smaller
//! `N` is also legal — the unused indices simply never appear).
//!
//! These are the bread-and-butter primitives behind register-file
//! address decode, demultiplexers, state-machine indicators, and the
//! grant fan-out half of an arbiter.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +-+BinaryToOneHot+-+         +-+OneHotToBinary+-+
     |                  |         |                  |
B<W> |                  | B<N> B<N>|                  | B<W>
+--->| index    one_hot +--->  +-->| one_hot    index +--->
     |                  |         |                  |
     +------------------+         +------------------+
")]
//!
//!# Internals
//!
//! [binary_to_one_hot] is a single shift: `1 << index`.  This
//! synthesizes to a small decoder tree.
//!
//! [one_hot_to_binary] unrolls a `for` loop and OR-accumulates the
//! indices of set bits.  For a strict one-hot input (exactly one bit
//! set) the result is the bit's index.  For an all-zeros input the
//! result is zero.  The behavior on multi-hot input is unspecified —
//! the OR of all set indices is what falls out, but callers should not
//! rely on that.  Use a [super::priority_encoder] when the input may
//! have multiple bits set.
//!
//!# Parameters
//!
//! - `N` — width of the one-hot vector
//! - `W` — width of the binary index, which must satisfy
//!   `2^W >= N` so every legal index is representable
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/one_hot.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/one_hot.md")]

use rhdl::prelude::*;

#[kernel]
/// Binary-to-one-hot decoder.
///
/// Given a `W`-bit binary `index`, produces an `N`-bit vector with
/// exactly bit `index` set.  When `index >= N` the output is zero
/// (the high-order bits of the shift fall off the top of the
/// `Bits<N>`).
pub fn binary_to_one_hot<const W: usize, const N: usize>(index: Bits<W>) -> Bits<N>
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<N>: BitWidth,
{
    bits::<N>(1) << index
}

#[kernel]
/// One-hot-to-binary encoder.
///
/// Given an `N`-bit one-hot input, returns the index of the set bit.
/// Behavior:
///
/// - Strict one-hot (exactly one bit set): returns that bit's index.
/// - All-zeros input: returns `0`.
/// - Multi-hot input: returns the bitwise OR of all set indices —
///   unspecified contract; use [super::priority_encoder] instead when
///   the input may have multiple bits set.
pub fn one_hot_to_binary<const N: usize, const W: usize>(input: Bits<N>) -> Bits<W>
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth,
{
    let mut result: Bits<W> = bits(0);
    for i in 0..N {
        let bit_i = (input >> (i as u128)) & bits(1);
        if bit_i != bits(0) {
            result |= bits(i as u128);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;
    use std::path::PathBuf;

    // Tier 1 — direct kernel unit tests for binary_to_one_hot

    #[test]
    fn test_b2oh_each_index_sets_correct_bit() {
        for i in 0u128..8 {
            let result = binary_to_one_hot::<3, 8>(bits(i));
            assert_eq!(result, bits(1 << i), "index {i}");
        }
    }

    #[test]
    fn test_b2oh_index_at_or_above_n_is_zero() {
        // For W=3, N=4, indices 4..7 are out of range.
        for i in 4u128..8 {
            let result = binary_to_one_hot::<3, 4>(bits(i));
            assert_eq!(result, bits(0), "index {i}");
        }
    }

    // Tier 1 — direct kernel unit tests for one_hot_to_binary

    #[test]
    fn test_oh2b_each_set_bit_returns_its_index() {
        for i in 0u128..8 {
            let result = one_hot_to_binary::<8, 3>(bits(1 << i));
            assert_eq!(result, bits(i), "bit {i}");
        }
    }

    #[test]
    fn test_oh2b_zero_input_returns_zero() {
        assert_eq!(one_hot_to_binary::<8, 3>(bits(0)), bits(0));
    }

    #[test]
    fn test_oh2b_multi_hot_returns_or_of_indices() {
        // Documented unspecified behavior: bits 1 and 2 set → 1 | 2 = 3.
        assert_eq!(one_hot_to_binary::<8, 3>(bits(0b0000_0110)), bits(3));
        // bits 0 and 5 set → 0 | 5 = 5.
        assert_eq!(one_hot_to_binary::<8, 3>(bits(0b0010_0001)), bits(5));
    }

    // Round-trip property: one_hot_to_binary . binary_to_one_hot == id
    #[test]
    fn test_round_trip_binary_through_one_hot() {
        for i in 0u128..8 {
            let oh = binary_to_one_hot::<3, 8>(bits(i));
            let back = one_hot_to_binary::<8, 3>(oh);
            assert_eq!(back, bits(i), "index {i}");
        }
    }

    // Tier 3+4 — kernel VM + Verilog cross-validation
    #[test]
    fn test_b2oh_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = (0u128..8).map(|v| (bits::<3>(v),));
        test_kernel_vm_and_verilog_synchronous::<binary_to_one_hot<3, 8>, _, _, _>(
            binary_to_one_hot::<3, 8>,
            inputs,
        )?;
        Ok(())
    }

    #[test]
    fn test_oh2b_kernel_vm_and_verilog() -> miette::Result<()> {
        // Sweep all 256 inputs (including non-one-hot) so the VM and
        // Verilog must agree on the documented multi-hot behavior too.
        let inputs = (0u128..256).map(|v| (bits::<8>(v),));
        test_kernel_vm_and_verilog_synchronous::<one_hot_to_binary<8, 3>, _, _, _>(
            one_hot_to_binary::<8, 3>,
            inputs,
        )?;
        Ok(())
    }

    // Tier 5 — VCD digest
    #[test]
    fn test_one_hot_trace() -> miette::Result<()> {
        #[kernel]
        fn wrap(_cr: ClockReset, idx: Bits<3>) -> Bits<8> {
            binary_to_one_hot::<3, 8>(idx)
        }
        let uut: Func<Bits<3>, Bits<8>> = Func::try_new::<wrap>()?;
        let inputs = (0u128..8)
            .map(bits)
            .collect::<Vec<_>>()
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);
        let vcd = uut.run(inputs).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("one_hot");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["b07b93cbeaf96ad41cd9c850e628a13a4f7b03a270feb217a840bdecfc868807"];
        let digest = vcd.dump_to_file(root.join("one_hot.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
