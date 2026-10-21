//! The Invariant CRC — the field a RoCEv2 implementation gets wrong.
//!
//! The ICRC is a CRC-32 over the whole packet from the IP header
//! onward, with every field a router is permitted to mutate replaced by
//! all-ones, and with eight all-ones bytes prepended where InfiniBand
//! would have a Global Routing Header. Get any part of it wrong and
//! there is no error message: the peer's NIC silently discards the
//! packet.
//!
//! It is also the expensive part of every open implementation. Coyote's
//! `icrc.sv` is 304 KB — the single largest RTL file in its RDMA tree,
//! against 21 KB for the stack wrapper itself — and FERoCE vendors two
//! separate CRC libraries out of three total dependencies. At one byte
//! per cycle none of that applies: the engine is
//! [`rhdl_fpga::core::crc32::crc32_step`] and a 32-bit register.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +------+Icrc+---------+
     |                     |
B<8> |                     |
+--->| byte           crc  +---> B<32>
     |                     |
B<16>|                     |
+--->| frame_index    wire +---> B<32>
     |                     |
bool |                     |
+--->| enable              |
bool |                     |
+--->| start               |
     +---------------------+
")]
//!
//! # What is covered, and what is masked
//!
//! The caller drives frame bytes in order and asserts `enable` over
//! exactly the covered range: from the first byte of the IP header up
//! to, but not including, the 4-byte ICRC field itself. The Ethernet
//! header is **not** covered, and neither is the FCS.
//!
//! Within that range these offsets read as `0xFF` to the CRC, because a
//! router may change them in flight:
//!
//! | Frame offset | Field |
//! |---|---|
//! | 15 | IPv4 TOS (DSCP + ECN) |
//! | 22 | IPv4 TTL |
//! | 24, 25 | IPv4 header checksum |
//! | 40, 41 | UDP checksum |
//! | 46 | BTH `fecn`, `becn` and `resv6` — the whole byte |
//!
//! Those are absolute offsets, which assumes **IPv4 and no VLAN tag**.
//! A VLAN tag shifts everything by four and IPv6 changes both the
//! length and the field set; neither is supported, and
//! [`icrc_mask`] would need the layer offsets as inputs rather than
//! baked in. That is a deliberate limitation of this phase, not an
//! oversight — see the crate docs.
//!
//! # The prefix costs nothing
//!
//! The eight all-ones bytes are constant, so their contribution to the
//! CRC is constant too: feeding them into a fresh CRC-32 leaves the
//! remainder at [`PREFIX_REMAINDER`] every time. Seeding the register
//! with that value is exactly equivalent to streaming them, so this
//! engine has **no startup cycles and no logic for the prefix at all**.
//!
//! The constant turns out to be `0xDEBB20E3`, which is the well-known
//! CRC-32 residue. That is not a coincidence — it is the same algebra
//! read in the other direction — but it is a satisfying check that the
//! arithmetic is right.
//!
//! # Byte order
//!
//! The on-wire ICRC field is the CRC value **little-endian**, which is
//! the opposite of every other multi-byte field in the packet. Rather
//! than leave that as a trap, [`Out`] carries both: `crc` is the value,
//! and `wire` is the same value byte-reversed so that emitting it
//! most-significant-byte-first — like every other header field — puts
//! the right bytes on the wire.
//!
//! # Example
//!
//!```
#![doc = include_str!("../examples/icrc.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../doc/icrc.md")]

use rhdl::prelude::*;
use rhdl_fpga::core::{crc32::crc32_step, dff};

/// The CRC-32 remainder after the ICRC's eight all-ones prefix bytes.
///
/// Equivalently: `crc32_step` applied eight times to `0xFF` starting
/// from `0xFFFFFFFF`, without the final XOR. Seeding the register with
/// this is what lets the prefix cost zero cycles.
pub const PREFIX_REMAINDER: u128 = 0xDEBB_20E3;

/// Frame offset of the IPv4 TOS byte — masked.
pub const OFF_IP_TOS: u128 = 15;
/// Frame offset of the IPv4 TTL byte — masked.
pub const OFF_IP_TTL: u128 = 22;
/// Frame offset of the IPv4 header checksum, high byte — masked.
pub const OFF_IP_CSUM_HI: u128 = 24;
/// Frame offset of the IPv4 header checksum, low byte — masked.
pub const OFF_IP_CSUM_LO: u128 = 25;
/// Frame offset of the UDP checksum, high byte — masked.
pub const OFF_UDP_CSUM_HI: u128 = 40;
/// Frame offset of the UDP checksum, low byte — masked.
pub const OFF_UDP_CSUM_LO: u128 = 41;
/// Frame offset of the BTH byte holding `fecn`, `becn` and `resv6` —
/// masked in its entirety.
pub const OFF_BTH_ECN: u128 = 46;

/// Frame offset at which ICRC coverage begins: the first byte of the
/// IPv4 header, i.e. just past the 14-byte Ethernet header.
pub const COVERAGE_START: u128 = 14;

#[kernel]
/// Substitute `0xFF` for the bytes a router may mutate.
///
/// `frame_index` is the byte's absolute offset in the Ethernet frame.
/// Offsets outside the masked set pass through unchanged, so this is
/// safe to apply to every covered byte unconditionally.
pub fn icrc_mask(byte: Bits<8>, frame_index: Bits<16>) -> Bits<8> {
    let masked = frame_index == bits(15)
        || frame_index == bits(22)
        || frame_index == bits(24)
        || frame_index == bits(25)
        || frame_index == bits(40)
        || frame_index == bits(41)
        || frame_index == bits(46);
    if masked { bits(0xFF) } else { byte }
}

/// Is this frame offset one the ICRC masks?
///
/// The host-side mirror of [`icrc_mask`]'s condition. It exists so a
/// test can assert the *contrapositive* — that flipping a masked byte
/// leaves the ICRC unchanged — which is the only way to prove the
/// masking is actually happening rather than being a no-op.
pub fn is_masked_offset(frame_index: u128) -> bool {
    matches!(
        frame_index,
        OFF_IP_TOS
            | OFF_IP_TTL
            | OFF_IP_CSUM_HI
            | OFF_IP_CSUM_LO
            | OFF_UDP_CSUM_HI
            | OFF_UDP_CSUM_LO
            | OFF_BTH_ECN
    )
}

#[kernel]
/// Reverse the byte order of a 32-bit word.
///
/// Used to turn the CRC value into the little-endian form the ICRC
/// field carries on the wire.
pub fn byte_reverse32(v: Bits<32>) -> Bits<32> {
    ((v & bits(0x0000_00FF)) << 24)
        | ((v & bits(0x0000_FF00)) << 8)
        | ((v >> 8) & bits(0x0000_FF00))
        | ((v >> 24) & bits(0x0000_00FF))
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// Invariant CRC engine for RoCEv2 frames.
///
/// Folds one covered frame byte per cycle, applying the mutable-field
/// masking of [`icrc_mask`] on the way in.
pub struct Icrc {
    remainder: dff::DFF<Bits<32>>,
}

impl Default for Icrc {
    fn default() -> Self {
        Self {
            // Seeded past the all-ones prefix, so the prefix costs no
            // cycles. Emphatically not Bits::default().
            remainder: dff::DFF::new(bits(PREFIX_REMAINDER)),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [`Icrc`] engine.
pub struct In {
    /// The frame byte, exactly as it will appear on the wire. Masking
    /// is applied internally, so the caller passes the real byte.
    pub byte: Bits<8>,
    /// This byte's absolute offset in the Ethernet frame, which is what
    /// selects the masking.
    pub frame_index: Bits<16>,
    /// Fold `byte` in this cycle. The caller asserts this over exactly
    /// the covered range — from [`COVERAGE_START`] up to but excluding
    /// the ICRC field. Ignored if `start` is also set.
    pub enable: bool,
    /// Re-seed for a new frame. Takes precedence over `enable`.
    pub start: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs of the [`Icrc`] engine.
pub struct Out {
    /// The CRC-32 value of everything folded in so far.
    pub crc: Bits<32>,
    /// The same value byte-reversed, ready to emit
    /// most-significant-byte-first like every other header field. This
    /// is what belongs in the packet's ICRC field.
    pub wire: Bits<32>,
}

impl SynchronousIO for Icrc {
    type I = In;
    type O = Out;
    type Kernel = icrc_engine;
}

#[kernel]
/// Kernel for [`Icrc`].
pub fn icrc_engine(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut o = Out::dont_care();

    d.remainder = if i.start {
        bits(0xDEBB_20E3)
    } else if i.enable {
        crc32_step(q.remainder, icrc_mask(i.byte, i.frame_index))
    } else {
        q.remainder
    };

    let finished = q.remainder ^ bits(0xFFFF_FFFF);
    o.crc = finished;
    o.wire = byte_reverse32(finished);

    if cr.reset.any() {
        d.remainder = bits(0xDEBB_20E3);
        o.crc = bits(0);
        o.wire = bits(0);
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;
    use std::path::PathBuf;

    /// Vector `rc_write_only_16` from `tests/vectors/write_vectors.txt`.
    /// Duplicated here rather than shared, so the widget's own tiers do
    /// not depend on the test-crate vector parser.
    const FRAME_HEX: &str = concat!(
        "ec0d9aaabbcc000a3501020308004500004c000000004011f714c0a80132c0a8010a",
        "123412b7003800000a00ffff000001110000000100007f00123400000000a5a50000",
        "0010000102030405060708090a0b0c0d0e0fb3d6ae42"
    );

    fn frame() -> Vec<u8> {
        (0..FRAME_HEX.len() / 2)
            .map(|i| u8::from_str_radix(&FRAME_HEX[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    fn stimulus() -> Vec<In> {
        let f = frame();
        let covered_end = f.len() - 4;
        let mut stream = vec![In {
            byte: bits(0),
            frame_index: bits(0),
            enable: false,
            start: true,
        }];
        for (idx, &byte) in f.iter().enumerate() {
            stream.push(In {
                byte: bits(byte as u128),
                frame_index: bits(idx as u128),
                enable: idx >= COVERAGE_START as usize && idx < covered_end,
                start: false,
            });
        }
        stream.push(In {
            byte: bits(0),
            frame_index: bits(0),
            enable: false,
            start: false,
        });
        stream
    }

    // ---------------- Tier 1 ----------------

    /// Every masked offset substitutes, and nothing else does.
    #[test]
    fn mask_substitutes_exactly_the_mutable_fields() {
        for idx in 0u128..64 {
            let got = icrc_mask(bits(0x5A), bits(idx));
            if is_masked_offset(idx) {
                assert_eq!(got, bits::<8>(0xFF), "offset {idx} should be masked");
            } else {
                assert_eq!(got, bits::<8>(0x5A), "offset {idx} should pass through");
            }
        }
    }

    /// The masked set is exactly the seven bytes the IBTA calls
    /// variant. Pinned as a count so that adding one by accident fails.
    #[test]
    fn exactly_seven_offsets_are_masked() {
        let n = (0u128..1024).filter(|i| is_masked_offset(*i)).count();
        assert_eq!(n, 7, "the ICRC masks seven bytes, not {n}");
    }

    /// `is_masked_offset` and the kernel agree. They are two
    /// expressions of one rule, and a test that used only one of them
    /// would not notice them drifting apart.
    #[test]
    fn host_predicate_agrees_with_the_kernel() {
        for idx in 0u128..1024 {
            let kernel_masks = icrc_mask(bits(0x00), bits(idx)) == bits::<8>(0xFF);
            assert_eq!(
                kernel_masks,
                is_masked_offset(idx),
                "disagreement at offset {idx}"
            );
        }
    }

    #[test]
    fn byte_reverse_is_an_involution() {
        for v in [
            0x0000_0000u128,
            0xDEAD_BEEF,
            0x42AE_D6B3,
            0xFFFF_FFFF,
            0x0000_00FF,
        ] {
            let once = byte_reverse32(bits(v));
            assert_eq!(
                byte_reverse32(once),
                bits::<32>(v),
                "not an involution for {v:#x}"
            );
        }
        assert_eq!(byte_reverse32(bits(0x42AE_D6B3)), bits::<32>(0xB3D6_AE42));
    }

    /// The seed is the CRC-32 residue, and it is what the all-ones
    /// prefix would have produced. Pinned because the whole
    /// zero-startup-cost argument rests on it.
    #[test]
    fn prefix_remainder_is_what_streaming_the_prefix_would_give() {
        use rhdl_fpga::core::crc32::{INIT, crc32_step};
        let mut c = bits::<32>(INIT);
        for _ in 0..8 {
            c = crc32_step(c, bits(0xFF));
        }
        assert_eq!(c, bits::<32>(PREFIX_REMAINDER));
    }

    #[test]
    fn reset_seeds_the_prefix_remainder() {
        let cr = clock_reset(clock(false), reset(true));
        let i = In {
            byte: bits(0xAB),
            frame_index: bits(100),
            enable: true,
            start: false,
        };
        let q = Q {
            remainder: bits(0x1234_5678),
        };
        let (o, d) = icrc_engine(cr, i, q);
        assert_eq!(d.remainder, bits::<32>(PREFIX_REMAINDER));
        assert_eq!(o.crc, bits::<32>(0));
        assert_eq!(o.wire, bits::<32>(0));
    }

    #[test]
    fn start_takes_precedence_over_enable() {
        let cr = ClockReset::dont_care();
        let i = In {
            byte: bits(0xAB),
            frame_index: bits(100),
            enable: true,
            start: true,
        };
        let q = Q {
            remainder: bits(0x1234_5678),
        };
        let (_o, d) = icrc_engine(cr, i, q);
        assert_eq!(d.remainder, bits::<32>(PREFIX_REMAINDER));
    }

    // ---------------- Tier 2 ----------------

    /// The widget reproduces the reference frame's ICRC.
    #[test]
    fn widget_reproduces_the_reference_icrc() {
        let uut = Icrc::default();
        let input = stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let wire = uut
            .run(input)
            .synchronous_sample()
            .map(|s| s.output.wire.raw() as u32)
            .last()
            .unwrap();
        assert_eq!(wire.to_be_bytes(), [0xb3, 0xd6, 0xae, 0x42]);
    }

    // ---------------- VM cross-validation ----------------

    /// Per CLAUDE.md §4, since both kernels are shift-heavy and a direct
    /// Rust call is more permissive than the kernel VM.
    #[test]
    fn mask_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = (0u128..128).map(|i| (bits::<8>(i), bits::<16>(i)));
        test_kernel_vm_and_verilog_synchronous::<icrc_mask, _, _, _>(icrc_mask, inputs)?;
        Ok(())
    }

    #[test]
    fn byte_reverse_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = (0u128..256).map(|i| (bits::<32>((i * 0x0101_0101 + i) & 0xFFFF_FFFF),));
        test_kernel_vm_and_verilog_synchronous::<byte_reverse32, _, _, _>(byte_reverse32, inputs)?;
        Ok(())
    }

    // ---------------- Tier 3 ----------------

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Icrc::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [25:0] i, output wire [63:0] o);
               wire [95:0] od;
               wire [31:0] d;
               wire [31:0] q;
               assign o = od[63:0];
               top_remainder c0(.clock_reset(clock_reset), .i(d[31:0]), .o(q[31:0]));
               assign d = od[95:64];
               assign od = kernel_icrc_engine(clock_reset, i, q);
               function [95:0] kernel_icrc_engine(input reg [1:0] arg_0, input reg [25:0] arg_1, input reg [31:0] arg_2);
                     reg [0:0] r0;
                     reg [25:0] r1;
                     reg [0:0] r2;
                     reg [31:0] r3;
                     reg [7:0] r4;
                     reg [15:0] r5;
                     reg [0:0] r6;
                     reg [0:0] r7;
                     reg [0:0] r8;
                     reg [0:0] r9;
                     reg [0:0] r10;
                     reg [0:0] r11;
                     reg [0:0] r12;
                     reg [0:0] r13;
                     reg [0:0] r14;
                     reg [0:0] r15;
                     reg [0:0] r16;
                     reg [0:0] r17;
                     reg [0:0] r18;
                     reg [7:0] r19;
                     reg [31:0] r20;
                     reg [31:0] r21;
                     reg [31:0] r22;
                     reg [0:0] r23;
                     reg [31:0] r24;
                     reg [31:0] r25;
                     reg [31:0] r26;
                     reg [31:0] r27;
                     reg [0:0] r28;
                     reg [31:0] r29;
                     reg [31:0] r30;
                     reg [31:0] r31;
                     reg [31:0] r32;
                     reg [0:0] r33;
                     reg [31:0] r34;
                     reg [31:0] r35;
                     reg [31:0] r36;
                     reg [31:0] r37;
                     reg [0:0] r38;
                     reg [31:0] r39;
                     reg [31:0] r40;
                     reg [31:0] r41;
                     reg [31:0] r42;
                     reg [0:0] r43;
                     reg [31:0] r44;
                     reg [31:0] r45;
                     reg [31:0] r46;
                     reg [31:0] r47;
                     reg [0:0] r48;
                     reg [31:0] r49;
                     reg [31:0] r50;
                     reg [31:0] r51;
                     reg [31:0] r52;
                     reg [0:0] r53;
                     reg [31:0] r54;
                     reg [31:0] r55;
                     reg [31:0] r56;
                     reg [31:0] r57;
                     reg [0:0] r58;
                     reg [31:0] r59;
                     reg [31:0] r60;
                     reg [31:0] r61;
                     reg [31:0] r62;
                     reg [31:0] r63;
                     // d
                     reg [31:0] r64;
                     reg [31:0] r65;
                     // o
                     reg [63:0] r66;
                     reg [31:0] r67;
                     reg [31:0] r68;
                     reg [31:0] r69;
                     reg [31:0] r70;
                     reg [31:0] r71;
                     reg [31:0] r72;
                     reg [31:0] r73;
                     reg [31:0] r74;
                     reg [31:0] r75;
                     reg [31:0] r76;
                     reg [31:0] r77;
                     // o
                     reg [63:0] r78;
                     reg [0:0] r79;
                     reg [1:0] r80;
                     reg [0:0] r81;
                     // d
                     reg [31:0] r82;
                     // o
                     reg [63:0] r83;
                     // o
                     reg [63:0] r84;
                     // d
                     reg [31:0] r85;
                     // o
                     reg [63:0] r86;
                     reg [95:0] r87;
                     reg [32:0] r88;
                     reg [32:0] r89;
                     reg [32:0] r90;
                     reg [32:0] r91;
                     reg [32:0] r92;
                     reg [32:0] r93;
                     reg [32:0] r94;
                     reg [32:0] r95;
                     reg [7:0] r96;
                     reg [23:0] r97;
                     reg [39:0] r98;
                     reg [55:0] r99;
                     localparam l0 = 16'b0000000000001111;
                     localparam l1 = 16'b0000000000010110;
                     localparam l2 = 16'b0000000000011000;
                     localparam l3 = 16'b0000000000011001;
                     localparam l4 = 16'b0000000000101000;
                     localparam l5 = 16'b0000000000101001;
                     localparam l6 = 16'b0000000000101110;
                     localparam l7 = 8'b11111111;
                     localparam l8 = 32'b00000000000000000000000000000001;
                     localparam l9 = 32'b11101101101110001000001100100000;
                     localparam l10 = 32'b00000000000000000000000000000001;
                     localparam l11 = 32'b11101101101110001000001100100000;
                     localparam l12 = 32'b00000000000000000000000000000001;
                     localparam l13 = 32'b11101101101110001000001100100000;
                     localparam l14 = 32'b00000000000000000000000000000001;
                     localparam l15 = 32'b11101101101110001000001100100000;
                     localparam l16 = 32'b00000000000000000000000000000001;
                     localparam l17 = 32'b11101101101110001000001100100000;
                     localparam l18 = 32'b00000000000000000000000000000001;
                     localparam l19 = 32'b11101101101110001000001100100000;
                     localparam l20 = 32'b00000000000000000000000000000001;
                     localparam l21 = 32'b11101101101110001000001100100000;
                     localparam l22 = 32'b00000000000000000000000000000001;
                     localparam l23 = 32'b11101101101110001000001100100000;
                     localparam l24 = 32'b11011110101110110010000011100011;
                     localparam l25 = 32'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l26 = 32'b11111111111111111111111111111111;
                     localparam l27 = 64'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l28 = 32'b00000000000000000000000011111111;
                     localparam l29 = 32'b00000000000000001111111100000000;
                     localparam l30 = 32'b00000000000000001111111100000000;
                     localparam l31 = 32'b00000000000000000000000011111111;
                     localparam l32 = 32'b11011110101110110010000011100011;
                     localparam l33 = 32'b00000000000000000000000000000000;
                     localparam l34 = 32'b00000000000000000000000000000000;
                     localparam l35 = 24'b000000000000000000000000;
                     localparam l36 = 8'b00000000;
                     begin
                        r80 = arg_0;
                        r1 = arg_1;
                        r3 = arg_2;
                        r0 = r1[25:25];
                        r2 = r1[24:24];
                        r4 = r1[7:0];
                        r5 = r1[23:8];
                        r6 = r5 == l0;
                        r7 = r5 == l1;
                        r8 = r6 | r7;
                        r9 = r5 == l2;
                        r10 = r8 | r9;
                        r11 = r5 == l3;
                        r12 = r10 | r11;
                        r13 = r5 == l4;
                        r14 = r12 | r13;
                        r15 = r5 == l5;
                        r16 = r14 | r15;
                        r17 = r5 == l6;
                        r18 = r16 | r17;
                        r19 = r18 ? l7 : r4;
                        r20 = {{24{1'b0}}, r19};
                        r21 = r3 ^ r20;
                        r22 = r21 & l8;
                        r23 = |r22;
                        r88 = {{1{1'b0}}, r21};
                        r24 = r88[32:1];
                        r25 = r24 ^ l9;
                        r26 = r23 ? r25 : r24;
                        r27 = r26 & l10;
                        r28 = |r27;
                        r89 = {{1{1'b0}}, r26};
                        r29 = r89[32:1];
                        r30 = r29 ^ l11;
                        r31 = r28 ? r30 : r29;
                        r32 = r31 & l12;
                        r33 = |r32;
                        r90 = {{1{1'b0}}, r31};
                        r34 = r90[32:1];
                        r35 = r34 ^ l13;
                        r36 = r33 ? r35 : r34;
                        r37 = r36 & l14;
                        r38 = |r37;
                        r91 = {{1{1'b0}}, r36};
                        r39 = r91[32:1];
                        r40 = r39 ^ l15;
                        r41 = r38 ? r40 : r39;
                        r42 = r41 & l16;
                        r43 = |r42;
                        r92 = {{1{1'b0}}, r41};
                        r44 = r92[32:1];
                        r45 = r44 ^ l17;
                        r46 = r43 ? r45 : r44;
                        r47 = r46 & l18;
                        r48 = |r47;
                        r93 = {{1{1'b0}}, r46};
                        r49 = r93[32:1];
                        r50 = r49 ^ l19;
                        r51 = r48 ? r50 : r49;
                        r52 = r51 & l20;
                        r53 = |r52;
                        r94 = {{1{1'b0}}, r51};
                        r54 = r94[32:1];
                        r55 = r54 ^ l21;
                        r56 = r53 ? r55 : r54;
                        r57 = r56 & l22;
                        r58 = |r57;
                        r95 = {{1{1'b0}}, r56};
                        r59 = r95[32:1];
                        r60 = r59 ^ l23;
                        r61 = r58 ? r60 : r59;
                        r62 = r2 ? r61 : r3;
                        r63 = r0 ? l24 : r62;
                        r64 = l25;
                        r64[31:0] = r63;
                        r65 = r3 ^ l26;
                        r66 = l27;
                        r66[31:0] = r65;
                        r67 = r65 & l28;
                        r96 = r67[7:0];
                        r68 = {r96, l35};
                        r69 = r65 & l29;
                        r97 = r69[23:0];
                        r70 = {r97, l36};
                        r71 = r68 | r70;
                        r98 = {{8{1'b0}}, r65};
                        r72 = r98[39:8];
                        r73 = r72 & l30;
                        r74 = r71 | r73;
                        r99 = {{24{1'b0}}, r65};
                        r75 = r99[55:24];
                        r76 = r75 & l31;
                        r77 = r74 | r76;
                        r78 = r66;
                        r78[63:32] = r77;
                        r79 = r80[1:1];
                        r81 = |r79;
                        r82 = r64;
                        r82[31:0] = l32;
                        r83 = r78;
                        r83[31:0] = l33;
                        r84 = r83;
                        r84[63:32] = l34;
                        r85 = r81 ? r82 : r64;
                        r86 = r81 ? r84 : r78;
                        r87 = {r85, r86};
                        kernel_icrc_engine = r87;
                     end
               endfunction
            endmodule
            module top_remainder(input wire [1:0] clock_reset, input wire [31:0] i, output reg [31:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 32'b11011110101110110010000011100011;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 32'b11011110101110110010000011100011;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    // ---------------- Tier 4 ----------------

    #[test]
    fn test_icrc_hdl_works() -> miette::Result<()> {
        let uut = Icrc::default();
        let input = stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        // `.skip(2)` for the documented non-zero-DFF-reset reason: the
        // seed is 0xDEBB20E3, so Verilog's `initial` block and the Rust
        // simulator's `dont_care` disagree at time 0 only.
        let opts = TestBenchOptions::default().skip(2);
        tb.rtl(&uut, &opts)?.run_iverilog()?;
        tb.ntl(&uut, &opts)?.run_iverilog()?;
        Ok(())
    }

    // ---------------- Tier 5 ----------------

    #[test]
    fn test_icrc_trace() -> miette::Result<()> {
        let uut = Icrc::default();
        let input = stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("icrc");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["d09c1142ba1ad180c057b8a51d9d8fb0ef371c277586e124885a43f2e1432732"];
        let digest = vcd.dump_to_file(root.join("icrc.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
