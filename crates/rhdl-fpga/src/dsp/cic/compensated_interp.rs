#![warn(missing_docs)]
//! `CompensatedInterp` — a CIC interpolator with its droop pre-corrected.
//!
//! The transmit counterpart of [`super::compensated::CompensatedCic`],
//! and the interesting difference is the *order*: the compensator runs
//! **before** the interpolator, at the envelope rate.
//!
//! Presents the same [`In`](super::interpolator::In) and
//! [`Out`](super::interpolator::Out) as
//! [`super::interpolator::CicInterpolate`], so it drops into any
//! interpolator slot — including
//! [`super::interp_stream::StreamInterpolator`]'s, which makes a whole
//! [`crate::dsp::duc`] chain compensated by a type substitution and
//! nothing else.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+CompensatedInterp+------+
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
  x[m]        +-------------+   y[m]   +-----------+   z[n]
  --------->  | F: sym FIR  | -------> |  C: CIC   | ------->
  fs/R        | pre-emphasis|   fs/R   | interp R  |   fs
              |  inverse    |          |           |
              |   sinc      |          +-----------+
              +-------------+
    lifts the band it is about      droops it back flat
    to droop, at the CHEAP rate
")]
//!
//! # Pre-compensation is the cheap side of the transposition
//!
//! A decimator compensates *after* decimating, so its FIR runs at the
//! low rate too — both chains put the multiplier at the slow end, which
//! is the point. But the asymmetry is worth naming: on receive the
//! compensator is downstream of the rate change and on transmit it is
//! upstream, and in both cases that places it at `fs/R`. The
//! transposition is doing the work.
//!
//! What differs is what the FIR can *do*:
//!
//! **A pre-compensator cannot improve image rejection.** Its response is
//! periodic with period one in envelope-rate units, so the image at
//! `k + u` sees exactly the gain the signal at `u` sees and the
//! image-to-signal ratio is unchanged. Image rejection is bought with
//! `N`, `R` and bandwidth, and nothing else — see
//! [`crate::dsp::cic::interp_chain`], which relies on this to decouple
//! its search. A receive compensator, sitting after the fold, *is* part
//! of the alias budget, which is why `chain`'s `min_stopband_db`
//! constrains the cascade and the FIR jointly and this widget has no
//! equivalent knob.
//!
//! # The latency is `(TAPS-1)/2 + 1` envelope samples
//!
//! Two contributions, and it is worth separating them because only one
//! is this widget's doing.
//!
//! **The compensator's own group delay, `(TAPS-1)/2`.** A symmetric FIR
//! is linear-phase about its centre tap, so a 5-tap compensator delays
//! by two envelope samples whatever its coefficients are — including a
//! unit impulse, which is why an "identity" compensator is `z^-2` and
//! not `z^0`.
//!
//! **The handover register, one more.** The interpolator asks for a
//! sample once every `R` cycles and the FIR cannot answer on the same
//! cycle the request arrives, because its result is registered. So a
//! holding register sits between them: the FIR is fed on the accept
//! cycle, its result lands one cycle later, and it waits there for the
//! *next* accept.
//!
//! Both are delays rather than errors — the response is multiplied by
//! `z^-((TAPS-1)/2 + 1)` at the envelope rate, whose magnitude is one.
//! But a phase-sensitive transmitter needs the number, and the number is
//! not one; `an_identity_compensator_is_the_bare_interpolator_delayed`
//! measures it, and an earlier version of this paragraph claimed one
//! sample and was wrong by the FIR's group delay.
//!
//! The holding register starts at `Some(0)`, not `None`. `None` would
//! make the first accept a *starvation* — and starvation is a fault
//! report, not a description of a filter that has not filled yet. Zero
//! is silence, which is what a transmitter emits before its first
//! sample, and it keeps [`super::interpolator::Out::starved`] meaning
//! only what it says.
//!
//! # The FIR runs at the envelope rate because it is gated
//!
//! `fir::In::sample` is `None` on every cycle the interpolator is not
//! accepting, and an idle cycle *holds* a FIR's delay line rather than
//! shifting a zero into it. So the FIR's window is a window over
//! envelope samples even though the widget is clocked at the converter
//! rate — which is what a compensator has to be. Feeding it every cycle
//! would make it a filter at the wrong rate, with a response that has
//! nothing to do with the droop it is meant to invert.
//!
//! # A compensator has gain above one, so this one can saturate
//!
//! Unlike a bare [`super::interpolator::CicInterpolate`], whose exact
//! widths make [`super::interpolator::Out::saturated`] permanently
//! false. Inverting a droop means lifting the band edge, and a near
//! full-scale envelope at the band edge can exceed `W_MID`. That is a
//! headroom budget the caller owns; the flag reports when it was wrong.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/cic_compensated_interp.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/cic_compensated_interp.md")]

use rhdl::prelude::*;

use crate::core::dff;
use crate::dsp::fir;

/// A pre-compensated CIC interpolator.
///
/// `W_IN` is the envelope width, `W_MID` the compensator's output width
/// — which needs headroom above `W_IN`, since the compensator has gain
/// above one — and `W_OUT` the interpolator's accumulator width.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct CompensatedInterp<
    const W_IN: usize,
    const W_MID: usize,
    const W_OUT: usize,
    const CW: usize,
    F,
    C,
> where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    F: SynchronousIO<I = fir::In<W_IN>, O = fir::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    C: SynchronousIO<I = super::interpolator::In<W_MID, CW>, O = super::interpolator::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// The pre-compensator, gated to the envelope rate.
    fir: F,
    /// The compensator's result, waiting for the interpolator's next
    /// request. See the module docs on why this exists and why it starts
    /// at `Some(0)`.
    held: dff::DFF<Option<SignedBits<W_MID>>>,
    /// The interpolator.
    cic: C,
}

impl<const W_IN: usize, const W_MID: usize, const W_OUT: usize, const CW: usize, F, C>
    CompensatedInterp<W_IN, W_MID, W_OUT, CW, F, C>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    F: SynchronousIO<I = fir::In<W_IN>, O = fir::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    C: SynchronousIO<I = super::interpolator::In<W_MID, CW>, O = super::interpolator::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// Assemble from a specific compensator and interpolator.
    pub fn new(fir: F, cic: C) -> Self {
        Self {
            fir,
            held: dff::DFF::new(Some(SignedBits::<W_MID>::default())),
            cic,
        }
    }
}

impl<const W_IN: usize, const W_MID: usize, const W_OUT: usize, const CW: usize, F, C> Default
    for CompensatedInterp<W_IN, W_MID, W_OUT, CW, F, C>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    F: SynchronousIO<I = fir::In<W_IN>, O = fir::Out<W_MID>>
        + Synchronous
        + Default
        + Clone
        + std::fmt::Debug,
    C: SynchronousIO<I = super::interpolator::In<W_MID, CW>, O = super::interpolator::Out<W_OUT>>
        + Synchronous
        + Default
        + Clone
        + std::fmt::Debug,
{
    fn default() -> Self {
        Self::new(F::default(), C::default())
    }
}

impl<const W_IN: usize, const W_MID: usize, const W_OUT: usize, const CW: usize, F, C> SynchronousIO
    for CompensatedInterp<W_IN, W_MID, W_OUT, CW, F, C>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    F: SynchronousIO<I = fir::In<W_IN>, O = fir::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    C: SynchronousIO<I = super::interpolator::In<W_MID, CW>, O = super::interpolator::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    type I = super::interpolator::In<W_IN, CW>;
    type O = super::interpolator::Out<W_OUT>;
    type Kernel = compensated_interp_kernel<W_IN, W_MID, W_OUT, CW, F, C>;
}

#[kernel]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn compensated_interp_kernel<
    const W_IN: usize,
    const W_MID: usize,
    const W_OUT: usize,
    const CW: usize,
    F,
    C,
>(
    cr: ClockReset,
    i: super::interpolator::In<W_IN, CW>,
    q: Q<W_IN, W_MID, W_OUT, CW, F, C>,
) -> (
    super::interpolator::Out<W_OUT>,
    D<W_IN, W_MID, W_OUT, CW, F, C>,
)
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    F: SynchronousIO<I = fir::In<W_IN>, O = fir::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    C: SynchronousIO<I = super::interpolator::In<W_MID, CW>, O = super::interpolator::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    let mut d = D::<W_IN, W_MID, W_OUT, CW, F, C>::dont_care();

    // The interpolator's request. Registered inside it -- it depends
    // only on the phase counter -- so using it to gate the FIR does not
    // close a combinational loop.
    let accept = q.cic.input_ready;

    // **The FIR is gated to the envelope rate.** `None` holds its delay
    // line; feeding it every cycle would make it a filter at the
    // converter rate, whose response has nothing to do with the droop
    // it exists to invert.
    let mut fir_sample = None;
    if accept {
        fir_sample = i.sample;
    }
    d.fir = fir::In::<W_IN> {
        sample: fir_sample,
        downstream_ready: i.downstream_ready,
    };

    // Capture the compensator's result whenever it appears, and hold it
    // until the interpolator asks. One envelope sample of latency; see
    // the module docs.
    //
    // **Cleared on the accept cycle, and that is not tidiness.** Without
    // it, an accept cycle that found no input leaves the previous
    // result sitting in the register, and the next accept consumes it a
    // second time -- so a missing envelope sample silently repeats the
    // one before it and `starved` never fires. Clearing on consumption
    // means the slot is only refilled by the compensator actually
    // producing something, so an absent input propagates as an absence.
    let mut held = q.held;
    if accept {
        held = None;
    }
    if let Some(v) = q.fir.sample {
        held = Some(v);
    }
    d.held = held;

    let mut to_cic = None;
    if accept {
        to_cic = q.held;
    }
    d.cic = super::interpolator::In::<W_MID, CW> {
        sample: to_cic,
        rate: i.rate,
        restart: i.restart,
        downstream_ready: i.downstream_ready,
    };

    let mut o = super::interpolator::Out::<W_OUT> {
        sample: q.cic.sample,
        input_ready: q.cic.input_ready,
        starved: q.cic.starved,
        overrun: q.cic.overrun || q.fir.overrun,
        // The compensator's gain is above one, so unlike a bare
        // interpolator this can genuinely clip.
        saturated: q.cic.saturated || q.fir.saturated,
    };

    if cr.reset.any() {
        d.fir = fir::In::<W_IN> {
            sample: None,
            downstream_ready: false,
        };
        d.held = Some(signed::<W_MID>(0));
        d.cic = super::interpolator::In::<W_MID, CW> {
            sample: None,
            rate: i.rate,
            restart: false,
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
    use super::super::interp;
    use super::super::interpolator::{self, CicInterpolate};
    use super::*;
    use crate::dsp::fir::SymmetricFir;
    use expect_test::expect;

    const WI: usize = 8;
    // Headroom over WI: a compensator has gain above one.
    const WM: usize = 12;
    const S: usize = 2;
    const RMAX: usize = 8;
    const M: usize = 1;
    const CW: usize = 4;
    const WO: usize = interp::accumulator_width(WM, S, RMAX, M);
    const RATE: usize = 4;

    // The compensator: five taps, 12-bit coefficients, shift 10 so unity
    // is 1024.
    const TAPS: usize = 5;
    const HALF: usize = 2;
    const WC: usize = 12;
    const WACC: usize = 28;
    const SHIFT: usize = 10;
    const UNITY: i128 = 1 << SHIFT;

    type Fir = SymmetricFir<WI, WC, WACC, TAPS, HALF, SHIFT, WM>;
    type Core = CicInterpolate<WM, WO, S, RMAX, M, CW>;
    type Uut = CompensatedInterp<WI, WM, WO, CW, Fir, Core>;

    fn taps(v: [i128; TAPS]) -> [SignedBits<WC>; TAPS] {
        [
            signed::<WC>(v[0]),
            signed::<WC>(v[1]),
            signed::<WC>(v[2]),
            signed::<WC>(v[3]),
            signed::<WC>(v[4]),
        ]
    }

    /// A unit impulse: passes the signal through unchanged.
    fn identity() -> Fir {
        SymmetricFir::new(taps([0, 0, UNITY, 0, 0]))
    }

    /// The band the compensator is designed for, as a fraction of the
    /// envelope Nyquist. At `N = 2, R = 4` this droops by 3.43 dB, so
    /// there is real work for the compensator to do — a narrower band
    /// would make the test vacuous.
    const PASSBAND: f64 = 0.7;

    /// The real inverse-sinc compensator for this configuration.
    ///
    /// Designed by [`crate::dsp::cic::compensator`] rather than guessed.
    /// An earlier version of this file hand-picked plausible-looking
    /// taps and got the *sign* of the outer pair wrong, which made the
    /// band measurably less flat rather than more — and the flatness
    /// test caught it. Taps come from the designer now, which is also
    /// the workflow a user should copy.
    fn lift() -> Fir {
        let d = crate::dsp::cic::compensator::design(crate::dsp::cic::compensator::Spec {
            cics: vec![crate::dsp::cic::compensator::CicShape {
                decimate: RATE,
                stages: S,
                delay: M,
            }],
            passband: PASSBAND,
            taps: TAPS,
            stopband_edge: 1.0,
            // Zero: a pre-compensator's stopband cannot affect image
            // rejection. See the module docs.
            min_stopband_db: 0.0,
            max_ripple_db: 1.0,
            method: crate::dsp::cic::compensator::Method::LeastSquares,
        })
        .expect("designable at this configuration");
        let scale = (1i128 << SHIFT) as f64;
        let q: Vec<i128> = d.taps.iter().map(|x| (x * scale).round() as i128).collect();
        assert_eq!(q.len(), TAPS);
        SymmetricFir::new(taps([q[0], q[1], q[2], q[3], q[4]]))
    }

    fn stimulus(x: &[i128], rate: usize, drain: usize) -> Vec<interpolator::In<WI, CW>> {
        let mut seq: Vec<interpolator::In<WI, CW>> = (0..x.len() * rate)
            .map(|n| interpolator::In::<WI, CW> {
                sample: Some(signed::<WI>(x[n / rate])),
                rate: bits::<CW>(rate as u128),
                restart: false,
                downstream_ready: true,
            })
            .collect();
        seq.extend(std::iter::repeat_n(
            interpolator::In::<WI, CW> {
                sample: None,
                rate: bits::<CW>(rate as u128),
                restart: false,
                downstream_ready: true,
            },
            drain,
        ));
        seq
    }

    fn run_uut(fir: Fir, seq: Vec<interpolator::In<WI, CW>>) -> Vec<i128> {
        let uut = Uut::new(fir, Core::default());
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.sample.raw())
            .collect()
    }

    /// **There is deliberately no `Default`.**
    ///
    /// A compensator without taps is not a compensator, so this widget
    /// is built with [`CompensatedInterp::new`] — and that is why the
    /// chain widgets in [`crate::dsp::duc`] needed a `new` of their own,
    /// which they did not have until this file wanted one.
    #[test]
    fn construction_takes_taps() {
        let _ = Uut::new(identity(), Core::default());
        assert_eq!(WO, interp::accumulator_width(WM, S, RMAX, M));
    }

    /// **An identity compensator makes this the bare interpolator,
    /// delayed by one envelope sample.**
    ///
    /// The exact test, and it checks three separate things at once that
    /// would otherwise need three approximate ones: that the FIR is
    /// gated to the envelope rate (a FIR fed every cycle would not be an
    /// identity even with a unit impulse — its delay line would advance
    /// `R` times per envelope sample), that the holding register hands
    /// over exactly one sample per accept, and that the latency is one
    /// envelope sample and not two.
    #[test]
    fn an_identity_compensator_is_the_bare_interpolator_delayed() {
        let x: Vec<i128> = (0..12).map(|k| (k % 7) - 3).collect();
        let got = run_uut(identity(), stimulus(&x, RATE, 2));

        // The bare interpolator, at the same widths, fed the same
        // envelope.
        let bare = Core::default();
        let bare_seq: Vec<interpolator::In<WM, CW>> = (0..x.len() * RATE)
            .map(|n| interpolator::In::<WM, CW> {
                sample: Some(signed::<WM>(x[n / RATE])),
                rate: bits::<CW>(RATE as u128),
                restart: false,
                downstream_ready: true,
            })
            .collect();
        let want: Vec<i128> = bare
            .run(bare_seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.sample.raw())
            .collect();

        // `(TAPS-1)/2` envelope samples of FIR group delay plus one for
        // the handover register. A 5-tap compensator therefore delays by
        // three envelope samples, not one — see the module docs.
        const LATENCY_SAMPLES: usize = HALF + 1;
        let delay = LATENCY_SAMPLES * RATE;
        let n = want.len() - delay;
        assert_eq!(
            &got[delay..delay + n],
            &want[..n],
            "an identity compensator must be transparent apart from the delay"
        );
        // And the delay really is that, not merely at-least-that: the
        // samples before it are the silence the handover register starts
        // with.
        assert!(
            got[..delay].iter().all(|v| *v == 0),
            "the pre-fill must be silence, got {:?}",
            &got[..delay]
        );
    }

    /// **The compensated band is flatter than the bare one.**
    ///
    /// The reason the widget exists. Sweep a tone across the designed
    /// passband through both filters, measure each output's amplitude at
    /// the tone with a single-bin DFT, and compare the peak-to-peak
    /// spread.
    ///
    /// A DFT rather than the largest sample: at `R = 4` the output has
    /// only four samples per envelope period near the band edge, so a
    /// peak-of-samples estimate misses the true peak by a
    /// frequency-dependent amount — which is an error that looks exactly
    /// like the droop being measured.
    #[test]
    fn the_compensated_band_is_flatter_than_the_bare_one() {
        use std::f64::consts::TAU;

        /// Amplitude at output-normalised frequency `f`.
        fn mag_at(x: &[i128], f: f64) -> f64 {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (n, v) in x.iter().enumerate() {
                let t = -TAU * f * n as f64;
                re += *v as f64 * t.cos();
                im += *v as f64 * t.sin();
            }
            2.0 * (re * re + im * im).sqrt() / x.len() as f64
        }

        // Tone amplitude through a given compensator, at envelope-rate
        // frequency `f`.
        let amplitude = |fir: Fir, f: f64| -> f64 {
            let cycles = 64usize;
            let x: Vec<i128> = (0..cycles)
                .map(|m| {
                    let t = TAU * f * m as f64;
                    (60.0 * t.cos()) as i128
                })
                .collect();
            let out = run_uut(fir, stimulus(&x, RATE, 2));
            // Settled tail only, and measured at the tone's position in
            // output units.
            let tail = &out[out.len() / 2..];
            mag_at(tail, f / RATE as f64)
        };

        let spread = |make: &dyn Fn() -> Fir| -> f64 {
            let mut lo = f64::INFINITY;
            let mut hi = 0.0f64;
            // Across the designed band: `passband` is a fraction of
            // Nyquist, so the edge is at `0.5 * PASSBAND` cycles per
            // envelope sample.
            for k in 1..=6 {
                let f = 0.5 * PASSBAND * k as f64 / 6.0;
                let a = amplitude(make(), f);
                lo = lo.min(a);
                hi = hi.max(a);
            }
            hi / lo
        };

        // In dB, which is the unit flatness means anything in. A
        // *ratio* comparison is the wrong test and reads plausibly: a
        // spread ratio is always at least one, so "compensated < half of
        // bare" can never hold no matter how well the compensator works.
        let bare_db = 20.0 * spread(&identity).log10();
        let comp_db = 20.0 * spread(&lift).log10();
        assert!(
            comp_db < bare_db,
            "the compensator must flatten the band: bare {bare_db:.3} dB, \
             compensated {comp_db:.3} dB"
        );
        // And by a worthwhile margin, not a rounding error. Measured:
        // 3.03 dB of droop across this band becomes 0.48 dB.
        assert!(
            comp_db < 0.25 * bare_db,
            "and by a worthwhile margin: bare {bare_db:.3} dB, \
             compensated {comp_db:.3} dB"
        );
    }

    /// It presents the interpolator's own interface, so it drops into a
    /// [`super::super::interp_stream::StreamInterpolator`] — and hence
    /// into a whole [`crate::dsp::duc`] chain — by type substitution.
    ///
    /// The composition claim, checked by building it rather than by
    /// asserting it in prose.
    #[test]
    fn it_drops_into_a_stream_interpolator() -> miette::Result<()> {
        type Stream = super::super::interp_stream::StreamInterpolator<WI, WO, CW, Uut>;
        let uut = Stream::new(Uut::new(lift(), Core::default()));
        let d = uut.descriptor("top".into())?;
        assert!(d.input_kind.bits() > 0);
        Ok(())
    }

    /// And into a `RealDuc`, which is the whole point of presenting the
    /// bare interpolator's interface.
    #[test]
    fn it_drops_into_a_real_up_converter() -> miette::Result<()> {
        const OW: usize = 12;
        const PROD_W: usize = WO + 18 + 1;
        const DROP: usize = PROD_W - OW;
        type Duc = crate::dsp::duc::real::RealDuc<WI, WO, CW, OW, PROD_W, DROP, Uut>;
        let uut = Duc::new(Uut::new(lift(), Core::default()));
        let _ = uut.descriptor("top".into())?;
        Ok(())
    }

    /// **`starved` still means starved.**
    ///
    /// The holding register starts at `Some(0)`, so the first accept is
    /// silence rather than a fault. A `starved` report from this widget
    /// means the *caller* had nothing, not that the compensator had not
    /// filled.
    #[test]
    fn a_full_run_reports_no_starvation() {
        let uut = Uut::new(identity(), Core::default());
        let seq = stimulus(&[5i128; 10], RATE, 2);
        assert!(
            uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
                .synchronous_sample()
                .all(|s| !s.output.starved),
            "a fully-fed run must not report starvation"
        );
    }

    /// And a genuinely absent sample is still reported.
    #[test]
    fn a_missing_sample_is_still_reported() {
        let uut = Uut::new(identity(), Core::default());
        let seq: Vec<interpolator::In<WI, CW>> = (0..6 * RATE)
            .map(|n| interpolator::In::<WI, CW> {
                sample: if n == 2 * RATE {
                    None
                } else {
                    Some(signed::<WI>(9))
                },
                rate: bits::<CW>(RATE as u128),
                restart: false,
                downstream_ready: true,
            })
            .collect();
        assert!(
            uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
                .synchronous_sample()
                .any(|s| s.output.starved),
            "a missing sample on an accept cycle must be reported"
        );
    }

    /// `input_ready` still has the once-per-rate cadence: wrapping the
    /// interpolator must not disturb its request.
    #[test]
    fn input_ready_still_fires_once_per_rate() {
        let uut = Uut::new(identity(), Core::default());
        let seq = stimulus(&[3i128; 8], RATE, 0);
        let readies: Vec<bool> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.input_ready)
            .collect();
        assert!(!readies[0]);
        for (n, r) in readies[1..].iter().enumerate() {
            assert_eq!(*r, n % RATE == 0, "cycle {n}");
        }
    }

    // ---- Tier 3 / 4 / 5 --------------------------------------------

    fn tb_stream() -> Vec<interpolator::In<WI, CW>> {
        let x: Vec<i128> = (0..8).map(|k| (k % 5) * 20 - 40).collect();
        stimulus(&x, RATE, 2)
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Uut::new(lift(), Core::default());
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let shape = hdl
            .lines()
            .filter(|l| l.starts_with("module "))
            .map(|l| l.split('(').next().unwrap().to_string())
            .filter(|m| m.matches('_').count() <= 1)
            .collect::<Vec<_>>()
            .join("\n");
        let expect = expect![[r#"
            module top
            module top_fir
            module top_held
            module top_cic"#]];
        expect.assert_eq(&shape);
        Ok(())
    }

    #[test]
    fn test_hdl_works() -> miette::Result<()> {
        let uut = Uut::new(lift(), Core::default());
        let input = tb_stream().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_trace() -> miette::Result<()> {
        let uut = Uut::new(lift(), Core::default());
        let input = tb_stream().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("cic_compensated_interp");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["739c47d409899f3705765861659af4e003dc0954cc174bf471d75c1d0c544aff"];
        let digest = vcd
            .dump_to_file(root.join("compensated_interp.vcd"))
            .unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
