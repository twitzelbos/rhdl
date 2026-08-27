#![warn(missing_docs)]
//! `PostCompensatedInterp` — the interpolator with its compensator on
//! the **far** side, where it can suppress images.
//!
//! [`super::compensated_interp::CompensatedInterp`] puts the FIR before
//! the rate change, which is cheap and cannot touch the images.
//! This one puts it after, which can — and costs a filter at the
//! converter clock.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+PostCompensatedInterp+--+
      |                          |
+---->+ sample                   |
      |   Option<SignedBits<WI>> |
+---->+ rate              sample |
      |   Bits<CW>   SignedBits  |
+---->+ restart          <WO>    +----->
      |                          |
+---->+ downstream_ready         |
      |            input_ready   +----->
      |                starved   +----->
      |                overrun   +----->
      |              saturated   +----->
      +--------------------------+
   same In/Out as cic::interpolator, so it drops into any slot
")]
#![doc = badascii_doc::badascii!(r"
  x[m]      +-----------+   y[n]     +-------------+   z[n]
  ------->  |  C: CIC   | ---------> | F: sym FIR  | ------->
  fs/R      | interp R  |    fs      | flatten AND |   fs
            |           |            | stop images |
            +-----------+            +-------------+
                                     every cycle: the price
")]
//!
//! # Why this can do what the pre-compensator cannot
//!
//! A pre-compensator runs at the envelope rate, so its response is
//! periodic with period one in envelope-rate units and the image at
//! `k + u` sees exactly the gain the signal at `u` sees. The
//! image-to-signal ratio is the cascade's alone —
//! [`super::interp_chain`] leans on that to decouple its search.
//!
//! Move the same filter to the converter rate and the periodicity goes
//! away: `u` and `k + u` are now *different* frequencies to it, and it
//! can pass one and stop the other. So this widget's FIR does two jobs —
//! invert the droop across the signal band, and attenuate the image
//! bands — which is the same double duty a receive-side compensator does
//! as a combined compensator and anti-alias filter.
//!
//! `it_suppresses_an_image_where_the_pre_compensator_cannot` measures
//! the difference rather than restating the argument: at `N = 2, R = 4`
//! with a 25-tap filter asked for 60 dB, images go from **29 dB down to
//! 52 dB down** through the actual hardware.
//!
//! The design maths predicts 60 dB for the same configuration, and the
//! 8 dB shortfall is the output's own quantisation rather than the
//! filter's: an image 60 dB below a 24000-LSB signal is 24 LSBs, close
//! enough to the rounding floor to be partly buried. Widen `W_OUT` and
//! the measurement moves toward the prediction. Worth knowing because it
//! is the *general* limit on image rejection once the filter is good
//! enough — past some point the converter word length is the constraint,
//! not the CIC and not the compensator.
//!
//! # `min_stopband_db` is a composite requirement, and that reads oddly
//! here
//!
//! [`super::interp_chain::post_compensator`] passes it through to
//! [`super::compensator`], which measures the **cascade and the filter
//! together**. So asking for less attenuation than the cascade already
//! delivers buys almost nothing: at the configuration above the cascade
//! alone gives 24 dB, and asking for 30 produced a filter that added
//! 2 dB. Ask for the number you want to *end up with*.
//!
//! # It is affordable at small `R` and hopeless at large `R`
//!
//! **Read this before reaching for it.** The signal band the FIR must
//! pass has been squeezed into `[0, edge/R]` and the first image it must
//! stop begins at `(1 - edge)/R`, so the transition band narrows in
//! proportion to `R` — and a narrow filter costs taps.
//! [`super::interp_chain::post_compensator_taps`] estimates how many, at
//! `passband = 0.4` and 60 dB:
//!
//! | `R` | taps, at the converter clock |
//! |---|---|
//! | 2 | 13 |
//! | 4 | 25 |
//! | 8 | 49 |
//! | 16 | 97 |
//! | 32 | 195 |
//! | 125 | 755 |
//!
//! Linear in `R`. At the 125 Msps configuration the up-converter was
//! written for, this is a 755-tap FIR at 125 MHz, which is not a widget
//! anybody wants.
//!
//! **So put it between chain stages, not after the whole
//! interpolation.** After the first stage of a `5 × 25` split the local
//! rate is five, the filter is a couple of dozen taps at 5 MHz, and it
//! suppresses the images that stage created — which the second stage's
//! `sinc^N` then cannot re-create, because they are gone. That is a
//! reason to split a transmit chain that has nothing to do with register
//! bits, and it is why
//! [`super::interp_chain::InterpSpec::max_chain_stages`] matters more on
//! transmit than the width figures suggest.
//!
//! # No holding register, unlike the pre-compensated form
//!
//! [`super::compensated_interp::CompensatedInterp`] needs one because
//! the interpolator asks for a sample once per `R` cycles and the FIR
//! cannot answer on the same cycle. Here the order is reversed: the
//! interpolator produces on *every* cycle and the FIR consumes on every
//! cycle, so they connect directly. The latency is the FIR's own group
//! delay, `(TAPS-1)/2` — at the **converter** rate this time, not the
//! envelope rate, so it is a much smaller delay in absolute terms.
//!
//! # This one can saturate, and for two reasons
//!
//! A droop-inverting filter has gain above one, as in the pre-compensated
//! form. On top of that this FIR sees the interpolator's full
//! `(R·M)^N / R` gain on its input rather than the raw envelope, so
//! `W_MID` is the interpolator's accumulator width and `W_OUT` has to
//! carry that times the compensator's peak gain.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/cic_post_compensated_interp.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/cic_post_compensated_interp.md")]

use rhdl::prelude::*;

use crate::dsp::fir;

/// A CIC interpolator followed by a converter-rate compensator.
///
/// `W_IN` is the envelope width, `W_MID` the interpolator's accumulator
/// width, `W_OUT` the compensator's output width.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct PostCompensatedInterp<
    const W_IN: usize,
    const W_MID: usize,
    const W_OUT: usize,
    const CW: usize,
    C,
    F,
> where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    C: SynchronousIO<I = super::interpolator::In<W_IN, CW>, O = super::interpolator::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    F: SynchronousIO<I = fir::In<W_MID>, O = fir::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// The interpolator.
    cic: C,
    /// The compensator, at the converter rate.
    fir: F,
}

impl<const W_IN: usize, const W_MID: usize, const W_OUT: usize, const CW: usize, C, F>
    PostCompensatedInterp<W_IN, W_MID, W_OUT, CW, C, F>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    C: SynchronousIO<I = super::interpolator::In<W_IN, CW>, O = super::interpolator::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    F: SynchronousIO<I = fir::In<W_MID>, O = fir::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// Assemble from a specific interpolator and compensator.
    pub fn new(cic: C, fir: F) -> Self {
        Self { cic, fir }
    }
}

impl<const W_IN: usize, const W_MID: usize, const W_OUT: usize, const CW: usize, C, F> Default
    for PostCompensatedInterp<W_IN, W_MID, W_OUT, CW, C, F>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    C: SynchronousIO<I = super::interpolator::In<W_IN, CW>, O = super::interpolator::Out<W_MID>>
        + Synchronous
        + Default
        + Clone
        + std::fmt::Debug,
    F: SynchronousIO<I = fir::In<W_MID>, O = fir::Out<W_OUT>>
        + Synchronous
        + Default
        + Clone
        + std::fmt::Debug,
{
    fn default() -> Self {
        Self::new(C::default(), F::default())
    }
}

impl<const W_IN: usize, const W_MID: usize, const W_OUT: usize, const CW: usize, C, F> SynchronousIO
    for PostCompensatedInterp<W_IN, W_MID, W_OUT, CW, C, F>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    C: SynchronousIO<I = super::interpolator::In<W_IN, CW>, O = super::interpolator::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    F: SynchronousIO<I = fir::In<W_MID>, O = fir::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    type I = super::interpolator::In<W_IN, CW>;
    type O = super::interpolator::Out<W_OUT>;
    type Kernel = post_compensated_interp_kernel<W_IN, W_MID, W_OUT, CW, C, F>;
}

#[kernel]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn post_compensated_interp_kernel<
    const W_IN: usize,
    const W_MID: usize,
    const W_OUT: usize,
    const CW: usize,
    C,
    F,
>(
    cr: ClockReset,
    i: super::interpolator::In<W_IN, CW>,
    q: Q<W_IN, W_MID, W_OUT, CW, C, F>,
) -> (
    super::interpolator::Out<W_OUT>,
    D<W_IN, W_MID, W_OUT, CW, C, F>,
)
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    C: SynchronousIO<I = super::interpolator::In<W_IN, CW>, O = super::interpolator::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    F: SynchronousIO<I = fir::In<W_MID>, O = fir::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    let mut d = D::<W_IN, W_MID, W_OUT, CW, C, F>::dont_care();

    // Straight through: the interpolator sees the caller's input
    // unchanged.
    d.cic = super::interpolator::In::<W_IN, CW> {
        sample: i.sample,
        rate: i.rate,
        restart: i.restart,
        downstream_ready: i.downstream_ready,
    };

    // **Every cycle, and no holding register.** The interpolator
    // produces continuously and the FIR consumes continuously, so unlike
    // the pre-compensated form there is nothing to buffer between them.
    d.fir = fir::In::<W_MID> {
        sample: Some(q.cic.sample),
        downstream_ready: i.downstream_ready,
    };

    // The FIR's output is an `Option` because a FIR may idle; this one
    // never does once out of reset, so the `None` arm is the reset cycle
    // and zero is the right value for it.
    let mut out = signed::<W_OUT>(0);
    if let Some(v) = q.fir.sample {
        out = v;
    }

    let mut o = super::interpolator::Out::<W_OUT> {
        sample: out,
        input_ready: q.cic.input_ready,
        starved: q.cic.starved,
        overrun: q.cic.overrun || q.fir.overrun,
        // Two reasons this can fire where a bare interpolator's cannot:
        // a droop-inverting filter has gain above one, and this one sees
        // the interpolator's full DC gain on its input.
        saturated: q.cic.saturated || q.fir.saturated,
    };

    if cr.reset.any() {
        d.cic = super::interpolator::In::<W_IN, CW> {
            sample: None,
            rate: i.rate,
            restart: false,
            downstream_ready: false,
        };
        d.fir = fir::In::<W_MID> {
            sample: None,
            downstream_ready: false,
        };
        o.sample = signed::<W_OUT>(0);
        o.input_ready = false;
        o.starved = false;
        o.overrun = false;
        o.saturated = false;
    }

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::super::interpolator::{self, CicInterpolate};
    use super::super::{interp, interp_chain};
    use super::*;
    use crate::dsp::fir::{SymmetricFir, accumulator_width as fir_acc};
    use expect_test::expect;
    use std::f64::consts::TAU;

    // Fourteen bits rather than ten, deliberately. At `WI = 10` the
    // signal is about 1600 output LSBs, so an image suppressed by 60 dB
    // is 1.6 LSBs -- indistinguishable from the output's own
    // quantisation, and the spectral test measured 5 dB of improvement
    // where the design maths predicted 36. The widget was right and the
    // measurement had no dynamic range. Widening the words puts the
    // image about 32 LSBs above the floor.
    const WI: usize = 14;
    const S: usize = 2;
    const RMAX: usize = 4;
    const M: usize = 1;
    const WMID: usize = interp::accumulator_width(WI, S, RMAX, M);
    const CW: usize = interp::rate_width(RMAX);

    /// Twenty-five taps, which is what
    /// [`interp_chain::post_compensator_taps`] asks for at `R = 4` and
    /// 60 dB. Affordable here precisely because `R` is small — see the
    /// module docs.
    const TAPS: usize = 25;
    const HALF: usize = 12;
    const WC: usize = 18;
    const WACC: usize = fir_acc(WMID, WC, TAPS);
    const SHIFT: usize = 14;
    const WOUT: usize = 18;

    /// The band the filter is designed for, as a fraction of the
    /// envelope Nyquist.
    const PASSBAND: f64 = 0.4;

    type Fir = SymmetricFir<WMID, WC, WACC, TAPS, HALF, SHIFT, WOUT>;
    type Core = CicInterpolate<WI, WMID, S, RMAX, M, CW>;
    type Uut = PostCompensatedInterp<WI, WMID, WOUT, CW, Core, Fir>;

    fn shapes() -> Vec<crate::dsp::cic::compensator::CicShape> {
        vec![crate::dsp::cic::compensator::CicShape {
            decimate: RMAX,
            stages: S,
            delay: M,
        }]
    }

    /// The designed taps as integers at `SHIFT` fractional bits.
    ///
    /// Scaled here rather than through `compensator::quantise` so that
    /// `SHIFT` stays a const this module controls; the assertion below is
    /// what keeps that honest.
    fn designed_taps() -> [SignedBits<WC>; TAPS] {
        let q = // 60 dB, not 30: `min_stopband_db` is a *composite*
        // requirement, and the cascade alone already delivers 24, so
        // asking for 30 buys almost nothing. That is the composite
        // metric working as designed and it is easy to misread.
        interp_chain::post_compensator(&shapes(), PASSBAND, RMAX, TAPS, 60.0, WC)
            .expect("designable at R = 4");
        let scale = (1u64 << q.shift) as f64;
        let real: Vec<f64> = q.taps.iter().map(|x| *x as f64 / scale).collect();
        let mut out = [SignedBits::<WC>::default(); TAPS];
        let unity = (1i128 << SHIFT) as f64;
        for (k, v) in real.iter().enumerate() {
            let scaled = (v * unity).round() as i128;
            assert!(
                scaled.abs() < (1i128 << (WC - 1)),
                "tap {k} = {scaled} does not fit {WC} signed bits"
            );
            out[k] = signed::<WC>(scaled);
        }
        out
    }

    /// A unit impulse: passes the signal through, delayed by the FIR's
    /// group delay and nothing else.
    fn identity_taps() -> [SignedBits<WC>; TAPS] {
        let mut out = [SignedBits::<WC>::default(); TAPS];
        out[HALF] = signed::<WC>(1 << SHIFT);
        out
    }

    fn uut(taps: [SignedBits<WC>; TAPS]) -> Uut {
        Uut::new(Core::default(), SymmetricFir::new(taps))
    }

    fn stimulus(x: &[i128], rate: usize) -> Vec<interpolator::In<WI, CW>> {
        (0..x.len() * rate)
            .map(|n| interpolator::In::<WI, CW> {
                sample: Some(signed::<WI>(x[n / rate])),
                rate: bits::<CW>(rate as u128),
                restart: false,
                downstream_ready: true,
            })
            .collect()
    }

    fn run(u: Uut, seq: Vec<interpolator::In<WI, CW>>) -> Vec<i128> {
        u.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.sample.raw())
            .collect()
    }

    /// Magnitude at output-normalised frequency `f`.
    fn mag_at(x: &[i128], f: f64) -> f64 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, v) in x.iter().enumerate() {
            let t = -TAU * f * n as f64;
            re += *v as f64 * t.cos();
            im += *v as f64 * t.sin();
        }
        2.0 * (re * re + im * im).sqrt() / x.len() as f64
    }

    #[test]
    fn construction_takes_taps() {
        let _ = uut(identity_taps());
        assert_eq!(WMID, interp::accumulator_width(WI, S, RMAX, M));
    }

    /// The tap count is the one the design maths asks for at this rate.
    #[test]
    fn the_tap_count_is_the_estimated_one() {
        assert_eq!(
            interp_chain::post_compensator_taps(PASSBAND, RMAX, 60.0),
            TAPS
        );
    }

    /// **It suppresses an image the bare interpolator leaves standing.**
    ///
    /// The whole reason this widget exists, measured end to end through
    /// the hardware rather than on the design maths.
    ///
    /// A tone envelope at `f0` cycles per envelope sample appears at
    /// `f0/R` in the output and its first image at `(1 - f0)/R`. A
    /// constant envelope would not do: its images sit exactly on the
    /// `sinc^N` nulls and are already gone, so the test would pass on a
    /// widget that did nothing.
    #[test]
    fn it_suppresses_an_image_where_the_pre_compensator_cannot() {
        let f0 = 0.15;
        let rate = RMAX;
        let envelope: Vec<i128> = (0..256)
            .map(|m| {
                let t = TAU * f0 * m as f64;
                (6000.0 * t.cos()) as i128
            })
            .collect();
        let seq = stimulus(&envelope, rate);

        let signal_f = f0 / rate as f64;
        let image_f = (1.0 - f0) / rate as f64;

        let bare = run(uut(identity_taps()), seq.clone());
        let comp = run(uut(designed_taps()), seq);
        // Measured: 29.4 dB bare, 51.6 dB compensated. The design maths
        // predicts 24.1 and 60.2 for the same configuration -- see
        // `a_post_compensator_suppresses_images_and_a_pre_one_does_not`
        // in `interp_chain` -- and the hardware falls short of the
        // compensated figure because the output is quantised: an image
        // 60 dB below a 24000-LSB signal is 24 LSBs, close enough to the
        // rounding floor to be partly buried. The threshold below is set
        // against the measurement, not the prediction, and the gap is
        // recorded in the module docs rather than papered over.

        // Settled tail only.
        let bare = &bare[bare.len() / 2..];
        let comp = &comp[comp.len() / 2..];

        let bare_rej = 20.0 * (mag_at(bare, signal_f) / mag_at(bare, image_f)).log10();
        let comp_rej = 20.0 * (mag_at(comp, signal_f) / mag_at(comp, image_f)).log10();

        assert!(
            comp_rej > bare_rej + 20.0,
            "the converter-rate filter must buy real rejection: \
             bare {bare_rej:.1} dB, compensated {comp_rej:.1} dB"
        );
    }

    /// **An identity FIR is the bare interpolator, delayed by the FIR's
    /// group delay.**
    ///
    /// And the delay is at the *converter* rate here, not the envelope
    /// rate — which is the one respect in which post-compensation is
    /// cheaper than pre-compensation. `HALF` output cycles, against
    /// `HALF` envelope samples for
    /// [`super::compensated_interp::CompensatedInterp`], a factor of `R`
    /// difference in absolute time.
    #[test]
    fn an_identity_filter_is_the_bare_interpolator_delayed() {
        let x: Vec<i128> = (0..16).map(|k| (k * 61 % 401) as i128 - 200).collect();
        let rate = RMAX;
        let got = run(uut(identity_taps()), stimulus(&x, rate));

        let bare = Core::default();
        let want: Vec<i128> = bare
            .run(
                stimulus(&x, rate)
                    .into_iter()
                    .with_reset(1)
                    .clock_pos_edge(100),
            )
            .synchronous_sample()
            .map(|s| s.output.sample.raw())
            .collect();

        // The FIR's group delay, in converter cycles, plus its output
        // register.
        let delay = HALF + 1;
        let n = want.len() - delay;
        assert_eq!(
            &got[delay..delay + n],
            &want[..n],
            "an identity filter must be transparent apart from the delay"
        );
    }

    /// `input_ready` keeps its cadence: wrapping the interpolator must
    /// not disturb its request.
    #[test]
    fn input_ready_still_fires_once_per_rate() {
        let u = uut(identity_taps());
        let readies: Vec<bool> = u
            .run(
                stimulus(&[7i128; 8], RMAX)
                    .into_iter()
                    .with_reset(1)
                    .clock_pos_edge(100),
            )
            .synchronous_sample()
            .map(|s| s.output.input_ready)
            .collect();
        assert!(!readies[0]);
        for (n, r) in readies[1..].iter().enumerate() {
            assert_eq!(*r, n % RMAX == 0, "cycle {n}");
        }
    }

    /// A missing envelope sample is still reported: the FIR is
    /// downstream of the interpolator and does not mask it.
    #[test]
    fn starvation_is_still_reported() {
        let u = uut(identity_taps());
        let seq: Vec<interpolator::In<WI, CW>> = (0..6 * RMAX)
            .map(|n| interpolator::In::<WI, CW> {
                sample: if n == 2 * RMAX {
                    None
                } else {
                    Some(signed::<WI>(9))
                },
                rate: bits::<CW>(RMAX as u128),
                restart: false,
                downstream_ready: true,
            })
            .collect();
        assert!(
            u.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
                .synchronous_sample()
                .any(|s| s.output.starved)
        );
    }

    /// It presents the interpolator's interface, so it drops into a
    /// whole up-converter.
    #[test]
    fn it_drops_into_a_real_up_converter() -> miette::Result<()> {
        const OW: usize = 12;
        const PROD_W: usize = WOUT + 18 + 1;
        const DROP: usize = PROD_W - OW;
        type Duc = crate::dsp::duc::real::RealDuc<WI, WOUT, CW, OW, PROD_W, DROP, Uut>;
        let duc = Duc::new(uut(designed_taps()));
        let _ = duc.descriptor("top".into())?;
        Ok(())
    }

    // ---- Tier 3 / 4 / 5 --------------------------------------------

    fn tb_stream() -> Vec<interpolator::In<WI, CW>> {
        let x: Vec<i128> = (0..8).map(|k| (k * 71 % 401) as i128 - 200).collect();
        stimulus(&x, RMAX)
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let u = uut(designed_taps());
        let hdl = u.descriptor("top".into())?.hdl()?.modules.pretty();
        let shape = hdl
            .lines()
            .filter(|l| l.starts_with("module "))
            .map(|l| l.split('(').next().unwrap().to_string())
            .filter(|m| m.matches('_').count() <= 1)
            .collect::<Vec<_>>()
            .join("\n");
        let expect = expect![[r#"
            module top
            module top_cic
            module top_fir"#]];
        expect.assert_eq(&shape);
        Ok(())
    }

    #[test]
    fn test_hdl_works() -> miette::Result<()> {
        let u = uut(designed_taps());
        let input = tb_stream().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = u.run(input).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&u, &Default::default())?.run_iverilog()?;
        tb.ntl(&u, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_trace() -> miette::Result<()> {
        let u = uut(designed_taps());
        let input = tb_stream().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = u.run(input).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("cic_post_compensated_interp");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["48792729b8f8482701ad5287faad87bd876100099befc03ec48081c1af8c54bf"];
        let digest = vcd
            .dump_to_file(root.join("post_compensated_interp.vcd"))
            .unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
