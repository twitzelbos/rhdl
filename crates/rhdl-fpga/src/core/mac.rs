//! Multiply-accumulate (MAC) unit
//!
//! Single-cycle unsigned multiply-accumulate.  On every enabled
//! cycle, computes `accumulator += a * b` with full-width
//! intermediate precision (`2N`-bit product, accumulated into an
//! `A_W`-bit register).  Foundation for FIR/IIR filters, DSP
//! pipelines, and integer-arithmetic neural-net inference.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +-----+MacUnit+-----+
     |                   |
B<N> |                   |
+--->| a                 |
B<N> |                   | B<A_W>
+--->| b      accumulator+--->
bool |                   |
+--->| enable            |
bool |                   |
+--->| clear             |
     +-------------------+
")]
//!
//!# Internals
//!
//! A single accumulator [DFF] of width `A_W`.  Each cycle the
//! kernel computes the full-precision `2N`-bit product (via
//! `DynBits::xmul`), zero-extends it to `A_W`, and adds it to the
//! current accumulator value.  `clear` overrides `enable` and zeros
//! the accumulator.
//!
//!# Behavior
//!
//! - `clear` (asserted): next-cycle accumulator = `0`.
//! - `enable` and not `clear`: next-cycle accumulator =
//!   accumulator + `a * b`.
//! - Neither: hold.
//!
//!# Parameters
//!
//! - `N` — width of the multiply operands (`a` and `b`)
//! - `A_W` — width of the accumulator.  Must satisfy `A_W >= 2N`
//!   (otherwise a single product overflows on the very first
//!   accumulate).  For `K` products without overflow, pick
//!   `A_W >= 2N + ceil(log2(K))`.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/mac.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/mac.md")]
use rhdl::prelude::*;

use super::dff;

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
/// Unsigned multiply-accumulate core.
pub struct MacUnit<const N: usize, const A_W: usize>
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<A_W>: BitWidth,
{
    acc: dff::DFF<Bits<A_W>>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [MacUnit].
pub struct In<const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    /// First multiply operand (consumed when `enable`).
    pub a: Bits<N>,
    /// Second multiply operand (consumed when `enable`).
    pub b: Bits<N>,
    /// Process the operands this cycle (`accumulator += a * b`).
    /// Ignored when `clear` is asserted.
    pub enable: bool,
    /// Reload the accumulator with `0`.  Takes precedence over
    /// `enable`.
    pub clear: bool,
}

impl<const N: usize, const A_W: usize> SynchronousIO for MacUnit<N, A_W>
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<A_W>: BitWidth,
{
    type I = In<N>;
    type O = Bits<A_W>;
    type Kernel = mac<N, A_W>;
}

#[kernel]
/// Kernel for [MacUnit].
pub fn mac<const N: usize, const A_W: usize>(
    cr: ClockReset,
    i: In<N>,
    q: Q<N, A_W>,
) -> (Bits<A_W>, D<N, A_W>)
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<A_W>: BitWidth,
{
    // Full-precision product via DynBits (size 2N), then resized to
    // the accumulator width.
    let a_dyn = i.a.dyn_bits();
    let b_dyn = i.b.dyn_bits();
    let product = a_dyn.xmul(b_dyn);
    let product_acc: Bits<A_W> = product.resize::<A_W>().as_bits();
    let next_acc = if i.clear {
        bits(0)
    } else if i.enable {
        q.acc + product_acc
    } else {
        q.acc
    };
    let mut d = D::<N, A_W>::dont_care();
    d.acc = next_acc;
    let o = q.acc;
    if cr.reset.any() {
        d.acc = bits(0);
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    // Tier 1 — direct kernel unit tests

    #[test]
    fn test_clear_zeros_accumulator() {
        let cr = ClockReset::dont_care();
        let q = Q::<8, 24> { acc: bits(0xDEAD) };
        let i = In {
            a: bits(7),
            b: bits(11),
            enable: true,
            clear: true,
        };
        let (_o, d) = mac::<8, 24>(cr, i, q);
        assert_eq!(d.acc, bits(0));
    }

    #[test]
    fn test_disabled_holds_accumulator() {
        let cr = ClockReset::dont_care();
        let q = Q::<8, 24> { acc: bits(0x1234) };
        let i = In {
            a: bits(7),
            b: bits(11),
            enable: false,
            clear: false,
        };
        let (_o, d) = mac::<8, 24>(cr, i, q);
        assert_eq!(d.acc, bits(0x1234));
    }

    #[test]
    fn test_enable_accumulates_product() {
        let cr = ClockReset::dont_care();
        let q = Q::<8, 24> { acc: bits(100) };
        let i = In {
            a: bits(7),
            b: bits(11),
            enable: true,
            clear: false,
        };
        let (_o, d) = mac::<8, 24>(cr, i, q);
        assert_eq!(d.acc, bits(100 + 77));
    }

    #[test]
    fn test_max_product_fits_in_accumulator() {
        // 0xFF * 0xFF = 0xFE01 (16-bit result fits in 24-bit acc).
        let cr = ClockReset::dont_care();
        let q = Q::<8, 24> { acc: bits(0) };
        let i = In {
            a: bits(0xFF),
            b: bits(0xFF),
            enable: true,
            clear: false,
        };
        let (_o, d) = mac::<8, 24>(cr, i, q);
        assert_eq!(d.acc, bits(0xFE01));
    }

    #[test]
    fn test_reset_zeros_accumulator() {
        let cr = clock_reset(clock(true), reset(true));
        let q = Q::<8, 24> {
            acc: bits(0xDEADBE),
        };
        let i = In {
            a: bits(7),
            b: bits(11),
            enable: true,
            clear: false,
        };
        let (_o, d) = mac::<8, 24>(cr, i, q);
        assert_eq!(d.acc, bits(0));
    }

    // Tier 2 — iterator simulation against software reference

    /// Stream a sequence of (a, b) pairs through the MAC and check
    /// the running accumulator equals the running sum of products.
    #[test]
    fn test_running_sum_of_products() -> miette::Result<()> {
        let pairs: Vec<(u128, u128)> = vec![
            (3, 4),
            (5, 7),
            (10, 10),
            (255, 255),
            (1, 1),
            (100, 50),
            (0, 17),
        ];
        let mut stream_in: Vec<In<8>> = vec![In {
            a: bits(0),
            b: bits(0),
            enable: false,
            clear: true,
        }];
        for &(a, b) in &pairs {
            stream_in.push(In {
                a: bits(a),
                b: bits(b),
                enable: true,
                clear: false,
            });
        }
        // Drain cycle to observe the final accumulator value.
        stream_in.push(In {
            a: bits(0),
            b: bits(0),
            enable: false,
            clear: false,
        });
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let uut = MacUnit::<8, 24>::default();
        let outputs = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output.raw())
            .collect::<Vec<_>>();
        let expected: u128 = pairs.iter().map(|(a, b)| a * b).sum();
        assert_eq!(*outputs.last().unwrap(), expected);
        Ok(())
    }

    // Tier 3 — HDL emission length sanity check
    //
    // Shrank by 122 characters when `XMul` stopped pre-widening its
    // operands to the result width: the two `Cast{Resize}` ops per
    // multiply are gone, and the emitted multiply is now `8x8` rather than
    // `16x16`. Operand widths are what decide a multiply's DSP cost, so
    // this widget benefits from that change even though it is not what the
    // change was written for.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = MacUnit::<8, 24>::default();
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        let expect = expect!["2094"];
        expect.assert_eq(&hdl.len().to_string());
        Ok(())
    }

    // Tier 4 — iverilog round-trip
    #[test]
    fn test_mac_hdl_works() -> miette::Result<()> {
        let uut = MacUnit::<8, 24>::default();
        let mut stream_in: Vec<In<8>> = vec![In {
            a: bits(0),
            b: bits(0),
            enable: false,
            clear: true,
        }];
        for k in 1u128..6 {
            stream_in.push(In {
                a: bits(k),
                b: bits(k * 3),
                enable: true,
                clear: false,
            });
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest
    #[test]
    fn test_mac_trace() -> miette::Result<()> {
        let uut = MacUnit::<8, 24>::default();
        let mut stream_in: Vec<In<8>> = vec![In {
            a: bits(0),
            b: bits(0),
            enable: false,
            clear: true,
        }];
        for k in 1u128..6 {
            stream_in.push(In {
                a: bits(k),
                b: bits(k * 3),
                enable: true,
                clear: false,
            });
        }
        stream_in.push(In {
            a: bits(0),
            b: bits(0),
            enable: false,
            clear: false,
        });
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("mac");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["4d3e1ff8fe0f7b0fe27da9df289989046af10b20cbef95f47094ced84bee9770"];
        let digest = vcd.dump_to_file(root.join("mac.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
