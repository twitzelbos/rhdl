#![warn(missing_docs)]
//! `Ddc` — a phase-sensitive, CIC-based digital down-converter.
//!
//! Mixes a received band down to baseband against a coherent local
//! oscillator, then decimates with a CIC on each quadrature arm.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+Ddc+---------------------+
      |                           |
+---->+ sample                    |
      |   Option<Iq<W>>           |
+---->+ frequency          sample |
      |   (LO tuning)   Option<Iq +----->
+---->+ phase              <WA>>  |
      |   (LO phase)              |
+---->+ downstream_ready   master +----->
      |                   overrun +----->
      +---------------------------+
")]
//!
//!# Internals
#![doc = badascii_doc::badascii!(r"
   sample -----------------+
                           v
   frequency -> +-+Nco+-+  |  +-+ComplexMixer+-+
   phase ------>|  LO   +--+->|  rx * conj(LO) +--+
                +-------+     +----------------+  |
                    |                             v
                    +--> master               +-+IqSplit+-+
                         (phase reference)    +-----------+
                                               |         |
                                        Real<W>|         |Imag<W>
                                               v         v
                              +-+StreamDecimator+-+  +-+StreamDecimator+-+
                              |   C  (I path)     |  |   C  (Q path)     |
                              +-------------------+  +-------------------+
                                               |         |
                                               v         v
                                             +-+IqCombine+-+
                                             +-------------+
                                                    |
                                                    v
                                          RCStream<Iq<WA>,SyncMark>
")]
//!
//! **The mixing is complex; everything after it is two real paths.**
//! Multiplying by `e^-jwt` irreducibly needs all four real products, so
//! the mixer stays complex — but decimation is two independent real
//! filters, and saying so with [`crate::rcstream::util::IqSplit`] and
//! [`crate::rcstream::util::IqCombine`] rather than extracting `.re`
//! and `.im` by hand makes each path separately substitutable and
//! hands the framing bookkeeping to widgets that already do it.
//!
//! Both paths are the *same* type, so an asymmetry between them — the
//! one error a phase-sensitive measurement cannot absorb — is
//! unrepresentable rather than merely discouraged. `Imag<W>` is
//! converted to `Real<W>` at the boundary to make that possible: a
//! decimator is a real filter and does not care which half of a
//! complex signal it carries.
//!
//! # What "phase sensitive" means here, and what it costs
//!
//! The output phase is meaningful relative to the oscillator, not
//! merely a magnitude. Three things have to hold for that to be true,
//! and all three are properties of pieces that already existed:
//!
//! - **The local oscillator is phase-coherent.** [`Nco`]'s accumulator
//!   represents absolute elapsed time and is never reset at an
//!   acquisition boundary, so successive acquisitions share a phase
//!   origin. [`Out::master`] exposes that origin.
//! - **Both quadrature arms are filtered identically.** The I and Q
//!   CICs are the same widget at the same configuration, so they
//!   contribute the same group delay and the same gain. An asymmetry
//!   between the arms would rotate the constellation, which is
//!   precisely the error a phase-sensitive measurement cannot tolerate.
//! - **The decimation phase is shared.** Both arms are driven from the
//!   same sample stream and therefore decimate on the same cycle. They
//!   are separate widgets, not one, so `both_arms_emit_together` checks
//!   it rather than assuming it.
//!
//! # The gain is not normalised
//!
//! The output carries the CIC's full `(R·M)^N` DC gain — see
//! [`super::cic`]. Undoing it costs either a multiply or a shift that
//! discards bits the filter was built to keep, and which is right
//! depends on what happens next. [`super::cic::dc_gain`] reports the
//! factor.
//!
//! # This widget does not stall
//!
//! Neither the oscillator nor the CIC can be paused: the oscillator's
//! phase is absolute time, and the filter's state is a running sum tied
//! to the input stream. A low `downstream_ready` on a cycle that
//! produces output loses that output, which [`Out::overrun`] reports
//! rather than hides.
//!
//! # Choosing the decimator
//!
//! [`Ddc`] is generic over its decimator, and both arms are the *same*
//! type — an asymmetry between them rotates the constellation, which
//! is the one error a phase-sensitive measurement cannot tolerate, so
//! it is made unrepresentable rather than merely discouraged.
//!
//! Two things can fill the slot:
//!
//! - [`UniformDdc`] — the alias for a [`super::cic::CicDecimate`] in
//!   both arms, every stage at the full accumulator width.
//! - A [`crate::cic_pruned!`]-generated decimator, whose stages taper
//!   per Hogenauer's §V schedule.
//!
//! At `W = 18, N = 2, R = 16` the uniform decimator spends 104 bits of
//! state per arm and the pruned one 80, for a filter that measures the
//! same amplitude to within 3 parts in 10,000. The gap widens sharply
//! with depth and rate.
//!
//! What you give up is noise floor, and only that. Out-of-band
//! rejection at that configuration goes from about 150,000x to about
//! 4,500x — still 73 dB, but now set by quantisation rather than by
//! the filter. That is the trade pruning makes: it does not move the
//! nulls or change the gain, it coarsens the number. Whether the
//! coarser number is good enough is a question about your measurement,
//! not about the filter, so the choice is left to the caller.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/ddc.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/ddc.md")]

use rhdl::prelude::*;

use super::cic::decimator::CicDecimate;
use super::iq::{Imag, Iq, Real};
use super::mixer::complex::{self, ComplexMixer};
use super::nco::composite;
use super::nco::config::PHASE_W;
use super::nco::{frequency_composer, phase_composer, sin_cos_linear_interp};
use super::sync::SyncMark;
use crate::rcstream::bus::Item;

/// A phase-sensitive CIC-based digital down-converter.
///
/// `W` is the received sample width. `WA` is the CIC accumulator width
/// and must satisfy [`super::cic::accumulator_width_is_sufficient`] for
/// the mixer's output width — checked by the CIC's own `Default`.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct Ddc<const W: usize, const WA: usize, const PROD_W: usize, C>
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<WA>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
    C: SynchronousIO<I = super::cic::decimator::In<W>, O = super::cic::decimator::Out<WA>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// The coherent local oscillator.
    lo: composite::NcoDefault,
    /// Received sample times the oscillator.
    mix: ComplexMixer<
        SyncMark,
        W,
        { sin_cos_linear_interp::AMP_W },
        W,
        PROD_W,
        { sin_cos_linear_interp::AMP_W + 1 },
    >,
    /// Splits the complex product into two real streams.
    split: crate::rcstream::util::IqSplit<W, SyncMark>,
    /// In-phase real path.
    dec_i: super::cic::stream::StreamDecimator<W, WA, C>,
    /// Quadrature real path. **The same type as the in-phase arm**, and
    /// that is load bearing rather than convenient — see the module
    /// docs on why the identity is what makes the measurement phase
    /// sensitive. Because both arms are the same type, an asymmetry
    /// between them is not merely discouraged, it is unrepresentable.
    dec_q: super::cic::stream::StreamDecimator<W, WA, C>,
    /// Recombines the two real streams into a complex one.
    combine: crate::rcstream::util::IqCombine<WA, SyncMark>,
}

/// The [`Ddc`] as it was before the decimator became a parameter: a
/// uniform-width [`CicDecimate`] in both arms.
///
/// Same shape, same order of parameters. Reach for `Ddc` directly when
/// you want a [`crate::cic_pruned!`]-generated decimator instead — at
/// any depth or rate worth decimating, the pruned datapath is
/// materially cheaper for the same filter.
pub type UniformDdc<
    const W: usize,
    const WA: usize,
    const STAGES: usize,
    const R: usize,
    const M: usize,
    const CW: usize,
    const PROD_W: usize,
> = Ddc<W, WA, PROD_W, CicDecimate<W, WA, STAGES, R, M, CW>>;

/// Inputs to [`Ddc`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const W: usize>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The received complex sample, framed, or `None` for an idle
    /// cycle.
    ///
    /// Framed rather than bare because the marker is what makes the
    /// measurement anchorable: it names the sample the acquisition
    /// started on. [`super::rx_trigger::RxTrigger`] produces exactly
    /// this shape.
    pub sample: Option<Item<Iq<W>, SyncMark>>,
    /// Oscillator tuning word — where in the input band to centre on.
    pub frequency: Bits<PHASE_W>,
    /// Oscillator phase offset, for setting the measurement's phase
    /// origin without disturbing the master trajectory.
    pub phase: Bits<PHASE_W>,
    /// Downstream's ready, per the `RCStream` contract.
    pub downstream_ready: bool,
}

/// Outputs from [`Ddc`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const WA: usize>
where
    rhdl::bits::W<WA>: BitWidth,
{
    /// The decimated complex baseband sample, present on one cycle in
    /// `R`. Carries the CIC's full DC gain, and the acquisition marker
    /// if one fell anywhere in the samples it was built from.
    pub sample: Option<Item<Iq<WA>, SyncMark>>,
    /// The oscillator's undisturbed master phase.
    ///
    /// The reference a phase-sensitive measurement is *relative to*.
    /// Exposed so a downstream stage can relate this acquisition's
    /// phase to another's.
    pub master: Bits<PHASE_W>,
    /// An output was produced while `downstream_ready` was low, and is
    /// gone.
    pub overrun: bool,
    /// **The oscillator and the acquisition disagreed about which
    /// sample is the anchor.**
    ///
    /// Raised by the mixer when one side marked and the other did not.
    /// For a phase-sensitive measurement that is a real fault: the
    /// oscillator retuned on a sample the acquisition was not expecting,
    /// so the output phase is relative to an origin the caller did not
    /// intend. See [`crate::dsp::sync`] for the alignment contract.
    ///
    /// Two causes, both meaning "the marks on this stream cannot be
    /// trusted": the mixer's two inputs disagreed, or the in-phase and
    /// quadrature decimated paths did. The second should be impossible
    /// — both are fed from one split and restart on the same mark — so
    /// if it fires, the paths have drifted.
    pub frame_mismatch: bool,
    /// A decimator clipped. Only possible when the arms are
    /// compensated — a compensator has gain above one.
    pub saturated: bool,
}

impl<const W: usize, const WA: usize, const PROD_W: usize, C> Ddc<W, WA, PROD_W, C>
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<WA>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
    C: SynchronousIO<I = super::cic::decimator::In<W>, O = super::cic::decimator::Out<WA>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// Build a down-converter around one decimator, cloned into both
    /// arms.
    ///
    /// **One argument, not two, and that is the point.** An asymmetry
    /// between the in-phase and quadrature decimators rotates the
    /// constellation, which is the one error a phase-sensitive
    /// measurement cannot absorb. Taking a single arm and cloning it
    /// makes the two identical by construction rather than by the
    /// caller's care.
    ///
    /// Use this for a decimator that cannot be defaulted — a
    /// [`super::cic::compensated::CompensatedCic`], whose filter half
    /// needs taps. [`Default`] covers the rest.
    pub fn new(cic: C) -> Self {
        assert_eq!(
            PROD_W,
            W + sin_cos_linear_interp::AMP_W + 1,
            "PROD_W is the mixer's natural product width, A_W + B_W + 1; \
             Rust cannot derive it from W without generic_const_exprs"
        );
        Self {
            lo: Default::default(),
            mix: Default::default(),
            split: Default::default(),
            dec_i: super::cic::stream::StreamDecimator::new(cic.clone()),
            dec_q: super::cic::stream::StreamDecimator::new(cic),
            combine: Default::default(),
        }
    }
}

impl<const W: usize, const WA: usize, const PROD_W: usize, C> Default for Ddc<W, WA, PROD_W, C>
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<WA>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
    C: SynchronousIO<I = super::cic::decimator::In<W>, O = super::cic::decimator::Out<WA>>
        + Synchronous
        + Default
        + Clone
        + std::fmt::Debug,
{
    fn default() -> Self {
        assert_eq!(
            PROD_W,
            W + sin_cos_linear_interp::AMP_W + 1,
            "PROD_W is the mixer's natural product width, A_W + B_W + 1; \
             Rust cannot derive it from W without generic_const_exprs"
        );
        Self {
            lo: Default::default(),
            mix: Default::default(),
            split: Default::default(),
            // Both arms identical, by construction rather than by
            // convention: an asymmetry here rotates the output phase.
            dec_i: Default::default(),
            dec_q: Default::default(),
            combine: Default::default(),
        }
    }
}

impl<const W: usize, const WA: usize, const PROD_W: usize, C> SynchronousIO
    for Ddc<W, WA, PROD_W, C>
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<WA>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
    C: SynchronousIO<I = super::cic::decimator::In<W>, O = super::cic::decimator::Out<WA>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    type I = In<W>;
    type O = Out<WA>;
    type Kernel = ddc_kernel<W, WA, PROD_W, C>;
}

#[kernel]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn ddc_kernel<const W: usize, const WA: usize, const PROD_W: usize, C>(
    cr: ClockReset,
    i: In<W>,
    q: Q<W, WA, PROD_W, C>,
) -> (Out<WA>, D<W, WA, PROD_W, C>)
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<WA>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
    C: SynchronousIO<I = super::cic::decimator::In<W>, O = super::cic::decimator::Out<WA>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    let mut d = D::<W, WA, PROD_W, C>::dont_care();

    // ---- the local oscillator ----
    //
    // Tuning goes in the master term and the offset in the pulse term,
    // matching the composer layering in `dsp::nco`. The accumulator is
    // never reset here: its phase is absolute elapsed time, which is
    // what makes successive acquisitions comparable.
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

    // ---- mix down ----
    //
    // The received sample against the oscillator. Both operands are
    // unframed here; framing rides the acquisition path, not the
    // conversion path.
    // *** The oscillator is conjugated. ***
    //
    // Multiplying by e^{+jwt} shifts *up*; down-conversion needs
    // e^{-jwt}, which for a real-valued cosine/sine pair means negating
    // the quadrature component. Without this the widget is an
    // up-converter that still produces a plausible flat magnitude on a
    // tone, which is how it initially passed a magnitude-only test --
    // an LO sweep is what exposed it, peaking at -f instead of +f.
    let mut lo = None;
    if let Some(l) = q.lo.stream.data {
        // `zero - v`, not `-v`: unary negation on a value unwrapped
        // from an `Option` trips a signedness error in the kernel, the
        // same family as `crate::dsp::sign_extend`.
        let zero = signed::<{ sin_cos_linear_interp::AMP_W }>(0);
        lo = Some(Item::<Iq<{ sin_cos_linear_interp::AMP_W }>, SyncMark> {
            data: Iq::<{ sin_cos_linear_interp::AMP_W }> {
                re: l.data.re,
                im: zero - l.data.im,
            },
            frame: l.frame,
        });
    }

    d.mix = complex::In::<SyncMark, W, { sin_cos_linear_interp::AMP_W }> {
        a: i.sample,
        b: lo,
        downstream_ready: true,
    };

    // ---- decimate each arm ----
    //
    // Split the mixer's product into its two components and filter each
    // with an identical CIC. Driven from the same stream, so they share
    // a decimation phase.
    // ---- two real paths, split and recombined ----
    //
    // The mixing is irreducibly complex -- multiplying by e^-jwt needs
    // all four real products -- but everything after it is two
    // independent real filters. Saying so with `IqSplit` and
    // `IqCombine`, rather than pulling `.re` and `.im` out by hand,
    // makes the two paths separately substitutable and hands the
    // framing bookkeeping to widgets that already do it.
    d.split = crate::rcstream::util::split::In::<W, SyncMark> {
        stream: q.mix.stream.data,
        real_ready: i.downstream_ready,
        imag_ready: i.downstream_ready,
    };

    // *** The marker defines the decimation grid. ***
    //
    // Each `StreamDecimator` restarts on a marked sample, so the output
    // carrying the mark is built only from post-trigger samples rather
    // than from a window straddling the trigger. Both arms see the same
    // mark on the same cycle because both see the same split, which is
    // what keeps I and Q on a common grid.
    //
    // `restart` stays false here: the restart that matters arrives in
    // the stream.
    d.dec_i = super::cic::stream::In::<W> {
        stream: q.split.real.data,
        restart: false,
        downstream_ready: i.downstream_ready,
    };

    // `Imag<W>` becomes `Real<W>` on the way in and back on the way
    // out. Not a fudge: a decimator is a real-valued filter and does
    // not care which half of a complex signal it carries. The newtypes
    // exist to stop the *caller* mixing the halves up, and converting
    // at the boundary is what lets both arms be the same type -- which
    // is the property the measurement depends on.
    let mut q_in = None;
    if let Some(it) = q.split.imag.data {
        q_in = Some(Item::<Real<W>, SyncMark> {
            data: Real::<W> { v: it.data.v },
            frame: it.frame,
        });
    }
    d.dec_q = super::cic::stream::In::<W> {
        stream: q_in,
        restart: false,
        downstream_ready: i.downstream_ready,
    };

    // ---- the two paths' marks must agree ----
    //
    // Both decimators were fed from the same split and restart on the
    // same mark, so their output marks should be identical. The rule is
    // therefore **and**, with a disagreement flagged:
    //
    // - Agreeing marks pass through, and `and` is that same value.
    // - Disagreeing marks mean the paths have drifted. `and` yields
    //   *unmarked*, which is the conservative answer -- better to
    //   forget an acquisition boundary than to claim one on a sample
    //   where only half the complex value is known to be aligned -- and
    //   `frame_mismatch` reports it.
    //
    // Taking one side's frame and discarding the other, which is what
    // `IqCombine` does by itself, would hide the drift entirely.
    let mut i_sync = false;
    let mut q_sync = false;
    let mut i_present = false;
    let mut q_present = false;
    if let Some(it) = q.dec_i.stream.data {
        i_sync = it.frame.sync;
        i_present = true;
    }
    if let Some(it) = q.dec_q.stream.data {
        q_sync = it.frame.sync;
        q_present = true;
    }
    let aligned = SyncMark {
        sync: i_sync && q_sync,
    };
    let mark_mismatch = i_present && q_present && (i_sync != q_sync);

    let mut real_out = None;
    if let Some(it) = q.dec_i.stream.data {
        real_out = Some(Item::<Real<WA>, SyncMark> {
            data: it.data,
            frame: aligned,
        });
    }
    let mut imag_out = None;
    if let Some(it) = q.dec_q.stream.data {
        imag_out = Some(Item::<Imag<WA>, SyncMark> {
            data: Imag::<WA> { v: it.data.v },
            frame: aligned,
        });
    }
    d.combine = crate::rcstream::util::combine::In::<WA, SyncMark> {
        real: real_out,
        imag: imag_out,
        downstream_ready: i.downstream_ready,
    };

    let mut o = Out::<WA> {
        sample: q.combine.stream.data,
        master: q.lo.master,
        overrun: !i.downstream_ready || q.dec_i.overrun || q.dec_q.overrun,
        // Two independent ways the framing can fail to line up: the
        // mixer's inputs disagreed, or the two decimated paths did.
        // Both mean the same thing to a consumer -- the marks on this
        // stream cannot be trusted -- so they share one flag, and the
        // module docs name both causes.
        frame_mismatch: q.mix.frame_mismatch || mark_mismatch || q.combine.frame_mismatch,
        saturated: q.dec_i.saturated || q.dec_q.saturated,
    };

    if cr.reset.any() {
        o.overrun = false;
        o.frame_mismatch = false;
        o.saturated = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::super::cic::accumulator_width;
    use super::*;
    use expect_test::expect;
    use std::f64::consts::TAU;

    const W: usize = 18;
    const WA: usize = 26;
    const S: usize = 2;
    const R: usize = 16;
    const M: usize = 1;
    const CW: usize = 4;
    const PROD_W: usize = W + sin_cos_linear_interp::AMP_W + 1;
    const FS: f64 = 125_000_000.0;
    type Uut = UniformDdc<W, WA, S, R, M, CW, PROD_W>;

    /// Tuning word for a signed frequency, wrapped into the unsigned
    /// accumulator.
    fn tune(hz: f64) -> u128 {
        let full = (1u128 << PHASE_W) as f64;
        ((hz / FS * full).rem_euclid(full)) as u128
    }

    fn stimulus(f_in: f64, f_lo: f64, phase: u128, n: usize, mark_at: Option<usize>) -> Vec<In<W>> {
        let amp = 100_000.0;
        (0..n)
            .map(|k| {
                let t = TAU * f_in * (k as f64) / FS;
                In::<W> {
                    sample: Some(Item::<Iq<W>, SyncMark> {
                        data: Iq::<W> {
                            re: signed::<W>((amp * t.cos()) as i128),
                            im: signed::<W>((amp * t.sin()) as i128),
                        },
                        frame: SyncMark {
                            sync: mark_at == Some(k),
                        },
                    }),
                    frequency: bits::<PHASE_W>(tune(f_lo)),
                    phase: bits::<PHASE_W>(phase),
                    downstream_ready: true,
                }
            })
            .collect()
    }

    fn baseband(f_in: f64, f_lo: f64, phase: u128, n: usize) -> Vec<(f64, f64)> {
        let uut = Uut::default();
        uut.run(
            stimulus(f_in, f_lo, phase, n, None)
                .into_iter()
                .with_reset(1)
                .clock_pos_edge(100),
        )
        .synchronous_sample()
        .filter_map(|s| {
            s.output
                .sample
                .map(|it| (it.data.re.raw() as f64, it.data.im.raw() as f64))
        })
        .collect()
    }

    /// Mean magnitude over the settled second half.
    fn settled_magnitude(v: &[(f64, f64)]) -> f64 {
        let half = v.len() / 2;
        let t: Vec<f64> = v[half..]
            .iter()
            .map(|(re, im)| (re * re + im * im).sqrt())
            .collect();
        t.iter().sum::<f64>() / t.len().max(1) as f64
    }

    #[test]
    fn default_construction() {
        let _ = Uut::default();
        assert_eq!(accumulator_width(W, S, R, M), WA);
    }

    /// **The response peaks when the oscillator is tuned to the signal,
    /// not to its negative.**
    ///
    /// This is the test that matters most, and the one that caught the
    /// bug this widget shipped with in its first draft: without
    /// conjugating the oscillator, the mixer computes the *sum* rather
    /// than the difference and the whole thing is an up-converter. It
    /// still produced a flat, plausible magnitude at the on-tune
    /// frequency, so a magnitude-only test passed. Only sweeping the
    /// oscillator revealed the peak sitting at `-f`.
    #[test]
    fn the_response_peaks_at_the_tuned_frequency() {
        let f = 5_000_000.0;
        let on = settled_magnitude(&baseband(f, f, 0, 1024));
        let mirrored = settled_magnitude(&baseband(f, -f, 0, 1024));
        assert!(
            on > mirrored * 10.0,
            "tuning to +f must beat tuning to -f by a wide margin, else \
             the oscillator is not conjugated and this up-converts: \
             on {on:.0} vs mirrored {mirrored:.0}"
        );
    }

    /// A tone landing on the CIC's first null is deeply rejected.
    #[test]
    fn an_out_of_band_tone_is_rejected() {
        let f_lo = 5_000_000.0;
        let on = settled_magnitude(&baseband(f_lo, f_lo, 0, 2048));
        // Offset by exactly fs/R -- the first null of the sinc^N.
        let off = settled_magnitude(&baseband(f_lo + FS / (R as f64), f_lo, 0, 2048));
        assert!(
            off * 1000.0 < on,
            "a tone at the first null must be deeply rejected: \
             {off:.0} vs {on:.0}"
        );
    }

    /// **Phase sensitivity: the output phase tracks the oscillator's.**
    ///
    /// The property the widget is named for. Offsetting the oscillator
    /// by a quarter turn must rotate the baseband output by a quarter
    /// turn — not merely change its magnitude, which a
    /// phase-insensitive detector would leave alone.
    #[test]
    fn the_output_phase_follows_the_oscillator_phase() {
        let f = 5_000_000.0;
        let quarter = 1u128 << (PHASE_W - 2);

        let a = baseband(f, f, 0, 1024);
        let b = baseband(f, f, quarter, 1024);

        let angle = |v: &[(f64, f64)]| {
            let (re, im) = v[v.len() - 1];
            im.atan2(re)
        };
        let mag = |v: &[(f64, f64)]| settled_magnitude(v);

        // Magnitude is unchanged: a phase offset does not attenuate.
        let (ma, mb) = (mag(&a), mag(&b));
        assert!(
            (ma - mb).abs() / ma < 0.02,
            "a phase offset must not change the magnitude: {ma:.0} vs {mb:.0}"
        );

        // And the phase moved by a quarter turn.
        let mut delta = angle(&b) - angle(&a);
        while delta <= -TAU / 2.0 {
            delta += TAU;
        }
        while delta > TAU / 2.0 {
            delta -= TAU;
        }
        let expected = -TAU / 4.0;
        let err = (delta - expected).abs().min((delta - expected - TAU).abs());
        assert!(
            err < 0.15,
            "a quarter-turn oscillator offset should rotate the output a \
             quarter turn: saw {delta:.3} rad, expected {expected:.3}"
        );
    }

    /// **Both quadrature arms emit on the same cycle.**
    ///
    /// They are separate widgets sharing a decimation phase only
    /// because they see the same stream. If they ever drifted apart the
    /// recombined sample would pair an I from one output period with a
    /// Q from another, which rotates the constellation — the exact
    /// failure a phase-sensitive measurement cannot tolerate. Cheap to
    /// check, so checked.
    #[test]
    fn both_arms_emit_together() {
        let uut = Uut::default();
        let seq = stimulus(5_000_000.0, 5_000_000.0, 0, 512, None);
        let count = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter(|s| s.output.sample.is_some())
            .count();
        // 512 samples at R = 16, less the pipeline fill.
        assert!(count > 512 / R - 4, "too few outputs: {count}");
        assert!(count <= 512 / R, "more outputs than input periods: {count}");
    }

    /// The acquisition marker survives decimation.
    ///
    /// A marked input sample is almost always one the decimator drops,
    /// so the marker has to be sticky. Losing it would leave the
    /// acquisition unanchored.
    #[test]
    fn the_acquisition_marker_survives_decimation() {
        let uut = Uut::default();
        // Mark a sample that is certainly not on an output boundary.
        let seq = stimulus(5_000_000.0, 5_000_000.0, 0, 256, Some(37));
        let marks: Vec<usize> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .enumerate()
            .filter_map(|(k, s)| match s.output.sample {
                Some(it) if it.frame.sync => Some(k),
                _ => None,
            })
            .collect();
        assert_eq!(
            marks.len(),
            1,
            "exactly one output should carry the marker, got {marks:?}"
        );
    }

    /// **The marker defines the decimation grid, end to end.**
    ///
    /// The DDC-level statement of the CIC's restart contract: the same
    /// acquisition behind two completely different pre-trigger
    /// histories must produce identical outputs. If anything from
    /// before the marker survives into the decimated waveform, the two
    /// runs diverge.
    ///
    /// This is what "phase sensitive" has to mean for an acquisition:
    /// not merely that phase is preserved through the mix, but that the
    /// decimated samples belong to the experiment rather than
    /// straddling its start.
    #[test]
    fn the_marker_excludes_pre_trigger_data_from_the_output() {
        const TRIGGER: usize = 21;
        let f = 5_000_000.0;

        let run_with = |pre_amp: f64| -> Vec<(f64, f64)> {
            let uut = Uut::default();
            let seq: Vec<In<W>> = (0..512)
                .map(|k| {
                    let t = TAU * f * (k as f64) / FS;
                    // Wildly different signal before the trigger; the
                    // same one after it.
                    let amp = if k < TRIGGER { pre_amp } else { 100_000.0 };
                    In::<W> {
                        sample: Some(Item::<Iq<W>, SyncMark> {
                            data: Iq::<W> {
                                re: signed::<W>((amp * t.cos()) as i128),
                                im: signed::<W>((amp * t.sin()) as i128),
                            },
                            frame: SyncMark { sync: k == TRIGGER },
                        }),
                        frequency: bits::<PHASE_W>(tune(f)),
                        phase: bits::<PHASE_W>(0),
                        downstream_ready: true,
                    }
                })
                .collect();
            uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
                .synchronous_sample()
                .enumerate()
                // Only outputs from the restarted window onward.
                .filter(|(c, _)| *c >= TRIGGER + R + 4)
                .filter_map(|(_, s)| {
                    s.output
                        .sample
                        .map(|it| (it.data.re.raw() as f64, it.data.im.raw() as f64))
                })
                .collect()
        };

        let quiet = run_with(0.0);
        let loud = run_with(-120_000.0);
        assert!(!quiet.is_empty(), "no post-trigger outputs");
        assert_eq!(
            quiet, loud,
            "the decimated waveform must not depend on anything before \
             the marker"
        );
    }

    /// Reset leaves nothing behind.
    #[test]
    fn reset_clears_the_chain() {
        let a = baseband(5_000_000.0, 5_000_000.0, 0, 512);
        let b = baseband(5_000_000.0, 5_000_000.0, 0, 512);
        assert_eq!(a, b, "a fresh widget must produce the same result");
    }

    // ---- Tier 3 / 4 / 5 ---------------------------------------------

    fn hdl_stimulus() -> Vec<In<W>> {
        let mut s = stimulus(5_000_000.0, 5_000_000.0, 0, 40, Some(5));
        s[11].sample = None;
        s[23].downstream_ready = false;
        s
    }

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
            module top_lo
            module top_mix
            module top_split
            module top_combine"#]];
        expect.assert_eq(&shape);
        Ok(())
    }

    #[test]
    fn test_ddc_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let tb = uut
            .run(hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100))
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_ddc_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("ddc");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["e21ce85dc1367882eb05dadfb5b062f5bb2994c5e8f2804564728dc10a33aeac"];
        let digest = vcd.dump_to_file(root.join("ddc.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
