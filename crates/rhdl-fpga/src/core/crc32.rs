//! CRC-32 (byte-parallel, reflected — the IEEE 802.3 / zlib algorithm)
//!
//! The one specific CRC that Ethernet, zlib, PNG, and the RoCEv2
//! Invariant CRC all use: polynomial `0x04C11DB7`, **reflected** input
//! and output, initial value `0xFFFFFFFF`, and a final XOR with
//! `0xFFFFFFFF`.  It consumes **one byte per cycle**, which is what
//! distinguishes it from [`super::crc::CrcEngine`] and is what makes it
//! usable at line rate — 8 bits per cycle at 125 MHz is exactly 1 Gbit/s.
//!
//! Unlike [`super::crc::CrcEngine`], this widget is *not* parameterised.
//! CRC-32 is a single named algorithm with a fixed parameter set, and
//! every one of those parameters is a chance to be subtly wrong in a way
//! that only shows up as a peer silently discarding your packets.  So
//! the reflection, the initial value and the final XOR are all baked in
//! and the output is the finished checksum, ready to place on a wire.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +------+Crc32+--------+
     |                     |
B<8> |                     |
+--->| byte                |
     |                     | B<32>
bool |              crc    +--->
+--->| enable              |
     |                     |
bool |                     |
+--->| clear               |
     +---------------------+
")]
//!
//!# Internals
//!
//! A single 32-bit [super::dff::DFF] holds the running remainder.  Each
//! enabled cycle applies [`crc32_step`], which is the textbook
//! reflected byte step: XOR the incoming byte into the low eight bits,
//! then shift right eight times, conditionally XORing the reflected
//! polynomial `0xEDB88320` whenever the bit shifted out was set.
//!
//! That eight-iteration loop has a constant bound, so it unrolls at
//! compile time into a combinational XOR network — the familiar
//! "CRC table" of software implementations, except that in hardware the
//! table is the network and there is no memory to look up.
#![doc = badascii_doc::badascii!(r"
   byte
    |
    v
  +-+crc32_step+-------+
  |                    |    +-+DFF+-+
  | XOR byte in, then  |--->|d    q+-+---> ^0xFFFFFFFF ---> crc
  | 8x (shift right,   |    +------+ |
  | conditional XOR)   |             |
  +-+------------------+             |
    ^                                |
    +--------remainder---------------+
")]
//!
//!# Latency and the `clear` strobe
//!
//! The remainder is registered, so `crc` on any given cycle reflects
//! every byte accepted **up to and including the previous cycle**.  A
//! message's checksum is therefore read on the cycle *after* its last
//! byte was accepted.
//!
//! `clear` reloads `0xFFFFFFFF` and takes precedence over `enable`, so
//! one engine can be reused across back-to-back messages without going
//! through reset.  Note that the reset value is non-zero, which is the
//! framework gotcha recorded in `CHANGELOG.md` — a stream test that
//! wants to see the pre-reset state has to account for it.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/crc32.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/crc32.md")]
use rhdl::prelude::*;

use super::dff;

/// The CRC-32 polynomial in **reflected** form.
///
/// `0x04C11DB7` with its bit order reversed.  Exposed because a
/// software reference implementation needs the same constant, and
/// having one definition is better than two that can drift.
pub const POLY_REFLECTED: u128 = 0xEDB8_8320;

/// The CRC-32 initial remainder, and the value `clear` reloads.
pub const INIT: u128 = 0xFFFF_FFFF;

/// The value the remainder is XORed with to produce the final checksum.
pub const XOR_OUT: u128 = 0xFFFF_FFFF;

#[kernel]
/// Advance a reflected CRC-32 remainder by one byte.
///
/// `crc` is the running *remainder* — not the finished checksum, so no
/// final XOR has been applied to it.  Feed [`INIT`] in for the first
/// byte of a message and XOR [`XOR_OUT`] into the result after the last.
///
/// This is the reusable primitive; [`Crc32`] is the registered widget
/// built from it.
pub fn crc32_step(crc: Bits<32>, byte: Bits<8>) -> Bits<32> {
    let mut c = crc ^ byte.resize::<32>();
    // Constant bound, so this unrolls into a combinational XOR network.
    for _i in 0..8 {
        let lsb_set = (c & bits(1)) != bits(0);
        let shifted = c >> 1;
        c = if lsb_set {
            shifted ^ bits(0xEDB8_8320)
        } else {
            shifted
        };
    }
    c
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// Byte-parallel reflected CRC-32 engine.
///
/// Consumes one byte per enabled cycle and presents the finished
/// checksum of everything consumed so far.
pub struct Crc32 {
    remainder: dff::DFF<Bits<32>>,
}

impl Default for Crc32 {
    fn default() -> Self {
        Self {
            // Not Bits::default(): CRC-32 starts at all-ones, and a
            // zero-initialised remainder silently computes a different
            // checksum rather than failing.
            remainder: dff::DFF::new(bits(INIT)),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [Crc32] engine.
pub struct In {
    /// The byte to fold in this cycle.  Consumed only when `enable` is
    /// set and `clear` is not.
    pub byte: Bits<8>,
    /// Fold `byte` into the remainder this cycle.  Ignored if `clear`
    /// is also set.
    pub enable: bool,
    /// Reload the initial remainder, readying the engine for a new
    /// message.  Takes precedence over `enable`.
    pub clear: bool,
}

impl SynchronousIO for Crc32 {
    type I = In;
    type O = Bits<32>;
    type Kernel = crc32_engine;
}

#[kernel]
/// Kernel for [Crc32].
pub fn crc32_engine(cr: ClockReset, i: In, q: Q) -> (Bits<32>, D) {
    let mut d = D::dont_care();
    d.remainder = if i.clear {
        bits(0xFFFF_FFFF)
    } else if i.enable {
        crc32_step(q.remainder, i.byte)
    } else {
        q.remainder
    };
    // The finished checksum, not the raw remainder -- see the module
    // docs for why this widget bakes the parameters in.
    let o = q.remainder ^ bits(0xFFFF_FFFF);
    if cr.reset.any() {
        d.remainder = bits(0xFFFF_FFFF);
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;
    use std::path::PathBuf;

    /// Independent software CRC-32, written from the algorithm rather
    /// than from the kernel, so agreement means something.
    fn crc32_ref(data: &[u8]) -> u32 {
        let mut crc: u32 = INIT as u32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ (POLY_REFLECTED as u32)
                } else {
                    crc >> 1
                };
            }
        }
        crc ^ (XOR_OUT as u32)
    }

    /// Drive a byte slice through the widget and read the checksum off
    /// the cycle after the last byte.
    fn run_bytes(data: &[u8]) -> u32 {
        let mut stream: Vec<In> = vec![In {
            byte: bits(0),
            enable: false,
            clear: true,
        }];
        for &b in data {
            stream.push(In {
                byte: bits(b as u128),
                enable: true,
                clear: false,
            });
        }
        // One idle cycle so the last byte's result reaches the output.
        stream.push(In {
            byte: bits(0),
            enable: false,
            clear: false,
        });
        let uut = Crc32::default();
        let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
        let out = uut
            .run(input)
            .synchronous_sample()
            .map(|s| s.output)
            .collect::<Vec<_>>();
        out.last().unwrap().raw() as u32
    }

    // ---------------- Tier 1 — direct kernel tests ----------------

    /// The canonical CRC-32 check value: the ASCII string "123456789"
    /// has checksum `0xCBF43926`.  This is the published value for this
    /// algorithm and is what pins the parameter set.
    #[test]
    fn check_value_matches_the_published_constant() {
        let mut crc = bits::<32>(INIT);
        for &b in b"123456789" {
            crc = crc32_step(crc, bits(b as u128));
        }
        let finished = crc ^ bits::<32>(XOR_OUT);
        assert_eq!(finished, bits::<32>(0xCBF4_3926));
    }

    /// An empty message has the checksum of no bytes at all: the
    /// initial remainder, XORed out, which is zero.
    #[test]
    fn empty_message_is_zero() {
        let finished = bits::<32>(INIT) ^ bits::<32>(XOR_OUT);
        assert_eq!(finished, bits::<32>(0));
        assert_eq!(crc32_ref(b""), 0);
    }

    /// A single zero byte is *not* the empty message.  This catches the
    /// classic bug of treating `enable` with a zero byte as a no-op.
    #[test]
    fn a_single_zero_byte_is_not_the_empty_message() {
        let crc = crc32_step(bits(INIT), bits(0));
        let finished = crc ^ bits::<32>(XOR_OUT);
        assert_ne!(finished, bits::<32>(0));
        assert_eq!(finished.raw() as u32, crc32_ref(&[0u8]));
    }

    /// The step function agrees with the software reference over a span
    /// of lengths and byte values, including every single-byte message.
    #[test]
    fn step_agrees_with_software_reference() {
        for len in 0..=24usize {
            let data: Vec<u8> = (0..len)
                .map(|i| (i as u8).wrapping_mul(31).wrapping_add(7))
                .collect();
            let mut crc = bits::<32>(INIT);
            for &b in &data {
                crc = crc32_step(crc, bits(b as u128));
            }
            let finished = (crc ^ bits::<32>(XOR_OUT)).raw() as u32;
            assert_eq!(
                finished,
                crc32_ref(&data),
                "mismatch at length {len} for {data:02x?}"
            );
        }
        for b in 0u8..=255 {
            let crc = crc32_step(bits(INIT), bits(b as u128));
            let finished = (crc ^ bits::<32>(XOR_OUT)).raw() as u32;
            assert_eq!(
                finished,
                crc32_ref(&[b]),
                "mismatch for single byte {b:#04x}"
            );
        }
    }

    /// Reset drives the remainder to the initial value, not to zero.
    #[test]
    fn reset_loads_the_initial_remainder() {
        let cr = clock_reset(clock(false), reset(true));
        let i = In {
            byte: bits(0xAB),
            enable: true,
            clear: false,
        };
        let q = Q {
            remainder: bits(0x1234_5678),
        };
        let (_o, d) = crc32_engine(cr, i, q);
        assert_eq!(d.remainder, bits::<32>(INIT));
    }

    /// `clear` beats `enable` — otherwise a back-to-back message would
    /// start from the tail of the previous one.
    #[test]
    fn clear_takes_precedence_over_enable() {
        let cr = ClockReset::dont_care();
        let i = In {
            byte: bits(0xAB),
            enable: true,
            clear: true,
        };
        let q = Q {
            remainder: bits(0x1234_5678),
        };
        let (_o, d) = crc32_engine(cr, i, q);
        assert_eq!(d.remainder, bits::<32>(INIT));
    }

    /// With neither strobe set the remainder holds.
    #[test]
    fn idle_holds_the_remainder() {
        let cr = ClockReset::dont_care();
        let i = In {
            byte: bits(0xAB),
            enable: false,
            clear: false,
        };
        let q = Q {
            remainder: bits(0x1234_5678),
        };
        let (o, d) = crc32_engine(cr, i, q);
        assert_eq!(d.remainder, bits::<32>(0x1234_5678));
        assert_eq!(o, bits::<32>(0x1234_5678 ^ 0xFFFF_FFFFu128));
    }

    // ---------------- Tier 2 — stream simulation ----------------

    /// The assembled widget reproduces the published check value.
    #[test]
    fn widget_reproduces_the_check_value() {
        assert_eq!(run_bytes(b"123456789"), 0xCBF4_3926);
    }

    /// And agrees with the reference on several messages, including one
    /// long enough to exercise many cycles.
    #[test]
    fn widget_agrees_with_software_reference() {
        for msg in [
            b"".as_slice(),
            b"a".as_slice(),
            b"abc".as_slice(),
            b"The quick brown fox jumps over the lazy dog".as_slice(),
        ] {
            assert_eq!(run_bytes(msg), crc32_ref(msg), "mismatch for {msg:?}");
        }
    }

    /// One engine, two messages, separated only by `clear`.  This is how
    /// the ICRC path will use it, so it is the shape worth testing.
    #[test]
    fn clear_allows_reuse_across_messages() {
        let mut stream: Vec<In> = Vec::new();
        let mut push = |byte: u8, enable: bool, clear: bool| {
            stream.push(In {
                byte: bits(byte as u128),
                enable,
                clear,
            })
        };
        push(0, false, true);
        for &b in b"123456789" {
            push(b, true, false);
        }
        push(0, false, false); // read point for message 1
        push(0, false, true); // clear
        for &b in b"abc" {
            push(b, true, false);
        }
        push(0, false, false); // read point for message 2

        let uut = Crc32::default();
        let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
        let out = uut
            .run(input)
            .synchronous_sample()
            .map(|s| s.output.raw() as u32)
            .collect::<Vec<_>>();
        // `with_reset(1)` prepends one cycle, so the read points sit one
        // sample later than the push indices.
        assert!(
            out.contains(&0xCBF4_3926),
            "first message's check value missing from {out:08x?}"
        );
        assert!(
            out.contains(&crc32_ref(b"abc")),
            "second message's checksum missing from {out:08x?}"
        );
    }

    // ---------------- VM cross-validation ----------------

    /// Per CLAUDE.md §4: a direct Rust call to a kernel is more
    /// permissive than the kernel VM, so the shift-heavy step function
    /// gets cross-validated against the VM and against Verilog.
    #[test]
    fn step_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = (0u128..256).map(|b| (bits::<32>(0xDEAD_BEEF ^ b), bits::<8>(b)));
        test_kernel_vm_and_verilog_synchronous::<crc32_step, _, _, _>(crc32_step, inputs)?;
        Ok(())
    }

    // ---------------- Tier 3 — HDL emission snapshot ----------------

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Crc32::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [9:0] i, output wire [31:0] o);
               wire [63:0] od;
               wire [31:0] d;
               wire [31:0] q;
               assign o = od[31:0];
               top_remainder c0(.clock_reset(clock_reset), .i(d[31:0]), .o(q[31:0]));
               assign d = od[63:32];
               assign od = kernel_crc32_engine(clock_reset, i, q);
               function [63:0] kernel_crc32_engine(input reg [1:0] arg_0, input reg [9:0] arg_1, input reg [31:0] arg_2);
                     reg [0:0] r0;
                     reg [9:0] r1;
                     reg [0:0] r2;
                     reg [31:0] r3;
                     reg [7:0] r4;
                     reg [31:0] r5;
                     reg [31:0] r6;
                     reg [31:0] r7;
                     reg [0:0] r8;
                     reg [31:0] r9;
                     reg [31:0] r10;
                     reg [31:0] r11;
                     reg [31:0] r12;
                     reg [0:0] r13;
                     reg [31:0] r14;
                     reg [31:0] r15;
                     reg [31:0] r16;
                     reg [31:0] r17;
                     reg [0:0] r18;
                     reg [31:0] r19;
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
                     reg [31:0] r48;
                     // d
                     reg [31:0] r49;
                     reg [31:0] r50;
                     reg [0:0] r51;
                     reg [1:0] r52;
                     reg [0:0] r53;
                     // d
                     reg [31:0] r54;
                     // d
                     reg [31:0] r55;
                     reg [63:0] r56;
                     reg [32:0] r57;
                     reg [32:0] r58;
                     reg [32:0] r59;
                     reg [32:0] r60;
                     reg [32:0] r61;
                     reg [32:0] r62;
                     reg [32:0] r63;
                     reg [32:0] r64;
                     localparam l0 = 32'b00000000000000000000000000000001;
                     localparam l1 = 32'b11101101101110001000001100100000;
                     localparam l2 = 32'b00000000000000000000000000000001;
                     localparam l3 = 32'b11101101101110001000001100100000;
                     localparam l4 = 32'b00000000000000000000000000000001;
                     localparam l5 = 32'b11101101101110001000001100100000;
                     localparam l6 = 32'b00000000000000000000000000000001;
                     localparam l7 = 32'b11101101101110001000001100100000;
                     localparam l8 = 32'b00000000000000000000000000000001;
                     localparam l9 = 32'b11101101101110001000001100100000;
                     localparam l10 = 32'b00000000000000000000000000000001;
                     localparam l11 = 32'b11101101101110001000001100100000;
                     localparam l12 = 32'b00000000000000000000000000000001;
                     localparam l13 = 32'b11101101101110001000001100100000;
                     localparam l14 = 32'b00000000000000000000000000000001;
                     localparam l15 = 32'b11101101101110001000001100100000;
                     localparam l16 = 32'b11111111111111111111111111111111;
                     localparam l17 = 32'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l18 = 32'b11111111111111111111111111111111;
                     localparam l19 = 32'b11111111111111111111111111111111;
                     begin
                        r52 = arg_0;
                        r1 = arg_1;
                        r3 = arg_2;
                        r0 = r1[9:9];
                        r2 = r1[8:8];
                        r4 = r1[7:0];
                        r5 = {{24{1'b0}}, r4};
                        r6 = r3 ^ r5;
                        r7 = r6 & l0;
                        r8 = |r7;
                        r57 = {{1{1'b0}}, r6};
                        r9 = r57[32:1];
                        r10 = r9 ^ l1;
                        r11 = r8 ? r10 : r9;
                        r12 = r11 & l2;
                        r13 = |r12;
                        r58 = {{1{1'b0}}, r11};
                        r14 = r58[32:1];
                        r15 = r14 ^ l3;
                        r16 = r13 ? r15 : r14;
                        r17 = r16 & l4;
                        r18 = |r17;
                        r59 = {{1{1'b0}}, r16};
                        r19 = r59[32:1];
                        r20 = r19 ^ l5;
                        r21 = r18 ? r20 : r19;
                        r22 = r21 & l6;
                        r23 = |r22;
                        r60 = {{1{1'b0}}, r21};
                        r24 = r60[32:1];
                        r25 = r24 ^ l7;
                        r26 = r23 ? r25 : r24;
                        r27 = r26 & l8;
                        r28 = |r27;
                        r61 = {{1{1'b0}}, r26};
                        r29 = r61[32:1];
                        r30 = r29 ^ l9;
                        r31 = r28 ? r30 : r29;
                        r32 = r31 & l10;
                        r33 = |r32;
                        r62 = {{1{1'b0}}, r31};
                        r34 = r62[32:1];
                        r35 = r34 ^ l11;
                        r36 = r33 ? r35 : r34;
                        r37 = r36 & l12;
                        r38 = |r37;
                        r63 = {{1{1'b0}}, r36};
                        r39 = r63[32:1];
                        r40 = r39 ^ l13;
                        r41 = r38 ? r40 : r39;
                        r42 = r41 & l14;
                        r43 = |r42;
                        r64 = {{1{1'b0}}, r41};
                        r44 = r64[32:1];
                        r45 = r44 ^ l15;
                        r46 = r43 ? r45 : r44;
                        r47 = r2 ? r46 : r3;
                        r48 = r0 ? l16 : r47;
                        r49 = l17;
                        r49[31:0] = r48;
                        r50 = r3 ^ l18;
                        r51 = r52[1:1];
                        r53 = |r51;
                        r54 = r49;
                        r54[31:0] = l19;
                        r55 = r53 ? r54 : r49;
                        r56 = {r55, r50};
                        kernel_crc32_engine = r56;
                     end
               endfunction
            endmodule
            module top_remainder(input wire [1:0] clock_reset, input wire [31:0] i, output reg [31:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 32'b11111111111111111111111111111111;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 32'b11111111111111111111111111111111;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    // ---------------- Tier 4 — iverilog round-trip ----------------

    #[test]
    fn test_crc32_hdl_works() -> miette::Result<()> {
        let uut = Crc32::default();
        let mut stream: Vec<In> = vec![In {
            byte: bits(0),
            enable: false,
            clear: true,
        }];
        for &b in b"123456789" {
            stream.push(In {
                byte: bits(b as u128),
                enable: true,
                clear: false,
            });
        }
        stream.push(In {
            byte: bits(0),
            enable: false,
            clear: false,
        });
        let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        // `.skip(2)` for the same reason `core::crc` needs it: this DFF's
        // reset value is non-zero, so Verilog's `initial` block sets the
        // register at time 0 while the Rust simulator starts from
        // `dont_care`. They agree from the first clock edge onward.
        let opts = TestBenchOptions::default().skip(2);
        tb.rtl(&uut, &opts)?.run_iverilog()?;
        tb.ntl(&uut, &opts)?.run_iverilog()?;
        Ok(())
    }

    // ---------------- Tier 5 — VCD digest ----------------

    #[test]
    fn test_crc32_trace() -> miette::Result<()> {
        let uut = Crc32::default();
        let mut stream: Vec<In> = vec![In {
            byte: bits(0),
            enable: false,
            clear: true,
        }];
        for &b in b"123456789" {
            stream.push(In {
                byte: bits(b as u128),
                enable: true,
                clear: false,
            });
        }
        stream.push(In {
            byte: bits(0),
            enable: false,
            clear: false,
        });
        let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("crc32");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["c1aca4705d9282080b6ddae6497c45e4eb41ec27af8ea0a8938ce31f86d9e2b2"];
        let digest = vcd.dump_to_file(root.join("crc32.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
