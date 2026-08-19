#![warn(missing_docs)]
//! `ComplexRealMixer` — an [`Iq`] carrier times a [`Real`] envelope.
//!
//! The transmit modulator, and by commutativity also the receive
//! down-converter's first stage (real ADC samples times a complex
//! carrier). **Two multiplies**, against four for the fully complex
//! case, because a real operand has no imaginary part to cross-multiply.
//!
//! ```text
//! (a + bi) · r = ar + bri
//! ```
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+ComplexRealMixer+---+
      |                      |
+---->+ carrier              |
      |               stream +----->
+---->+ envelope             |
      |              starved +----->
+---->+ downstream_ready     |
      +----------------------+
")]
//!
//! # Widths
//!
//! `A_W` and `B_W` are independent — there is no reason to force a
//! 14-bit envelope and an 18-bit carrier to a common width. The product
//! is exactly `A_W + B_W` bits and is carried at that width, so the
//! maximum-negative-squared case cannot overflow; see the module docs
//! on the absence of saturation.
//!
//! `OUT_W` is the narrowed result. `DROP` is how many bits the
//! narrowing discards and must equal `A_W + B_W − OUT_W`; a
//! compile-time assertion enforces it, because the kernel needs the
//! shift as a value rather than deriving it from the other three.
//!
//! # Starvation is reported, not handled
//!
//! Both inputs are isochronous — one sample per clock, phase-locked to
//! the timebase — so a cycle where only one side presents data cannot
//! happen in a correct design. Buffering the other side would mean an
//! elastic buffer with data-dependent occupancy, which makes the
//! transmit path's latency data-dependent and breaks the scheduler's
//! arithmetic.
//!
//! So a mismatch sets `starved` and the output is idle for that cycle.
//! Alignment itself is the scheduler's job: it issues each source's
//! control changes at the right lead time using the per-path latency
//! constants. What this widget provides is **detectability**.

//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/complex_real_mixer.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/complex_real_mixer.md")]

use rhdl::prelude::*;

use crate::core::dff;
use crate::dsp::iq::{Iq, Real};
use crate::rcstream::bus::{Item, RCStream};

use super::rounding::convergent;

/// `Iq × Real → Iq`, two multiplies.
#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
pub struct ComplexRealMixer<
    const A_W: usize,
    const B_W: usize,
    const OUT_W: usize,
    const PROD_W: usize,
    const DROP: usize,
> where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
    rhdl::bits::W<OUT_W>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
{
    /// Registered result, so the widget has one cycle of latency and
    /// the multiplier's carry chain does not extend downstream.
    out: dff::DFF<Iq<OUT_W>>,
    /// A cycle where the two inputs did not both present data.
    starved: dff::DFF<bool>,
}

/// Inputs to [`ComplexRealMixer`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const A_W: usize, const B_W: usize>
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
{
    /// The complex operand — a carrier on transmit, ADC samples on
    /// receive.
    pub carrier: Option<Item<Iq<A_W>, ()>>,
    /// The real operand — an envelope on transmit, a carrier component
    /// on receive.
    pub envelope: Option<Item<Real<B_W>, ()>>,
    /// Downstream's ready, per the `RCStream` contract.
    pub downstream_ready: bool,
}

/// Outputs from [`ComplexRealMixer`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const OUT_W: usize>
where
    rhdl::bits::W<OUT_W>: BitWidth,
{
    /// The modulated sample stream.
    pub stream: RCStream<Iq<OUT_W>, ()>,
    /// The inputs did not both present data on some cycle.
    pub starved: bool,
}

impl<const A_W: usize, const B_W: usize, const OUT_W: usize, const PROD_W: usize, const DROP: usize>
    SynchronousIO for ComplexRealMixer<A_W, B_W, OUT_W, PROD_W, DROP>
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
    rhdl::bits::W<OUT_W>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
{
    type I = In<A_W, B_W>;
    type O = Out<OUT_W>;
    type Kernel = complex_real_mixer_kernel<A_W, B_W, OUT_W, PROD_W, DROP>;
}

#[kernel]
#[doc(hidden)]
pub fn complex_real_mixer_kernel<
    const A_W: usize,
    const B_W: usize,
    const OUT_W: usize,
    const PROD_W: usize,
    const DROP: usize,
>(
    cr: ClockReset,
    i: In<A_W, B_W>,
    q: Q<A_W, B_W, OUT_W, PROD_W, DROP>,
) -> (Out<OUT_W>, D<A_W, B_W, OUT_W, PROD_W, DROP>)
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
    rhdl::bits::W<OUT_W>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
{
    let mut d = D::<A_W, B_W, OUT_W, PROD_W, DROP>::dont_care();
    d.out = q.out;
    d.starved = false;

    // Tuple patterns are not accepted in kernel match arms, so the two
    // streams are unpacked separately.
    // Zero rather than dont_care: these are read on the merge path, and
    // reading a dont_care is a partial-initialisation error.
    let mut carrier = Iq::<A_W> {
        re: signed::<A_W>(0),
        im: signed::<A_W>(0),
    };
    let mut have_carrier = false;
    match i.carrier {
        Some(c) => {
            carrier = c.data;
            have_carrier = true;
        }
        None => {}
    }

    let mut envelope = signed::<B_W>(0);
    let mut have_envelope = false;
    match i.envelope {
        Some(e) => {
            envelope = e.data.v;
            have_envelope = true;
        }
        None => {}
    }

    if have_carrier && have_envelope {
        // Widen both operands to the product width before multiplying,
        // so the multiply is exact and the maximum-negative-squared
        // case has room.
        let e = envelope.resize::<PROD_W>();
        let re = carrier.re.resize::<PROD_W>() * e;
        let im = carrier.im.resize::<PROD_W>() * e;
        d.out = Iq::<OUT_W> {
            re: convergent::<PROD_W, OUT_W, DROP>(re),
            im: convergent::<PROD_W, OUT_W, DROP>(im),
        };
    } else {
        // Isochronous inputs, so this cannot happen in a correct
        // design.  Report it; do not buffer the other side.  The sample
        // emitted is zero -- the idle value for a transmit chain, and a
        // defined one rather than dont_care, which would be read back
        // through `q.out` next cycle.
        d.starved = true;
        d.out = Iq::<OUT_W> {
            re: signed::<OUT_W>(0),
            im: signed::<OUT_W>(0),
        };
    }

    let o = Out::<OUT_W> {
        stream: RCStream::<Iq<OUT_W>, ()> {
            data: Some(Item::<Iq<OUT_W>, ()> {
                data: q.out,
                frame: (),
            }),
            ready: i.downstream_ready,
        },
        starved: q.starved,
    };

    if cr.reset.any() {
        d.out = Iq::<OUT_W> {
            re: signed::<OUT_W>(0),
            im: signed::<OUT_W>(0),
        };
        d.starved = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;

    // Carrier 18 bits, envelope 16, output 18. Product is 34, so the
    // narrowing drops 16.
    const A: usize = 18;
    const B: usize = 16;
    const O: usize = 18;
    const P: usize = 34;
    const DR: usize = 16;
    type Uut = ComplexRealMixer<A, B, O, P, DR>;

    const _: () = assert!(P == A + B && DR == P - O);

    fn feed(re: i128, im: i128, env: i128) -> In<A, B> {
        In::<A, B> {
            carrier: Some(Item::<Iq<A>, ()> {
                data: Iq::<A> {
                    re: signed::<A>(re),
                    im: signed::<A>(im),
                },
                frame: (),
            }),
            envelope: Some(Item::<Real<B>, ()> {
                data: Real::<B> {
                    v: signed::<B>(env),
                },
                frame: (),
            }),
            downstream_ready: true,
        }
    }

    fn gap() -> In<A, B> {
        In::<A, B> {
            carrier: None,
            envelope: Some(Item::<Real<B>, ()> {
                data: Real::<B> {
                    v: signed::<B>(100),
                },
                frame: (),
            }),
            downstream_ready: true,
        }
    }

    fn run(seq: Vec<In<A, B>>) -> Vec<Out<O>> {
        let uut = Uut::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect()
    }

    fn got(o: &Out<O>) -> (i128, i128) {
        match o.stream.data {
            Some(item) => (item.data.re.raw(), item.data.im.raw()),
            None => panic!("the mixer must emit every cycle"),
        }
    }

    /// The reference: exact product, then convergent narrowing.
    fn want(re: i128, im: i128, env: i128) -> (i128, i128) {
        let conv = |v: i128| -> i128 {
            let half = 1i128 << (DR - 1);
            let lsbs = v & ((1i128 << DR) - 1);
            let q = (v + half) >> DR;
            if lsbs == half && q % 2 != 0 { q - 1 } else { q }
        };
        (conv(re * env), conv(im * env))
    }

    #[test]
    fn default_construction() {
        let _uut = Uut::default();
    }

    /// Tier 1/2 — the product is right across sign combinations.
    #[test]
    fn the_product_is_correct() {
        let cases = [
            (100_000i128, 50_000i128, 20_000i128),
            (-100_000, 50_000, 20_000),
            (100_000, -50_000, -20_000),
            (-100_000, -50_000, -20_000),
            (131_070, 131_070, 32_767),
            (-131_070, -131_070, -32_768),
        ];
        let seq: Vec<In<A, B>> = cases.iter().map(|(r, i, e)| feed(*r, *i, *e)).collect();
        let out = run(seq);
        let mut checked = 0;
        for (k, (r, i, e)) in cases.iter().enumerate() {
            // one cycle of latency plus one reset cycle
            if k + 2 < out.len() {
                assert_eq!(got(&out[k + 2]), want(*r, *i, *e), "case {k}: {r} {i} {e}");
                checked += 1;
            }
        }
        assert!(checked >= 4, "only {checked} cases actually compared");
    }

    /// **Tier 4 — the emitted Verilog agrees.**
    ///
    /// The load-bearing test for this widget. Both operands are
    /// extracted from `Option` payloads, which is exactly the shape that
    /// silently loses signedness elsewhere in the tree
    /// (`dsp::nco::modulation`), and an unsigned multiply would still
    /// produce plausible-looking numbers.
    #[test]
    fn test_complex_real_mixer_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let seq: Vec<In<A, B>> = (0..24i128)
            .map(|k| feed((k - 12) * 8000, (12 - k) * 6000, (k - 12) * 2000))
            .collect();
        let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// The resource claim, made checkable.
    ///
    /// Two multiplies, not four — the entire justification for having a
    /// separate widget for a real operand rather than tying an `Iq`'s
    /// `im` to zero.
    #[test]
    fn multiplier_count_is_as_claimed() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let mults = hdl.matches(" * ").count();
        assert_eq!(
            mults, 2,
            "expected exactly 2 multiplies for Iq x Real; found {mults}.\n{hdl}"
        );
        Ok(())
    }

    /// Tier 3 — HDL emission shape.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let shape = hdl
            .lines()
            .filter(|l| l.starts_with("module "))
            .map(|l| l.split('(').next().unwrap().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let expect = expect![[r#"
            module top
            module top_out
            module top_starved"#]];
        expect.assert_eq(&shape);
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn test_complex_real_mixer_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let seq: Vec<In<A, B>> = (0..24i128)
            .map(|k| feed((k - 12) * 8000, (12 - k) * 6000, (k - 12) * 2000))
            .collect();
        let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("complex_real_mixer");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["e1aaf89dade4e1c28c710a569e5c13a392bb29e7d6509f7fd2e749756bbb53ef"];
        let digest = vcd
            .dump_to_file(root.join("complex_real_mixer.vcd"))
            .unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    /// The output has no DC offset, the property truncation destroys.
    #[test]
    fn the_output_has_no_dc_offset() {
        const PAIRS: usize = 64;
        let mut seq: Vec<In<A, B>> = Vec::new();
        for k in 1..=PAIRS as i128 {
            seq.push(feed(k * 1500, k * 700, 9_000));
            seq.push(feed(-k * 1500, -k * 700, 9_000));
        }
        seq.extend(vec![feed(0, 0, 0); 4]);
        let out = run(seq);
        let samples = 2 * PAIRS;
        let (sr, si) = out
            .iter()
            .skip(2)
            .take(samples)
            .map(got)
            .fold((0i128, 0i128), |(a, b), (r, i)| (a + r, b + i));
        let n = samples as i128;
        let tolerance = n / 8;
        assert!(
            sr.abs() <= tolerance && si.abs() <= tolerance,
            "output mean is not zero: re {sr}, im {si} over {n}, tolerance {tolerance}"
        );
    }

    /// A cycle where only one input presents data is reported.
    #[test]
    fn starvation_is_reported() {
        let mut seq = vec![feed(1000, 2000, 300); 4];
        seq.push(gap());
        seq.extend(vec![feed(1000, 2000, 300); 4]);
        let out = run(seq);
        assert!(
            out.iter().any(|o| o.starved),
            "a starved cycle was not reported"
        );
        assert!(out.iter().any(|o| !o.starved), "starved is stuck high");
    }
}
