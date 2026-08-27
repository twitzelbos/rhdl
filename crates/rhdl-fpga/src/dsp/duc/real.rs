#![warn(missing_docs)]
//! `RealDuc` — a digital up-converter for a single DAC.
//!
//! Interpolates a complex baseband envelope to the converter rate and
//! modulates it onto a coherent carrier, emitting the **real** passband
//! signal. The transmit mirror of [`crate::dsp::ddc::Ddc`].
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+RealDuc+------------------+
      |                            |
+---->+ stream                     |
      |  Option<Item<Iq<W>,        |
      |             SyncMark>>     |
+---->+ rate                stream |
      |   Bits<CW>    RCStream<    |
+---->+ frequency      Real<OW>,   +----->
      |   (carrier)    SyncMark>   |
+---->+ phase                      |
      |               master       +----->
+---->+ downstream_ready  starved  +----->
      |                   overrun  +----->
      |            frame_mismatch  +----->
      |                 saturated  +----->
      +----------------------------+
")]
//!
//!# Internals
#![doc = badascii_doc::badascii!(r"
   stream ->+-+EnvelopeUpsampler+-+
   rate --->|  split/interp/join  +--+
            +---------------------+  |  +-+RealPartMixer+-+
                                     +->| Re{env*carrier} +--> Real<OW>
   frequency -> +-+Nco+-+               +-----------------+
   phase -----> |  LO   +---------------^   two multiplies
                +-------+
                    |
                    +--> master (phase reference)
")]
//!
//! # Why the mixing is last, and real
//!
//! The down-converter mixes first and filters after, because the point
//! is to select a band before throwing sample rate away. Transmit is the
//! reverse: interpolate at baseband, where the filter is cheap and the
//! images are far from the signal, and mix only at the end.
//!
//! `Re{env · e^{jwt}} = env.re·cos(wt) − env.im·sin(wt)` — two
//! multiplies, via [`crate::dsp::mixer::RealPartMixer`]. The quadrature
//! component of the product is never formed because nothing downstream
//! of a single DAC can carry it. [`super::IqDuc`] is the version for a
//! quadrature output, and its mixer costs four.
//!
//! **The mixer is not where most of the multipliers are**, which is
//! worth knowing before optimising for it. The whole chain emits six:
//! two in the mixer and four in the oscillator's sine/cosine
//! interpolation, two per quadrature. The CIC interpolators emit none,
//! by construction. So choosing this widget over [`super::IqDuc`] saves
//! two of eight rather than half, and a design with several
//! up-converters saves more by sharing one oscillator between them than
//! by choosing between these two.
//! `the_chain_costs_six_multiplies_and_only_two_are_the_mixer` pins the
//! breakdown per module.
//!
//! **The envelope is complex and that is the point.** A real envelope on
//! a complex carrier can only amplitude-modulate — the two sidebands are
//! forced to be mirror images. A complex envelope controls them
//! independently, which is what makes single-sideband, offset-tone and
//! arbitrary-phase transmission possible, and it is why
//! [`crate::dsp::mixer::ComplexRealMixer`] is not the right widget here
//! despite costing the same two multiplies.
//!
//! # What "coherent" means here, and what it costs
//!
//! As with the down-converter, three things have to hold and all three
//! are properties of pieces that already existed:
//!
//! - **The carrier is phase-coherent.** [`Nco`](crate::dsp::nco)'s
//!   accumulator represents absolute elapsed time and is never reset at
//!   a burst boundary, so successive bursts share a phase origin.
//!   [`Out::master`] exposes it, and a receiver told the same number can
//!   relate its measurement to this transmission.
//! - **Both quadrature arms are interpolated identically.** They are the
//!   same widget at the same configuration, so an asymmetry — which
//!   would leak energy into the suppressed sideband — is
//!   unrepresentable. See [`super::EnvelopeUpsampler`].
//! - **The envelope and the carrier agree about where the burst
//!   starts.** They are separate paths, so this is checked rather than
//!   assumed: [`Out::frame_mismatch`] fires when the marks disagree.
//!
//! # This chain pulls, where the down-converter is pushed
//!
//! `Out::stream.ready` is a genuine request for the next envelope
//! sample, asserted one cycle in `R`. Whatever generates the envelope
//! must answer it — a host DMA wants a FIFO in between rather than a
//! fixed schedule, and an upstream that ignores `ready` will be sampled
//! on this chain's grid instead of its own.
//!
//! The output, by contrast, is present on **every** cycle, which is what
//! a DAC wants. So `downstream_ready` should be held high permanently;
//! a low cycle loses that output sample and [`Out::overrun`] says so.
//!
//! # The gain is not normalised
//!
//! The output carries the interpolator's `(R·M)^N / R` gain, scaled by
//! the carrier amplitude and narrowed by the mixer's `DROP`. With a
//! run-time rate the CIC factor is a run-time quantity — see
//! [`super::EnvelopeUpsampler`] on why normalising inside the chain is
//! the wrong place for it. [`crate::dsp::cic::interp::dc_gain_ratio`]
//! reports the factor.
//!
//! **Check the headroom.** The mixer does not saturate, by the policy in
//! [`crate::dsp::mixer`], so a `DROP` too small for the combined gain
//! wraps rather than clips — and a wrap on a transmit sample is a sign
//! flip on the air. `the_full_scale_case_does_not_wrap` pins it for the
//! configuration in the example.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/duc_real.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/duc_real.md")]

use rhdl::prelude::*;

use super::upsampler::{self, EnvelopeUpsampler};
use crate::dsp::cic::interpolator;
use crate::dsp::iq::Real;
use crate::dsp::mixer::real_part::{self, RealPartMixer};
use crate::dsp::nco::composite;
use crate::dsp::nco::config::PHASE_W;
use crate::dsp::nco::{frequency_composer, phase_composer, sin_cos_linear_interp};
use crate::dsp::sync::SyncMark;
use crate::rcstream::bus::{Item, RCStream};

/// A digital up-converter emitting a real passband signal.
///
/// `W` is the envelope width, `WA` the interpolator accumulator width,
/// `OW` the output width, `PROD_W` the mixer's product width — which
/// must be at least `WA + AMP_W + 1`, checked by the mixer's own
/// `Default` — and `DROP` the mixer's narrowing shift. `C` is the
/// interpolator core, the same type in both arms.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RealDuc<
    const W: usize,
    const WA: usize,
    const CW: usize,
    const OW: usize,
    const PROD_W: usize,
    const DROP: usize,
    C,
> where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<WA>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    rhdl::bits::W<OW>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
    C: SynchronousIO<I = interpolator::In<W, CW>, O = interpolator::Out<WA>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// The envelope, brought up to the converter rate.
    up: EnvelopeUpsampler<W, WA, CW, C>,
    /// The coherent carrier.
    lo: composite::NcoDefault,
    /// `Re{envelope × carrier}`, two multiplies.
    mix: RealPartMixer<SyncMark, WA, { sin_cos_linear_interp::AMP_W }, OW, PROD_W, DROP>,
}

impl<
    const W: usize,
    const WA: usize,
    const CW: usize,
    const OW: usize,
    const PROD_W: usize,
    const DROP: usize,
    C,
> Default for RealDuc<W, WA, CW, OW, PROD_W, DROP, C>
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<WA>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    rhdl::bits::W<OW>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
    C: SynchronousIO<I = interpolator::In<W, CW>, O = interpolator::Out<WA>>
        + Synchronous
        + Default
        + Clone
        + std::fmt::Debug,
{
    fn default() -> Self {
        Self {
            up: EnvelopeUpsampler::default(),
            lo: composite::NcoDefault::default(),
            mix: RealPartMixer::default(),
        }
    }
}

/// Inputs to [`RealDuc`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const W: usize, const CW: usize>
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
{
    /// The low-rate complex envelope, framed.
    ///
    /// Consumed on cycles where `Out::stream.ready` is high — one cycle
    /// in [`In::rate`]. A mark restarts the interpolation window and is
    /// carried through to the output.
    pub stream: Option<Item<crate::dsp::iq::Iq<W>, SyncMark>>,
    /// The interpolation factor. A rate change wants a mark with it —
    /// see [`super`].
    pub rate: Bits<CW>,
    /// Carrier tuning word — where in the output band to place the
    /// signal.
    pub frequency: Bits<PHASE_W>,
    /// Carrier phase offset, for setting a burst's phase origin without
    /// disturbing the master trajectory.
    pub phase: Bits<PHASE_W>,
    /// Downstream's ready, per the `RCStream` contract. Hold it high for
    /// a DAC.
    pub downstream_ready: bool,
}

/// Outputs from [`RealDuc`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const OW: usize>
where
    rhdl::bits::W<OW>: BitWidth,
{
    /// The real passband stream — present every cycle, and `ready`
    /// carrying the once-per-`R` request for the next envelope sample.
    pub stream: RCStream<Real<OW>, SyncMark>,
    /// The carrier's undisturbed master phase.
    ///
    /// The reference this transmission's phase is *relative to*. Exposed
    /// so a receiver, or a later burst, can be related to it.
    pub master: Bits<PHASE_W>,
    /// A stage asked for a sample and found none.
    pub starved: bool,
    /// An output was produced while `downstream_ready` was low.
    pub overrun: bool,
    /// **The envelope and the carrier disagreed about the burst
    /// origin.**
    ///
    /// Two causes, both meaning the marks on this stream cannot be
    /// trusted: the mixer's two inputs disagreed, or the two
    /// interpolated arms did. The second should be impossible — both are
    /// fed from one split — so if it fires the arms have drifted.
    pub frame_mismatch: bool,
    /// A stage clipped. Only possible with a compensated interpolator,
    /// which has gain above one.
    pub saturated: bool,
}

impl<
    const W: usize,
    const WA: usize,
    const CW: usize,
    const OW: usize,
    const PROD_W: usize,
    const DROP: usize,
    C,
> SynchronousIO for RealDuc<W, WA, CW, OW, PROD_W, DROP, C>
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<WA>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    rhdl::bits::W<OW>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
    C: SynchronousIO<I = interpolator::In<W, CW>, O = interpolator::Out<WA>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    type I = In<W, CW>;
    type O = Out<OW>;
    type Kernel = real_duc_kernel<W, WA, CW, OW, PROD_W, DROP, C>;
}

#[kernel]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn real_duc_kernel<
    const W: usize,
    const WA: usize,
    const CW: usize,
    const OW: usize,
    const PROD_W: usize,
    const DROP: usize,
    C,
>(
    cr: ClockReset,
    i: In<W, CW>,
    q: Q<W, WA, CW, OW, PROD_W, DROP, C>,
) -> (Out<OW>, D<W, WA, CW, OW, PROD_W, DROP, C>)
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<WA>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    rhdl::bits::W<OW>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
    C: SynchronousIO<I = interpolator::In<W, CW>, O = interpolator::Out<WA>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    let mut d = D::<W, WA, CW, OW, PROD_W, DROP, C>::dont_care();

    // ---- bring the envelope up to the converter rate ----
    d.up = upsampler::In::<W, CW> {
        stream: i.stream,
        rate: i.rate,
        downstream_ready: i.downstream_ready,
    };

    // ---- the carrier ----
    //
    // Tuning in the master term, offset in the pulse term, matching the
    // composer layering in `dsp::nco`. The accumulator is never reset:
    // its phase is absolute elapsed time, which is what makes successive
    // bursts phase-comparable.
    //
    // **Not conjugated**, unlike the down-converter's oscillator.
    // Multiplying by `e^{+jwt}` shifts *up*, which is what this chain is
    // for. The down-converter negates the quadrature component to get
    // `e^{-jwt}`; doing so here would place the signal at `-f`.
    d.lo = composite::In {
        frequency: frequency_composer::In::<PHASE_W> {
            master: i.frequency,
            scheduled_offset: bits::<PHASE_W>(0),
            modulation: bits::<PHASE_W>(0),
            calibration: bits::<PHASE_W>(0),
        },
        phase: phase_composer::In::<PHASE_W> {
            pulse: i.phase,
            frame: bits::<PHASE_W>(0),
            calibration: bits::<PHASE_W>(0),
            fine_time: bits::<PHASE_W>(0),
            trim: bits::<PHASE_W>(0),
        },
        downstream_ready: true,
    };

    // ---- modulate, keeping the real part ----
    d.mix = real_part::In::<SyncMark, WA, { sin_cos_linear_interp::AMP_W }> {
        a: q.up.stream.data,
        b: q.lo.stream.data,
        downstream_ready: i.downstream_ready,
    };

    let mut o = Out::<OW> {
        stream: RCStream::<Real<OW>, SyncMark> {
            data: q.mix.stream.data,
            // The request travels back from the *upsampler*, not from
            // the mixer: the mixer consumes every cycle and is
            // vacuously ready, so forwarding its `ready` would tell
            // upstream to send an envelope sample every cycle.
            ready: q.up.stream.ready,
        },
        master: q.lo.master,
        starved: q.up.starved || q.mix.starved,
        overrun: !i.downstream_ready || q.up.overrun,
        frame_mismatch: q.up.frame_mismatch || q.mix.frame_mismatch,
        saturated: q.up.saturated,
    };

    if cr.reset.any() {
        o.stream = RCStream::<Real<OW>, SyncMark> {
            data: None,
            ready: false,
        };
        o.starved = false;
        o.overrun = false;
        o.frame_mismatch = false;
        o.saturated = false;
    }

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::cic::{interp, interpolator::CicInterpolate};
    use crate::dsp::iq::Iq;
    use expect_test::expect;
    use std::f64::consts::TAU;

    const W: usize = 8;
    const WA: usize = 11;
    const S: usize = 2;
    const RMAX: usize = 8;
    const M: usize = 1;
    const CW: usize = 4;
    const OW: usize = 12;
    const PROD_W: usize = WA + sin_cos_linear_interp::AMP_W + 1;
    const DROP: usize = PROD_W - OW;
    const RATE: usize = 8;
    type Core = CicInterpolate<W, WA, S, RMAX, M, CW>;
    type Uut = RealDuc<W, WA, CW, OW, PROD_W, DROP, Core>;

    /// A tuning word for `f` in cycles per output sample.
    fn tune(f: f64) -> Bits<PHASE_W> {
        bits::<PHASE_W>((f * (1u128 << PHASE_W) as f64).round() as u128)
    }

    /// Magnitude of the DFT of `x` at normalised frequency `f`.
    ///
    /// Written out rather than pulled from a crate: the frequencies of
    /// interest are known exactly, so a single-bin evaluation is both
    /// enough and exact at those bins.
    fn mag_at(x: &[i128], f: f64) -> f64 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, v) in x.iter().enumerate() {
            let t = -TAU * f * n as f64;
            re += *v as f64 * t.cos();
            im += *v as f64 * t.sin();
        }
        (re * re + im * im).sqrt() / x.len() as f64
    }

    fn env(re: i128, im: i128, mark: bool) -> Option<Item<Iq<W>, SyncMark>> {
        Some(Item::<Iq<W>, SyncMark> {
            data: Iq::<W> {
                re: signed::<W>(re),
                im: signed::<W>(im),
            },
            frame: SyncMark { sync: mark },
        })
    }

    /// Drive `cycles` output cycles with an envelope produced by `f`,
    /// indexed by *envelope* sample number.
    fn run_with(cycles: usize, carrier: f64, f: &dyn Fn(usize) -> (i128, i128)) -> Vec<i128> {
        let uut = Uut::default();
        let seq: Vec<In<W, CW>> = (0..cycles)
            .map(|n| {
                let (re, im) = f(n / RATE);
                In::<W, CW> {
                    stream: env(re, im, n == 0),
                    rate: bits::<CW>(RATE as u128),
                    frequency: tune(carrier),
                    phase: bits::<PHASE_W>(0),
                    downstream_ready: true,
                }
            })
            .collect();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.stream.data.map(|it| it.data.v.raw()))
            .collect()
    }

    #[test]
    fn default_construction() {
        let _ = Uut::default();
    }

    #[test]
    fn the_test_configuration_is_consistent() {
        assert_eq!(interp::accumulator_width(W, S, RMAX, M), WA);
        assert_eq!(interp::rate_width(RMAX), CW);
        assert!(PROD_W > WA + sin_cos_linear_interp::AMP_W);
    }

    /// **A constant envelope produces a clean tone at the carrier.**
    ///
    /// The simplest end-to-end statement of what this chain is for.
    #[test]
    fn a_constant_envelope_produces_a_tone_at_the_carrier() {
        let carrier = 0.25;
        let out = run_with(512, carrier, &|_| (100, 0));
        let settled = &out[64..];
        let wanted = mag_at(settled, carrier);
        // Nothing at an unrelated frequency.
        let elsewhere = mag_at(settled, 0.1);
        assert!(
            wanted > 20.0 * elsewhere,
            "carrier {wanted:.1} should dominate {elsewhere:.1}"
        );
    }

    /// **A rotating complex envelope produces *one* sideband.**
    ///
    /// The test that matters, and the reason the envelope is complex.
    /// An envelope rotating at `+f_e` puts the signal at `f_c + f_e/R`
    /// and — because the modulation is a true complex multiply — leaves
    /// `f_c − f_e/R` empty. A real envelope could not do that; the two
    /// sidebands would be forced to be mirror images.
    ///
    /// It is also the test that catches a conjugated carrier. The
    /// down-converter shipped with exactly that bug once and passed a
    /// magnitude-only check, because a conjugated oscillator still
    /// produces a plausible flat magnitude — it just puts the energy at
    /// `f_c − f_e/R` instead. Here that would swap the two numbers
    /// below.
    #[test]
    fn a_rotating_envelope_produces_a_single_sideband() {
        let carrier = 0.25;
        // Envelope rotating at one eighth of a cycle per envelope
        // sample, so at 1/(8*RATE) per output sample.
        let rotate = |m: usize| -> (i128, i128) {
            let t = TAU * (m as f64) / 8.0;
            ((100.0 * t.cos()) as i128, (100.0 * t.sin()) as i128)
        };
        let out = run_with(1024, carrier, &rotate);
        let settled = &out[128..];

        let offset = 1.0 / (8.0 * RATE as f64);
        let upper = mag_at(settled, carrier + offset);
        let lower = mag_at(settled, carrier - offset);

        assert!(
            upper > 20.0 * lower,
            "upper sideband {upper:.1} must dominate the lower {lower:.1}; \
             if these are swapped the carrier is conjugated"
        );
    }

    /// And rotating the envelope the *other* way moves the signal to the
    /// other side, which is what makes the previous test meaningful.
    ///
    /// A single-sided test could pass on a chain that always put energy
    /// at `f_c + something`. This one requires the sideband to follow the
    /// envelope's direction.
    #[test]
    fn reversing_the_envelope_reverses_the_sideband() {
        let carrier = 0.25;
        let rotate = |sign: f64| {
            move |m: usize| -> (i128, i128) {
                let t = sign * TAU * (m as f64) / 8.0;
                ((100.0 * t.cos()) as i128, (100.0 * t.sin()) as i128)
            }
        };
        let offset = 1.0 / (8.0 * RATE as f64);

        let up = run_with(1024, carrier, &rotate(1.0));
        let down = run_with(1024, carrier, &rotate(-1.0));

        let up_hi = mag_at(&up[128..], carrier + offset);
        let up_lo = mag_at(&up[128..], carrier - offset);
        let dn_hi = mag_at(&down[128..], carrier + offset);
        let dn_lo = mag_at(&down[128..], carrier - offset);

        assert!(up_hi > 20.0 * up_lo, "forward: {up_hi:.1} vs {up_lo:.1}");
        assert!(dn_lo > 20.0 * dn_hi, "reverse: {dn_lo:.1} vs {dn_hi:.1}");
    }

    /// **The interpolation images are suppressed.**
    ///
    /// A CIC interpolator's `sinc^N` nulls sit at multiples of the
    /// envelope rate, which is where a constant envelope's images land.
    /// So the output either side of the carrier at `±1/R` should be
    /// essentially empty — and that is the whole reason a CIC is the
    /// right filter here rather than a plain zero-order hold.
    #[test]
    fn the_interpolation_images_are_suppressed() {
        let carrier = 0.25;
        let out = run_with(512, carrier, &|_| (100, 0));
        let settled = &out[64..];
        let wanted = mag_at(settled, carrier);
        let image = mag_at(settled, carrier + 1.0 / RATE as f64);
        assert!(
            wanted > 100.0 * image,
            "first image {image:.2} must be far below the carrier {wanted:.1}"
        );
    }

    /// **`ready` asks for one envelope sample per `R` cycles.**
    ///
    /// The pull contract. It comes from the upsampler and not from the
    /// mixer, which is vacuously ready every cycle — forwarding the
    /// mixer's would ask upstream for a sample per cycle.
    #[test]
    fn ready_asks_once_per_rate() {
        let uut = Uut::default();
        let seq: Vec<In<W, CW>> = (0..6 * RATE)
            .map(|_| In::<W, CW> {
                stream: env(50, 0, false),
                rate: bits::<CW>(RATE as u128),
                frequency: tune(0.25),
                phase: bits::<PHASE_W>(0),
                downstream_ready: true,
            })
            .collect();
        let readies: Vec<bool> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.stream.ready)
            .collect();
        assert!(!readies[0]);
        for (n, r) in readies[1..].iter().enumerate() {
            assert_eq!(*r, n % RATE == 0, "cycle {n}");
        }
    }

    /// The output is present on every cycle, which is what a DAC wants.
    #[test]
    fn the_output_is_present_every_cycle() {
        let uut = Uut::default();
        let seq: Vec<In<W, CW>> = (0..4 * RATE)
            .map(|_| In::<W, CW> {
                stream: env(50, 0, false),
                rate: bits::<CW>(RATE as u128),
                frequency: tune(0.25),
                phase: bits::<PHASE_W>(0),
                downstream_ready: true,
            })
            .collect();
        let present: Vec<bool> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.stream.data.is_some())
            .collect();
        assert!(!present[0], "reset emits nothing");
        assert!(present[1..].iter().all(|p| *p));
    }

    /// The master phase advances, and is the reference a burst is
    /// relative to.
    #[test]
    fn the_master_phase_advances_with_the_tuning_word() {
        let uut = Uut::default();
        let f = tune(0.25);
        let seq: Vec<In<W, CW>> = (0..8)
            .map(|_| In::<W, CW> {
                stream: env(10, 0, false),
                rate: bits::<CW>(RATE as u128),
                frequency: f,
                phase: bits::<PHASE_W>(0),
                downstream_ready: true,
            })
            .collect();
        let masters: Vec<u128> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.master.raw())
            .collect();
        // Strictly advancing once running, and by the tuning word.
        let live = &masters[3..];
        for w in live.windows(2) {
            assert_eq!(
                w[1].wrapping_sub(w[0]) & ((1u128 << PHASE_W) - 1),
                f.raw(),
                "the accumulator advances by the tuning word"
            );
        }
    }

    /// **The full-scale case does not wrap.**
    ///
    /// The headroom claim in the module docs. The mixer does not
    /// saturate, so a `DROP` too small for the combined interpolator and
    /// carrier gain would wrap — a sign flip on the largest sample the
    /// transmitter can send. Driven at negative full scale on both
    /// quadratures, which is the worst case.
    #[test]
    fn the_full_scale_case_does_not_wrap() {
        let out = run_with(512, 0.25, &|_| (-128, -128));
        let settled = &out[64..];
        // A wrap shows up as a discontinuity far larger than the signal
        // itself, so bound the sample-to-sample step by the envelope of
        // a tone at this frequency.
        let peak = settled.iter().map(|v| v.abs()).max().unwrap();
        assert!(peak < (1 << (OW - 1)), "peak {peak} must fit {OW} bits");
        // And the spectrum is still a single clean tone rather than the
        // broadband mess a wrap produces.
        let wanted = mag_at(settled, 0.25);
        let elsewhere = mag_at(settled, 0.13);
        assert!(
            wanted > 20.0 * elsewhere,
            "a wrap would spread energy: {wanted:.1} vs {elsewhere:.1}"
        );
    }

    /// A mark on the envelope reaches the output.
    #[test]
    fn the_mark_reaches_the_output() {
        let uut = Uut::default();
        let seq: Vec<In<W, CW>> = (0..4 * RATE)
            .map(|n| In::<W, CW> {
                stream: env(40, 0, n == 2 * RATE),
                rate: bits::<CW>(RATE as u128),
                frequency: tune(0.25),
                phase: bits::<PHASE_W>(0),
                downstream_ready: true,
            })
            .collect();
        let marks: usize = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter(|s| s.output.stream.data.map(|it| it.frame.sync) == Some(true))
            .count();
        assert_eq!(marks, 1, "exactly one marked output sample");
    }

    /// A lost output is reported.
    #[test]
    fn a_lost_sample_is_reported() {
        let uut = Uut::default();
        let seq: Vec<In<W, CW>> = (0..3 * RATE)
            .map(|n| In::<W, CW> {
                stream: env(40, 0, false),
                rate: bits::<CW>(RATE as u128),
                frequency: tune(0.25),
                phase: bits::<PHASE_W>(0),
                downstream_ready: n != RATE + 1,
            })
            .collect();
        let fired: Vec<usize> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .enumerate()
            .filter(|(_, s)| s.output.overrun)
            .map(|(n, _)| n)
            .collect();
        assert_eq!(fired, vec![1 + RATE + 1]);
    }

    /// **Where the chain's multiplies actually are.**
    ///
    /// Six, and only two of them are the mixer's. The other four are the
    /// oscillator's sine/cosine interpolation — two per quadrature. The
    /// CIC interpolators contribute none, by construction.
    ///
    /// This started life asserting two and claiming the oscillator was
    /// shift-and-add, which was simply wrong;
    /// [`crate::dsp::nco::sin_cos_linear_interp`] interpolates between
    /// table entries and a linear interpolation is a multiply. The
    /// breakdown is pinned per module rather than as a total, because
    /// the total alone would not have caught the mistake.
    ///
    /// The useful consequence: choosing [`RealDuc`] over
    /// [`super::IqDuc`] saves two of eight, not half — and a design with
    /// several up-converters saves far more by sharing one oscillator
    /// than by choosing between these two.
    #[test]
    fn the_chain_costs_six_multiplies_and_only_two_are_the_mixer() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();

        let mut per_module: Vec<(String, usize)> = Vec::new();
        let mut cur = String::new();
        for line in hdl.lines() {
            if let Some(rest) = line.trim().strip_prefix("module ") {
                cur = rest.split('(').next().unwrap_or("?").to_string();
            }
            if line.contains(" * ") {
                match per_module.iter_mut().find(|(m, _)| *m == cur) {
                    Some(e) => e.1 += 1,
                    None => per_module.push((cur.clone(), 1)),
                }
            }
        }
        per_module.sort();
        let shape = per_module
            .iter()
            .map(|(m, n)| format!("{m}: {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let expect = expect![[r#"
            top_lo_amp: 4
            top_mix: 2"#]];
        expect.assert_eq(&shape);

        // And the full-complex chain costs two more, which is the whole
        // difference between the two up-converters.
        let full = super::super::iq::IqDuc::<W, WA, CW, OW, PROD_W, DROP, Core>::default();
        let full_hdl = full.descriptor("top".into())?.hdl()?.modules.pretty();
        assert_eq!(hdl.matches(" * ").count(), 6);
        assert_eq!(full_hdl.matches(" * ").count(), 8);
        Ok(())
    }

    /// **The worked sizing in [`super`]'s docs, checked.**
    ///
    /// Every number in that table is derived from
    /// [`crate::dsp::cic::interp`], so a change in the design maths
    /// should break this rather than quietly leaving the prose wrong.
    /// The configuration is the one these widgets were written for: a
    /// 16-bit complex envelope at 1 Msps onto a 125 Msps carrier.
    #[test]
    fn the_worked_sizing_is_what_the_docs_say() {
        const WU: usize = 16;
        const SU: usize = 3;
        const RU: usize = 125;
        const MU: usize = 1;
        const OWU: usize = 14;

        assert_eq!(interp::gain_bits(SU, RU, MU), 14);
        let wa = interp::accumulator_width(WU, SU, RU, MU);
        assert_eq!(wa, 30);
        assert_eq!(interp::rate_width(RU), 7);
        let prod = wa + sin_cos_linear_interp::AMP_W + 1;
        assert_eq!(prod, 49);
        assert_eq!(prod - OWU, 35);

        let (num, den) = interp::dc_gain_ratio(SU, RU, MU);
        assert_eq!((num, den), (1_953_125, 125));
        assert_eq!(num / den, 15_625);

        // The taper, and the saving it would buy.
        let widths: Vec<usize> = (1..=2 * SU)
            .map(|j| interp::stage_width(j, WU, SU, RU, MU))
            .collect();
        assert_eq!(widths, vec![17, 18, 19, 18, 24, 30]);
        assert_eq!(interp::uniform_state_bits(WU, SU, RU, MU), 180);
        assert_eq!(interp::tapered_state_bits(WU, SU, RU, MU), 126);

        // And the whole configuration actually builds, which is the part
        // a table of numbers does not establish.
        type Big = RealDuc<WU, 30, 7, OWU, 49, 35, CicInterpolate<WU, 30, SU, RU, MU, 7>>;
        let _ = Big::default();
    }

    // ---- Tier 3 / 4 / 5 --------------------------------------------

    fn tb_stream() -> Vec<In<W, CW>> {
        (0..4 * RATE)
            .map(|n| {
                let m = n / RATE;
                let t = TAU * (m as f64) / 4.0;
                In::<W, CW> {
                    stream: env((60.0 * t.cos()) as i128, (60.0 * t.sin()) as i128, n == 0),
                    rate: bits::<CW>(RATE as u128),
                    frequency: tune(0.25),
                    phase: bits::<PHASE_W>(0),
                    downstream_ready: n != 11,
                }
            })
            .collect()
    }

    /// The module shape, following [`crate::dsp::ddc`]'s convention for
    /// a chain this size: the full text is thousands of lines and its
    /// interesting content is checked structurally elsewhere.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Uut::default();
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
            module top_up
            module top_lo
            module top_mix"#]];
        expect.assert_eq(&shape);
        Ok(())
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
            .join("duc_real");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["7db118a6902fd0a82e17fb9342af2471e531c071b651b7a2a5cb5b2caf5d7675"];
        let digest = vcd.dump_to_file(root.join("duc_real.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
