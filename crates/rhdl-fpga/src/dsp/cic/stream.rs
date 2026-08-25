#![warn(missing_docs)]
//! `StreamDecimator` — a decimator on a framed real stream.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+StreamDecimator+--------------+
      |                                |
+---->+ stream                         |
      |  Option<Item<Real<WI>,SyncMark>|
      |                         stream |
      | RCStream<Real<WO>,SyncMark>    +----->
+---->+ downstream_ready               |
      |                        overrun |
+---->+ restart                        +----->
      |                      saturated |
      |                                +----->
      +--------------------------------+
")]
//!
//! [`super::decimator::CicDecimate`] and friends take a bare
//! `Option<SignedBits<W>>`: a sample, with no framing. That is the
//! right interface for a filter — framing is not the filter's
//! business — but it means a decimator cannot sit between
//! [`crate::rcstream::util::IqSplit`] and
//! [`crate::rcstream::util::IqCombine`], which speak in framed
//! [`Item`]s. This widget is the adapter, and the framing rule it
//! implements is the interesting part.
//!
//! # A decimator throws away most of the frames
//!
//! One output emerges per `R` inputs, so `R - 1` of every `R` frames
//! belong to samples nobody downstream ever sees. Passing through the
//! frame of whichever sample happened to land on the output boundary
//! would silently discard the rest — and for [`SyncMark`] the whole
//! point of the mark is that it identifies the start of an
//! acquisition, which is almost never the sample that survives
//! decimation.
//!
//! So the mark is **latched**: seen anywhere in the window, it rides
//! out on the next output and the latch clears. That makes the output
//! stream's marks mean "the acquisition began somewhere in the window
//! this sample summarises", which is the only statement the decimated
//! rate can support.
//!
//! # Why this is not generic over the framing type
//!
//! Because there would be nothing to implement. Reducing `R` frames to
//! one requires knowing how frames combine, and a `#[kernel]` has no
//! closures to be told. [`SyncMark`] has a definite answer — boolean
//! or — and a different framing type would have a different one. Being
//! specific here is honest; a generic version would have to pick a
//! rule and would be wrong for most `F`.
//!
//! # A marked sample also restarts the window
//!
//! Latching alone would put the mark on an output built from a window
//! straddling the trigger. The mark also restarts the decimator, so
//! the marked output is built only from post-trigger samples — see
//! [`super::decimator::In::restart`] for why clearing the state is
//! part of that and not optional.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/cic_stream.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/cic_stream.md")]

use rhdl::prelude::*;

use crate::core::dff;
use crate::dsp::iq::Real;
use crate::dsp::sync::SyncMark;
use crate::rcstream::{Item, RCStream};

/// A decimator adapted to a framed real stream.
///
/// `C` is any decimator presenting [`super::decimator`]'s interface —
/// plain [`super::CicDecimate`], a [`crate::cic_pruned!`]-generated
/// one, or a [`super::compensated::CompensatedCic`].
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct StreamDecimator<const W_IN: usize, const W_OUT: usize, C>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    C: SynchronousIO<I = super::decimator::In<W_IN>, O = super::decimator::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// The decimator proper.
    cic: C,
    /// A mark seen since the last output, waiting to ride out with it.
    marked: dff::DFF<bool>,
}

impl<const W_IN: usize, const W_OUT: usize, C> StreamDecimator<W_IN, W_OUT, C>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    C: SynchronousIO<I = super::decimator::In<W_IN>, O = super::decimator::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// Wrap a decimator.
    pub fn new(cic: C) -> Self {
        Self {
            cic,
            marked: dff::DFF::new(false),
        }
    }
}

impl<const W_IN: usize, const W_OUT: usize, C> Default for StreamDecimator<W_IN, W_OUT, C>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    C: SynchronousIO<I = super::decimator::In<W_IN>, O = super::decimator::Out<W_OUT>>
        + Synchronous
        + Default
        + Clone
        + std::fmt::Debug,
{
    fn default() -> Self {
        Self::new(C::default())
    }
}

/// Inputs to [`StreamDecimator`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const W_IN: usize>
where
    rhdl::bits::W<W_IN>: BitWidth,
{
    /// The framed input sample, or `None` for an idle cycle.
    pub stream: Option<Item<Real<W_IN>, SyncMark>>,
    /// Downstream's ready.
    pub downstream_ready: bool,
}

/// Outputs from [`StreamDecimator`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const W_OUT: usize>
where
    rhdl::bits::W<W_OUT>: BitWidth,
{
    /// The decimated, framed stream.
    pub stream: RCStream<Real<W_OUT>, SyncMark>,
    /// A sample was produced while `downstream_ready` was low.
    pub overrun: bool,
    /// The decimator clipped — only possible for a compensated one.
    pub saturated: bool,
}

impl<const W_IN: usize, const W_OUT: usize, C> SynchronousIO for StreamDecimator<W_IN, W_OUT, C>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    C: SynchronousIO<I = super::decimator::In<W_IN>, O = super::decimator::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    type I = In<W_IN>;
    type O = Out<W_OUT>;
    type Kernel = stream_decimator_kernel<W_IN, W_OUT, C>;
}

#[kernel]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn stream_decimator_kernel<const W_IN: usize, const W_OUT: usize, C>(
    cr: ClockReset,
    i: In<W_IN>,
    q: Q<W_IN, W_OUT, C>,
) -> (Out<W_OUT>, D<W_IN, W_OUT, C>)
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    C: SynchronousIO<I = super::decimator::In<W_IN>, O = super::decimator::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    let mut d = D::<W_IN, W_OUT, C>::dont_care();

    // Unwrap the framed sample.
    let mut sample = None;
    let mut marked_now = false;
    if let Some(it) = i.stream {
        sample = Some(it.data.v);
        marked_now = it.frame.sync;
    }

    // A marked sample restarts the window, so the output carrying the
    // mark is built only from post-trigger samples.
    d.cic = super::decimator::In::<W_IN> {
        sample,
        // **The mark is the only restart.** There is deliberately no
        // out-of-band restart input: widgets connect through the
        // stream and its framing, and a second mechanism for the same
        // thing is a second mechanism to keep consistent. A host that
        // wants to restart marks a sample.
        restart: marked_now,
        downstream_ready: i.downstream_ready,
    };

    // Latch the mark until an output carries it out. Seen anywhere in
    // the window, not just on the sample that survives -- see the
    // module docs.
    //
    // **Only a *carried* mark may ride out on this cycle's output.** A
    // mark arriving now restarts the window now, so the output emerging
    // now was registered from the previous cycle and belongs entirely
    // to the *old* window -- it is pre-trigger data. Attaching the new
    // mark to it would label a sample from before the trigger as the
    // start of the acquisition, which is precisely the error the
    // restart exists to prevent.
    //
    // So `carry` and not `carry || marked_now`. The distinction only
    // shows up when a mark lands one cycle after a window boundary,
    // which is one input in R -- rare enough to survive a test that
    // marks a single fixed offset, and wrong every time it happens.
    let carry = q.marked;
    let mut pending = carry || marked_now;

    let mut out_data = None;
    if let Some(v) = q.cic.sample {
        out_data = Some(Item::<Real<W_OUT>, SyncMark> {
            data: Real::<W_OUT> { v },
            frame: SyncMark { sync: carry },
        });
        if carry {
            // Consumed; a mark arriving this cycle stays pending.
            pending = marked_now;
        }
    }
    d.marked = pending;

    let mut o = Out::<W_OUT> {
        stream: RCStream::<Real<W_OUT>, SyncMark> {
            data: out_data,
            ready: i.downstream_ready,
        },
        overrun: q.cic.overrun,
        saturated: q.cic.saturated,
    };

    if cr.reset.any() {
        d.cic = super::decimator::In::<W_IN> {
            sample: None,
            restart: false,
            downstream_ready: false,
        };
        d.marked = false;
        o.stream = RCStream::<Real<W_OUT>, SyncMark> {
            data: None,
            ready: false,
        };
        o.overrun = false;
        o.saturated = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::super::{CicDecimate, accumulator_width, counter_width};
    use super::*;
    use expect_test::expect;

    const WI: usize = 12;
    const N: usize = 2;
    const R: usize = 4;
    const M: usize = 1;
    const WA: usize = accumulator_width(WI, N, R, M);
    const CW: usize = counter_width(R);
    type Cic = CicDecimate<WI, WA, N, R, M, CW>;
    type Uut = StreamDecimator<WI, WA, Cic>;

    fn item(v: i128, sync: bool) -> In<WI> {
        In::<WI> {
            stream: Some(Item::<Real<WI>, SyncMark> {
                data: Real::<WI> { v: signed::<WI>(v) },
                frame: SyncMark { sync },
            }),
            downstream_ready: true,
        }
    }

    fn idle() -> In<WI> {
        In::<WI> {
            stream: None,
            downstream_ready: true,
        }
    }

    /// Run and return `(value, sync)` for each emitted sample.
    fn run(seq: Vec<In<WI>>) -> Vec<(i128, bool)> {
        Uut::default()
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| {
                s.output
                    .stream
                    .data
                    .map(|it| (it.data.v.raw(), it.frame.sync))
            })
            .collect()
    }

    #[test]
    fn it_decimates_by_r() {
        let n = 40;
        let seq: Vec<In<WI>> = (0..n).map(|k| item((k % 7) as i128, false)).collect();
        let out = run(seq);
        // One output per R inputs, give or take the registered tail.
        assert!(
            out.len() == n / R || out.len() == n / R - 1,
            "expected about {} outputs, got {}",
            n / R,
            out.len()
        );
    }

    /// **The framing rule.** A mark anywhere in the window rides out on
    /// the next output, not only when it lands on the surviving sample.
    #[test]
    fn a_mark_anywhere_in_the_window_reaches_the_output() {
        // Place the mark at every offset within a window and check it
        // always emerges exactly once. This is the property the whole
        // widget exists for: at R = 4, three of every four marks would
        // be lost by passing through the surviving sample's frame.
        for offset in 0..R {
            let mut seq: Vec<In<WI>> = Vec::new();
            // A clean run-up, then a marked sample at `offset`.
            for _ in 0..(4 * R) {
                seq.push(item(10, false));
            }
            for k in 0..(4 * R) {
                seq.push(item(10, k == offset));
            }
            seq.extend(std::iter::repeat_n(idle(), 4));
            let out = run(seq);
            let marks = out.iter().filter(|(_, s)| *s).count();
            assert_eq!(
                marks, 1,
                "offset {offset}: expected exactly one marked output, got {marks}"
            );
        }
    }

    #[test]
    fn an_unmarked_stream_produces_no_marks() {
        let seq: Vec<In<WI>> = (0..40).map(|_| item(7, false)).collect();
        let out = run(seq);
        assert!(!out.is_empty());
        assert!(
            out.iter().all(|(_, s)| !*s),
            "no mark went in, so none may come out"
        );
    }

    /// A mark restarts the window, so the marked output is built only
    /// from post-trigger samples.
    #[test]
    fn the_marked_output_excludes_pre_trigger_data() {
        // Two different pre-trigger histories, identical afterwards.
        // The marked output and everything after it must agree.
        let after: Vec<i128> = (0..(R as i128 * 6)).map(|k| (k * 13) % 41 - 20).collect();
        let go = |hist: i128| -> Vec<(i128, bool)> {
            let mut seq: Vec<In<WI>> = (0..(R as i128 * 5))
                .map(|k| item((k * hist) % 61 - 30, false))
                .collect();
            for (n, v) in after.iter().enumerate() {
                seq.push(item(*v, n == 0));
            }
            seq.extend(std::iter::repeat_n(idle(), 4));
            run(seq)
        };
        let a = go(3);
        let b = go(29);
        let mark_a = a.iter().position(|(_, s)| *s).expect("a mark in a");
        let mark_b = b.iter().position(|(_, s)| *s).expect("a mark in b");
        assert_eq!(
            &a[mark_a..],
            &b[mark_b..],
            "post-trigger must not depend on history"
        );
        assert_ne!(a, b, "the histories must differ, or this proves nothing");
    }

    #[test]
    fn an_idle_cycle_holds_the_filter() {
        let x: Vec<i128> = (0..24).map(|k| (k * 11) % 37 - 18).collect();
        // Both runs need the same drain: the output is registered, so a
        // longer run lets one more sample emerge. That is a drain
        // artifact, not a behavioural difference, and comparing without
        // it fails for the wrong reason.
        let mut dense: Vec<In<WI>> = x.iter().map(|v| item(*v, false)).collect();
        dense.extend(std::iter::repeat_n(idle(), 4));
        let mut sparse: Vec<In<WI>> = Vec::new();
        for v in &x {
            sparse.push(item(*v, false));
            sparse.push(idle());
        }
        sparse.extend(std::iter::repeat_n(idle(), 4));
        assert_eq!(run(dense), run(sparse), "a gap must not advance the filter");
    }

    #[test]
    fn reset_does_not_duplicate_or_lose_the_mark() {
        // `with_reset` holds the first input through the reset cycles,
        // so a marked first sample is presented during reset *and*
        // after it. The latch is cleared on reset, so the mark must
        // emerge exactly once -- not twice from being latched on both
        // sides, and not zero times from being cleared after arrival.
        let mut seq: Vec<In<WI>> = vec![item(5, true)];
        seq.extend((0..40).map(|_| item(5, false)));
        seq.extend(std::iter::repeat_n(idle(), 4));
        let out = Uut::default()
            .run(seq.into_iter().with_reset(4).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.stream.data.map(|it| it.frame.sync))
            .collect::<Vec<_>>();
        assert!(!out.is_empty());
        assert_eq!(
            out.iter().filter(|s| **s).count(),
            1,
            "exactly one marked output: {out:?}"
        );
    }

    // ---- Tier 3 / 4 / 5 --------------------------------------------

    fn hdl_stimulus() -> Vec<In<WI>> {
        let mut v: Vec<In<WI>> = (0..32)
            .map(|k: i128| item((k * 17) % 61 - 30, k == 5))
            .collect();
        v[9].downstream_ready = false;
        v.extend(std::iter::repeat_n(idle(), 3));
        v
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let hdl = Uut::default()
            .descriptor("top".into())?
            .hdl()?
            .modules
            .pretty();
        assert!(hdl.contains("module top"), "no top module");
        Ok(())
    }

    #[test]
    fn test_stream_decimator_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let tb = uut
            .run(hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100))
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_stream_decimator_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let vcd = uut
            .run(hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100))
            .collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("cic_stream");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["be7bb4c1db08658cea162c4fa5697d616517aca05e685e93fee7af2f6f905d98"];
        let digest = vcd.dump_to_file(root.join("cic_stream.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
