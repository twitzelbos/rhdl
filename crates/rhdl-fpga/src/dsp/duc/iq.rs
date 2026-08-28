#![warn(missing_docs)]
//! `IqDuc` — a digital up-converter with a quadrature output.
//!
//! [`super::RealDuc`] with the full complex mixer in place of the
//! real-part one, so both components of the product leave the widget.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+IqDuc+-------------------+
      |                           |
+---->+ stream                    |
      |  Option<Item<Iq<W>,       |
      |             SyncMark>>    |
+---->+ rate               stream |
      |   Bits<CW>    RCStream<   |
+---->+ frequency      Iq<OW>,    +----->
      |   (carrier)    SyncMark>  |
+---->+ phase                     |
      |               master      +----->
+---->+ downstream_ready starved  +----->
      |                  overrun  +----->
      |           frame_mismatch  +----->
      |                saturated  +----->
      +---------------------------+
")]
//!
//!# Internals
#![doc = badascii_doc::badascii!(r"
   stream ->+-+EnvelopeUpsampler+-+
   rate --->|  split/interp/join  +--+
            +---------------------+  |  +-+ComplexMixer+-+
                                     +->|  env*carrier   +--> Iq<OW>
   frequency -> +-+Nco+-+               +----------------+
   phase -----> |  LO   +---------------^  four multiplies
                +-------+
                    |
                    +--> master (phase reference)
")]
//!
//! # When this rather than [`super::RealDuc`]
//!
//! When the passband is formed **outside** the FPGA: a quadrature
//! modulator, an I/Q DAC pair, or a transceiver that takes baseband I
//! and Q. Both components have to leave the chip, so both have to be
//! computed, and the mixer is the full four-multiply
//! [`crate::dsp::mixer::ComplexMixer`].
//!
//! If a single DAC carries the signal, use [`super::RealDuc`] — it is
//! the same chain with two multiplies instead of four, because
//! `ad + bc` is a value nothing downstream of one converter can carry.
//!
//! # A frequency-shifted complex baseband, not a real signal
//!
//! The output is `env · e^{jwt}`, which is the envelope translated in
//! frequency and still complex. That is a different thing from
//! [`super::RealDuc`]'s output and not merely a wider one: a real
//! passband signal has conjugate-symmetric spectrum and this does not,
//! so a downstream stage that treats `Out::stream` as two independent
//! real signals will get the sideband arithmetic wrong.
//!
//! It is also worth noting that `Iq` out at a *zero* tuning word is the
//! interpolated envelope itself, so this widget doubles as the
//! interpolate-only path when [`In::frequency`] is zero — at the cost of
//! four idle multiplies. [`super::EnvelopeUpsampler`] is the honest
//! spelling of that.
//!
//! # Everything else follows `RealDuc`
//!
//! Coherence, the pull-not-push `ready` contract, the un-normalised
//! gain, the mark handling and the headroom warning are all identical
//! and documented on [`super::RealDuc`] rather than repeated here. The
//! only differences are the mixer and the output type.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/duc_iq.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/duc_iq.md")]

use rhdl::prelude::*;

use super::upsampler::{self, EnvelopeUpsampler};
use crate::dsp::cic::interpolator;
use crate::dsp::iq::Iq;
use crate::dsp::mixer::complex::{self, ComplexMixer};
use crate::dsp::nco::composite;
use crate::dsp::nco::config::PHASE_W;
use crate::dsp::nco::{frequency_composer, phase_composer, sin_cos_linear_interp};
use crate::dsp::sync::SyncMark;
use crate::rcstream::bus::{Item, RCStream};

/// A digital up-converter emitting a complex passband signal.
///
/// Parameters as [`super::RealDuc`], with `PROD_W` at least
/// `WA + AMP_W + 1` — checked by the mixer's own `Default`.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct IqDuc<
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
    /// `envelope × carrier`, four multiplies.
    mix: ComplexMixer<SyncMark, WA, { sin_cos_linear_interp::AMP_W }, OW, PROD_W, DROP>,
}

impl<
    const W: usize,
    const WA: usize,
    const CW: usize,
    const OW: usize,
    const PROD_W: usize,
    const DROP: usize,
    C,
> IqDuc<W, WA, CW, OW, PROD_W, DROP, C>
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
    /// Build the chain around one interpolator, cloned into both arms.
    ///
    /// For an interpolator that cannot be defaulted — a
    /// [`crate::dsp::cic::compensated_interp::CompensatedInterp`], whose
    /// filter half needs taps. [`Default`] covers the rest. See
    /// [`super::EnvelopeUpsampler::new`] on why this takes one arm and
    /// not two.
    pub fn new(cic: C) -> Self {
        assert_eq!(
            PROD_W,
            WA + sin_cos_linear_interp::AMP_W + 1,
            "PROD_W is the mixer's natural product width, A_W + B_W + 1; \
             Rust cannot derive it from WA without generic_const_exprs"
        );
        Self {
            up: EnvelopeUpsampler::new(cic),
            lo: composite::NcoDefault::default(),
            mix: ComplexMixer::default(),
        }
    }
}

impl<
    const W: usize,
    const WA: usize,
    const CW: usize,
    const OW: usize,
    const PROD_W: usize,
    const DROP: usize,
    C,
> Default for IqDuc<W, WA, CW, OW, PROD_W, DROP, C>
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
            mix: ComplexMixer::default(),
        }
    }
}

/// Inputs to [`IqDuc`]. Identical to [`super::real::In`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const W: usize, const CW: usize>
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
{
    /// The low-rate complex envelope, framed. Consumed on cycles where
    /// `Out::stream.ready` is high.
    pub stream: Option<Item<Iq<W>, SyncMark>>,
    /// The interpolation factor. A rate change wants a mark with it.
    pub rate: Bits<CW>,
    /// Carrier tuning word.
    pub frequency: Bits<PHASE_W>,
    /// Carrier phase offset.
    pub phase: Bits<PHASE_W>,
    /// Downstream's ready, per the `RCStream` contract.
    pub downstream_ready: bool,
}

/// Outputs from [`IqDuc`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const OW: usize>
where
    rhdl::bits::W<OW>: BitWidth,
{
    /// The complex passband stream — present every cycle, and `ready`
    /// carrying the once-per-`R` request for the next envelope sample.
    ///
    /// **Not two independent real signals.** See the module docs.
    pub stream: RCStream<Iq<OW>, SyncMark>,
    /// The carrier's undisturbed master phase.
    pub master: Bits<PHASE_W>,
    /// A stage asked for a sample and found none.
    pub starved: bool,
    /// An output was produced while `downstream_ready` was low.
    pub overrun: bool,
    /// The envelope and the carrier disagreed about the burst origin.
    pub frame_mismatch: bool,
    /// A stage clipped.
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
> SynchronousIO for IqDuc<W, WA, CW, OW, PROD_W, DROP, C>
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
    type Kernel = iq_duc_kernel<W, WA, CW, OW, PROD_W, DROP, C>;
}

#[kernel]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn iq_duc_kernel<
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

    d.up = upsampler::In::<W, CW> {
        stream: i.stream,
        rate: i.rate,
        downstream_ready: i.downstream_ready,
    };

    // **Not conjugated.** `e^{+jwt}` shifts up, which is what this chain
    // is for; the down-converter negates the quadrature component to get
    // `e^{-jwt}` and doing so here would place the signal at `-f`.
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

    d.mix = complex::In::<SyncMark, WA, { sin_cos_linear_interp::AMP_W }> {
        a: q.up.stream.data,
        b: q.lo.stream.data,
        downstream_ready: i.downstream_ready,
    };

    let mut o = Out::<OW> {
        stream: RCStream::<Iq<OW>, SyncMark> {
            data: q.mix.stream.data,
            // From the upsampler, not the mixer: the mixer consumes
            // every cycle and is vacuously ready, so forwarding its
            // `ready` would ask upstream for an envelope sample every
            // cycle.
            ready: q.up.stream.ready,
        },
        master: q.lo.master,
        starved: q.up.starved || q.mix.starved,
        overrun: !i.downstream_ready || q.up.overrun,
        frame_mismatch: q.up.frame_mismatch || q.mix.frame_mismatch,
        saturated: q.up.saturated,
    };

    if cr.reset.any() {
        o.stream = RCStream::<Iq<OW>, SyncMark> {
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
    type Uut = IqDuc<W, WA, CW, OW, PROD_W, DROP, Core>;

    fn tune(f: f64) -> Bits<PHASE_W> {
        bits::<PHASE_W>((f * (1u128 << PHASE_W) as f64).round() as u128)
    }

    /// Magnitude of the DFT of a complex sequence at `f`.
    fn mag_at(x: &[(i128, i128)], f: f64) -> f64 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, (xr, xi)) in x.iter().enumerate() {
            let t = -TAU * f * n as f64;
            let (c, s) = (t.cos(), t.sin());
            re += *xr as f64 * c - *xi as f64 * s;
            im += *xr as f64 * s + *xi as f64 * c;
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

    fn stimulus(cycles: usize, carrier: f64, f: &dyn Fn(usize) -> (i128, i128)) -> Vec<In<W, CW>> {
        (0..cycles)
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
            .collect()
    }

    fn run(seq: Vec<In<W, CW>>) -> Vec<(i128, i128)> {
        let uut = Uut::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| {
                s.output
                    .stream
                    .data
                    .map(|it| (it.data.re.raw(), it.data.im.raw()))
            })
            .collect()
    }

    #[test]
    fn default_construction() {
        let _ = Uut::default();
    }

    #[test]
    fn the_test_configuration_is_consistent() {
        assert_eq!(interp::accumulator_width(W, S, RMAX, M), WA);
        assert!(PROD_W > WA + sin_cos_linear_interp::AMP_W);
    }

    /// **The real part is exactly what [`super::RealDuc`] emits.**
    ///
    /// The cross-check that validates both widgets against each other:
    /// two independently wired chains, one computing four products and
    /// one computing two, must agree bit for bit on the component they
    /// share. A rounding difference, a swapped operand, or a sign error
    /// in either mixer shows up here and nowhere else in either file's
    /// tests.
    #[test]
    fn the_real_part_matches_the_real_up_converter() {
        let rotate = |m: usize| -> (i128, i128) {
            let t = TAU * (m as f64) / 5.0;
            ((90.0 * t.cos()) as i128, (90.0 * t.sin()) as i128)
        };
        let carrier = 0.3;
        let mine: Vec<i128> = run(stimulus(24 * RATE, carrier, &rotate))
            .iter()
            .map(|(re, _)| *re)
            .collect();

        let real = super::super::real::RealDuc::<W, WA, CW, OW, PROD_W, DROP, Core>::default();
        let seq: Vec<super::super::real::In<W, CW>> = (0..24 * RATE)
            .map(|n| {
                let (re, im) = rotate(n / RATE);
                super::super::real::In::<W, CW> {
                    stream: env(re, im, n == 0),
                    rate: bits::<CW>(RATE as u128),
                    frequency: tune(carrier),
                    phase: bits::<PHASE_W>(0),
                    downstream_ready: true,
                }
            })
            .collect();
        let theirs: Vec<i128> = real
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.stream.data.map(|it| it.data.v.raw()))
            .collect();

        assert_eq!(mine, theirs);
    }

    /// **The output spectrum is one-sided, which `RealDuc`'s is not.**
    ///
    /// The property that makes this a different widget rather than a
    /// wider one. A real passband signal has conjugate-symmetric
    /// spectrum, so energy at `+f` implies energy at `−f`. This output is
    /// a frequency-translated *complex* baseband: energy at `+f` and
    /// nothing at `−f`.
    ///
    /// It is also why the module docs warn against treating
    /// `Out::stream` as two independent real signals — a stage that does
    /// will get the sideband arithmetic wrong.
    #[test]
    fn the_complex_output_is_one_sided() {
        let carrier = 0.25;
        let out = run(stimulus(1024, carrier, &|_| (100, 0)));
        let settled = &out[128..];
        let positive = mag_at(settled, carrier);
        let negative = mag_at(settled, -carrier);
        assert!(
            positive > 20.0 * negative,
            "complex output must be one-sided: +f {positive:.1}, -f {negative:.1}"
        );

        // And the real up-converter's output, by contrast, is symmetric.
        let real = super::super::real::RealDuc::<W, WA, CW, OW, PROD_W, DROP, Core>::default();
        let seq: Vec<super::super::real::In<W, CW>> = (0..1024)
            .map(|n| super::super::real::In::<W, CW> {
                stream: env(100, 0, n == 0),
                rate: bits::<CW>(RATE as u128),
                frequency: tune(carrier),
                phase: bits::<PHASE_W>(0),
                downstream_ready: true,
            })
            .collect();
        let r: Vec<(i128, i128)> = real
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.stream.data.map(|it| (it.data.v.raw(), 0i128)))
            .collect();
        let rs = &r[128..];
        let rp = mag_at(rs, carrier);
        let rn = mag_at(rs, -carrier);
        assert!(
            (rp - rn).abs() < 0.05 * rp,
            "a real output must be symmetric: +f {rp:.1}, -f {rn:.1}"
        );
    }

    /// A rotating envelope lands on one side of the carrier.
    #[test]
    fn a_rotating_envelope_produces_a_single_sideband() {
        let carrier = 0.25;
        let rotate = |m: usize| -> (i128, i128) {
            let t = TAU * (m as f64) / 8.0;
            ((100.0 * t.cos()) as i128, (100.0 * t.sin()) as i128)
        };
        let out = run(stimulus(1024, carrier, &rotate));
        let settled = &out[128..];
        let offset = 1.0 / (8.0 * RATE as f64);
        let upper = mag_at(settled, carrier + offset);
        let lower = mag_at(settled, carrier - offset);
        assert!(
            upper > 20.0 * lower,
            "upper {upper:.1} must dominate lower {lower:.1}"
        );
    }

    /// `ready` asks for one envelope sample per `R` cycles.
    #[test]
    fn ready_asks_once_per_rate() {
        let uut = Uut::default();
        let seq = stimulus(6 * RATE, 0.25, &|_| (50, 0));
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

    /// **Eight multiplies: four in the mixer, four in the oscillator.**
    ///
    /// Two more than [`super::RealDuc`], which is the entire difference
    /// between them. Note the oscillator's four are the same either way,
    /// so the choice is two multiplies out of eight rather than the half
    /// a mixer-only comparison suggests.
    #[test]
    fn the_mixer_costs_four_and_the_oscillator_four() -> miette::Result<()> {
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
            top_mix: 4"#]];
        expect.assert_eq(&shape);
        Ok(())
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
        assert_eq!(marks, 1);
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

    /// The module shape, per [`crate::dsp::ddc`]'s convention.
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
            .join("duc_iq");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["b495bb9d79a2f49b0cc964b2c7cb67fb4a9e623e6cf2d7d4b3bb201856e5c706"];
        let digest = vcd.dump_to_file(root.join("duc_iq.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
