#![warn(missing_docs)]
//! `RealPartMixer` — `Iq × Iq → Real`, **two** multiplies.
//!
//! The output stage of a transmitter that drives a single DAC. Given a
//! complex envelope and a complex carrier, the real passband signal is
//!
//! ```text
//!   Re{ env · e^{jwt} } = env.re · cos(wt) − env.im · sin(wt)
//! ```
//!
//! which is two of the four products a full complex multiply computes.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+RealPartMixer+---------+
      |                         |
+---->+ a                       |
      |  Option<Item<Iq<AW>,F>> |
      |                  stream |
+---->+ b     RCStream<Real<OW>,+----->
      |  Option<Item<Iq<BW>,F>>F|
      |                         |
+---->+ downstream_ready starved+----->
      |          frame_mismatch +----->
      |                 overrun +----->
      +-------------------------+
")]
//!
//! # Why this exists rather than `.re` on a [`ComplexMixer`]
//!
//! Taking the real part of
//! [`ComplexMixer`](super::complex::ComplexMixer)'s output is the
//! obvious spelling and it computes `ad + bc` as well, then throws it
//! away. Two DSP slices, permanently, for a value nothing reads.
//!
//! The tempting reply is that synthesis will prune it. It will not
//! reliably, and more to the point [`super::super::mixer`]'s module
//! docs already record the decision this widget follows: **a resource
//! claim that cannot be tested is not a resource claim.** With a
//! separate widget the multiplier count is structural, visible in the
//! Tier 3 snapshot, and asserted by `multiplier_count_is_as_claimed` —
//! the same treatment the other three mixers get.
//!
//! So the multiplier table in [`super::super::mixer`] gains a row:
//!
//! | A | B | result | multiplies |
//! |---|---|---|---|
//! | `Iq` | `Iq` | `Iq` | 4 |
//! | `Iq` | `Iq` | **`Real`** | **2** |
//! | `Iq` | `Real` | `Iq` | 2 |
//! | `Real` | `Real` | `Real` | 1 |
//!
//! Note the second and third rows cost the same and are not
//! interchangeable: `Iq × Real` modulates a *real* envelope onto a
//! complex carrier and keeps both quadratures, which is amplitude
//! modulation. This widget modulates a *complex* envelope and keeps one
//! quadrature, which is single-sideband-capable and is what a digital
//! up-converter needs.
//!
//! # `PROD_W` must be `A_W + B_W + 1`, and construction checks it
//!
//! One more bit than a single product needs, because this widget forms
//! a **difference of two** of them: `ar·br` and `ai·bi` can both reach
//! `2^(A+B-2)` with the same sign, so `ar·br − ai·bi` reaches
//! `2^(A+B-1)` and needs `A+B+1` bits once
//! [`convergent`](super::rounding::convergent) adds its half-LSB on
//! top.
//!
//! `ComplexMixer` has the identical requirement — it is the same
//! difference — and `ComplexRealMixer` does not, forming only one
//! product. The requirement was recorded in `convergent`'s
//! preconditions and nothing checked it, and getting it wrong is
//! silent: at `A_W = B_W = 8, PROD_W = 16` the full-scale case
//! `(-128 − 128j)(−128 + 127j)` gives `+32640`, the half-LSB pushes it
//! to `32768`, `SignedBits<16>` wraps it to `-32768`, and a
//! transmitter's largest sample comes out with the wrong sign. Which is
//! how this paragraph came to be written.
//!
//! So [`Default`] asserts it. `DROP >= 1` too, which is `convergent`'s
//! other precondition.
//!
//! # Rounding, saturation and framing follow the other mixers
//!
//! Convergent rounding on the narrowing step, chosen by the spur
//! measurement in [`super::super::mixer`]. No saturation, because the
//! product is carried at its natural width. Framing rides with the
//! product, and a disagreement between the two operands' markers is
//! reported rather than silently resolved — see [`Out::frame_mismatch`].
//!
//! All three of those are decisions the mixer module already made and
//! documented; this widget inherits them rather than relitigating them.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/real_part_mixer.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/real_part_mixer.md")]

use rhdl::prelude::*;

use crate::core::constant::Constant;
use crate::core::dff;
use crate::dsp::iq::{Iq, Real};
use crate::rcstream::bus::{Item, RCStream};

use super::rounding::convergent;

/// `Iq × Iq → Real`, two multiplies.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RealPartMixer<
    F: Digital + Default,
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
    /// Registered result **and the marker travelling with it**.
    ///
    /// One register rather than two, so the marker cannot come adrift
    /// from the sample it belongs to.
    out: dff::DFF<Item<Real<OUT_W>, F>>,
    /// A cycle where the two inputs did not both present data.
    starved: dff::DFF<bool>,
    /// The two inputs disagreed about framing.
    mismatch: dff::DFF<bool>,
    /// The idle item: a zero sample carrying `F::default()`.
    ///
    /// Needed because reset and starvation must both drive `out` with
    /// an *un-marked* item; a don't-care would let `F = SyncMark`
    /// assert a spurious anchor on exactly the cycles where the data is
    /// known to be invalid. Bundling the marker with the zero sample
    /// keeps this constant `OUT_W + |F|` bits wide for every `F`, which
    /// keeps it clear of the zero-width issues recorded in
    /// [`super::complex::ComplexMixer`]'s `idle` field.
    idle: Constant<Item<Real<OUT_W>, F>>,
}

impl<
    F: Digital + Default,
    const A_W: usize,
    const B_W: usize,
    const OUT_W: usize,
    const PROD_W: usize,
    const DROP: usize,
> Default for RealPartMixer<F, A_W, B_W, OUT_W, PROD_W, DROP>
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
    rhdl::bits::W<OUT_W>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
{
    fn default() -> Self {
        // Checked, not trusted: an under-width product wraps silently
        // on the largest sample a transmitter can send. See the module
        // docs for the worked case.
        assert!(
            PROD_W >= A_W + B_W + 1,
            "PROD_W must be at least A_W + B_W + 1: this mixer forms a difference \
             of two products, and `convergent` adds half an LSB on top of it"
        );
        assert!(DROP >= 1, "DROP must be at least one to round at all");
        Self {
            out: dff::DFF::default(),
            starved: dff::DFF::default(),
            mismatch: dff::DFF::default(),
            idle: Constant::new(Item::<Real<OUT_W>, F>::default()),
        }
    }
}

/// Inputs to [`RealPartMixer`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<F: Digital + Default, const A_W: usize, const B_W: usize>
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
{
    /// First operand — the complex envelope, on transmit.
    pub a: Option<Item<Iq<A_W>, F>>,
    /// Second operand — the complex carrier, on transmit.
    pub b: Option<Item<Iq<B_W>, F>>,
    /// Downstream's ready, per the `RCStream` contract.
    pub downstream_ready: bool,
}

/// Outputs from [`RealPartMixer`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<F: Digital + Default, const OUT_W: usize>
where
    rhdl::bits::W<OUT_W>: BitWidth,
{
    /// The real passband stream — what a single DAC wants.
    ///
    /// `stream.ready` is vacuously `true`: this mixer consumes on every
    /// cycle and has no stall path.
    pub stream: RCStream<Real<OUT_W>, F>,
    /// The inputs did not both present data on some cycle.
    pub starved: bool,
    /// **The two inputs disagreed about framing.**
    ///
    /// For a transmit chain this means the envelope and the carrier
    /// disagree about where the burst starts, so the transmitted phase
    /// is relative to an origin the caller did not intend. The product
    /// still carries the `a` side's marker on a mismatched cycle; that
    /// value is not trustworthy and this flag is the one to act on.
    pub frame_mismatch: bool,
    /// A sample was presented while `downstream_ready` was low, and is
    /// gone.
    pub overrun: bool,
}

impl<
    F: Digital + Default,
    const A_W: usize,
    const B_W: usize,
    const OUT_W: usize,
    const PROD_W: usize,
    const DROP: usize,
> SynchronousIO for RealPartMixer<F, A_W, B_W, OUT_W, PROD_W, DROP>
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
    rhdl::bits::W<OUT_W>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
{
    type I = In<F, A_W, B_W>;
    type O = Out<F, OUT_W>;
    type Kernel = real_part_mixer_kernel<F, A_W, B_W, OUT_W, PROD_W, DROP>;
}

#[kernel]
#[doc(hidden)]
pub fn real_part_mixer_kernel<
    F: Digital + Default,
    const A_W: usize,
    const B_W: usize,
    const OUT_W: usize,
    const PROD_W: usize,
    const DROP: usize,
>(
    cr: ClockReset,
    i: In<F, A_W, B_W>,
    q: Q<F, A_W, B_W, OUT_W, PROD_W, DROP>,
) -> (Out<F, OUT_W>, D<F, A_W, B_W, OUT_W, PROD_W, DROP>)
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
    rhdl::bits::W<OUT_W>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
{
    let mut d = D::<F, A_W, B_W, OUT_W, PROD_W, DROP>::dont_care();
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

    // The `None` arms carry real zeros and the real un-marked marker,
    // deliberately *not* `Item::dont_care()`. Both arms of a mux always
    // evaluate in hardware, so a don't-care operand reaches the
    // multiplier: the Rust simulator reads it as zero and `iverilog`
    // reads it as `x` and propagates. See
    // [`super::complex::ComplexMixer`] for the failure that taught this.
    let (have_a, item_a) = match i.a {
        Some(it) => (true, it),
        None => (
            false,
            Item::<Iq<A_W>, F> {
                data: zero_a,
                frame: q.idle.frame,
            },
        ),
    };
    let (have_b, item_b) = match i.b {
        Some(it) => (true, it),
        None => (
            false,
            Item::<Iq<B_W>, F> {
                data: zero_b,
                frame: q.idle.frame,
            },
        ),
    };
    let av = item_a.data;
    let bv = item_b.data;
    let af = item_a.frame;
    let bf = item_b.frame;

    // Gated on both operands being present: a starved cycle is already
    // reported, and comparing a real marker against a placeholder would
    // double-report it as a framing fault.
    d.mismatch = have_a && have_b && (af != bf);

    if have_a && have_b {
        let ar = av.re.resize::<PROD_W>();
        let ai = av.im.resize::<PROD_W>();
        let br = bv.re.resize::<PROD_W>();
        let bi = bv.im.resize::<PROD_W>();

        // Re{(ar + ai·j)(br + bi·j)} = ar·br − ai·bi.
        //
        // **Two multiplies, and the imaginary part is never formed.**
        // `ar·bi + ai·br` is the value a full complex multiply would
        // also compute and this widget exists not to.
        let re = ar * br - ai * bi;

        d.out = Item::<Real<OUT_W>, F> {
            data: Real::<OUT_W> {
                v: convergent::<PROD_W, OUT_W, DROP>(re),
            },
            frame: af,
        };
    } else {
        d.starved = true;
        // Zero sample, un-marked: a cycle with no valid product must not
        // anchor anything.
        d.out = q.idle;
    }

    let mut o = Out::<F, OUT_W> {
        stream: RCStream::<Real<OUT_W>, F> {
            data: Some(q.out),
            // Vacuously true: `out` is overwritten every cycle, so the
            // mixer is always ready to accept from upstream.
            ready: true,
        },
        starved: q.starved,
        overrun: !i.downstream_ready,
        frame_mismatch: q.mismatch,
    };

    if cr.reset.any() {
        d.out = q.idle;
        d.starved = false;
        d.mismatch = false;
        o.overrun = false;
        o.frame_mismatch = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::sync::SyncMark;
    use expect_test::expect;

    const AW: usize = 8;
    const BW: usize = 8;
    const OW: usize = 9;
    // A_W + B_W + 1, per the module docs. `16` here wraps the
    // full-scale case, which is how the requirement was found.
    const PW: usize = 17;
    // The tree's convention: DROP = PROD_W - OUT_W.
    const DROP: usize = 8;
    type Uut = RealPartMixer<SyncMark, AW, BW, OW, PW, DROP>;

    fn item_a(re: i128, im: i128, mark: bool) -> Option<Item<Iq<AW>, SyncMark>> {
        Some(Item::<Iq<AW>, SyncMark> {
            data: Iq::<AW> {
                re: signed::<AW>(re),
                im: signed::<AW>(im),
            },
            frame: SyncMark { sync: mark },
        })
    }
    fn item_b(re: i128, im: i128, mark: bool) -> Option<Item<Iq<BW>, SyncMark>> {
        Some(Item::<Iq<BW>, SyncMark> {
            data: Iq::<BW> {
                re: signed::<BW>(re),
                im: signed::<BW>(im),
            },
            frame: SyncMark { sync: mark },
        })
    }

    /// Convergent rounding of `x / 2^DROP`, as the widget computes it.
    fn round(x: i128) -> i128 {
        let half = 1i128 << (DROP - 1);
        let q = x >> DROP;
        let r = x - (q << DROP);
        if r > half || (r == half && (q & 1) == 1) {
            q + 1
        } else {
            q
        }
    }

    #[test]
    fn default_construction() {
        let _ = Uut::default();
    }

    /// An idle cycle to let the last registered result emerge.
    fn drain() -> In<SyncMark, AW, BW> {
        In::<SyncMark, AW, BW> {
            a: item_a(0, 0, false),
            b: item_b(0, 0, false),
            downstream_ready: true,
        }
    }

    /// **Tier 1: the kernel computes the real part of the product.**
    ///
    /// Read off `d`, because the result is registered and `o` carries
    /// the *previous* cycle's value.
    #[test]
    fn the_kernel_computes_the_real_part() {
        for &(ar, ai, br, bi) in &[
            (10i128, 0i128, 20i128, 0i128),
            (10, 5, 20, 30),
            (-40, 17, 33, -12),
            (127, -128, -128, 127),
            (0, 0, 100, 100),
        ] {
            let (_, d) = real_part_mixer_kernel::<SyncMark, AW, BW, OW, PW, DROP>(
                ClockReset::dont_care(),
                In::<SyncMark, AW, BW> {
                    a: item_a(ar, ai, false),
                    b: item_b(br, bi, false),
                    downstream_ready: true,
                },
                Q::<SyncMark, AW, BW, OW, PW, DROP> {
                    out: Item::<Real<OW>, SyncMark>::default(),
                    starved: false,
                    mismatch: false,
                    idle: Item::<Real<OW>, SyncMark>::default(),
                },
            );
            assert_eq!(
                d.out.data.v.raw(),
                round(ar * br - ai * bi),
                "({ar},{ai}) x ({br},{bi})"
            );
        }
    }

    /// **Tier 2: and the assembled widget agrees, through the
    /// simulator.**
    #[test]
    fn it_computes_the_real_part() {
        let uut = Uut::default();
        let cases = [
            (10i128, 0i128, 20i128, 0i128),
            (10, 5, 20, 30),
            (-40, 17, 33, -12),
            (127, -128, -128, 127),
        ];
        let mut seq: Vec<In<SyncMark, AW, BW>> = cases
            .iter()
            .map(|&(ar, ai, br, bi)| In::<SyncMark, AW, BW> {
                a: item_a(ar, ai, false),
                b: item_b(br, bi, false),
                downstream_ready: true,
            })
            .collect();
        seq.push(drain());
        let got: Vec<i128> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.stream.data.map(|it| it.data.v.raw()))
            .collect();
        // One reset cycle plus one register of latency.
        for (k, &(ar, ai, br, bi)) in cases.iter().enumerate() {
            let want = round(ar * br - ai * bi);
            assert_eq!(got[k + 2], want, "case {k}: ({ar},{ai})x({br},{bi})");
        }
    }

    /// **Two multiplies, and the claim is structural.**
    ///
    /// The reason this widget exists rather than `.re` on a
    /// [`super::super::complex::ComplexMixer`], which would emit four.
    #[test]
    fn multiplier_count_is_as_claimed() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let mults = hdl.matches(" * ").count();
        assert_eq!(
            mults, 2,
            "expected 2 multiplies for Re{{Iq x Iq}}; found {mults}"
        );
        Ok(())
    }

    /// And a full complex mixer at the same widths emits four, so the
    /// saving is measured rather than asserted.
    #[test]
    fn the_full_complex_mixer_costs_twice_as_much() -> miette::Result<()> {
        let full = super::super::complex::ComplexMixer::<SyncMark, AW, BW, OW, PW, DROP>::default();
        let hdl = full.descriptor("top".into())?.hdl()?.modules.pretty();
        assert_eq!(hdl.matches(" * ").count(), 4);
        Ok(())
    }

    /// A missing operand is reported and produces an un-marked zero.
    #[test]
    fn starvation_is_reported_and_emits_an_unmarked_zero() {
        let uut = Uut::default();
        let seq = vec![
            In::<SyncMark, AW, BW> {
                a: item_a(10, 10, true),
                b: None,
                downstream_ready: true,
            },
            In::<SyncMark, AW, BW> {
                a: item_a(10, 10, false),
                b: item_b(10, 10, false),
                downstream_ready: true,
            },
        ];
        let got: Vec<(i128, bool, bool)> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| {
                (
                    s.output.stream.data.map(|it| it.data.v.raw()).unwrap_or(0),
                    s.output
                        .stream
                        .data
                        .map(|it| it.frame.sync)
                        .unwrap_or(false),
                    s.output.starved,
                )
            })
            .collect();
        // Cycle 1 starves; its effect lands on cycle 2.
        assert!(got[2].2, "starvation reported");
        assert_eq!(got[2].0, 0, "and emits zero");
        assert!(
            !got[2].1,
            "un-marked: a cycle with no valid product must not anchor"
        );
    }

    /// **Disagreeing markers are reported, not resolved.**
    #[test]
    fn a_framing_disagreement_is_reported() {
        let uut = Uut::default();
        let seq = vec![
            In::<SyncMark, AW, BW> {
                a: item_a(10, 0, true),
                b: item_b(10, 0, false),
                downstream_ready: true,
            },
            In::<SyncMark, AW, BW> {
                a: item_a(10, 0, true),
                b: item_b(10, 0, true),
                downstream_ready: true,
            },
            drain(),
        ];
        let got: Vec<bool> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.frame_mismatch)
            .collect();
        assert!(got[2], "the disagreeing cycle is reported");
        assert!(!got[3], "the agreeing cycle is not");
    }

    /// The marker rides with the product it belongs to.
    #[test]
    fn the_marker_rides_with_its_product() {
        let uut = Uut::default();
        let seq: Vec<In<SyncMark, AW, BW>> = (0..5)
            .map(|n| In::<SyncMark, AW, BW> {
                a: item_a(10, 0, n == 2),
                b: item_b(10, 0, n == 2),
                downstream_ready: true,
            })
            .collect();
        let marks: Vec<usize> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .enumerate()
            .filter(|(_, s)| s.output.stream.data.map(|it| it.frame.sync) == Some(true))
            .map(|(n, _)| n)
            .collect();
        // Reset cycle, then the marked input at index 3, emerging at 4.
        assert_eq!(marks, vec![1 + 2 + 1]);
    }

    /// A lost sample is reported.
    #[test]
    fn a_lost_sample_is_reported() {
        let uut = Uut::default();
        let seq: Vec<In<SyncMark, AW, BW>> = (0..4)
            .map(|n| In::<SyncMark, AW, BW> {
                a: item_a(1, 1, false),
                b: item_b(1, 1, false),
                downstream_ready: n != 2,
            })
            .collect();
        let fired: Vec<usize> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .enumerate()
            .filter(|(_, s)| s.output.overrun)
            .map(|(n, _)| n)
            .collect();
        assert_eq!(fired, vec![1 + 2], "combinational on downstream_ready");
    }

    /// **The worst case cannot overflow the *product* width.**
    ///
    /// `-2^(n-1)` squared needs `A+B` bits rather than `A+B-1`, which is
    /// why `PROD_W` is the sum of the input widths and not one less.
    /// Checked at an output width wide enough to represent the answer,
    /// so that a wrap here would have to be the multiplier's.
    #[test]
    fn the_most_negative_product_fits() {
        // (-128)(-128) - (-128)(127) = 16384 + 16256 = 32640, which
        // rounds to 128 -- one more than an 8-bit signed output can
        // hold, hence the wider output here.
        type Wide = RealPartMixer<SyncMark, AW, BW, 9, PW, DROP>;
        let uut = Wide::default();
        let seq = vec![
            In::<SyncMark, AW, BW> {
                a: item_a(-128, -128, false),
                b: item_b(-128, 127, false),
                downstream_ready: true,
            },
            In::<SyncMark, AW, BW> {
                a: item_a(0, 0, false),
                b: item_b(0, 0, false),
                downstream_ready: true,
            },
        ];
        let got: Vec<i128> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.stream.data.map(|it| it.data.v.raw()))
            .collect();
        assert_eq!(got[2], round(32640), "the intermediate did not wrap");
        assert_eq!(round(32640), 128);
    }

    /// **And the narrowing stage does not saturate — it wraps.**
    ///
    /// The other half, and deliberately so: `super::super::mixer`'s
    /// module docs record that overflow at a narrowing stage is a
    /// consequence of the chosen output width, not of the multiplier,
    /// and that there is no saturation logic.
    ///
    /// Shown with an output one bit narrower than `PROD_W - DROP`, which
    /// is a caller's choice rather than a misconfiguration this widget
    /// can refuse — the product width is checkable and the caller's
    /// headroom is not. The same operands that give `+128` above give
    /// `-128` here.
    ///
    /// Worth a test rather than a sentence, because "no saturation" is
    /// the kind of policy a later reader is tempted to fix. A caller who
    /// needs headroom widens `OUT_W` or increases `DROP`; a caller who
    /// needs saturation puts it downstream, where the clipping policy is
    /// theirs to choose.
    #[test]
    fn the_narrowing_stage_wraps_rather_than_saturating() {
        type Narrow = RealPartMixer<SyncMark, AW, BW, 8, PW, DROP>;
        let uut = Narrow::default();
        let seq = vec![
            In::<SyncMark, AW, BW> {
                a: item_a(-128, -128, false),
                b: item_b(-128, 127, false),
                downstream_ready: true,
            },
            drain(),
        ];
        let got: Vec<i128> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.stream.data.map(|it| it.data.v.raw()))
            .collect();
        assert_eq!(got[2], -128, "128 does not fit eight signed bits; it wraps");
    }

    /// **An under-width product is refused at construction.**
    ///
    /// The check that would have caught the sign flip described in the
    /// module docs, instead of it reaching a DAC.
    #[test]
    #[should_panic(expected = "PROD_W must be at least")]
    fn an_under_width_product_is_rejected() {
        let _ = RealPartMixer::<SyncMark, AW, BW, OW, { AW + BW }, DROP>::default();
    }

    /// As is a `DROP` of zero, which cannot round.
    #[test]
    #[should_panic(expected = "DROP must be at least one")]
    fn a_zero_drop_is_rejected() {
        let _ = RealPartMixer::<SyncMark, AW, BW, OW, PW, 0>::default();
    }

    /// **The example's single-sideband claim, checked.**
    ///
    /// `examples/real_part_mixer.rs` asserts in prose that a
    /// counter-rotating envelope cancels the carrier's rotation. Prose
    /// drifts; this does not. A constant envelope on an fs/4 carrier
    /// gives a period-4 output; a counter-rotating envelope gives a
    /// constant one, because the two rotations sum to zero frequency.
    #[test]
    fn a_counter_rotating_envelope_cancels_the_carrier() {
        let uut = Uut::default();
        let carrier = |n: usize| match n % 4 {
            0 => (100i128, 0i128),
            1 => (0, 100),
            2 => (-100, 0),
            _ => (0, -100),
        };
        let counter_rotating = |n: usize| match n % 4 {
            0 => (80i128, 0i128),
            1 => (0, -80),
            2 => (-80, 0),
            _ => (0, 80),
        };
        let sample = |f: &dyn Fn(usize) -> (i128, i128)| -> Vec<i128> {
            let seq: Vec<In<SyncMark, AW, BW>> = (0..16)
                .map(|n| {
                    let (er, ei) = f(n);
                    let (cr, ci) = carrier(n);
                    In::<SyncMark, AW, BW> {
                        a: item_a(er, ei, false),
                        b: item_b(cr, ci, false),
                        downstream_ready: true,
                    }
                })
                .collect();
            uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
                .synchronous_sample()
                .filter_map(|s| s.output.stream.data.map(|it| it.data.v.raw()))
                .skip(4)
                .collect()
        };

        let rotating = sample(&counter_rotating);
        assert!(
            rotating.windows(2).all(|w| w[0] == w[1]),
            "a counter-rotating envelope must cancel the carrier, got {rotating:?}"
        );

        let constant = sample(&|_| (80, 0));
        assert!(
            constant.windows(2).any(|w| w[0] != w[1]),
            "and a constant envelope must not, got {constant:?}"
        );
    }

    // ---- Tier 3 / 4 / 5 --------------------------------------------

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [36:0] i, output wire [14:0] o);
               wire [26:0] od;
               wire [11:0] d;
               wire [21:0] q;
               assign o = od[14:0];
               top_out c0(.clock_reset(clock_reset), .i(d[9:0]), .o(q[9:0]));
               top_starved c1(.clock_reset(clock_reset), .i(d[10:10]), .o(q[10:10]));
               top_mismatch c2(.clock_reset(clock_reset), .i(d[11:11]), .o(q[11:11]));
               top_idle c3(.clock_reset(clock_reset), .o(q[21:12]));
               assign d = od[26:15];
               assign od = kernel_real_part_mixer_kernel(clock_reset, i, q);
               function [26:0] kernel_real_part_mixer_kernel(input reg [1:0] arg_0, input reg [36:0] arg_1, input reg [21:0] arg_2);
                     reg [9:0] r0;
                     reg [21:0] r1;
                     // d
                     reg [11:0] r2;
                     // d
                     reg [11:0] r3;
                     reg [17:0] r4;
                     reg [36:0] r5;
                     reg [0:0] r6;
                     reg [16:0] r7;
                     reg [17:0] r8;
                     reg [9:0] r9;
                     reg [0:0] r10;
                     reg [16:0] r11;
                     reg [16:0] r12;
                     reg [17:0] r13;
                     reg [17:0] r14;
                     reg [0:0] r15;
                     reg [16:0] r16;
                     reg [17:0] r17;
                     reg [0:0] r18;
                     reg [16:0] r19;
                     reg [17:0] r20;
                     reg [9:0] r21;
                     reg [0:0] r22;
                     reg [16:0] r23;
                     reg [16:0] r24;
                     reg [17:0] r25;
                     reg [17:0] r26;
                     reg [0:0] r27;
                     reg [16:0] r28;
                     reg [15:0] r29;
                     reg [15:0] r30;
                     reg [0:0] r31;
                     reg [0:0] r32;
                     reg [0:0] r33;
                     reg [0:0] r34;
                     reg [0:0] r35;
                     // d
                     reg [11:0] r36;
                     reg [0:0] r37;
                     reg signed [7:0] r38;
                     reg signed [16:0] r39;
                     reg signed [7:0] r40;
                     reg signed [16:0] r41;
                     reg signed [7:0] r42;
                     reg signed [16:0] r43;
                     reg signed [7:0] r44;
                     reg signed [16:0] r45;
                     reg signed [16:0] r46;
                     reg signed [16:0] r47;
                     reg signed [16:0] r48;
                     reg [16:0] r49;
                     reg [16:0] r50;
                     reg signed [16:0] r51;
                     reg signed [16:0] r52;
                     reg [0:0] r53;
                     reg [16:0] r54;
                     reg [16:0] r55;
                     reg [0:0] r56;
                     reg [0:0] r57;
                     reg signed [16:0] r58;
                     reg signed [16:0] r59;
                     reg signed [8:0] r60;
                     reg [8:0] r61;
                     reg [9:0] r62;
                     reg [9:0] r63;
                     // d
                     reg [11:0] r64;
                     // d
                     reg [11:0] r65;
                     reg [9:0] r66;
                     // d
                     reg [11:0] r67;
                     // d
                     reg [11:0] r68;
                     reg [9:0] r69;
                     reg [10:0] r70;
                     reg [9:0] r71;
                     reg [11:0] r72;
                     reg [11:0] r73;
                     reg [0:0] r74;
                     reg [0:0] r75;
                     reg [0:0] r76;
                     reg [0:0] r77;
                     reg [14:0] r78;
                     reg [14:0] r79;
                     reg [14:0] r80;
                     reg [14:0] r81;
                     reg [0:0] r82;
                     reg [1:0] r83;
                     reg [0:0] r84;
                     reg [9:0] r85;
                     // d
                     reg [11:0] r86;
                     // d
                     reg [11:0] r87;
                     // d
                     reg [11:0] r88;
                     // o
                     reg [14:0] r89;
                     // o
                     reg [14:0] r90;
                     // d
                     reg [11:0] r91;
                     // o
                     reg [14:0] r92;
                     reg [26:0] r93;
                     reg signed [24:0] r94;
                     localparam l0 = 12'bXXXXXXXXXXXX;
                     localparam l1 = 1'b0;
                     localparam l2 = 1'b1;
                     localparam l3 = 1'b0;
                     localparam l4 = 1'b1;
                     localparam l5 = 1'b0;
                     localparam l6 = 1'b1;
                     localparam l7 = 1'b0;
                     localparam l8 = 1'b1;
                     localparam l9 = 1'b0;
                     localparam l10 = 17'b00000000011111111;
                     localparam l11 = 17'sb00000000010000000;
                     localparam l12 = 17'b00000000010000000;
                     localparam l13 = 17'b00000000000000001;
                     localparam l14 = 17'sb00000000000000001;
                     localparam l15 = 9'b000000000;
                     localparam l16 = 10'b0000000000;
                     localparam l17 = 1'b1;
                     localparam l18 = 1'b1;
                     localparam l19 = 12'b000000000000;
                     localparam l20 = 1'b1;
                     localparam l21 = 15'b000000000000000;
                     localparam l22 = 1'b0;
                     localparam l23 = 1'b0;
                     localparam l24 = 1'b0;
                     localparam l25 = 1'b0;
                     localparam l26 = 17'b00000000000000000;
                     localparam l27 = 17'b00000000000000000;
                     begin
                        r83 = arg_0;
                        r5 = arg_1;
                        r1 = arg_2;
                        r0 = r1[9:0];
                        r2 = l0;
                        r2[9:0] = r0;
                        r3 = r2;
                        r3[10:10] = l1;
                        r4 = r5[17:0];
                        r6 = r4[17:17];
                        r7 = r4[16:0];
                        r8 = {r7, l2};
                        r9 = r1[21:12];
                        r10 = r9[9:9];
                        r11 = l26;
                        r12 = r11;
                        r12[16:16] = r10;
                        r13 = {r12, l3};
                        case (r6)
                           1'b1 : r14 = r8;
                           1'b0 : r14 = r13;
                        endcase
                        r15 = r14[0:0];
                        r16 = r14[17:1];
                        r17 = r5[35:18];
                        r18 = r17[17:17];
                        r19 = r17[16:0];
                        r20 = {r19, l6};
                        r21 = r1[21:12];
                        r22 = r21[9:9];
                        r23 = l27;
                        r24 = r23;
                        r24[16:16] = r22;
                        r25 = {r24, l7};
                        case (r18)
                           1'b1 : r26 = r20;
                           1'b0 : r26 = r25;
                        endcase
                        r27 = r26[0:0];
                        r28 = r26[17:1];
                        r29 = r16[15:0];
                        r30 = r28[15:0];
                        r31 = r16[16:16];
                        r32 = r28[16:16];
                        r33 = r15 & r27;
                        r34 = r31 != r32;
                        r35 = r33 & r34;
                        r36 = r3;
                        r36[11:11] = r35;
                        r37 = r15 & r27;
                        r38 = r29[7:0];
                        r39 = $signed({{9{r38[7]}}, r38});
                        r40 = r29[15:8];
                        r41 = $signed({{9{r40[7]}}, r40});
                        r42 = r30[7:0];
                        r43 = $signed({{9{r42[7]}}, r42});
                        r44 = r30[15:8];
                        r45 = $signed({{9{r44[7]}}, r44});
                        r46 = r39 * r43;
                        r47 = r41 * r45;
                        r48 = r46 - r47;
                        r49 = $unsigned(r48);
                        r50 = r49 & l10;
                        r51 = r48 + l11;
                        r94 = $signed({{8{r51[16]}}, r51});
                        r52 = r94[24:8];
                        r53 = r50 == l12;
                        r54 = $unsigned(r52);
                        r55 = r54 & l13;
                        r56 = |r55;
                        r57 = r53 & r56;
                        r58 = r52 - l14;
                        r59 = r57 ? r58 : r52;
                        r60 = $signed(r59[8:0]);
                        r61 = l15;
                        r61[8:0] = r60;
                        r62 = l16;
                        r62[8:0] = r61;
                        r63 = r62;
                        r63[9:9] = r31;
                        r64 = r36;
                        r64[9:0] = r63;
                        r65 = r36;
                        r65[10:10] = l17;
                        r66 = r1[21:12];
                        r67 = r65;
                        r67[9:0] = r66;
                        r68 = r37 ? r64 : r67;
                        r69 = r1[9:0];
                        r71 = r69[9:0];
                        r70 = {l18, r71};
                        r72 = l19;
                        r72[10:0] = r70;
                        r73 = r72;
                        r73[11:11] = l20;
                        r74 = r1[10:10];
                        r75 = r5[36:36];
                        r76 = ~r75;
                        r77 = r1[11:11];
                        r78 = l21;
                        r78[11:0] = r73;
                        r79 = r78;
                        r79[12:12] = r74;
                        r80 = r79;
                        r80[14:14] = r76;
                        r81 = r80;
                        r81[13:13] = r77;
                        r82 = r83[1:1];
                        r84 = |r82;
                        r85 = r1[21:12];
                        r86 = r68;
                        r86[9:0] = r85;
                        r87 = r86;
                        r87[10:10] = l22;
                        r88 = r87;
                        r88[11:11] = l23;
                        r89 = r81;
                        r89[14:14] = l24;
                        r90 = r89;
                        r90[13:13] = l25;
                        r91 = r84 ? r88 : r68;
                        r92 = r84 ? r90 : r81;
                        r93 = {r91, r92};
                        kernel_real_part_mixer_kernel = r93;
                     end
               endfunction
            endmodule
            module top_out(input wire [1:0] clock_reset, input wire [9:0] i, output reg [9:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 10'b0000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 10'b0000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_starved(input wire [1:0] clock_reset, input wire [0:0] i, output reg [0:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 1'b0;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 1'b0;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_mismatch(input wire [1:0] clock_reset, input wire [0:0] i, output reg [0:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 1'b0;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 1'b0;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_idle(input wire [1:0] clock_reset, output wire [9:0] o);
               assign o = 10'b0000000000;
            endmodule
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    fn tb_stream() -> Vec<In<SyncMark, AW, BW>> {
        (0..16)
            .map(|n| In::<SyncMark, AW, BW> {
                a: item_a((n * 7) % 61 - 30, (n * 11) % 53 - 26, n == 4),
                b: item_b((n * 13) % 47 - 23, (n * 5) % 41 - 20, n == 4),
                downstream_ready: n != 9,
            })
            .collect()
    }

    #[test]
    fn test_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let input = tb_stream().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let input = tb_stream().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("real_part_mixer");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["ad5cb3e68677ac414259e3b26f2efe4cc7d1119cdae8c9745f80caf80d680f86"];
        let digest = vcd.dump_to_file(root.join("real_part_mixer.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
