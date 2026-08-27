#![warn(missing_docs)]
//! `StreamInterpolator` — an [`RCStream`] front end for
//! [`CicInterpolate`](super::interpolator::CicInterpolate).
//!
//! The transmit counterpart of [`super::stream::StreamDecimator`], and
//! the same job: take the framing bookkeeping off the core filter, so
//! the filter deals in samples and the wrapper deals in items, marks
//! and ready.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+StreamInterpolator+----------+
      |                               |
+---->+ stream                        |
      | Option<Item<Real<WI>,SyncMark>|
      |                        stream |
+---->+ rate      RCStream<Real<WO>,  +----->
      |   Bits<CW>          SyncMark> |
      |                               |
+---->+ downstream_ready      starved +----->
      |                       overrun +----->
      |                     saturated +----->
      +-------------------------------+
")]
//!
//! # `stream.ready` means something here
//!
//! [`super::stream::StreamDecimator`] passes `downstream_ready` straight
//! through to its upstream-facing `ready`: a decimator consumes on every
//! cycle and has no reason ever to refuse one.
//!
//! An interpolator is the opposite. It consumes **one sample every `R`
//! cycles**, so `Out::stream.ready` carries
//! [`super::interpolator::Out::input_ready`] — a genuine, once-per-`R`
//! request. This widget is the rate-controlling element of a transmit
//! chain, and an upstream that ignores `ready` will not stall it; it
//! will simply be sampled on this widget's grid instead of its own.
//!
//! `ready` depends only on the core's registered phase counter, so it
//! never depends combinationally on `data` — which is the direction the
//! `RCStream` contract forbids and the property that keeps
//! [`crate::rcstream::relay::RCStreamRelay`] insertion sound.
//!
//! # The output is always present, so `data` is always `Some`
//!
//! An interpolator emits on every cycle. That makes
//! `Out::stream.data` an `Option` that is always `Some` once out of
//! reset — the `Option` is there because [`RCStream`] requires it, not
//! because this widget has idle cycles. A DUC feeding a DAC wants
//! exactly that.
//!
//! # Where the mark rides, and the delay it does not account for
//!
//! A marked sample restarts the window, as in the decimator, and the
//! mark rides out on the **first output of the new window** — the cycle
//! after the restarting input.
//!
//! That is the first sample of the new burst, which is what a mark
//! means. It is deliberately *not* the first output the marked sample
//! measurably influenced: the core's integrator cascade is pipelined,
//! so a new sample does not reach the output for `STAGES` cycles, and
//! the cascade then fills over `N·R·M` more. **A phase-sensitive
//! transmitter has to account for that group delay itself** — the mark
//! names the window boundary, not the point at which the signal
//! arrives. Marking the boundary is the choice that composes, because
//! the boundary is the thing both ends of the link can agree on without
//! knowing the filter's configuration.
//!
//! # There is no out-of-band restart
//!
//! As with the decimator: widgets connect through the stream and its
//! framing, and a second mechanism for the same thing is a second
//! mechanism to keep consistent. A caller that wants to restart marks a
//! sample.
//!
//! Note this composes with the rate: [`super::interpolator`]'s docs
//! explain that a rate change needs a restart to take effect on the
//! level, and marking the first sample at the new rate is how that is
//! spelled here.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/cic_interp_stream.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/cic_interp_stream.md")]

use rhdl::prelude::*;

use crate::core::dff;
use crate::dsp::iq::Real;
use crate::dsp::sync::SyncMark;
use crate::rcstream::{Item, RCStream};

/// An [`RCStream`] wrapper around any CIC interpolator core.
///
/// Generic over the core so that a width-tapered interpolator drops
/// into the same slot as the uniform one — see
/// [`super::interp`] for why tapering an interpolator is lossless.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct StreamInterpolator<const W_IN: usize, const W_OUT: usize, const CW: usize, C>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    C: SynchronousIO<I = super::interpolator::In<W_IN, CW>, O = super::interpolator::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// The filter.
    cic: C,
    /// The mark, held one cycle so it lands on the same output as the
    /// sample it belongs to.
    marked: dff::DFF<bool>,
}

impl<const W_IN: usize, const W_OUT: usize, const CW: usize, C>
    StreamInterpolator<W_IN, W_OUT, CW, C>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    C: SynchronousIO<I = super::interpolator::In<W_IN, CW>, O = super::interpolator::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// Wrap a specific interpolator core.
    pub fn new(cic: C) -> Self {
        Self {
            cic,
            marked: dff::DFF::new(false),
        }
    }
}

impl<const W_IN: usize, const W_OUT: usize, const CW: usize, C> Default
    for StreamInterpolator<W_IN, W_OUT, CW, C>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    C: SynchronousIO<I = super::interpolator::In<W_IN, CW>, O = super::interpolator::Out<W_OUT>>
        + Synchronous
        + Default
        + Clone
        + std::fmt::Debug,
{
    fn default() -> Self {
        Self::new(C::default())
    }
}

/// Inputs to [`StreamInterpolator`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const W_IN: usize, const CW: usize>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
{
    /// The low-rate envelope stream.
    ///
    /// Consumed on cycles where `Out::stream.ready` is high, which is
    /// one cycle in [`In::rate`]. A mark restarts the window.
    pub stream: Option<Item<Real<W_IN>, SyncMark>>,
    /// The interpolation factor. See
    /// [`super::interpolator::In::rate`].
    pub rate: Bits<CW>,
    /// Downstream's ready, per the `RCStream` contract.
    pub downstream_ready: bool,
}

/// Outputs from [`StreamInterpolator`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const W_OUT: usize>
where
    rhdl::bits::W<W_OUT>: BitWidth,
{
    /// The high-rate stream, and the upstream-facing `ready`.
    ///
    /// `data` is `Some` on every cycle out of reset; `ready` is the
    /// core's once-per-`R` request. See the module docs on both.
    pub stream: RCStream<Real<W_OUT>, SyncMark>,
    /// An input cycle found nothing and fed zero.
    pub starved: bool,
    /// An output was produced while `downstream_ready` was low.
    pub overrun: bool,
    /// The filter clipped. Always false for an exact-width core.
    pub saturated: bool,
}

impl<const W_IN: usize, const W_OUT: usize, const CW: usize, C> SynchronousIO
    for StreamInterpolator<W_IN, W_OUT, CW, C>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    C: SynchronousIO<I = super::interpolator::In<W_IN, CW>, O = super::interpolator::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    type I = In<W_IN, CW>;
    type O = Out<W_OUT>;
    type Kernel = stream_interpolator_kernel<W_IN, W_OUT, CW, C>;
}

#[kernel]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn stream_interpolator_kernel<const W_IN: usize, const W_OUT: usize, const CW: usize, C>(
    cr: ClockReset,
    i: In<W_IN, CW>,
    q: Q<W_IN, W_OUT, CW, C>,
) -> (Out<W_OUT>, D<W_IN, W_OUT, CW, C>)
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
    C: SynchronousIO<I = super::interpolator::In<W_IN, CW>, O = super::interpolator::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    let mut d = D::<W_IN, W_OUT, CW, C>::dont_care();

    // Unwrap the framed sample.
    let mut sample = None;
    let mut marked_now = false;
    if let Some(it) = i.stream {
        sample = Some(it.data.v);
        marked_now = it.frame.sync;
    }

    // **The mark only counts on a cycle the core actually takes the
    // sample.** A mark presented while the core is not asking is not
    // consumed and must not restart anything -- the sample it came with
    // is still waiting upstream, and restarting now would anchor the
    // window to a sample that has not been read yet.
    let taken = q.cic.input_ready;
    let restart = marked_now && taken;

    d.cic = super::interpolator::In::<W_IN, CW> {
        sample,
        rate: i.rate,
        restart,
        downstream_ready: i.downstream_ready,
    };

    // Held one cycle, so it lands on the first output of the new
    // window rather than on the last output of the old one.
    d.marked = restart;

    let mut o = Out::<W_OUT> {
        stream: RCStream::<Real<W_OUT>, SyncMark> {
            data: Some(Item::<Real<W_OUT>, SyncMark> {
                data: Real::<W_OUT> { v: q.cic.sample },
                frame: SyncMark { sync: q.marked },
            }),
            // The real thing, not a pass-through -- see the module docs.
            ready: q.cic.input_ready,
        },
        starved: q.cic.starved,
        overrun: q.cic.overrun,
        saturated: q.cic.saturated,
    };

    if cr.reset.any() {
        d.cic = super::interpolator::In::<W_IN, CW> {
            sample: None,
            rate: i.rate,
            restart: false,
            downstream_ready: false,
        };
        d.marked = false;
        o.stream = RCStream::<Real<W_OUT>, SyncMark> {
            data: None,
            ready: false,
        };
        o.starved = false;
        o.overrun = false;
        o.saturated = false;
    }

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::super::interp;
    use super::super::interpolator::CicInterpolate;
    use super::*;
    use expect_test::expect;

    const WI: usize = 8;
    const WA: usize = 11;
    const S: usize = 2;
    const RMAX: usize = 8;
    const M: usize = 1;
    const CW: usize = 4;
    const RATE: usize = 4;
    type Core = CicInterpolate<WI, WA, S, RMAX, M, CW>;
    type Uut = StreamInterpolator<WI, WA, CW, Core>;

    fn item(v: i128, mark: bool) -> Option<Item<Real<WI>, SyncMark>> {
        Some(Item::<Real<WI>, SyncMark> {
            data: Real::<WI> { v: signed::<WI>(v) },
            frame: SyncMark { sync: mark },
        })
    }

    /// `n` cycles presenting `v`, marking the cycle at `mark_at`.
    fn stimulus(cycles: usize, v: i128, mark_at: Option<usize>) -> Vec<In<WI, CW>> {
        (0..cycles)
            .map(|n| In::<WI, CW> {
                stream: item(v, mark_at == Some(n)),
                rate: bits::<CW>(RATE as u128),
                downstream_ready: true,
            })
            .collect()
    }

    fn run(seq: Vec<In<WI, CW>>) -> Vec<Out<WA>> {
        let uut = Uut::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect()
    }

    #[test]
    fn default_construction() {
        let _ = Uut::default();
    }

    /// **`ready` is a real request, once per rate.**
    ///
    /// The difference from [`super::super::stream::StreamDecimator`],
    /// which passes `downstream_ready` through because a decimator
    /// never refuses a sample.
    #[test]
    fn ready_fires_once_per_rate() {
        let got = run(stimulus(6 * RATE, 3, None));
        // Cycle 0 is reset, where ready is forced low.
        assert!(!got[0].stream.ready, "reset holds ready low");
        for (n, o) in got[1..].iter().enumerate() {
            assert_eq!(
                o.stream.ready,
                n % RATE == 0,
                "cycle {n}: ready {}, expected {}",
                o.stream.ready,
                n % RATE == 0
            );
        }
    }

    /// The output is present on every cycle once out of reset.
    ///
    /// An `Option` that is always `Some`, because [`RCStream`] requires
    /// the shape and this widget has no idle cycles.
    #[test]
    fn data_is_present_on_every_cycle() {
        let got = run(stimulus(4 * RATE, 3, None));
        assert!(got[0].stream.data.is_none(), "reset emits nothing");
        assert!(
            got[1..].iter().all(|o| o.stream.data.is_some()),
            "every live cycle carries a sample"
        );
    }

    /// It computes what the bare core computes, sample for sample.
    ///
    /// The wrapper is framing and ready and nothing else; if this
    /// disagrees, the wrapper has grown arithmetic it should not have.
    #[test]
    fn it_agrees_with_the_bare_core() {
        let seq = stimulus(6 * RATE, 5, None);
        let wrapped: Vec<i128> = run(seq)
            .iter()
            .map(|o| match o.stream.data {
                Some(it) => it.data.v.raw(),
                None => 0,
            })
            .collect();

        let core = Core::default();
        let bare: Vec<i128> = core
            .run(
                (0..6 * RATE)
                    .map(|_| super::super::interpolator::In::<WI, CW> {
                        sample: Some(signed::<WI>(5)),
                        rate: bits::<CW>(RATE as u128),
                        restart: false,
                        downstream_ready: true,
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .with_reset(1)
                    .clock_pos_edge(100),
            )
            .synchronous_sample()
            .map(|s| s.output.sample.raw())
            .collect();
        assert_eq!(wrapped, bare);
    }

    /// **A mark restarts the window.**
    ///
    /// Checked behaviourally: the output after a mark must match the
    /// same burst run from reset, or the previous transmission is
    /// leaking into this one through the integrators.
    #[test]
    fn a_mark_restarts_the_window() {
        let clean: Vec<i128> = run(stimulus(6 * RATE, 6, None))
            .iter()
            .map(|o| match o.stream.data {
                Some(it) => it.data.v.raw(),
                None => 0,
            })
            .collect();

        // Junk at a different amplitude, then a marked burst.
        let mut seq = stimulus(4 * RATE, -60, None);
        seq.extend(stimulus(6 * RATE, 6, Some(0)));
        let got: Vec<i128> = run(seq)
            .iter()
            .map(|o| match o.stream.data {
                Some(it) => it.data.v.raw(),
                None => 0,
            })
            .collect();

        // The restart is taken on sample index `start`; that cycle's
        // output is still the *old* window's last value, and the new
        // window's first output emerges the cycle after -- exactly as
        // it does at index 1 -> 2 in the clean run.
        let start = 1 + 4 * RATE;
        assert_eq!(
            &got[start + 1..],
            &clean[2..],
            "a marked burst must match the same burst from reset"
        );
    }

    /// The mark rides the first output of the new window.
    #[test]
    fn the_mark_rides_the_first_output_of_the_new_window() {
        let mut seq = stimulus(2 * RATE, 4, None);
        seq.extend(stimulus(3 * RATE, 4, Some(0)));
        let got = run(seq);
        let marks: Vec<usize> = got
            .iter()
            .enumerate()
            .filter(|(_, o)| match o.stream.data {
                Some(it) => it.frame.sync,
                None => false,
            })
            .map(|(n, _)| n)
            .collect();
        // Reset cycle, then 2*RATE unmarked cycles, then the marked
        // input is taken -- and the mark appears on the output one
        // cycle later.
        assert_eq!(marks, vec![1 + 2 * RATE + 1], "exactly one marked output");
    }

    /// **A mark offered on a cycle the core is not asking is not
    /// consumed.**
    ///
    /// The subtle one. Upstream holds a marked sample until `ready`
    /// comes up, so the mark is presented for several cycles before it
    /// is taken. Restarting on the first of those would anchor the
    /// window to a sample that has not been read yet — and would then
    /// restart again on every subsequent cycle until it was.
    ///
    /// The check is that exactly one restart happens, and that it
    /// happens on the `ready` cycle: a marked sample held across a
    /// whole window must behave identically to one presented only on
    /// the `ready` cycle.
    #[test]
    fn a_mark_held_across_a_window_restarts_exactly_once() {
        // Present the mark from cycle 0 of a window and hold it high
        // for the whole window.
        let held: Vec<In<WI, CW>> = (0..4 * RATE)
            .map(|n| In::<WI, CW> {
                stream: item(7, n < RATE),
                rate: bits::<CW>(RATE as u128),
                downstream_ready: true,
            })
            .collect();
        // The same thing, marked only on the cycle it is taken.
        let once: Vec<In<WI, CW>> = (0..4 * RATE)
            .map(|n| In::<WI, CW> {
                stream: item(7, n == 0),
                rate: bits::<CW>(RATE as u128),
                downstream_ready: true,
            })
            .collect();
        let a = run(held);
        let b = run(once);
        let strip = |v: &Vec<Out<WA>>| -> Vec<(i128, bool)> {
            v.iter()
                .map(|o| match o.stream.data {
                    Some(it) => (it.data.v.raw(), it.frame.sync),
                    None => (0, false),
                })
                .collect()
        };
        assert_eq!(strip(&a), strip(&b), "holding the mark must change nothing");
        // And exactly one mark came out.
        assert_eq!(
            strip(&a).iter().filter(|(_, m)| *m).count(),
            1,
            "one restart, not one per cycle the mark was held"
        );
    }

    /// Starvation is passed through from the core.
    #[test]
    fn starvation_is_reported() {
        let seq: Vec<In<WI, CW>> = (0..4 * RATE)
            .map(|n| In::<WI, CW> {
                stream: if n == 2 * RATE { None } else { item(9, false) },
                rate: bits::<CW>(RATE as u128),
                downstream_ready: true,
            })
            .collect();
        let fired: Vec<usize> = run(seq)
            .iter()
            .enumerate()
            .filter(|(_, o)| o.starved)
            .map(|(n, _)| n)
            .collect();
        assert_eq!(fired, vec![1 + 2 * RATE + 1]);
    }

    /// The configuration is at the design maths' bound.
    #[test]
    fn the_test_configuration_is_at_the_bound() {
        assert_eq!(interp::accumulator_width(WI, S, RMAX, M), WA);
        assert_eq!(interp::rate_width(RMAX), CW);
    }

    // ---- Tier 3 / 4 / 5 --------------------------------------------

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [14:0] i, output wire [16:0] o);
               wire [32:0] od;
               wire [15:0] d;
               wire [15:0] q;
               assign o = od[16:0];
               top_cic c0(.clock_reset(clock_reset), .i(d[14:0]), .o(q[14:0]));
               top_marked c1(.clock_reset(clock_reset), .i(d[15:15]), .o(q[15:15]));
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
            module top_cic(input wire [1:0] clock_reset, input wire [14:0] i, output wire [14:0] o);
               wire [74:0] od;
               wire [59:0] d;
               wire [59:0] q;
               assign o = od[14:0];
               top_cic_combs c0(.clock_reset(clock_reset), .i(d[21:0]), .o(q[21:0]));
               top_cic_integrators c1(.clock_reset(clock_reset), .i(d[43:22]), .o(q[43:22]));
               top_cic_phase c2(.clock_reset(clock_reset), .i(d[47:44]), .o(q[47:44]));
               top_cic_out c3(.clock_reset(clock_reset), .i(d[58:48]), .o(q[58:48]));
               top_cic_starved c4(.clock_reset(clock_reset), .i(d[59:59]), .o(q[59:59]));
               assign d = od[74:15];
               assign od = kernel_cic_interpolate_kernel(clock_reset, i, q);
               function [74:0] kernel_cic_interpolate_kernel(input reg [1:0] arg_0, input reg [14:0] arg_1, input reg [59:0] arg_2);
                     reg [21:0] r0;
                     reg [59:0] r1;
                     // d
                     reg [59:0] r2;
                     reg [3:0] r3;
                     // d
                     reg [59:0] r4;
                     reg [3:0] r5;
                     reg [0:0] r6;
                     reg [0:0] r7;
                     reg [14:0] r8;
                     reg [0:0] r9;
                     reg [0:0] r10;
                     reg [3:0] r11;
                     reg [3:0] r12;
                     reg [3:0] r13;
                     reg [3:0] r14;
                     reg [0:0] r15;
                     reg [3:0] r16;
                     reg [3:0] r17;
                     // d
                     reg [59:0] r18;
                     reg [8:0] r19;
                     reg [0:0] r20;
                     reg [7:0] r21;
                     reg [7:0] r22;
                     reg [7:0] r23;
                     reg [0:0] r24;
                     reg [10:0] r25;
                     reg [10:0] r26;
                     reg [10:0] r27;
                     reg signed [10:0] r28;
                     // starved_now
                     reg [0:0] r29;
                     // x
                     reg signed [10:0] r30;
                     // starved_now
                     reg [0:0] r31;
                     // x
                     reg signed [10:0] r32;
                     // d
                     reg [59:0] r33;
                     reg [0:0] r34;
                     reg [21:0] r35;
                     reg [21:0] r36;
                     reg [10:0] r37;
                     reg signed [10:0] r38;
                     reg [10:0] r39;
                     // line
                     reg [10:0] r40;
                     // cs
                     reg [21:0] r41;
                     reg [10:0] r42;
                     reg signed [10:0] r43;
                     reg [10:0] r44;
                     // line
                     reg [10:0] r45;
                     // cs
                     reg [21:0] r46;
                     // d
                     reg [59:0] r47;
                     // d
                     reg [59:0] r48;
                     // feed
                     reg signed [10:0] r49;
                     reg [21:0] r50;
                     reg [0:0] r51;
                     reg [21:0] r52;
                     reg signed [10:0] r53;
                     reg signed [10:0] r54;
                     reg signed [10:0] r55;
                     // ints
                     reg [21:0] r56;
                     reg [0:0] r57;
                     reg [21:0] r58;
                     reg signed [10:0] r59;
                     reg signed [10:0] r60;
                     reg [0:0] r61;
                     reg [21:0] r62;
                     reg signed [10:0] r63;
                     reg signed [10:0] r64;
                     reg signed [10:0] r65;
                     // ints
                     reg [21:0] r66;
                     // d
                     reg [59:0] r67;
                     reg signed [10:0] r68;
                     // d
                     reg [59:0] r69;
                     reg signed [10:0] r70;
                     reg [0:0] r71;
                     reg [0:0] r72;
                     reg [0:0] r73;
                     reg [14:0] r74;
                     reg [14:0] r75;
                     reg [14:0] r76;
                     reg [14:0] r77;
                     reg [14:0] r78;
                     reg [0:0] r79;
                     reg [1:0] r80;
                     reg [0:0] r81;
                     // d
                     reg [59:0] r82;
                     // d
                     reg [59:0] r83;
                     // d
                     reg [59:0] r84;
                     // d
                     reg [59:0] r85;
                     // d
                     reg [59:0] r86;
                     // o
                     reg [14:0] r87;
                     // o
                     reg [14:0] r88;
                     // o
                     reg [14:0] r89;
                     // o
                     reg [14:0] r90;
                     // o
                     reg [14:0] r91;
                     // d
                     reg [59:0] r92;
                     // o
                     reg [14:0] r93;
                     reg [74:0] r94;
                     localparam l0 = 60'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
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
                     localparam l15 = 11'sb00000000000;
                     localparam l16 = 11'sb00000000000;
                     localparam l17 = 11'sb00000000000;
                     localparam l18 = 11'sb00000000000;
                     localparam l19 = 15'b000000000000000;
                     localparam l20 = 1'b0;
                     localparam l21 = 22'b0000000000000000000000;
                     localparam l22 = 22'b0000000000000000000000;
                     localparam l23 = 4'b0000;
                     localparam l24 = 11'sb00000000000;
                     localparam l25 = 1'b0;
                     localparam l26 = 11'sb00000000000;
                     localparam l27 = 1'b0;
                     localparam l28 = 1'b0;
                     localparam l29 = 1'b0;
                     localparam l30 = 1'b0;
                     begin
                        r80 = arg_0;
                        r8 = arg_1;
                        r1 = arg_2;
                        r0 = r1[21:0];
                        r2 = l0;
                        r2[21:0] = r0;
                        r3 = r1[47:44];
                        r4 = r2;
                        r4[47:44] = r3;
                        r5 = r1[47:44];
                        r6 = r5 == l1;
                        r7 = r8[13:13];
                        r9 = r6 | r7;
                        r10 = r8[13:13];
                        r11 = r1[47:44];
                        r12 = r10 ? l2 : r11;
                        r13 = r12 + l3;
                        r14 = r8[12:9];
                        r15 = r13 >= r14;
                        r16 = r12 + l4;
                        r17 = r15 ? l5 : r16;
                        r18 = r4;
                        r18[47:44] = r17;
                        r19 = r8[8:0];
                        r20 = r19[8:8];
                        r21 = r19[7:0];
                        r22 = $unsigned(r21);
                        r23 = r22 & l6;
                        r24 = |r23;
                        r25 = {{3{1'b0}}, r22};
                        r26 = r24 ? l7 : l8;
                        r27 = r25 + r26;
                        r28 = $signed(r27);
                        case (r20)
                           1'b1 : r29 = l10;
                           1'b0 : r29 = l12;
                        endcase
                        case (r20)
                           1'b1 : r30 = r28;
                           1'b0 : r30 = l13;
                        endcase
                        r31 = r9 ? r29 : l10;
                        r32 = r9 ? r30 : l13;
                        r33 = r18;
                        r33[59:59] = r31;
                        r34 = r8[13:13];
                        r35 = r1[21:0];
                        r36 = r34 ? l14 : r35;
                        r37 = r36[10:0];
                        r38 = r32 - r37;
                        r39 = r36[10:0];
                        r40 = r39;
                        r40[10:0] = r32;
                        r41 = r36;
                        r41[10:0] = r40;
                        r42 = r36[21:11];
                        r43 = r38 - r42;
                        r44 = r36[21:11];
                        r45 = r44;
                        r45[10:0] = r38;
                        r46 = r41;
                        r46[21:11] = r45;
                        r47 = r33;
                        r47[21:0] = r46;
                        r48 = r9 ? r47 : r33;
                        r49 = r9 ? r43 : l15;
                        r50 = r1[43:22];
                        r51 = r8[13:13];
                        r52 = r1[43:22];
                        r53 = r52[10:0];
                        r54 = r51 ? l16 : r53;
                        r55 = r54 + r49;
                        r56 = r50;
                        r56[10:0] = r55;
                        r57 = r8[13:13];
                        r58 = r1[43:22];
                        r59 = r58[21:11];
                        r60 = r57 ? l17 : r59;
                        r61 = r8[13:13];
                        r62 = r1[43:22];
                        r63 = r62[10:0];
                        r64 = r61 ? l18 : r63;
                        r65 = r60 + r64;
                        r66 = r56;
                        r66[21:11] = r65;
                        r67 = r48;
                        r67[43:22] = r66;
                        r68 = r66[21:11];
                        r69 = r67;
                        r69[58:48] = r68;
                        r70 = r1[58:48];
                        r71 = r1[59:59];
                        r72 = r8[14:14];
                        r73 = ~r72;
                        r74 = l19;
                        r74[10:0] = r70;
                        r75 = r74;
                        r75[11:11] = r6;
                        r76 = r75;
                        r76[12:12] = r71;
                        r77 = r76;
                        r77[13:13] = r73;
                        r78 = r77;
                        r78[14:14] = l20;
                        r79 = r80[1:1];
                        r81 = |r79;
                        r82 = r69;
                        r82[21:0] = l21;
                        r83 = r82;
                        r83[43:22] = l22;
                        r84 = r83;
                        r84[47:44] = l23;
                        r85 = r84;
                        r85[58:48] = l24;
                        r86 = r85;
                        r86[59:59] = l25;
                        r87 = r78;
                        r87[10:0] = l26;
                        r88 = r87;
                        r88[11:11] = l27;
                        r89 = r88;
                        r89[12:12] = l28;
                        r90 = r89;
                        r90[13:13] = l29;
                        r91 = r90;
                        r91[14:14] = l30;
                        r92 = r81 ? r86 : r69;
                        r93 = r81 ? r91 : r78;
                        r94 = {r92, r93};
                        kernel_cic_interpolate_kernel = r94;
                     end
               endfunction
            endmodule
            module top_cic_combs(input wire [1:0] clock_reset, input wire [21:0] i, output reg [21:0] o);
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
            module top_cic_integrators(input wire [1:0] clock_reset, input wire [21:0] i, output reg [21:0] o);
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
            module top_cic_phase(input wire [1:0] clock_reset, input wire [3:0] i, output reg [3:0] o);
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
            module top_cic_out(input wire [1:0] clock_reset, input wire [10:0] i, output reg [10:0] o);
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
            module top_cic_starved(input wire [1:0] clock_reset, input wire [0:0] i, output reg [0:0] o);
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
            module top_marked(input wire [1:0] clock_reset, input wire [0:0] i, output reg [0:0] o);
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
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    #[test]
    fn test_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let mut seq = stimulus(2 * RATE, 4, None);
        seq.extend(stimulus(3 * RATE, 4, Some(0)));
        let input = seq.into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let mut seq = stimulus(2 * RATE, 4, None);
        seq.extend(stimulus(3 * RATE, 4, Some(0)));
        let input = seq.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("cic_interp_stream");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["767aa020aff472449fea0ab476f6eec929184712f1b47164331c69bfcffe935a"];
        let digest = vcd.dump_to_file(root.join("interp_stream.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
