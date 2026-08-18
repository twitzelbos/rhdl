//! Barrel shifter
//!
//! Pure combinational variable-amount shifter / rotator over an
//! `N`-bit word.  Supports five operations selected by the
//! [ShiftOp] enum:
//!
//! - `LogicalLeft` (`LSL`): shift left, fill with zeros.
//! - `LogicalRight` (`LSR`): shift right, fill with zeros.
//! - `ArithmeticRight` (`ASR`): shift right, sign-extend with the input's MSB.
//! - `RotateLeft` (`ROL`): rotate left.
//! - `RotateRight` (`ROR`): rotate right.
//!
//! `LSL` and `LSR` are thin wrappers over the built-in `Bits<N>::<<`
//! and `>>` operators.  `ASR`, `ROL`, and `ROR` are the operations
//! that justify having a named widget — they are not directly
//! provided by `Bits<N>`, and they keep the same naming convention
//! as the LSL/LSR cases so calling code can treat all five
//! uniformly.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +---+BarrelShifter+---+
     |                     |
B<N> |                     |
+--->| data                | B<N>
     |              result +--->
B<W> |                     |
+--->| amount              |
     |                     |
?Mode|                     |
+--->| op                  |
     +---------------------+
")]
//!
//!# Internals
//!
//! Each variant lowers to the obvious combinational circuit:
//!
//! - `LSL`/`LSR`: a single barrel mux tree (the synthesizer will
//!   build a `log2(N)`-deep mux for variable shifts).
//! - `ASR`: `LSR | sign_extend_mask`, where the mask covers the top
//!   `amount` bits when the input MSB is set.
//! - `ROL`/`ROR`: `(data << amount) | (data >> (N - amount))` with
//!   the mirror image for `ROR`.
//!
//! `amount` must be in `[0, N)`.  The kernel VM enforces
//! `shift < N` for the built-in shift operators, so passing an
//! out-of-range `amount` will trip a runtime error during simulation
//! and an undefined value during synthesis.  Callers should mask
//! `amount` to `[0, N)` before calling.  For widely-spaced rotates
//! (where `amount` could exceed `N`), pre-reduce by `amount % N`.
//!
//!# Parameters
//!
//! - `N` — width of the data word
//! - `W` — width of the shift-amount input.  Must be wide enough to
//!   represent values up to `N` (so `2^W >= N + 1` for the rotate
//!   variants, which compute `N - amount` internally).
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/barrel_shifter.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/barrel_shifter.md")]

use rhdl::prelude::*;

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// Operation selector for [barrel_shifter].
pub enum ShiftOp {
    /// Logical left shift — fill with zeros.
    #[default]
    LogicalLeft,
    /// Logical right shift — fill with zeros.
    LogicalRight,
    /// Arithmetic right shift — sign-extend with the input MSB.
    ArithmeticRight,
    /// Rotate left.
    RotateLeft,
    /// Rotate right.
    RotateRight,
}

#[kernel]
/// Variable-amount shift / rotate over `N`-bit data.
pub fn barrel_shifter<const N: usize, const W: usize>(
    data: Bits<N>,
    amount: Bits<W>,
    op: ShiftOp,
) -> Bits<N>
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth,
{
    let n_minus_amount: Bits<W> = bits(N as u128) - amount;
    let sign_bit = (data >> ((N - 1) as u128)) & bits(1);
    // amount == 0 is a special case for everything except plain LSL/LSR:
    // the rotate and ASR formulas would shift by `n_minus_amount = N`,
    // which the kernel VM rejects (shift must be strictly less than N).
    //
    // Kernel `if/else` lowers to a combinational mux — *both branches*
    // are always evaluated.  So we cannot just guard the unused branch
    // with `if is_zero { ... } else { shift_by_N_branch }`; the VM would
    // still execute the shift-by-N inside `else`.  Instead, we clamp
    // the shift amount itself to `[0, N-1]` and let the mux pick the
    // logically-correct result.
    let is_zero = amount == bits(0);
    let safe_n_minus: Bits<W> = if is_zero { bits(0) } else { n_minus_amount };
    match op {
        ShiftOp::LogicalLeft => data << amount,
        ShiftOp::LogicalRight => data >> amount,
        ShiftOp::ArithmeticRight => {
            let lsr = data >> amount;
            // OR-in a sign-extension mask covering the top `amount` bits
            // when the input MSB is 1.  Mask = NOT((1 << (N-amount)) - 1).
            let lower_mask = (bits::<N>(1) << safe_n_minus) - bits(1);
            let upper_mask = !lower_mask;
            let sign_extend: Bits<N> = if sign_bit != bits(0) {
                upper_mask
            } else {
                bits(0)
            };
            let asr_with_extend = lsr | sign_extend;
            // amount == 0: no extension; just return LSR (= data).
            if is_zero { lsr } else { asr_with_extend }
        }
        ShiftOp::RotateLeft => {
            let shifted = (data << amount) | (data >> safe_n_minus);
            if is_zero { data } else { shifted }
        }
        ShiftOp::RotateRight => {
            let shifted = (data >> amount) | (data << safe_n_minus);
            if is_zero { data } else { shifted }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;
    use std::path::PathBuf;

    // Tier 1 — direct kernel unit tests

    #[test]
    fn test_lsl_zero_amount_is_identity() {
        for v in [0u128, 0xA5, 0xFF, 0x80] {
            assert_eq!(
                barrel_shifter::<8, 4>(bits(v), bits(0), ShiftOp::LogicalLeft),
                bits(v)
            );
        }
    }

    #[test]
    fn test_lsl_shifts_in_zeros() {
        // 0xA5 << 4 = 0x50 (high nibble dropped, zeros in low)
        assert_eq!(
            barrel_shifter::<8, 4>(bits(0xA5), bits(4), ShiftOp::LogicalLeft),
            bits(0x50)
        );
    }

    #[test]
    fn test_lsr_shifts_in_zeros() {
        assert_eq!(
            barrel_shifter::<8, 4>(bits(0xA5), bits(4), ShiftOp::LogicalRight),
            bits(0x0A)
        );
    }

    #[test]
    fn test_asr_extends_sign() {
        // 0xA5 = 0b1010_0101 (negative as i8). ASR by 4 → 0b1111_1010 = 0xFA.
        assert_eq!(
            barrel_shifter::<8, 4>(bits(0xA5), bits(4), ShiftOp::ArithmeticRight),
            bits(0xFA)
        );
        // 0x55 = 0b0101_0101 (positive). ASR by 4 → 0x05 (no sign ext).
        assert_eq!(
            barrel_shifter::<8, 4>(bits(0x55), bits(4), ShiftOp::ArithmeticRight),
            bits(0x05)
        );
        // ASR by 0 = identity.
        assert_eq!(
            barrel_shifter::<8, 4>(bits(0xA5), bits(0), ShiftOp::ArithmeticRight),
            bits(0xA5)
        );
    }

    #[test]
    fn test_rol_wraps_high_bit_into_low() {
        // 0xA5 = 0b1010_0101. ROL by 1 → 0b0100_1011 = 0x4B.
        assert_eq!(
            barrel_shifter::<8, 4>(bits(0xA5), bits(1), ShiftOp::RotateLeft),
            bits(0x4B)
        );
        // ROL by 4 → swap nibbles: 0xA5 → 0x5A.
        assert_eq!(
            barrel_shifter::<8, 4>(bits(0xA5), bits(4), ShiftOp::RotateLeft),
            bits(0x5A)
        );
        // ROL by 0 = identity.
        assert_eq!(
            barrel_shifter::<8, 4>(bits(0xA5), bits(0), ShiftOp::RotateLeft),
            bits(0xA5)
        );
    }

    #[test]
    fn test_ror_wraps_low_bit_into_high() {
        // ROR by 1 of 0xA5 = 0b1010_0101 → 0b1101_0010 = 0xD2.
        assert_eq!(
            barrel_shifter::<8, 4>(bits(0xA5), bits(1), ShiftOp::RotateRight),
            bits(0xD2)
        );
        // ROR by 4 = swap nibbles: 0xA5 → 0x5A (matches ROL by 4 for 8-bit).
        assert_eq!(
            barrel_shifter::<8, 4>(bits(0xA5), bits(4), ShiftOp::RotateRight),
            bits(0x5A)
        );
    }

    #[test]
    fn test_rotates_compose_to_identity() {
        // ROL by k then ROR by k = identity, for k in [0, N).
        for k in 0u128..8 {
            let v = bits::<8>(0xA5);
            let rol = barrel_shifter::<8, 4>(v, bits(k), ShiftOp::RotateLeft);
            let back = barrel_shifter::<8, 4>(rol, bits(k), ShiftOp::RotateRight);
            assert_eq!(back, v, "rotate by {k} round-trip");
        }
    }

    #[test]
    fn test_lsl_exhaustive_matches_rust_shift() {
        // amount in [0, N-1] — kernel VM rejects amount == N for shift.
        for v in 0u128..256 {
            for k in 0u128..8 {
                let expected = (v as u8).wrapping_shl(k as u32) as u128;
                assert_eq!(
                    barrel_shifter::<8, 4>(bits(v), bits(k), ShiftOp::LogicalLeft).raw(),
                    expected,
                    "{v:#x} << {k}"
                );
            }
        }
    }

    #[test]
    fn test_lsr_exhaustive_matches_rust_shift() {
        for v in 0u128..256 {
            for k in 0u128..8 {
                let expected = (v as u8).wrapping_shr(k as u32) as u128;
                assert_eq!(
                    barrel_shifter::<8, 4>(bits(v), bits(k), ShiftOp::LogicalRight).raw(),
                    expected,
                    "{v:#x} >> {k}"
                );
            }
        }
    }

    // Tier 3+4 — kernel VM + Verilog cross-validation
    #[test]
    fn test_barrel_shifter_kernel_vm_and_verilog() -> miette::Result<()> {
        // amount sweep limited to [0, N-1] = [0, 7] per kernel VM constraint.
        let mut inputs = Vec::new();
        for &v in &[0x00u128, 0x01, 0x55, 0xAA, 0xFF, 0x80, 0x7F] {
            for k in 0u128..8 {
                for &op in &[
                    ShiftOp::LogicalLeft,
                    ShiftOp::LogicalRight,
                    ShiftOp::ArithmeticRight,
                    ShiftOp::RotateLeft,
                    ShiftOp::RotateRight,
                ] {
                    inputs.push((bits::<8>(v), bits::<4>(k), op));
                }
            }
        }
        test_kernel_vm_and_verilog_synchronous::<barrel_shifter<8, 4>, _, _, _>(
            barrel_shifter::<8, 4>,
            inputs.into_iter(),
        )?;
        Ok(())
    }

    // Tier 5 — VCD digest via Func wrapper
    #[test]
    fn test_barrel_shifter_trace() -> miette::Result<()> {
        #[derive(PartialEq, Debug, Digital, Clone, Copy)]
        struct In {
            data: Bits<8>,
            amount: Bits<4>,
            op: ShiftOp,
        }
        #[kernel]
        fn wrap(_cr: ClockReset, i: In) -> Bits<8> {
            barrel_shifter::<8, 4>(i.data, i.amount, i.op)
        }
        let uut: Func<In, Bits<8>> = Func::try_new::<wrap>()?;
        let inputs = [
            In {
                data: bits(0xA5),
                amount: bits(0),
                op: ShiftOp::LogicalLeft,
            },
            In {
                data: bits(0xA5),
                amount: bits(4),
                op: ShiftOp::LogicalLeft,
            },
            In {
                data: bits(0xA5),
                amount: bits(4),
                op: ShiftOp::LogicalRight,
            },
            In {
                data: bits(0xA5),
                amount: bits(4),
                op: ShiftOp::ArithmeticRight,
            },
            In {
                data: bits(0xA5),
                amount: bits(1),
                op: ShiftOp::RotateLeft,
            },
            In {
                data: bits(0xA5),
                amount: bits(4),
                op: ShiftOp::RotateLeft,
            },
            In {
                data: bits(0xA5),
                amount: bits(1),
                op: ShiftOp::RotateRight,
            },
        ]
        .into_iter()
        .with_reset(1)
        .clock_pos_edge(100);
        let vcd = uut.run(inputs).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("barrel_shifter");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["c285319896c44e5fae5577e3334da506ce51279bcd54d945ee434380d14002dc"];
        let digest = vcd.dump_to_file(root.join("barrel_shifter.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
