#![warn(missing_docs)]
//! `EnvelopeUpsampler` — a complex envelope, interpolated to the
//! converter rate.
//!
//! The front end both digital up-converters in [`super`] share: split
//! the complex envelope into two real arms, interpolate each with an
//! identical CIC, and recombine. What comes out is the same signal at
//! the converter's sample rate, ready to be modulated onto a carrier.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+EnvelopeUpsampler+---------+
      |                             |
+---->+ stream                      |
      | Option<Item<Iq<W>,SyncMark>>|
      |                      stream |
+---->+ rate      RCStream<Iq<WA>,  |
      |   Bits<CW>         SyncMark>+----->
+---->+ downstream_ready            |
      |                     starved +----->
      |                     overrun +----->
      |              frame_mismatch +----->
      |                   saturated +----->
      +-----------------------------+
")]
//!
//!# Internals
#![doc = badascii_doc::badascii!(r"
   stream --> +-+IqSplit+-+
              +-----------+
                |       |
         Real<W>|       |Imag<W>
                v       v
   +-+StreamInterpolator+-+  +-+StreamInterpolator+-+
   |      C  (I arm)      |  |      C  (Q arm)      |
   +----------------------+  +----------------------+
                |       |
                v       v
              +-+IqCombine+-+
              +-------------+
                     |
                     v
        RCStream<Iq<WA>,SyncMark>
")]
//!
//! # Both arms are the same type, and that is load bearing
//!
//! Exactly as in [`crate::dsp::ddc::Ddc`], and for the mirror-image
//! reason. A difference in gain or group delay between the in-phase and
//! quadrature arms rotates the constellation — on receive that corrupts
//! a phase measurement, and on transmit it puts energy in the sideband
//! the modulation was supposed to suppress.
//!
//! Both arms are the *same* generic `C` at the same configuration, so an
//! asymmetry between them is unrepresentable rather than merely
//! discouraged. `Imag<W>` is converted to `Real<W>` at the boundary to
//! make that possible: an interpolator is a real filter and does not
//! care which half of a complex signal it carries. The newtypes exist to
//! stop the *caller* mixing the halves up.
//!
//! They also share an interpolation phase, because both are driven from
//! one [`crate::rcstream::util::IqSplit`] and therefore see every mark
//! on the same cycle. They are separate widgets rather than one, so
//! `each_arm_matches_a_lone_interpolator` checks it instead of assuming
//! it -- an arm on a different grid could not reproduce what a single
//! interpolator produces from the same input.
//!
//! # `ready` is the whole point of the ordering
//!
//! An interpolator asks for a sample once every `R` cycles, so this
//! widget is the rate-controlling element of a transmit chain and
//! `Out::stream.ready` is a real request rather than a pass-through. It
//! is the conjunction of the two arms' requests, via `IqSplit` — which
//! is the same signal twice in practice, since the arms are identical,
//! and is written as a conjunction so that it stays correct if they
//! ever are not.
//!
//! # The gain is not normalised, and with a variable rate it cannot be
//!
//! The output carries the interpolator's full
//! `(R·M)^N / R` gain, and [`crate::dsp::cic::interp::dc_gain_ratio`]
//! reports it.
//!
//! Normalising it inside the chain would need a division by that factor
//! — and because the rate is a run-time input, the factor is a run-time
//! quantity, so it would be a run-time divide or a run-time barrel
//! shift on the full accumulator width at the converter clock. That is a
//! large price for something the caller can do more cheaply once it
//! knows what happens next, which is the same decision
//! [`crate::dsp::cic`] makes everywhere else.
//!
//! The practical consequence is that the mixer downstream multiplies at
//! the accumulator width. A caller who wants a narrower multiplier
//! inserts a scaler here and accepts a fixed rate, or scales at the low
//! rate before this widget and accepts the lost bits.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/duc_upsampler.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/duc_upsampler.md")]

use rhdl::prelude::*;

use crate::dsp::cic::{interp_stream, interpolator};
use crate::dsp::iq::{Imag, Iq, Real};
use crate::dsp::sync::SyncMark;
use crate::rcstream::bus::{Item, RCStream};
use crate::rcstream::util::{IqCombine, IqSplit};

/// A complex envelope interpolated to the converter rate.
///
/// `W` is the envelope width, `WA` the interpolator's accumulator width,
/// `CW` the rate field's width, and `C` the interpolator core — the same
/// type in both arms.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct EnvelopeUpsampler<const W: usize, const WA: usize, const CW: usize, C>
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<WA>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    C: SynchronousIO<I = interpolator::In<W, CW>, O = interpolator::Out<WA>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// Splits the complex envelope into two real streams.
    split: IqSplit<W, SyncMark>,
    /// In-phase arm.
    interp_i: interp_stream::StreamInterpolator<W, WA, CW, C>,
    /// Quadrature arm. **The same type as the in-phase arm** — see the
    /// module docs on why that identity is what keeps the sideband
    /// suppressed.
    interp_q: interp_stream::StreamInterpolator<W, WA, CW, C>,
    /// Recombines the two real streams into a complex one.
    combine: IqCombine<WA, SyncMark>,
}

impl<const W: usize, const WA: usize, const CW: usize, C> EnvelopeUpsampler<W, WA, CW, C>
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<WA>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    C: SynchronousIO<I = interpolator::In<W, CW>, O = interpolator::Out<WA>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// Build an upsampler around one interpolator, cloned into both
    /// arms.
    ///
    /// **One argument, not two, and that is the point.** An asymmetry
    /// between the in-phase and quadrature arms rotates the
    /// constellation, which on transmit leaks into the sideband the
    /// modulation was meant to suppress. Taking a single arm and cloning
    /// it makes the two identical by construction rather than by the
    /// caller's care — the same reasoning, and the same shape, as
    /// [`crate::dsp::ddc::Ddc::new`].
    ///
    /// Use this for an interpolator that cannot be defaulted — a
    /// [`crate::dsp::cic::compensated_interp::CompensatedInterp`], whose
    /// filter half needs taps. [`Default`] covers the rest.
    pub fn new(cic: C) -> Self {
        Self {
            split: IqSplit::default(),
            interp_i: interp_stream::StreamInterpolator::new(cic.clone()),
            interp_q: interp_stream::StreamInterpolator::new(cic),
            combine: IqCombine::default(),
        }
    }
}

impl<const W: usize, const WA: usize, const CW: usize, C> Default
    for EnvelopeUpsampler<W, WA, CW, C>
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<WA>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    C: SynchronousIO<I = interpolator::In<W, CW>, O = interpolator::Out<WA>>
        + Synchronous
        + Default
        + Clone
        + std::fmt::Debug,
{
    fn default() -> Self {
        Self {
            split: IqSplit::default(),
            interp_i: interp_stream::StreamInterpolator::default(),
            interp_q: interp_stream::StreamInterpolator::default(),
            combine: IqCombine::default(),
        }
    }
}

/// Inputs to [`EnvelopeUpsampler`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const W: usize, const CW: usize>
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
{
    /// The low-rate complex envelope, framed.
    ///
    /// A mark restarts the interpolation window on both arms at once.
    /// Consumed on cycles where `Out::stream.ready` is high.
    pub stream: Option<Item<Iq<W>, SyncMark>>,
    /// The interpolation factor. See
    /// [`crate::dsp::cic::interpolator::In::rate`], and note that a
    /// rate change wants a mark with it.
    pub rate: Bits<CW>,
    /// Downstream's ready, per the `RCStream` contract.
    pub downstream_ready: bool,
}

/// Outputs from [`EnvelopeUpsampler`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const WA: usize>
where
    rhdl::bits::W<WA>: BitWidth,
{
    /// The interpolated complex envelope, present every cycle, plus the
    /// upstream-facing once-per-`R` request on `ready`.
    pub stream: RCStream<Iq<WA>, SyncMark>,
    /// An arm asked for a sample and found none.
    pub starved: bool,
    /// An output was produced while `downstream_ready` was low.
    pub overrun: bool,
    /// **The two arms disagreed about framing.**
    ///
    /// Should be impossible: both are fed from one split and restart on
    /// the same mark. If it fires, the arms have drifted, and on
    /// transmit that means the constellation is rotating relative to
    /// where the caller put it.
    pub frame_mismatch: bool,
    /// An arm clipped. Always false for an exact-width core.
    pub saturated: bool,
}

impl<const W: usize, const WA: usize, const CW: usize, C> SynchronousIO
    for EnvelopeUpsampler<W, WA, CW, C>
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<WA>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    C: SynchronousIO<I = interpolator::In<W, CW>, O = interpolator::Out<WA>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    type I = In<W, CW>;
    type O = Out<WA>;
    type Kernel = envelope_upsampler_kernel<W, WA, CW, C>;
}

#[kernel]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn envelope_upsampler_kernel<const W: usize, const WA: usize, const CW: usize, C>(
    cr: ClockReset,
    i: In<W, CW>,
    q: Q<W, WA, CW, C>,
) -> (Out<WA>, D<W, WA, CW, C>)
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<WA>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    C: SynchronousIO<I = interpolator::In<W, CW>, O = interpolator::Out<WA>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    let mut d = D::<W, WA, CW, C>::dont_care();

    // ---- split the envelope into two real arms ----
    //
    // The arms' `ready` signals come back here, so the split reports one
    // request upstream. There is no combinational loop through it: each
    // arm's `ready` depends only on its own registered phase counter,
    // never on the data the split hands it.
    d.split = crate::rcstream::util::split::In::<W, SyncMark> {
        stream: i.stream,
        real_ready: q.interp_i.stream.ready,
        imag_ready: q.interp_q.stream.ready,
    };

    // ---- interpolate each arm ----
    d.interp_i = interp_stream::In::<W, CW> {
        stream: q.split.real.data,
        rate: i.rate,
        downstream_ready: i.downstream_ready,
    };

    // `Imag<W>` becomes `Real<W>` on the way in and back on the way out.
    // Not a fudge: an interpolator is a real-valued filter and does not
    // care which half of a complex signal it carries. Converting at the
    // boundary is what lets both arms be the same type, which is the
    // property the sideband suppression depends on.
    let mut q_in = None;
    if let Some(it) = q.split.imag.data {
        q_in = Some(Item::<Real<W>, SyncMark> {
            data: Real::<W> { v: it.data.v },
            frame: it.frame,
        });
    }
    d.interp_q = interp_stream::In::<W, CW> {
        stream: q_in,
        rate: i.rate,
        downstream_ready: i.downstream_ready,
    };

    // ---- recombine ----
    let mut q_out = None;
    if let Some(it) = q.interp_q.stream.data {
        q_out = Some(Item::<Imag<WA>, SyncMark> {
            data: Imag::<WA> { v: it.data.v },
            frame: it.frame,
        });
    }
    d.combine = crate::rcstream::util::combine::In::<WA, SyncMark> {
        real: q.interp_i.stream.data,
        imag: q_out,
        downstream_ready: i.downstream_ready,
    };

    let mut o = Out::<WA> {
        stream: RCStream::<Iq<WA>, SyncMark> {
            data: q.combine.stream.data,
            // The upstream-facing request: the conjunction of the two
            // arms', which the split already forms.
            ready: q.split.ready,
        },
        starved: q.interp_i.starved || q.interp_q.starved || q.combine.starved,
        overrun: q.interp_i.overrun || q.interp_q.overrun,
        // From the recombiner: the two arms presented markers that
        // disagreed. See `Out::frame_mismatch`.
        frame_mismatch: q.combine.frame_mismatch,
        saturated: q.interp_i.saturated || q.interp_q.saturated,
    };

    if cr.reset.any() {
        o.stream = RCStream::<Iq<WA>, SyncMark> {
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

    const W: usize = 8;
    const WA: usize = 11;
    const S: usize = 2;
    const RMAX: usize = 8;
    const M: usize = 1;
    const CW: usize = 4;
    const RATE: usize = 4;
    type Core = CicInterpolate<W, WA, S, RMAX, M, CW>;
    type Uut = EnvelopeUpsampler<W, WA, CW, Core>;

    fn env(re: i128, im: i128, mark: bool) -> Option<Item<Iq<W>, SyncMark>> {
        Some(Item::<Iq<W>, SyncMark> {
            data: Iq::<W> {
                re: signed::<W>(re),
                im: signed::<W>(im),
            },
            frame: SyncMark { sync: mark },
        })
    }

    fn stimulus(cycles: usize, re: i128, im: i128, mark_at: Option<usize>) -> Vec<In<W, CW>> {
        (0..cycles)
            .map(|n| In::<W, CW> {
                stream: env(re, im, mark_at == Some(n)),
                rate: bits::<CW>(RATE as u128),
                downstream_ready: true,
            })
            .collect()
    }

    /// The complex output samples, `(re, im)`, one per cycle.
    fn run(seq: Vec<In<W, CW>>) -> Vec<(i128, i128)> {
        let uut = Uut::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| match s.output.stream.data {
                Some(it) => (it.data.re.raw(), it.data.im.raw()),
                None => (0, 0),
            })
            .collect()
    }

    #[test]
    fn default_construction() {
        let _ = Uut::default();
    }

    #[test]
    fn the_test_configuration_is_at_the_bound() {
        assert_eq!(interp::accumulator_width(W, S, RMAX, M), WA);
        assert_eq!(interp::rate_width(RMAX), CW);
    }

    /// **The request has the once-per-rate cadence, and only that.**
    ///
    /// The arms' individual `ready` signals are not observable from
    /// outside this widget, so this checks the conjunction `IqSplit`
    /// forms. Two other things carry the "both arms agree" claim, and
    /// they carry it better:
    ///
    /// - The type system. Both arms are the same generic `C` at the same
    ///   configuration, so an asymmetric pair is unrepresentable.
    /// - `each_arm_matches_a_lone_interpolator`, which shows each arm
    ///   producing exactly what a single interpolator produces from the
    ///   same input — which it could not do if the two were on different
    ///   grids.
    ///
    /// Stated rather than left implied, because a test named for the
    /// stronger claim while checking the weaker one is worse than no
    /// test.
    #[test]
    fn the_request_has_the_once_per_rate_cadence() {
        let uut = Uut::default();
        let seq = stimulus(6 * RATE, 20, -13, None);
        let readies: Vec<bool> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.stream.ready)
            .collect();
        assert!(!readies[0], "reset holds it low");
        for (n, r) in readies[1..].iter().enumerate() {
            assert_eq!(*r, n % RATE == 0, "cycle {n}");
        }
    }

    /// **The arms do not cross.**
    ///
    /// A real-only envelope must produce a real-only output, and a
    /// quadrature-only one a quadrature-only output. Cheap, and it is
    /// the test that catches a swapped `IqSplit`/`IqCombine` wiring —
    /// which would otherwise look entirely plausible on a magnitude
    /// plot and put the transmission 90 degrees out.
    #[test]
    fn the_arms_do_not_cross() {
        let only_re = run(stimulus(6 * RATE, 30, 0, None));
        assert!(
            only_re.iter().all(|(_, im)| *im == 0),
            "a real envelope must give a real output"
        );
        assert!(only_re.iter().any(|(re, _)| *re != 0), "and a non-zero one");

        let only_im = run(stimulus(6 * RATE, 0, 30, None));
        assert!(
            only_im.iter().all(|(re, _)| *re == 0),
            "a quadrature envelope must give a quadrature output"
        );
        assert!(only_im.iter().any(|(_, im)| *im != 0), "and a non-zero one");
    }

    /// A constant envelope settles at the interpolator's gain, on both
    /// arms, exactly.
    #[test]
    fn a_constant_envelope_settles_at_the_gain() {
        let (re, im) = (25i128, -17i128);
        let got = run(stimulus(8 * RATE, re, im, None));
        let (num, den) = interp::dc_gain_ratio(S, RATE, M);
        let g = num as i128 / den as i128;
        let settled = got[got.len() - 1];
        assert_eq!(settled, (re * g, im * g));
    }

    /// Each arm computes what a lone [`interp_stream::StreamInterpolator`]
    /// computes.
    ///
    /// The cross-check that this widget is split, filter, join and
    /// nothing else.
    #[test]
    fn each_arm_matches_a_lone_interpolator() {
        let (re, im) = (19i128, -23i128);
        let got = run(stimulus(6 * RATE, re, im, None));

        let lone = |v: i128| -> Vec<i128> {
            let uut = interp_stream::StreamInterpolator::<W, WA, CW, Core>::default();
            let seq: Vec<interp_stream::In<W, CW>> = (0..6 * RATE)
                .map(|_| interp_stream::In::<W, CW> {
                    stream: Some(Item::<Real<W>, SyncMark> {
                        data: Real::<W> { v: signed::<W>(v) },
                        frame: SyncMark { sync: false },
                    }),
                    rate: bits::<CW>(RATE as u128),
                    downstream_ready: true,
                })
                .collect();
            uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
                .synchronous_sample()
                .map(|s| match s.output.stream.data {
                    Some(it) => it.data.v.raw(),
                    None => 0,
                })
                .collect()
        };

        assert_eq!(
            got.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
            lone(re),
            "in-phase arm"
        );
        assert_eq!(
            got.iter().map(|(_, q)| *q).collect::<Vec<_>>(),
            lone(im),
            "quadrature arm"
        );
    }

    /// A mark restarts both arms, so the burst after it matches the same
    /// burst from reset.
    #[test]
    fn a_mark_restarts_both_arms() {
        let clean = run(stimulus(6 * RATE, 22, -9, None));
        let mut seq = stimulus(4 * RATE, -70, 60, None);
        seq.extend(stimulus(6 * RATE, 22, -9, Some(0)));
        let got = run(seq);
        let start = 1 + 4 * RATE;
        assert_eq!(&got[start + 1..], &clean[2..]);
    }

    /// The two arms never disagree about framing, which is what
    /// `frame_mismatch` exists to detect.
    #[test]
    fn the_arms_never_disagree_about_framing() {
        let uut = Uut::default();
        let mut seq = stimulus(3 * RATE, 15, -15, None);
        seq.extend(stimulus(5 * RATE, 15, -15, Some(0)));
        assert!(
            uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
                .synchronous_sample()
                .all(|s| !s.output.frame_mismatch),
            "both arms are fed from one split; a mismatch means they drifted"
        );
    }

    // ---- Tier 3 / 4 / 5 --------------------------------------------

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [22:0] i, output wire [28:0] o);
               wire [105:0] od;
               wire [76:0] d;
               wire [83:0] q;
               assign o = od[28:0];
               top_split c0(.clock_reset(clock_reset), .i(d[19:0]), .o(q[22:0]));
               top_interp_i c1(.clock_reset(clock_reset), .i(d[34:20]), .o(q[39:23]));
               top_interp_q c2(.clock_reset(clock_reset), .i(d[49:35]), .o(q[56:40]));
               top_combine c3(.clock_reset(clock_reset), .i(d[76:50]), .o(q[83:57]));
               assign d = od[105:29];
               assign od = kernel_envelope_upsampler_kernel(clock_reset, i, q);
               function [105:0] kernel_envelope_upsampler_kernel(input reg [1:0] arg_0, input reg [22:0] arg_1, input reg [83:0] arg_2);
                     reg [17:0] r0;
                     reg [22:0] r1;
                     reg [16:0] r2;
                     reg [83:0] r3;
                     reg [13:0] r4;
                     reg [0:0] r5;
                     reg [16:0] r6;
                     reg [13:0] r7;
                     reg [0:0] r8;
                     reg [19:0] r9;
                     reg [19:0] r10;
                     reg [19:0] r11;
                     // d
                     reg [76:0] r12;
                     reg [22:0] r13;
                     reg [10:0] r14;
                     reg [9:0] r15;
                     reg [3:0] r16;
                     reg [0:0] r17;
                     reg [14:0] r18;
                     reg [14:0] r19;
                     reg [14:0] r20;
                     // d
                     reg [76:0] r21;
                     reg [22:0] r22;
                     reg [10:0] r23;
                     reg [9:0] r24;
                     reg [0:0] r25;
                     reg [8:0] r26;
                     reg [7:0] r27;
                     reg [7:0] r28;
                     reg [0:0] r29;
                     reg [8:0] r30;
                     reg [8:0] r31;
                     reg [9:0] r32;
                     reg [8:0] r33;
                     // q_in
                     reg [9:0] r34;
                     reg [3:0] r35;
                     reg [0:0] r36;
                     reg [14:0] r37;
                     reg [14:0] r38;
                     reg [14:0] r39;
                     // d
                     reg [76:0] r40;
                     reg [16:0] r41;
                     reg [13:0] r42;
                     reg [12:0] r43;
                     reg [0:0] r44;
                     reg [11:0] r45;
                     reg [10:0] r46;
                     reg [10:0] r47;
                     reg [0:0] r48;
                     reg [11:0] r49;
                     reg [11:0] r50;
                     reg [12:0] r51;
                     reg [11:0] r52;
                     // q_out
                     reg [12:0] r53;
                     reg [16:0] r54;
                     reg [13:0] r55;
                     reg [12:0] r56;
                     reg [0:0] r57;
                     reg [26:0] r58;
                     reg [26:0] r59;
                     reg [26:0] r60;
                     // d
                     reg [76:0] r61;
                     reg [26:0] r62;
                     reg [24:0] r63;
                     reg [23:0] r64;
                     reg [22:0] r65;
                     reg [0:0] r66;
                     reg [24:0] r67;
                     reg [24:0] r68;
                     reg [16:0] r69;
                     reg [0:0] r70;
                     reg [16:0] r71;
                     reg [0:0] r72;
                     reg [0:0] r73;
                     reg [26:0] r74;
                     reg [0:0] r75;
                     reg [0:0] r76;
                     reg [16:0] r77;
                     reg [0:0] r78;
                     reg [16:0] r79;
                     reg [0:0] r80;
                     reg [0:0] r81;
                     reg [26:0] r82;
                     reg [0:0] r83;
                     reg [16:0] r84;
                     reg [0:0] r85;
                     reg [16:0] r86;
                     reg [0:0] r87;
                     reg [0:0] r88;
                     reg [28:0] r89;
                     reg [28:0] r90;
                     reg [28:0] r91;
                     reg [28:0] r92;
                     reg [28:0] r93;
                     reg [0:0] r94;
                     reg [1:0] r95;
                     reg [0:0] r96;
                     // o
                     reg [28:0] r97;
                     // o
                     reg [28:0] r98;
                     // o
                     reg [28:0] r99;
                     // o
                     reg [28:0] r100;
                     // o
                     reg [28:0] r101;
                     // o
                     reg [28:0] r102;
                     reg [105:0] r103;
                     localparam l0 = 20'b00000000000000000000;
                     localparam l1 = 77'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l2 = 15'b000000000000000;
                     localparam l3 = 8'b00000000;
                     localparam l4 = 9'b000000000;
                     localparam l5 = 1'b1;
                     localparam l6 = 1'b1;
                     localparam l7 = 10'b0000000000;
                     localparam l8 = 15'b000000000000000;
                     localparam l9 = 11'b00000000000;
                     localparam l10 = 12'b000000000000;
                     localparam l11 = 1'b1;
                     localparam l12 = 1'b1;
                     localparam l13 = 13'b0000000000000;
                     localparam l14 = 27'b000000000000000000000000000;
                     localparam l15 = 25'b0000000000000000000000000;
                     localparam l16 = 29'b00000000000000000000000000000;
                     localparam l17 = 25'b0000000000000000000000000;
                     localparam l18 = 1'b0;
                     localparam l19 = 1'b0;
                     localparam l20 = 1'b0;
                     localparam l21 = 1'b0;
                     begin
                        r95 = arg_0;
                        r1 = arg_1;
                        r3 = arg_2;
                        r0 = r1[17:0];
                        r2 = r3[39:23];
                        r4 = r2[13:0];
                        r5 = r4[13:13];
                        r6 = r3[56:40];
                        r7 = r6[13:0];
                        r8 = r7[13:13];
                        r9 = l0;
                        r9[17:0] = r0;
                        r10 = r9;
                        r10[18:18] = r5;
                        r11 = r10;
                        r11[19:19] = r8;
                        r12 = l1;
                        r12[19:0] = r11;
                        r13 = r3[22:0];
                        r14 = r13[10:0];
                        r15 = r14[9:0];
                        r16 = r1[21:18];
                        r17 = r1[22:22];
                        r18 = l2;
                        r18[9:0] = r15;
                        r19 = r18;
                        r19[13:10] = r16;
                        r20 = r19;
                        r20[14:14] = r17;
                        r21 = r12;
                        r21[34:20] = r20;
                        r22 = r3[22:0];
                        r23 = r22[21:11];
                        r24 = r23[9:0];
                        r25 = r24[9:9];
                        r26 = r24[8:0];
                        r27 = r26[7:0];
                        r28 = l3;
                        r28[7:0] = r27;
                        r29 = r26[8:8];
                        r30 = l4;
                        r30[7:0] = r28;
                        r31 = r30;
                        r31[8:8] = r29;
                        r33 = r31[8:0];
                        r32 = {l5, r33};
                        case (r25)
                           1'b1 : r34 = r32;
                           default : r34 = l7;
                        endcase
                        r35 = r1[21:18];
                        r36 = r1[22:22];
                        r37 = l8;
                        r37[9:0] = r34;
                        r38 = r37;
                        r38[13:10] = r35;
                        r39 = r38;
                        r39[14:14] = r36;
                        r40 = r21;
                        r40[49:35] = r39;
                        r41 = r3[56:40];
                        r42 = r41[13:0];
                        r43 = r42[12:0];
                        r44 = r43[12:12];
                        r45 = r43[11:0];
                        r46 = r45[10:0];
                        r47 = l9;
                        r47[10:0] = r46;
                        r48 = r45[11:11];
                        r49 = l10;
                        r49[10:0] = r47;
                        r50 = r49;
                        r50[11:11] = r48;
                        r52 = r50[11:0];
                        r51 = {l11, r52};
                        case (r44)
                           1'b1 : r53 = r51;
                           default : r53 = l13;
                        endcase
                        r54 = r3[39:23];
                        r55 = r54[13:0];
                        r56 = r55[12:0];
                        r57 = r1[22:22];
                        r58 = l14;
                        r58[12:0] = r56;
                        r59 = r58;
                        r59[25:13] = r53;
                        r60 = r59;
                        r60[26:26] = r57;
                        r61 = r40;
                        r61[76:50] = r60;
                        r62 = r3[83:57];
                        r63 = r62[24:0];
                        r64 = r63[23:0];
                        r65 = r3[22:0];
                        r66 = r65[22:22];
                        r67 = l15;
                        r67[23:0] = r64;
                        r68 = r67;
                        r68[24:24] = r66;
                        r69 = r3[39:23];
                        r70 = r69[14:14];
                        r71 = r3[56:40];
                        r72 = r71[14:14];
                        r73 = r70 | r72;
                        r74 = r3[83:57];
                        r75 = r74[25:25];
                        r76 = r73 | r75;
                        r77 = r3[39:23];
                        r78 = r77[15:15];
                        r79 = r3[56:40];
                        r80 = r79[15:15];
                        r81 = r78 | r80;
                        r82 = r3[83:57];
                        r83 = r82[26:26];
                        r84 = r3[39:23];
                        r85 = r84[16:16];
                        r86 = r3[56:40];
                        r87 = r86[16:16];
                        r88 = r85 | r87;
                        r89 = l16;
                        r89[24:0] = r68;
                        r90 = r89;
                        r90[25:25] = r76;
                        r91 = r90;
                        r91[26:26] = r81;
                        r92 = r91;
                        r92[27:27] = r83;
                        r93 = r92;
                        r93[28:28] = r88;
                        r94 = r95[1:1];
                        r96 = |r94;
                        r97 = r93;
                        r97[24:0] = l17;
                        r98 = r97;
                        r98[25:25] = l18;
                        r99 = r98;
                        r99[26:26] = l19;
                        r100 = r99;
                        r100[27:27] = l20;
                        r101 = r100;
                        r101[28:28] = l21;
                        r102 = r96 ? r101 : r93;
                        r103 = {r61, r102};
                        kernel_envelope_upsampler_kernel = r103;
                     end
               endfunction
            endmodule
            module top_split(input wire [1:0] clock_reset, input wire [19:0] i, output wire [22:0] o);
               wire [22:0] od;
               wire [0:0] q;
               assign o = od[22:0];
               top_split_marker c0(.clock_reset(clock_reset), .o(q[0:0]));
               assign od = kernel_iq_split_kernel(clock_reset, i, q);
               function [22:0] kernel_iq_split_kernel(input reg [1:0] arg_0, input reg [19:0] arg_1, input reg [0:0] arg_2);
                     reg [17:0] r0;
                     reg [19:0] r1;
                     reg [0:0] r2;
                     reg [16:0] r3;
                     reg [15:0] r4;
                     reg signed [7:0] r5;
                     reg [7:0] r6;
                     reg [0:0] r7;
                     reg [8:0] r8;
                     reg [8:0] r9;
                     reg [9:0] r10;
                     reg [8:0] r11;
                     reg [15:0] r12;
                     reg signed [7:0] r13;
                     reg [7:0] r14;
                     reg [0:0] r15;
                     reg [8:0] r16;
                     reg [8:0] r17;
                     reg [9:0] r18;
                     reg [8:0] r19;
                     // imag_data
                     reg [9:0] r20;
                     // real_data
                     reg [9:0] r21;
                     reg [0:0] r22;
                     reg [10:0] r23;
                     reg [10:0] r24;
                     reg [0:0] r25;
                     reg [10:0] r26;
                     reg [10:0] r27;
                     reg [0:0] r28;
                     reg [0:0] r29;
                     reg [0:0] r30;
                     reg [22:0] r31;
                     reg [22:0] r32;
                     reg [22:0] r33;
                     reg [1:0] r34;
                     reg [0:0] r35;
                     localparam l0 = 8'b00000000;
                     localparam l1 = 9'b000000000;
                     localparam l2 = 1'b1;
                     localparam l3 = 8'b00000000;
                     localparam l4 = 9'b000000000;
                     localparam l5 = 1'b1;
                     localparam l6 = 1'b1;
                     localparam l7 = 10'b0000000000;
                     localparam l8 = 10'b0000000000;
                     localparam l9 = 11'b00000000000;
                     localparam l10 = 11'b00000000000;
                     localparam l11 = 23'b00000000000000000000000;
                     begin
                        r34 = arg_0;
                        r1 = arg_1;
                        r35 = arg_2;
                        r0 = r1[17:0];
                        r2 = r0[17:17];
                        r3 = r0[16:0];
                        r4 = r3[15:0];
                        r5 = r4[7:0];
                        r6 = l0;
                        r6[7:0] = r5;
                        r7 = r3[16:16];
                        r8 = l1;
                        r8[7:0] = r6;
                        r9 = r8;
                        r9[8:8] = r7;
                        r11 = r9[8:0];
                        r10 = {l2, r11};
                        r12 = r3[15:0];
                        r13 = r12[15:8];
                        r14 = l3;
                        r14[7:0] = r13;
                        r15 = r3[16:16];
                        r16 = l4;
                        r16[7:0] = r14;
                        r17 = r16;
                        r17[8:8] = r15;
                        r19 = r17[8:0];
                        r18 = {l5, r19};
                        case (r2)
                           1'b1 : r20 = r18;
                           default : r20 = l7;
                        endcase
                        case (r2)
                           1'b1 : r21 = r10;
                           default : r21 = l8;
                        endcase
                        r22 = r1[18:18];
                        r23 = l9;
                        r23[9:0] = r21;
                        r24 = r23;
                        r24[10:10] = r22;
                        r25 = r1[19:19];
                        r26 = l10;
                        r26[9:0] = r20;
                        r27 = r26;
                        r27[10:10] = r25;
                        r28 = r1[18:18];
                        r29 = r1[19:19];
                        r30 = r28 & r29;
                        r31 = l11;
                        r31[10:0] = r24;
                        r32 = r31;
                        r32[21:11] = r27;
                        r33 = r32;
                        r33[22:22] = r30;
                        kernel_iq_split_kernel = r33;
                     end
               endfunction
            endmodule
            module top_split_marker(input wire [1:0] clock_reset, output wire [0:0] o);
               assign o = 1'b0;
            endmodule
            module top_interp_i(input wire [1:0] clock_reset, input wire [14:0] i, output wire [16:0] o);
               wire [32:0] od;
               wire [15:0] d;
               wire [15:0] q;
               assign o = od[16:0];
               top_interp_i_cic c0(.clock_reset(clock_reset), .i(d[14:0]), .o(q[14:0]));
               top_interp_i_marked c1(.clock_reset(clock_reset), .i(d[15:15]), .o(q[15:15]));
               assign d = od[32:17];
               assign od = kernel_stream_interpolator_kernel(clock_reset, i, q);
               function [32:0] kernel_stream_interpolator_kernel(input reg [1:0] arg_0, input reg [14:0] arg_1, input reg [15:0] arg_2);
                     reg [9:0] r0;
                     reg [14:0] r1;
                     reg [0:0] r2;
                     reg [8:0] r3;
                     reg [7:0] r4;
                     reg [8:0] r5;
                     reg [7:0] r6;
                     reg [0:0] r7;
                     // marked_now
                     reg [0:0] r8;
                     // sample
                     reg [8:0] r9;
                     reg [14:0] r10;
                     reg [15:0] r11;
                     reg [0:0] r12;
                     reg [0:0] r13;
                     reg [3:0] r14;
                     reg [0:0] r15;
                     reg [14:0] r16;
                     reg [14:0] r17;
                     reg [14:0] r18;
                     reg [14:0] r19;
                     // d
                     reg [15:0] r20;
                     // d
                     reg [15:0] r21;
                     reg [14:0] r22;
                     reg signed [10:0] r23;
                     reg [10:0] r24;
                     reg [0:0] r25;
                     reg [0:0] r26;
                     reg [11:0] r27;
                     reg [11:0] r28;
                     reg [12:0] r29;
                     reg [11:0] r30;
                     reg [14:0] r31;
                     reg [0:0] r32;
                     reg [13:0] r33;
                     reg [13:0] r34;
                     reg [14:0] r35;
                     reg [0:0] r36;
                     reg [14:0] r37;
                     reg [0:0] r38;
                     reg [14:0] r39;
                     reg [0:0] r40;
                     reg [16:0] r41;
                     reg [16:0] r42;
                     reg [16:0] r43;
                     reg [16:0] r44;
                     reg [0:0] r45;
                     reg [1:0] r46;
                     reg [0:0] r47;
                     reg [3:0] r48;
                     reg [14:0] r49;
                     reg [14:0] r50;
                     reg [14:0] r51;
                     reg [14:0] r52;
                     // d
                     reg [15:0] r53;
                     // d
                     reg [15:0] r54;
                     // o
                     reg [16:0] r55;
                     // o
                     reg [16:0] r56;
                     // o
                     reg [16:0] r57;
                     // o
                     reg [16:0] r58;
                     // d
                     reg [15:0] r59;
                     // o
                     reg [16:0] r60;
                     reg [32:0] r61;
                     localparam l0 = 1'b1;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b0;
                     localparam l3 = 9'b000000000;
                     localparam l4 = 15'b000000000000000;
                     localparam l5 = 16'bXXXXXXXXXXXXXXXX;
                     localparam l6 = 11'b00000000000;
                     localparam l7 = 1'b0;
                     localparam l8 = 12'b000000000000;
                     localparam l9 = 1'b1;
                     localparam l10 = 14'b00000000000000;
                     localparam l11 = 17'b00000000000000000;
                     localparam l12 = 1'b0;
                     localparam l13 = 1'b0;
                     localparam l14 = 1'b0;
                     localparam l15 = 14'b00000000000000;
                     localparam l16 = 1'b0;
                     localparam l17 = 1'b0;
                     localparam l18 = 1'b0;
                     localparam l19 = 15'b000000000000000;
                     begin
                        r46 = arg_0;
                        r1 = arg_1;
                        r11 = arg_2;
                        r0 = r1[9:0];
                        r2 = r0[9:9];
                        r3 = r0[8:0];
                        r4 = r3[7:0];
                        r6 = r4[7:0];
                        r5 = {l0, r6};
                        r7 = r3[8:8];
                        case (r2)
                           1'b1 : r8 = r7;
                           default : r8 = l2;
                        endcase
                        case (r2)
                           1'b1 : r9 = r5;
                           default : r9 = l3;
                        endcase
                        r10 = r11[14:0];
                        r12 = r10[11:11];
                        r13 = r8 & r12;
                        r14 = r1[13:10];
                        r15 = r1[14:14];
                        r16 = l4;
                        r16[8:0] = r9;
                        r17 = r16;
                        r17[12:9] = r14;
                        r18 = r17;
                        r18[13:13] = r13;
                        r19 = r18;
                        r19[14:14] = r15;
                        r20 = l5;
                        r20[14:0] = r19;
                        r21 = r20;
                        r21[15:15] = r13;
                        r22 = r11[14:0];
                        r23 = r22[10:0];
                        r24 = l6;
                        r24[10:0] = r23;
                        r25 = r11[15:15];
                        r26 = l7;
                        r26[0:0] = r25;
                        r27 = l8;
                        r27[10:0] = r24;
                        r28 = r27;
                        r28[11:11] = r26;
                        r30 = r28[11:0];
                        r29 = {l9, r30};
                        r31 = r11[14:0];
                        r32 = r31[11:11];
                        r33 = l10;
                        r33[12:0] = r29;
                        r34 = r33;
                        r34[13:13] = r32;
                        r35 = r11[14:0];
                        r36 = r35[12:12];
                        r37 = r11[14:0];
                        r38 = r37[13:13];
                        r39 = r11[14:0];
                        r40 = r39[14:14];
                        r41 = l11;
                        r41[13:0] = r34;
                        r42 = r41;
                        r42[14:14] = r36;
                        r43 = r42;
                        r43[15:15] = r38;
                        r44 = r43;
                        r44[16:16] = r40;
                        r45 = r46[1:1];
                        r47 = |r45;
                        r48 = r1[13:10];
                        r49 = l19;
                        r50 = r49;
                        r50[12:9] = r48;
                        r51 = r50;
                        r51[13:13] = l12;
                        r52 = r51;
                        r52[14:14] = l13;
                        r53 = r21;
                        r53[14:0] = r52;
                        r54 = r53;
                        r54[15:15] = l14;
                        r55 = r44;
                        r55[13:0] = l15;
                        r56 = r55;
                        r56[14:14] = l16;
                        r57 = r56;
                        r57[15:15] = l17;
                        r58 = r57;
                        r58[16:16] = l18;
                        r59 = r47 ? r54 : r21;
                        r60 = r47 ? r58 : r44;
                        r61 = {r59, r60};
                        kernel_stream_interpolator_kernel = r61;
                     end
               endfunction
            endmodule
            module top_interp_i_cic(input wire [1:0] clock_reset, input wire [14:0] i, output wire [14:0] o);
               wire [96:0] od;
               wire [81:0] d;
               wire [81:0] q;
               assign o = od[14:0];
               top_interp_i_cic_combs c0(.clock_reset(clock_reset), .i(d[21:0]), .o(q[21:0]));
               top_interp_i_cic_comb_out c1(.clock_reset(clock_reset), .i(d[43:22]), .o(q[43:22]));
               top_interp_i_cic_integrators c2(.clock_reset(clock_reset), .i(d[65:44]), .o(q[65:44]));
               top_interp_i_cic_phase c3(.clock_reset(clock_reset), .i(d[69:66]), .o(q[69:66]));
               top_interp_i_cic_out c4(.clock_reset(clock_reset), .i(d[80:70]), .o(q[80:70]));
               top_interp_i_cic_starved c5(.clock_reset(clock_reset), .i(d[81:81]), .o(q[81:81]));
               assign d = od[96:15];
               assign od = kernel_cic_interpolate_kernel(clock_reset, i, q);
               function [96:0] kernel_cic_interpolate_kernel(input reg [1:0] arg_0, input reg [14:0] arg_1, input reg [81:0] arg_2);
                     reg [21:0] r0;
                     reg [81:0] r1;
                     // d
                     reg [81:0] r2;
                     reg [21:0] r3;
                     // d
                     reg [81:0] r4;
                     reg [3:0] r5;
                     // d
                     reg [81:0] r6;
                     reg [3:0] r7;
                     reg [0:0] r8;
                     reg [0:0] r9;
                     reg [14:0] r10;
                     reg [0:0] r11;
                     reg [0:0] r12;
                     reg [3:0] r13;
                     reg [3:0] r14;
                     reg [3:0] r15;
                     reg [3:0] r16;
                     reg [0:0] r17;
                     reg [3:0] r18;
                     reg [3:0] r19;
                     // d
                     reg [81:0] r20;
                     reg [8:0] r21;
                     reg [0:0] r22;
                     reg [7:0] r23;
                     reg [7:0] r24;
                     reg [7:0] r25;
                     reg [0:0] r26;
                     reg [10:0] r27;
                     reg [10:0] r28;
                     reg [10:0] r29;
                     reg signed [10:0] r30;
                     // starved_now
                     reg [0:0] r31;
                     // x
                     reg signed [10:0] r32;
                     // starved_now
                     reg [0:0] r33;
                     // x
                     reg signed [10:0] r34;
                     // d
                     reg [81:0] r35;
                     reg [0:0] r36;
                     reg [21:0] r37;
                     reg [21:0] r38;
                     reg [0:0] r39;
                     reg [21:0] r40;
                     reg [21:0] r41;
                     reg [10:0] r42;
                     reg signed [10:0] r43;
                     // outs
                     reg [21:0] r44;
                     reg [10:0] r45;
                     // line
                     reg [10:0] r46;
                     // cs
                     reg [21:0] r47;
                     reg signed [10:0] r48;
                     reg [10:0] r49;
                     reg signed [10:0] r50;
                     // outs
                     reg [21:0] r51;
                     reg [10:0] r52;
                     // line
                     reg [10:0] r53;
                     // cs
                     reg [21:0] r54;
                     // d
                     reg [81:0] r55;
                     // d
                     reg [81:0] r56;
                     reg signed [10:0] r57;
                     // d
                     reg [81:0] r58;
                     // feed
                     reg signed [10:0] r59;
                     reg [21:0] r60;
                     reg [0:0] r61;
                     reg [21:0] r62;
                     reg signed [10:0] r63;
                     reg signed [10:0] r64;
                     reg signed [10:0] r65;
                     // ints
                     reg [21:0] r66;
                     reg [0:0] r67;
                     reg [21:0] r68;
                     reg signed [10:0] r69;
                     reg signed [10:0] r70;
                     reg [0:0] r71;
                     reg [21:0] r72;
                     reg signed [10:0] r73;
                     reg signed [10:0] r74;
                     reg signed [10:0] r75;
                     // ints
                     reg [21:0] r76;
                     // d
                     reg [81:0] r77;
                     reg signed [10:0] r78;
                     // d
                     reg [81:0] r79;
                     reg signed [10:0] r80;
                     reg [0:0] r81;
                     reg [0:0] r82;
                     reg [0:0] r83;
                     reg [14:0] r84;
                     reg [14:0] r85;
                     reg [14:0] r86;
                     reg [14:0] r87;
                     reg [14:0] r88;
                     reg [0:0] r89;
                     reg [1:0] r90;
                     reg [0:0] r91;
                     // d
                     reg [81:0] r92;
                     // d
                     reg [81:0] r93;
                     // d
                     reg [81:0] r94;
                     // d
                     reg [81:0] r95;
                     // d
                     reg [81:0] r96;
                     // d
                     reg [81:0] r97;
                     // o
                     reg [14:0] r98;
                     // o
                     reg [14:0] r99;
                     // o
                     reg [14:0] r100;
                     // o
                     reg [14:0] r101;
                     // o
                     reg [14:0] r102;
                     // d
                     reg [81:0] r103;
                     // o
                     reg [14:0] r104;
                     reg [96:0] r105;
                     localparam l0 = 82'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l1 = 4'b0000;
                     localparam l2 = 4'b0000;
                     localparam l3 = 4'b0001;
                     localparam l4 = 4'b0001;
                     localparam l5 = 4'b0000;
                     localparam l6 = 8'b10000000;
                     localparam l7 = 11'b11100000000;
                     localparam l8 = 11'b00000000000;
                     localparam l9 = 1'b1;
                     localparam l10 = 1'b0;
                     localparam l11 = 1'b0;
                     localparam l12 = 1'b1;
                     localparam l13 = 11'sb00000000000;
                     localparam l14 = 22'b0000000000000000000000;
                     localparam l15 = 22'b0000000000000000000000;
                     localparam l16 = 11'sb00000000000;
                     localparam l17 = 11'sb00000000000;
                     localparam l18 = 11'sb00000000000;
                     localparam l19 = 11'sb00000000000;
                     localparam l20 = 15'b000000000000000;
                     localparam l21 = 1'b0;
                     localparam l22 = 22'b0000000000000000000000;
                     localparam l23 = 22'b0000000000000000000000;
                     localparam l24 = 22'b0000000000000000000000;
                     localparam l25 = 4'b0000;
                     localparam l26 = 11'sb00000000000;
                     localparam l27 = 1'b0;
                     localparam l28 = 11'sb00000000000;
                     localparam l29 = 1'b0;
                     localparam l30 = 1'b0;
                     localparam l31 = 1'b0;
                     localparam l32 = 1'b0;
                     begin
                        r90 = arg_0;
                        r10 = arg_1;
                        r1 = arg_2;
                        r0 = r1[21:0];
                        r2 = l0;
                        r2[21:0] = r0;
                        r3 = r1[43:22];
                        r4 = r2;
                        r4[43:22] = r3;
                        r5 = r1[69:66];
                        r6 = r4;
                        r6[69:66] = r5;
                        r7 = r1[69:66];
                        r8 = r7 == l1;
                        r9 = r10[13:13];
                        r11 = r8 | r9;
                        r12 = r10[13:13];
                        r13 = r1[69:66];
                        r14 = r12 ? l2 : r13;
                        r15 = r14 + l3;
                        r16 = r10[12:9];
                        r17 = r15 >= r16;
                        r18 = r14 + l4;
                        r19 = r17 ? l5 : r18;
                        r20 = r6;
                        r20[69:66] = r19;
                        r21 = r10[8:0];
                        r22 = r21[8:8];
                        r23 = r21[7:0];
                        r24 = $unsigned(r23);
                        r25 = r24 & l6;
                        r26 = |r25;
                        r27 = {{3{1'b0}}, r24};
                        r28 = r26 ? l7 : l8;
                        r29 = r27 + r28;
                        r30 = $signed(r29);
                        case (r22)
                           1'b1 : r31 = l10;
                           1'b0 : r31 = l12;
                        endcase
                        case (r22)
                           1'b1 : r32 = r30;
                           1'b0 : r32 = l13;
                        endcase
                        r33 = r11 ? r31 : l10;
                        r34 = r11 ? r32 : l13;
                        r35 = r20;
                        r35[81:81] = r33;
                        r36 = r10[13:13];
                        r37 = r1[21:0];
                        r38 = r36 ? l14 : r37;
                        r39 = r10[13:13];
                        r40 = r1[43:22];
                        r41 = r39 ? l15 : r40;
                        r42 = r38[10:0];
                        r43 = r34 - r42;
                        r44 = r41;
                        r44[10:0] = r43;
                        r45 = r38[10:0];
                        r46 = r45;
                        r46[10:0] = r34;
                        r47 = r38;
                        r47[10:0] = r46;
                        r48 = r41[10:0];
                        r49 = r38[21:11];
                        r50 = r48 - r49;
                        r51 = r44;
                        r51[21:11] = r50;
                        r52 = r38[21:11];
                        r53 = r52;
                        r53[10:0] = r48;
                        r54 = r47;
                        r54[21:11] = r53;
                        r55 = r35;
                        r55[21:0] = r54;
                        r56 = r55;
                        r56[43:22] = r51;
                        r57 = r41[21:11];
                        r58 = r11 ? r56 : r35;
                        r59 = r11 ? r57 : l16;
                        r60 = r1[65:44];
                        r61 = r10[13:13];
                        r62 = r1[65:44];
                        r63 = r62[10:0];
                        r64 = r61 ? l17 : r63;
                        r65 = r64 + r59;
                        r66 = r60;
                        r66[10:0] = r65;
                        r67 = r10[13:13];
                        r68 = r1[65:44];
                        r69 = r68[21:11];
                        r70 = r67 ? l18 : r69;
                        r71 = r10[13:13];
                        r72 = r1[65:44];
                        r73 = r72[10:0];
                        r74 = r71 ? l19 : r73;
                        r75 = r70 + r74;
                        r76 = r66;
                        r76[21:11] = r75;
                        r77 = r58;
                        r77[65:44] = r76;
                        r78 = r76[21:11];
                        r79 = r77;
                        r79[80:70] = r78;
                        r80 = r1[80:70];
                        r81 = r1[81:81];
                        r82 = r10[14:14];
                        r83 = ~r82;
                        r84 = l20;
                        r84[10:0] = r80;
                        r85 = r84;
                        r85[11:11] = r8;
                        r86 = r85;
                        r86[12:12] = r81;
                        r87 = r86;
                        r87[13:13] = r83;
                        r88 = r87;
                        r88[14:14] = l21;
                        r89 = r90[1:1];
                        r91 = |r89;
                        r92 = r79;
                        r92[21:0] = l22;
                        r93 = r92;
                        r93[43:22] = l23;
                        r94 = r93;
                        r94[65:44] = l24;
                        r95 = r94;
                        r95[69:66] = l25;
                        r96 = r95;
                        r96[80:70] = l26;
                        r97 = r96;
                        r97[81:81] = l27;
                        r98 = r88;
                        r98[10:0] = l28;
                        r99 = r98;
                        r99[11:11] = l29;
                        r100 = r99;
                        r100[12:12] = l30;
                        r101 = r100;
                        r101[13:13] = l31;
                        r102 = r101;
                        r102[14:14] = l32;
                        r103 = r91 ? r97 : r79;
                        r104 = r91 ? r102 : r88;
                        r105 = {r103, r104};
                        kernel_cic_interpolate_kernel = r105;
                     end
               endfunction
            endmodule
            module top_interp_i_cic_combs(input wire [1:0] clock_reset, input wire [21:0] i, output reg [21:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 22'b0000000000000000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 22'b0000000000000000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_interp_i_cic_comb_out(input wire [1:0] clock_reset, input wire [21:0] i, output reg [21:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 22'b0000000000000000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 22'b0000000000000000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_interp_i_cic_integrators(input wire [1:0] clock_reset, input wire [21:0] i, output reg [21:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 22'b0000000000000000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 22'b0000000000000000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_interp_i_cic_phase(input wire [1:0] clock_reset, input wire [3:0] i, output reg [3:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 4'b0000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 4'b0000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_interp_i_cic_out(input wire [1:0] clock_reset, input wire [10:0] i, output reg [10:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 11'sb00000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 11'sb00000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_interp_i_cic_starved(input wire [1:0] clock_reset, input wire [0:0] i, output reg [0:0] o);
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
            module top_interp_i_marked(input wire [1:0] clock_reset, input wire [0:0] i, output reg [0:0] o);
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
            module top_interp_q(input wire [1:0] clock_reset, input wire [14:0] i, output wire [16:0] o);
               wire [32:0] od;
               wire [15:0] d;
               wire [15:0] q;
               assign o = od[16:0];
               top_interp_q_cic c0(.clock_reset(clock_reset), .i(d[14:0]), .o(q[14:0]));
               top_interp_q_marked c1(.clock_reset(clock_reset), .i(d[15:15]), .o(q[15:15]));
               assign d = od[32:17];
               assign od = kernel_stream_interpolator_kernel(clock_reset, i, q);
               function [32:0] kernel_stream_interpolator_kernel(input reg [1:0] arg_0, input reg [14:0] arg_1, input reg [15:0] arg_2);
                     reg [9:0] r0;
                     reg [14:0] r1;
                     reg [0:0] r2;
                     reg [8:0] r3;
                     reg [7:0] r4;
                     reg [8:0] r5;
                     reg [7:0] r6;
                     reg [0:0] r7;
                     // marked_now
                     reg [0:0] r8;
                     // sample
                     reg [8:0] r9;
                     reg [14:0] r10;
                     reg [15:0] r11;
                     reg [0:0] r12;
                     reg [0:0] r13;
                     reg [3:0] r14;
                     reg [0:0] r15;
                     reg [14:0] r16;
                     reg [14:0] r17;
                     reg [14:0] r18;
                     reg [14:0] r19;
                     // d
                     reg [15:0] r20;
                     // d
                     reg [15:0] r21;
                     reg [14:0] r22;
                     reg signed [10:0] r23;
                     reg [10:0] r24;
                     reg [0:0] r25;
                     reg [0:0] r26;
                     reg [11:0] r27;
                     reg [11:0] r28;
                     reg [12:0] r29;
                     reg [11:0] r30;
                     reg [14:0] r31;
                     reg [0:0] r32;
                     reg [13:0] r33;
                     reg [13:0] r34;
                     reg [14:0] r35;
                     reg [0:0] r36;
                     reg [14:0] r37;
                     reg [0:0] r38;
                     reg [14:0] r39;
                     reg [0:0] r40;
                     reg [16:0] r41;
                     reg [16:0] r42;
                     reg [16:0] r43;
                     reg [16:0] r44;
                     reg [0:0] r45;
                     reg [1:0] r46;
                     reg [0:0] r47;
                     reg [3:0] r48;
                     reg [14:0] r49;
                     reg [14:0] r50;
                     reg [14:0] r51;
                     reg [14:0] r52;
                     // d
                     reg [15:0] r53;
                     // d
                     reg [15:0] r54;
                     // o
                     reg [16:0] r55;
                     // o
                     reg [16:0] r56;
                     // o
                     reg [16:0] r57;
                     // o
                     reg [16:0] r58;
                     // d
                     reg [15:0] r59;
                     // o
                     reg [16:0] r60;
                     reg [32:0] r61;
                     localparam l0 = 1'b1;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b0;
                     localparam l3 = 9'b000000000;
                     localparam l4 = 15'b000000000000000;
                     localparam l5 = 16'bXXXXXXXXXXXXXXXX;
                     localparam l6 = 11'b00000000000;
                     localparam l7 = 1'b0;
                     localparam l8 = 12'b000000000000;
                     localparam l9 = 1'b1;
                     localparam l10 = 14'b00000000000000;
                     localparam l11 = 17'b00000000000000000;
                     localparam l12 = 1'b0;
                     localparam l13 = 1'b0;
                     localparam l14 = 1'b0;
                     localparam l15 = 14'b00000000000000;
                     localparam l16 = 1'b0;
                     localparam l17 = 1'b0;
                     localparam l18 = 1'b0;
                     localparam l19 = 15'b000000000000000;
                     begin
                        r46 = arg_0;
                        r1 = arg_1;
                        r11 = arg_2;
                        r0 = r1[9:0];
                        r2 = r0[9:9];
                        r3 = r0[8:0];
                        r4 = r3[7:0];
                        r6 = r4[7:0];
                        r5 = {l0, r6};
                        r7 = r3[8:8];
                        case (r2)
                           1'b1 : r8 = r7;
                           default : r8 = l2;
                        endcase
                        case (r2)
                           1'b1 : r9 = r5;
                           default : r9 = l3;
                        endcase
                        r10 = r11[14:0];
                        r12 = r10[11:11];
                        r13 = r8 & r12;
                        r14 = r1[13:10];
                        r15 = r1[14:14];
                        r16 = l4;
                        r16[8:0] = r9;
                        r17 = r16;
                        r17[12:9] = r14;
                        r18 = r17;
                        r18[13:13] = r13;
                        r19 = r18;
                        r19[14:14] = r15;
                        r20 = l5;
                        r20[14:0] = r19;
                        r21 = r20;
                        r21[15:15] = r13;
                        r22 = r11[14:0];
                        r23 = r22[10:0];
                        r24 = l6;
                        r24[10:0] = r23;
                        r25 = r11[15:15];
                        r26 = l7;
                        r26[0:0] = r25;
                        r27 = l8;
                        r27[10:0] = r24;
                        r28 = r27;
                        r28[11:11] = r26;
                        r30 = r28[11:0];
                        r29 = {l9, r30};
                        r31 = r11[14:0];
                        r32 = r31[11:11];
                        r33 = l10;
                        r33[12:0] = r29;
                        r34 = r33;
                        r34[13:13] = r32;
                        r35 = r11[14:0];
                        r36 = r35[12:12];
                        r37 = r11[14:0];
                        r38 = r37[13:13];
                        r39 = r11[14:0];
                        r40 = r39[14:14];
                        r41 = l11;
                        r41[13:0] = r34;
                        r42 = r41;
                        r42[14:14] = r36;
                        r43 = r42;
                        r43[15:15] = r38;
                        r44 = r43;
                        r44[16:16] = r40;
                        r45 = r46[1:1];
                        r47 = |r45;
                        r48 = r1[13:10];
                        r49 = l19;
                        r50 = r49;
                        r50[12:9] = r48;
                        r51 = r50;
                        r51[13:13] = l12;
                        r52 = r51;
                        r52[14:14] = l13;
                        r53 = r21;
                        r53[14:0] = r52;
                        r54 = r53;
                        r54[15:15] = l14;
                        r55 = r44;
                        r55[13:0] = l15;
                        r56 = r55;
                        r56[14:14] = l16;
                        r57 = r56;
                        r57[15:15] = l17;
                        r58 = r57;
                        r58[16:16] = l18;
                        r59 = r47 ? r54 : r21;
                        r60 = r47 ? r58 : r44;
                        r61 = {r59, r60};
                        kernel_stream_interpolator_kernel = r61;
                     end
               endfunction
            endmodule
            module top_interp_q_cic(input wire [1:0] clock_reset, input wire [14:0] i, output wire [14:0] o);
               wire [96:0] od;
               wire [81:0] d;
               wire [81:0] q;
               assign o = od[14:0];
               top_interp_q_cic_combs c0(.clock_reset(clock_reset), .i(d[21:0]), .o(q[21:0]));
               top_interp_q_cic_comb_out c1(.clock_reset(clock_reset), .i(d[43:22]), .o(q[43:22]));
               top_interp_q_cic_integrators c2(.clock_reset(clock_reset), .i(d[65:44]), .o(q[65:44]));
               top_interp_q_cic_phase c3(.clock_reset(clock_reset), .i(d[69:66]), .o(q[69:66]));
               top_interp_q_cic_out c4(.clock_reset(clock_reset), .i(d[80:70]), .o(q[80:70]));
               top_interp_q_cic_starved c5(.clock_reset(clock_reset), .i(d[81:81]), .o(q[81:81]));
               assign d = od[96:15];
               assign od = kernel_cic_interpolate_kernel(clock_reset, i, q);
               function [96:0] kernel_cic_interpolate_kernel(input reg [1:0] arg_0, input reg [14:0] arg_1, input reg [81:0] arg_2);
                     reg [21:0] r0;
                     reg [81:0] r1;
                     // d
                     reg [81:0] r2;
                     reg [21:0] r3;
                     // d
                     reg [81:0] r4;
                     reg [3:0] r5;
                     // d
                     reg [81:0] r6;
                     reg [3:0] r7;
                     reg [0:0] r8;
                     reg [0:0] r9;
                     reg [14:0] r10;
                     reg [0:0] r11;
                     reg [0:0] r12;
                     reg [3:0] r13;
                     reg [3:0] r14;
                     reg [3:0] r15;
                     reg [3:0] r16;
                     reg [0:0] r17;
                     reg [3:0] r18;
                     reg [3:0] r19;
                     // d
                     reg [81:0] r20;
                     reg [8:0] r21;
                     reg [0:0] r22;
                     reg [7:0] r23;
                     reg [7:0] r24;
                     reg [7:0] r25;
                     reg [0:0] r26;
                     reg [10:0] r27;
                     reg [10:0] r28;
                     reg [10:0] r29;
                     reg signed [10:0] r30;
                     // starved_now
                     reg [0:0] r31;
                     // x
                     reg signed [10:0] r32;
                     // starved_now
                     reg [0:0] r33;
                     // x
                     reg signed [10:0] r34;
                     // d
                     reg [81:0] r35;
                     reg [0:0] r36;
                     reg [21:0] r37;
                     reg [21:0] r38;
                     reg [0:0] r39;
                     reg [21:0] r40;
                     reg [21:0] r41;
                     reg [10:0] r42;
                     reg signed [10:0] r43;
                     // outs
                     reg [21:0] r44;
                     reg [10:0] r45;
                     // line
                     reg [10:0] r46;
                     // cs
                     reg [21:0] r47;
                     reg signed [10:0] r48;
                     reg [10:0] r49;
                     reg signed [10:0] r50;
                     // outs
                     reg [21:0] r51;
                     reg [10:0] r52;
                     // line
                     reg [10:0] r53;
                     // cs
                     reg [21:0] r54;
                     // d
                     reg [81:0] r55;
                     // d
                     reg [81:0] r56;
                     reg signed [10:0] r57;
                     // d
                     reg [81:0] r58;
                     // feed
                     reg signed [10:0] r59;
                     reg [21:0] r60;
                     reg [0:0] r61;
                     reg [21:0] r62;
                     reg signed [10:0] r63;
                     reg signed [10:0] r64;
                     reg signed [10:0] r65;
                     // ints
                     reg [21:0] r66;
                     reg [0:0] r67;
                     reg [21:0] r68;
                     reg signed [10:0] r69;
                     reg signed [10:0] r70;
                     reg [0:0] r71;
                     reg [21:0] r72;
                     reg signed [10:0] r73;
                     reg signed [10:0] r74;
                     reg signed [10:0] r75;
                     // ints
                     reg [21:0] r76;
                     // d
                     reg [81:0] r77;
                     reg signed [10:0] r78;
                     // d
                     reg [81:0] r79;
                     reg signed [10:0] r80;
                     reg [0:0] r81;
                     reg [0:0] r82;
                     reg [0:0] r83;
                     reg [14:0] r84;
                     reg [14:0] r85;
                     reg [14:0] r86;
                     reg [14:0] r87;
                     reg [14:0] r88;
                     reg [0:0] r89;
                     reg [1:0] r90;
                     reg [0:0] r91;
                     // d
                     reg [81:0] r92;
                     // d
                     reg [81:0] r93;
                     // d
                     reg [81:0] r94;
                     // d
                     reg [81:0] r95;
                     // d
                     reg [81:0] r96;
                     // d
                     reg [81:0] r97;
                     // o
                     reg [14:0] r98;
                     // o
                     reg [14:0] r99;
                     // o
                     reg [14:0] r100;
                     // o
                     reg [14:0] r101;
                     // o
                     reg [14:0] r102;
                     // d
                     reg [81:0] r103;
                     // o
                     reg [14:0] r104;
                     reg [96:0] r105;
                     localparam l0 = 82'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l1 = 4'b0000;
                     localparam l2 = 4'b0000;
                     localparam l3 = 4'b0001;
                     localparam l4 = 4'b0001;
                     localparam l5 = 4'b0000;
                     localparam l6 = 8'b10000000;
                     localparam l7 = 11'b11100000000;
                     localparam l8 = 11'b00000000000;
                     localparam l9 = 1'b1;
                     localparam l10 = 1'b0;
                     localparam l11 = 1'b0;
                     localparam l12 = 1'b1;
                     localparam l13 = 11'sb00000000000;
                     localparam l14 = 22'b0000000000000000000000;
                     localparam l15 = 22'b0000000000000000000000;
                     localparam l16 = 11'sb00000000000;
                     localparam l17 = 11'sb00000000000;
                     localparam l18 = 11'sb00000000000;
                     localparam l19 = 11'sb00000000000;
                     localparam l20 = 15'b000000000000000;
                     localparam l21 = 1'b0;
                     localparam l22 = 22'b0000000000000000000000;
                     localparam l23 = 22'b0000000000000000000000;
                     localparam l24 = 22'b0000000000000000000000;
                     localparam l25 = 4'b0000;
                     localparam l26 = 11'sb00000000000;
                     localparam l27 = 1'b0;
                     localparam l28 = 11'sb00000000000;
                     localparam l29 = 1'b0;
                     localparam l30 = 1'b0;
                     localparam l31 = 1'b0;
                     localparam l32 = 1'b0;
                     begin
                        r90 = arg_0;
                        r10 = arg_1;
                        r1 = arg_2;
                        r0 = r1[21:0];
                        r2 = l0;
                        r2[21:0] = r0;
                        r3 = r1[43:22];
                        r4 = r2;
                        r4[43:22] = r3;
                        r5 = r1[69:66];
                        r6 = r4;
                        r6[69:66] = r5;
                        r7 = r1[69:66];
                        r8 = r7 == l1;
                        r9 = r10[13:13];
                        r11 = r8 | r9;
                        r12 = r10[13:13];
                        r13 = r1[69:66];
                        r14 = r12 ? l2 : r13;
                        r15 = r14 + l3;
                        r16 = r10[12:9];
                        r17 = r15 >= r16;
                        r18 = r14 + l4;
                        r19 = r17 ? l5 : r18;
                        r20 = r6;
                        r20[69:66] = r19;
                        r21 = r10[8:0];
                        r22 = r21[8:8];
                        r23 = r21[7:0];
                        r24 = $unsigned(r23);
                        r25 = r24 & l6;
                        r26 = |r25;
                        r27 = {{3{1'b0}}, r24};
                        r28 = r26 ? l7 : l8;
                        r29 = r27 + r28;
                        r30 = $signed(r29);
                        case (r22)
                           1'b1 : r31 = l10;
                           1'b0 : r31 = l12;
                        endcase
                        case (r22)
                           1'b1 : r32 = r30;
                           1'b0 : r32 = l13;
                        endcase
                        r33 = r11 ? r31 : l10;
                        r34 = r11 ? r32 : l13;
                        r35 = r20;
                        r35[81:81] = r33;
                        r36 = r10[13:13];
                        r37 = r1[21:0];
                        r38 = r36 ? l14 : r37;
                        r39 = r10[13:13];
                        r40 = r1[43:22];
                        r41 = r39 ? l15 : r40;
                        r42 = r38[10:0];
                        r43 = r34 - r42;
                        r44 = r41;
                        r44[10:0] = r43;
                        r45 = r38[10:0];
                        r46 = r45;
                        r46[10:0] = r34;
                        r47 = r38;
                        r47[10:0] = r46;
                        r48 = r41[10:0];
                        r49 = r38[21:11];
                        r50 = r48 - r49;
                        r51 = r44;
                        r51[21:11] = r50;
                        r52 = r38[21:11];
                        r53 = r52;
                        r53[10:0] = r48;
                        r54 = r47;
                        r54[21:11] = r53;
                        r55 = r35;
                        r55[21:0] = r54;
                        r56 = r55;
                        r56[43:22] = r51;
                        r57 = r41[21:11];
                        r58 = r11 ? r56 : r35;
                        r59 = r11 ? r57 : l16;
                        r60 = r1[65:44];
                        r61 = r10[13:13];
                        r62 = r1[65:44];
                        r63 = r62[10:0];
                        r64 = r61 ? l17 : r63;
                        r65 = r64 + r59;
                        r66 = r60;
                        r66[10:0] = r65;
                        r67 = r10[13:13];
                        r68 = r1[65:44];
                        r69 = r68[21:11];
                        r70 = r67 ? l18 : r69;
                        r71 = r10[13:13];
                        r72 = r1[65:44];
                        r73 = r72[10:0];
                        r74 = r71 ? l19 : r73;
                        r75 = r70 + r74;
                        r76 = r66;
                        r76[21:11] = r75;
                        r77 = r58;
                        r77[65:44] = r76;
                        r78 = r76[21:11];
                        r79 = r77;
                        r79[80:70] = r78;
                        r80 = r1[80:70];
                        r81 = r1[81:81];
                        r82 = r10[14:14];
                        r83 = ~r82;
                        r84 = l20;
                        r84[10:0] = r80;
                        r85 = r84;
                        r85[11:11] = r8;
                        r86 = r85;
                        r86[12:12] = r81;
                        r87 = r86;
                        r87[13:13] = r83;
                        r88 = r87;
                        r88[14:14] = l21;
                        r89 = r90[1:1];
                        r91 = |r89;
                        r92 = r79;
                        r92[21:0] = l22;
                        r93 = r92;
                        r93[43:22] = l23;
                        r94 = r93;
                        r94[65:44] = l24;
                        r95 = r94;
                        r95[69:66] = l25;
                        r96 = r95;
                        r96[80:70] = l26;
                        r97 = r96;
                        r97[81:81] = l27;
                        r98 = r88;
                        r98[10:0] = l28;
                        r99 = r98;
                        r99[11:11] = l29;
                        r100 = r99;
                        r100[12:12] = l30;
                        r101 = r100;
                        r101[13:13] = l31;
                        r102 = r101;
                        r102[14:14] = l32;
                        r103 = r91 ? r97 : r79;
                        r104 = r91 ? r102 : r88;
                        r105 = {r103, r104};
                        kernel_cic_interpolate_kernel = r105;
                     end
               endfunction
            endmodule
            module top_interp_q_cic_combs(input wire [1:0] clock_reset, input wire [21:0] i, output reg [21:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 22'b0000000000000000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 22'b0000000000000000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_interp_q_cic_comb_out(input wire [1:0] clock_reset, input wire [21:0] i, output reg [21:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 22'b0000000000000000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 22'b0000000000000000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_interp_q_cic_integrators(input wire [1:0] clock_reset, input wire [21:0] i, output reg [21:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 22'b0000000000000000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 22'b0000000000000000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_interp_q_cic_phase(input wire [1:0] clock_reset, input wire [3:0] i, output reg [3:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 4'b0000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 4'b0000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_interp_q_cic_out(input wire [1:0] clock_reset, input wire [10:0] i, output reg [10:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 11'sb00000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 11'sb00000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_interp_q_cic_starved(input wire [1:0] clock_reset, input wire [0:0] i, output reg [0:0] o);
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
            module top_interp_q_marked(input wire [1:0] clock_reset, input wire [0:0] i, output reg [0:0] o);
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
            module top_combine(input wire [1:0] clock_reset, input wire [26:0] i, output wire [26:0] o);
               wire [26:0] od;
               wire [0:0] q;
               assign o = od[26:0];
               top_combine_marker c0(.clock_reset(clock_reset), .o(q[0:0]));
               assign od = kernel_iq_combine_kernel(clock_reset, i, q);
               function [26:0] kernel_iq_combine_kernel(input reg [1:0] arg_0, input reg [26:0] arg_1, input reg [0:0] arg_2);
                     reg [12:0] r0;
                     reg [26:0] r1;
                     reg [0:0] r2;
                     // have_re
                     reg [0:0] r3;
                     reg [12:0] r4;
                     reg [0:0] r5;
                     // have_im
                     reg [0:0] r6;
                     reg [12:0] r7;
                     reg [0:0] r8;
                     reg [11:0] r9;
                     reg [12:0] r10;
                     reg [0:0] r11;
                     reg [11:0] r12;
                     reg [0:0] r13;
                     reg [0:0] r14;
                     reg [0:0] r15;
                     // frame_mismatch
                     reg [0:0] r16;
                     reg [10:0] r17;
                     reg [10:0] r18;
                     reg [21:0] r19;
                     reg [21:0] r20;
                     reg [0:0] r21;
                     reg [22:0] r22;
                     reg [22:0] r23;
                     reg [23:0] r24;
                     reg [22:0] r25;
                     // frame_mismatch
                     reg [0:0] r26;
                     // out_data
                     reg [23:0] r27;
                     // frame_mismatch
                     reg [0:0] r28;
                     // out_data
                     reg [23:0] r29;
                     reg [0:0] r30;
                     reg [0:0] r31;
                     reg [24:0] r32;
                     reg [24:0] r33;
                     reg [26:0] r34;
                     reg [26:0] r35;
                     reg [26:0] r36;
                     reg [1:0] r37;
                     reg [0:0] r38;
                     localparam l0 = 1'b1;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b0;
                     localparam l3 = 1'b1;
                     localparam l4 = 1'b1;
                     localparam l5 = 1'b0;
                     localparam l6 = 1'b1;
                     localparam l7 = 1'b0;
                     localparam l8 = 22'b0000000000000000000000;
                     localparam l9 = 23'b00000000000000000000000;
                     localparam l10 = 1'b1;
                     localparam l11 = 1'b1;
                     localparam l12 = 24'b000000000000000000000000;
                     localparam l13 = 1'b1;
                     localparam l14 = 25'b0000000000000000000000000;
                     localparam l15 = 27'b000000000000000000000000000;
                     begin
                        r37 = arg_0;
                        r1 = arg_1;
                        r38 = arg_2;
                        r0 = r1[12:0];
                        r2 = r0[12:12];
                        case (r2)
                           1'b1 : r3 = l1;
                           default : r3 = l2;
                        endcase
                        r4 = r1[25:13];
                        r5 = r4[12:12];
                        case (r5)
                           1'b1 : r6 = l4;
                           default : r6 = l5;
                        endcase
                        r7 = r1[12:0];
                        r8 = r7[12:12];
                        r9 = r7[11:0];
                        r10 = r1[25:13];
                        r11 = r10[12:12];
                        r12 = r10[11:0];
                        r13 = r9[11:11];
                        r14 = r12[11:11];
                        r15 = r13 != r14;
                        r16 = r15 ? l6 : l7;
                        r17 = r9[10:0];
                        r18 = r12[10:0];
                        r19 = l8;
                        r19[10:0] = r17;
                        r20 = r19;
                        r20[21:11] = r18;
                        r21 = r9[11:11];
                        r22 = l9;
                        r22[21:0] = r20;
                        r23 = r22;
                        r23[22:22] = r21;
                        r25 = r23[22:0];
                        r24 = {l10, r25};
                        case (r11)
                           1'b1 : r26 = r16;
                           default : r26 = l7;
                        endcase
                        case (r11)
                           1'b1 : r27 = r24;
                           default : r27 = l12;
                        endcase
                        case (r8)
                           1'b1 : r28 = r26;
                           default : r28 = l7;
                        endcase
                        case (r8)
                           1'b1 : r29 = r27;
                           default : r29 = l12;
                        endcase
                        r30 = r3 != r6;
                        r31 = r1[26:26];
                        r32 = l14;
                        r32[23:0] = r29;
                        r33 = r32;
                        r33[24:24] = r31;
                        r34 = l15;
                        r34[24:0] = r33;
                        r35 = r34;
                        r35[25:25] = r30;
                        r36 = r35;
                        r36[26:26] = r28;
                        kernel_iq_combine_kernel = r36;
                     end
               endfunction
            endmodule
            module top_combine_marker(input wire [1:0] clock_reset, output wire [0:0] o);
               assign o = 1'b0;
            endmodule
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    fn tb_stream() -> Vec<In<W, CW>> {
        let mut seq = stimulus(3 * RATE, 25, -17, None);
        seq.extend(stimulus(4 * RATE, -12, 30, Some(0)));
        seq
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
            .join("duc_upsampler");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["dea58ecd6415785d277061b4273bf184e8bc99f975878cf136803e421a6d834c"];
        let digest = vcd.dump_to_file(root.join("upsampler.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
