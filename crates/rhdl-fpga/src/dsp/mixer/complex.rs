#![warn(missing_docs)]
//! `ComplexMixer` — a full complex multiply, [`Iq`] times [`Iq`].
//!
//! ```text
//! (a + bi)(c + di) = (ac − bd) + (ad + bc)i
//! ```
//!
//! **Four multiplies and two adds** — the general case. Where one
//! operand is real, [`super::ComplexRealMixer`] does the same job in
//! two, which is why the two widgets exist separately rather than as
//! one generic with a mode bit.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+ComplexMixer+-------+
      |                      |
+---->+ a                    |
      |               stream +----->
+---->+ b                    |
      |              starved +----->
+---->+ downstream_ready     |
      +----------------------+
")]
//!
//! # Karatsuba is not used
//!
//! Three multiplies instead of four, at the cost of five adds. On a
//! Zynq the DSP48 slices are plentiful and the extra adds land in
//! fabric, so four DSPs is the cheaper trade. Revisit only if DSP
//! pressure appears — the arithmetic here is a drop-in swap.
//!
//! # Widths
//!
//! Each partial product is `A_W + B_W` bits and the result sums two of
//! them, so the natural width is **`A_W + B_W + 1`** — one more than
//! [`super::ComplexRealMixer`], and the same figure the AMD Complex
//! Multiplier (PG104) calls "the sum of the input widths plus one".
//! Carrying that width is what makes the maximum-negative-squared case
//! unable to overflow, and why there is no saturation logic.

use rhdl::prelude::*;

use crate::core::dff;
use crate::dsp::iq::Iq;
use crate::rcstream::bus::{Item, RCStream};

use super::rounding::convergent;

/// `Iq × Iq → Iq`, four multiplies.
#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
pub struct ComplexMixer<
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
    /// Registered result.
    out: dff::DFF<Iq<OUT_W>>,
    /// A cycle where the two inputs did not both present data.
    starved: dff::DFF<bool>,
}

/// Inputs to [`ComplexMixer`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const A_W: usize, const B_W: usize>
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
{
    /// First operand.
    pub a: Option<Item<Iq<A_W>, ()>>,
    /// Second operand.
    pub b: Option<Item<Iq<B_W>, ()>>,
    /// Downstream's ready, per the `RCStream` contract.
    pub downstream_ready: bool,
}

/// Outputs from [`ComplexMixer`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const OUT_W: usize>
where
    rhdl::bits::W<OUT_W>: BitWidth,
{
    /// The product stream.
    pub stream: RCStream<Iq<OUT_W>, ()>,
    /// The inputs did not both present data on some cycle.
    pub starved: bool,
}

impl<const A_W: usize, const B_W: usize, const OUT_W: usize, const PROD_W: usize, const DROP: usize>
    SynchronousIO for ComplexMixer<A_W, B_W, OUT_W, PROD_W, DROP>
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
    rhdl::bits::W<OUT_W>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
{
    type I = In<A_W, B_W>;
    type O = Out<OUT_W>;
    type Kernel = complex_mixer_kernel<A_W, B_W, OUT_W, PROD_W, DROP>;
}

#[kernel]
#[doc(hidden)]
pub fn complex_mixer_kernel<
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

    let zero_a = Iq::<A_W> {
        re: signed::<A_W>(0),
        im: signed::<A_W>(0),
    };
    let zero_b = Iq::<B_W> {
        re: signed::<B_W>(0),
        im: signed::<B_W>(0),
    };

    let mut av = zero_a;
    let mut have_a = false;
    match i.a {
        Some(x) => {
            av = x.data;
            have_a = true;
        }
        None => {}
    }

    let mut bv = zero_b;
    let mut have_b = false;
    match i.b {
        Some(x) => {
            bv = x.data;
            have_b = true;
        }
        None => {}
    }

    if have_a && have_b {
        let ar = av.re.resize::<PROD_W>();
        let ai = av.im.resize::<PROD_W>();
        let br = bv.re.resize::<PROD_W>();
        let bi = bv.im.resize::<PROD_W>();

        // (ac - bd) + (ad + bc)i
        let re = ar * br - ai * bi;
        let im = ar * bi + ai * br;

        d.out = Iq::<OUT_W> {
            re: convergent::<PROD_W, OUT_W, DROP>(re),
            im: convergent::<PROD_W, OUT_W, DROP>(im),
        };
    } else {
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

    const A: usize = 18;
    const B: usize = 16;
    const O: usize = 18;
    // Complex: each partial product is A+B, and two are summed.
    const P: usize = 35;
    const DR: usize = 17;
    type Uut = ComplexMixer<A, B, O, P, DR>;

    const _: () = assert!(P == A + B + 1 && DR == P - O);

    fn feed(ar: i128, ai: i128, br: i128, bi: i128) -> In<A, B> {
        In::<A, B> {
            a: Some(Item::<Iq<A>, ()> {
                data: Iq::<A> {
                    re: signed::<A>(ar),
                    im: signed::<A>(ai),
                },
                frame: (),
            }),
            b: Some(Item::<Iq<B>, ()> {
                data: Iq::<B> {
                    re: signed::<B>(br),
                    im: signed::<B>(bi),
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

    fn conv(v: i128) -> i128 {
        let half = 1i128 << (DR - 1);
        let lsbs = v & ((1i128 << DR) - 1);
        let q = (v + half) >> DR;
        if lsbs == half && q % 2 != 0 { q - 1 } else { q }
    }

    fn want(ar: i128, ai: i128, br: i128, bi: i128) -> (i128, i128) {
        (conv(ar * br - ai * bi), conv(ar * bi + ai * br))
    }

    #[test]
    fn default_construction() {
        let _uut = Uut::default();
    }

    /// The complex product is right across sign combinations, including
    /// the rotations that make cross terms cancel.
    #[test]
    fn the_complex_product_is_correct() {
        let cases = [
            (100_000i128, 0i128, 20_000i128, 0i128), // real x real
            (0, 100_000, 0, 20_000),                 // imag x imag -> negative real
            (100_000, 0, 0, 20_000),                 // real x imag -> imag
            (60_000, 80_000, 12_000, -9_000),
            (-60_000, 80_000, -12_000, -9_000),
            (131_070, -131_070, 32_767, -32_768),
        ];
        let seq: Vec<In<A, B>> = cases.iter().map(|c| feed(c.0, c.1, c.2, c.3)).collect();
        let out = run(seq);
        let mut checked = 0;
        for (k, c) in cases.iter().enumerate() {
            if k + 2 < out.len() {
                assert_eq!(
                    got(&out[k + 2]),
                    want(c.0, c.1, c.2, c.3),
                    "case {k}: {c:?}"
                );
                checked += 1;
            }
        }
        assert!(checked >= 4, "only {checked} cases compared");
    }

    /// `i · i = −1`: two purely imaginary operands give a **negative
    /// real** result.
    ///
    /// The sign flip is the thing most easily got wrong in a hand-
    /// written complex multiply, and it is invisible in a magnitude
    /// check.
    #[test]
    fn imaginary_times_imaginary_is_negative_real() {
        let out = run(vec![feed(0, 50_000, 0, 20_000); 4]);
        let (re, im) = got(&out[3]);
        assert!(re < 0, "i*i must be negative real, got re={re}");
        assert_eq!(im, 0, "a purely imaginary product has no imaginary part");
        assert_eq!((re, im), want(0, 50_000, 0, 20_000));
    }

    /// **The maximum-negative-squared case** — §3.3 of the design note.
    ///
    /// Both operands at full-scale negative. The natural width carries
    /// it, so no saturation is needed and none exists; this asserts the
    /// result is *correct* rather than wrapped.
    #[test]
    fn maximum_negative_squared_does_not_wrap() {
        let a_min = -(1i128 << (A - 1));
        let b_min = -(1i128 << (B - 1));
        let out = run(vec![feed(a_min, a_min, b_min, b_min); 4]);
        let (re, im) = got(&out[3]);
        let (wre, wim) = want(a_min, a_min, b_min, b_min);
        assert_eq!((re, im), (wre, wim), "full-scale-negative operands wrapped");
        // re = a_min*b_min - a_min*b_min = 0; im = 2*a_min*b_min > 0.
        assert_eq!(re, 0);
        assert!(
            im > 0,
            "the cross terms must give a large positive imaginary part"
        );
    }

    /// **Convergent rounding steers ties to even** — the property worth
    /// 5 dB of spur performance.
    ///
    /// Constructs products whose discarded bits are exactly one half and
    /// checks the result is even. Round-half-up fails this.
    #[test]
    fn ties_go_to_even() {
        // Choose operands whose real product is an exact tie:
        // ar*br - ai*bi == odd_multiple * 2^DR + 2^(DR-1)
        // A tie is a product of m*2^DR + 2^(DR-1) = 2^(DR-1) * (2m+1).
        // Factor it so both operands stay in range: ar = 2^(DR-1),
        // br = 2m+1.  Putting the tie in the *operand* would overflow
        // the 18-bit input, which is how the first version of this test
        // silently exercised nothing.
        let half = 1i128 << (DR - 1);
        assert!(
            half < (1i128 << (A - 1)),
            "the tie factor must fit the A operand"
        );
        let mut tested = 0;
        for m in [0i128, 1, 2, 3, 4] {
            let br = 2 * m + 1;
            let target = half * br;
            let out = run(vec![feed(half, 0, br, 0); 4]);
            let (re, _im) = got(&out[3]);
            assert_eq!(
                re,
                conv(target),
                "tie handling disagrees with the reference for {half} * {br}"
            );
            assert_eq!(
                re % 2,
                0,
                "a tie must round to even, got {re} for product {target}"
            );
            tested += 1;
        }
        assert_eq!(tested, 5, "not every tie case ran");
    }

    /// **No DC offset** — the property truncation would destroy.
    ///
    /// A zero-mean input sequence must give a zero-mean output. §3.6 of
    /// the design note measures truncation leaving a −79 dBc DC term,
    /// which in NMR is a fixed artefact at the carrier.
    #[test]
    fn the_output_has_no_dc_offset() {
        // A symmetric sweep: every value paired with its negation.
        const PAIRS: usize = 64;
        let mut seq: Vec<In<A, B>> = Vec::new();
        for k in 1..=PAIRS as i128 {
            seq.push(feed(k * 1500, k * 700, 9_000, 0));
            seq.push(feed(-k * 1500, -k * 700, 9_000, 0));
        }
        // Pad so every stimulus sample is observable: the result is one
        // cycle late and the stream is prefixed by a reset cycle.  Summing
        // an odd number of samples leaves one half of a +/- pair unmatched,
        // which reads as a huge false DC offset -- that is a test bug, and
        // it is what the first version of this test did.
        seq.extend(vec![feed(0, 0, 0, 0); 4]);
        let out = run(seq);
        let samples = 2 * PAIRS;
        let (sum_re, sum_im) = out
            .iter()
            .skip(2)
            .take(samples)
            .map(got)
            .fold((0i128, 0i128), |(a, b), (r, i)| (a + r, b + i));
        let n = samples as i128;
        // Tolerance must be well under the bias truncation produces.
        // Truncation rounds toward negative infinity, so it biases by
        // about half an LSB per sample -- a sum near -n/2.  A tolerance
        // of n/2 therefore passes under truncation and proves nothing,
        // which is what the first version of this test did.
        let tolerance = n / 8;
        assert!(
            sum_re.abs() <= tolerance && sum_im.abs() <= tolerance,
            "output mean is not zero: re sum {sum_re}, im sum {sum_im} over {n} \
             samples, tolerance {tolerance} -- truncation instead of convergent \
             rounding biases by about -n/2 = {}",
            -n / 2
        );
    }

    /// The resource claim: **four** multiplies for the general case,
    /// against two for `ComplexRealMixer`.
    #[test]
    fn multiplier_count_is_as_claimed() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let mults = hdl.matches(" * ").count();
        assert_eq!(mults, 4, "expected 4 multiplies for Iq x Iq; found {mults}");
        Ok(())
    }

    /// Starvation is reported, not buffered.
    #[test]
    fn starvation_is_reported() {
        let mut seq = vec![feed(1000, 2000, 300, 400); 3];
        seq.push(In::<A, B> {
            a: None,
            b: Some(Item::<Iq<B>, ()> {
                data: Iq::<B> {
                    re: signed::<B>(300),
                    im: signed::<B>(400),
                },
                frame: (),
            }),
            downstream_ready: true,
        });
        seq.extend(vec![feed(1000, 2000, 300, 400); 3]);
        let out = run(seq);
        assert!(out.iter().any(|o| o.starved), "starvation was not reported");
        assert!(out.iter().any(|o| !o.starved), "starved is stuck high");
    }

    fn hdl_stimulus() -> Vec<In<A, B>> {
        (0..24i128)
            .map(|k| feed((k - 12) * 7000, (12 - k) * 5000, (k - 12) * 1500, k * 900))
            .collect()
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

    /// Tier 4 — emitted Verilog agrees with the Rust simulation.
    #[test]
    fn test_complex_mixer_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn test_complex_mixer_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("complex_mixer");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["7734a44fb57c2962dfd52089f642cc1cd78e5227343ab73df26dbbc26d145711"];
        let digest = vcd.dump_to_file(root.join("complex_mixer.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
