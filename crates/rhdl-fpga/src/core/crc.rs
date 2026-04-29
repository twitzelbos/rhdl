//! CRC engine (bit-serial)
//!
//! A bit-serial cyclic redundancy check (CRC) engine.  The polynomial,
//! width, and initial value are all configurable at construction time;
//! reflection and final XOR-out are intentionally *not* baked in — most
//! callers want to apply them in software at the message boundary, and
//! folding them into the streaming engine couples to the message-end
//! signal in a way that varies by use site.  Wrap this engine to add
//! those if you need them for a specific protocol.
//!
//! The engine processes one input bit per cycle (MSB-first) when
//! `enable` is asserted, and exposes a `clear` strobe that reloads the
//! configured initial value so the same engine can be reused across
//! many messages without going through the full reset path.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +-----+CrcEngine+-----+
     |                     |
bool |                     |
+--->| bit                 |
     |                     | B<W>
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
//! The CRC register is a single [super::dff::DFF] of width `W`.  On
//! every enabled cycle the register shifts left by one and conditionally
//! XORs with the polynomial when the just-shifted-out top bit XOR the
//! incoming bit is `1` — the textbook MSB-first shift-register CRC.
//! `clear` overrides `enable` and reloads the initial value.
//!
#![doc = badascii_doc::badascii!(r"
   poly  init
    |     |
    v     v
  +-+CrcKernel+--------+
  |                    |    +-+DFF+-+
  | (shift, conditional|--->|d    q+-+--->crc
  |  XOR with poly)    |    +------+ |
  +-+------+-----------+             |
    ^      ^                         |
    |      +-----feedback------------+
    bit
")]
//!
//!# Parameters
//!
//! - `W` — width of the CRC register / polynomial (e.g. 16 for
//!   CRC-16-CCITT, 32 for CRC-32)
//!
//! The polynomial is supplied in *normal* form (the implicit `x^W`
//! coefficient is dropped — store only the low `W` bits).  Examples:
//!
//! | Name           | `W` | polynomial | init   |
//! |----------------|-----|-----------:|-------:|
//! | CRC-16-CCITT   |  16 |    `0x1021`| `0xFFFF` |
//! | CRC-32 (IEEE)  |  32 |`0x04C11DB7`| `0xFFFFFFFF` |
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/crc.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/crc.md")]
use rhdl::prelude::*;

use super::{constant::Constant, dff};

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// CRC engine (bit-serial).
///
/// `W` is the CRC width (and the polynomial / register width).  The
/// polynomial is provided in *normal* form (drop the implicit `x^W`
/// coefficient).  `init` is the value loaded into the register on
/// reset and on the `clear` strobe.
pub struct CrcEngine<const W: usize>
where
    rhdl::bits::W<W>: BitWidth,
{
    register: dff::DFF<Bits<W>>,
    poly: Constant<Bits<W>>,
    init_val: Constant<Bits<W>>,
}

impl<const W: usize> CrcEngine<W>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// Create a new CRC engine with the supplied polynomial (normal
    /// form) and initial register value.
    pub fn new(poly: Bits<W>, init: Bits<W>) -> Self {
        Self {
            register: dff::DFF::new(init),
            poly: Constant::new(poly),
            init_val: Constant::new(init),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [CrcEngine].
pub struct In {
    /// The data bit to process this cycle (consumed only when `enable`).
    pub bit: bool,
    /// Process the input bit this cycle.  Ignored if `clear` is also set.
    pub enable: bool,
    /// Reload the register with the configured initial value.  Takes
    /// precedence over `enable`.
    pub clear: bool,
}

impl<const W: usize> SynchronousIO for CrcEngine<W>
where
    rhdl::bits::W<W>: BitWidth,
{
    type I = In;
    type O = Bits<W>;
    type Kernel = crc_engine<W>;
}

#[kernel]
/// Kernel for [CrcEngine].
pub fn crc_engine<const W: usize>(cr: ClockReset, i: In, q: Q<W>) -> (Bits<W>, D<W>)
where
    rhdl::bits::W<W>: BitWidth,
{
    // Standard MSB-first shift-register CRC step:
    //   top = MSB of register
    //   feedback = top XOR input_bit
    //   register <<= 1
    //   if feedback: register ^= poly
    let top_bit = (q.register >> ((W - 1) as u128)) & bits(1);
    let top_set = top_bit != bits(0);
    let feedback = top_set != i.bit;
    let shifted = q.register << 1;
    let stepped = if feedback { shifted ^ q.poly } else { shifted };
    let mut d = D::<W>::dont_care();
    d.register = if i.clear {
        q.init_val
    } else if i.enable {
        stepped
    } else {
        q.register
    };
    let o = q.register;
    if cr.reset.any() {
        d.register = q.init_val;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    /// Software reference implementation of the same CRC algorithm
    /// the kernel implements.  Used to validate the hardware against
    /// independently-computed expected values.
    fn crc_ref<const W: usize>(poly: u128, init: u128, data_bits: &[bool]) -> u128 {
        let mask = if W == 128 { !0u128 } else { (1u128 << W) - 1 };
        let mut reg = init & mask;
        for &b in data_bits {
            let top = (reg >> (W - 1)) & 1;
            let xor = (top ^ u128::from(b)) != 0;
            reg = (reg << 1) & mask;
            if xor {
                reg ^= poly;
            }
        }
        reg
    }

    fn bytes_to_bits_msb_first(bytes: &[u8]) -> Vec<bool> {
        let mut out = Vec::with_capacity(bytes.len() * 8);
        for &byte in bytes {
            for i in (0..8).rev() {
                out.push(((byte >> i) & 1) != 0);
            }
        }
        out
    }

    // Tier 1 — direct kernel unit tests

    #[test]
    fn test_clear_loads_init_value() {
        let cr = ClockReset::dont_care();
        let q = Q::<16> {
            register: bits(0xDEAD),
            poly: bits(0x1021),
            init_val: bits(0xFFFF),
        };
        let i = In {
            bit: true,
            enable: true,
            clear: true,
        };
        let (_o, d) = crc_engine::<16>(cr, i, q);
        // clear takes precedence over enable.
        assert_eq!(d.register, bits(0xFFFF));
    }

    #[test]
    fn test_disabled_holds_register() {
        let cr = ClockReset::dont_care();
        let q = Q::<16> {
            register: bits(0x1234),
            poly: bits(0x1021),
            init_val: bits(0xFFFF),
        };
        let i = In {
            bit: true,
            enable: false,
            clear: false,
        };
        let (_o, d) = crc_engine::<16>(cr, i, q);
        assert_eq!(d.register, bits(0x1234));
    }

    #[test]
    fn test_kernel_step_matches_reference() {
        // Single-step the kernel and the reference, verify they agree.
        let cr = ClockReset::dont_care();
        let q = Q::<16> {
            register: bits(0xFFFF),
            poly: bits(0x1021),
            init_val: bits(0xFFFF),
        };
        for &b in &[false, true] {
            let i = In {
                bit: b,
                enable: true,
                clear: false,
            };
            let (_o, d) = crc_engine::<16>(cr, i, q);
            let expected = crc_ref::<16>(0x1021, 0xFFFF, &[b]);
            assert_eq!(d.register.raw(), expected, "bit={b}");
        }
    }

    #[test]
    fn test_reset_loads_init() {
        let cr = clock_reset(clock(true), reset(true));
        let q = Q::<16> {
            register: bits(0x1234),
            poly: bits(0x1021),
            init_val: bits(0xABCD),
        };
        let i = In {
            bit: true,
            enable: true,
            clear: false,
        };
        let (_o, d) = crc_engine::<16>(cr, i, q);
        assert_eq!(d.register, bits(0xABCD));
    }

    // Tier 2 — iterator simulation against the software reference.

    /// Streaming "123456789" through CRC-16-CCITT with init 0xFFFF
    /// should give the reference value 0x29B1 (KERMIT/false standard).
    /// Our engine doesn't apply reflection or xor-out, so we compare
    /// against the in-house `crc_ref` (same MSB-first algorithm)
    /// rather than a published table — but we *also* check that the
    /// known good "123456789" → 0x29B1 holds, which it does for this
    /// particular variant.
    #[test]
    fn test_crc16_ccitt_streaming_matches_reference() -> miette::Result<()> {
        let bytes = b"123456789";
        let bit_inputs = bytes_to_bits_msb_first(bytes);
        // Build (clear, then enable for each bit, then idle).
        let mut stream_in: Vec<In> = Vec::with_capacity(bit_inputs.len() + 2);
        stream_in.push(In {
            bit: false,
            enable: false,
            clear: true,
        });
        for &b in &bit_inputs {
            stream_in.push(In {
                bit: b,
                enable: true,
                clear: false,
            });
        }
        // Drain cycle so we observe the post-final register value.
        stream_in.push(In {
            bit: false,
            enable: false,
            clear: false,
        });
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let uut = CrcEngine::<16>::new(bits(0x1021), bits(0xFFFF));
        let outputs = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output.raw())
            .collect::<Vec<_>>();
        // The final observed CRC value (after all bits processed) is
        // the last element.  Compare to the in-house reference.
        let expected = crc_ref::<16>(0x1021, 0xFFFF, &bit_inputs);
        assert_eq!(*outputs.last().unwrap(), expected);
        // And confirm the well-known result for this CRC variant.
        assert_eq!(expected, 0x29B1);
        Ok(())
    }

    /// Same engine, second message after `clear`.  Reusing the engine
    /// for back-to-back messages should give the right CRC each time.
    #[test]
    fn test_back_to_back_messages_via_clear() -> miette::Result<()> {
        let m1 = bytes_to_bits_msb_first(b"123456789");
        let m2 = bytes_to_bits_msb_first(b"abc");
        let build_message = |bits_in: &[bool]| -> Vec<In> {
            let mut v = vec![In {
                bit: false,
                enable: false,
                clear: true,
            }];
            for &b in bits_in {
                v.push(In {
                    bit: b,
                    enable: true,
                    clear: false,
                });
            }
            v.push(In {
                bit: false,
                enable: false,
                clear: false,
            });
            v
        };
        let mut stream_in: Vec<In> = build_message(&m1);
        stream_in.extend(build_message(&m2));
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let uut = CrcEngine::<16>::new(bits(0x1021), bits(0xFFFF));
        let outputs = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output.raw())
            .collect::<Vec<_>>();
        // Find the value just before the second `clear` — that's the
        // CRC for message 1.  And the very last value is the CRC for
        // message 2.
        let m1_crc = crc_ref::<16>(0x1021, 0xFFFF, &m1);
        let m2_crc = crc_ref::<16>(0x1021, 0xFFFF, &m2);
        assert_eq!(*outputs.last().unwrap(), m2_crc);
        // The CRC for m1 should appear at index = 1 + m1.len() (the
        // sample after the last enabled bit of m1, before m2 starts
        // with a clear).
        let m1_end_idx = 1 + m1.len();
        assert_eq!(outputs[m1_end_idx], m1_crc);
        Ok(())
    }

    // Tier 3 — HDL emission length sanity check
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = CrcEngine::<16>::new(bits(0x1021), bits(0xFFFF));
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        let expect = expect!["3065"];
        expect.assert_eq(&hdl.len().to_string());
        Ok(())
    }

    // Tier 4 — iverilog round-trip
    //
    // The DFF inside the engine resets to a non-zero value (0xFFFF).
    // In Verilog the `initial` block sets the register to that value
    // at time 0; in the Rust simulator the state starts as
    // `dont_care` and only takes the reset value after the first
    // rising edge.  Both agree once the first clock edge has fired,
    // so we skip the pre-edge sample window with `skip(2)`.
    #[test]
    fn test_crc_hdl_works() -> miette::Result<()> {
        let uut = CrcEngine::<16>::new(bits(0x1021), bits(0xFFFF));
        let bits_in = bytes_to_bits_msb_first(b"123456789");
        let mut stream_in: Vec<In> = vec![In {
            bit: false,
            enable: false,
            clear: true,
        }];
        for &b in &bits_in {
            stream_in.push(In {
                bit: b,
                enable: true,
                clear: false,
            });
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &TestBenchOptions::default().skip(2))?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &TestBenchOptions::default().skip(2))?;
        tm.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest
    #[test]
    fn test_crc_trace() -> miette::Result<()> {
        let uut = CrcEngine::<16>::new(bits(0x1021), bits(0xFFFF));
        let bits_in = bytes_to_bits_msb_first(b"abc");
        let mut stream_in: Vec<In> = vec![In {
            bit: false,
            enable: false,
            clear: true,
        }];
        for &b in &bits_in {
            stream_in.push(In {
                bit: b,
                enable: true,
                clear: false,
            });
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("crc");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["5e11c52ded6639bbc755dcdc1017f2f819bf0b1c512d0cb14ccb76e74d565b6b"];
        let digest = vcd.dump_to_file(root.join("crc.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
