#![warn(missing_docs)]
//! `CascadedDecimator` — two framed decimators in series.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+CascadedDecimator+------------+
      |                                |
+---->+ stream                         |
      |  Option<Item<Real<WI>,SyncMark>|
      |                         stream |
      |  RCStream<Real<WO>,SyncMark>   +----->
+---->+ downstream_ready               |
      |                        overrun |
      |                                +----->
      |                      saturated |
      |                                +----->
      +--------------------------------+
")]
//!
//! # There is almost nothing here, and that is the point
//!
//! Splitting a large decimation across two stages is what makes it
//! affordable — a single `/488` needs 66-bit accumulators at the full
//! converter rate, where an `8 × 61` split keeps nothing wider than 16
//! bits fast. [`super::chain`] searches those splits. This widget
//! composes the result, and its entire kernel is:
//!
//! ```text
//!   first  <- the incoming stream
//!   second <- whatever first emitted
//!   out    <- whatever second emitted
//! ```
//!
//! No restart wiring, no latch, no state of its own.
//!
//! # Why it needs no restart logic
//!
//! Because **the second stage has no idea there is anything in front of
//! it, and does not need one.** A [`super::stream::StreamDecimator`]
//! restarts its window when its own input carries a
//! [`crate::dsp::sync::SyncMark`], and emits a marked sample when that
//! window completes. So the first stage's marked output *is* the second
//! stage's restart, arriving by the only channel widgets share: the
//! stream and its framing.
//!
//! An earlier version of this widget composed the *bare*
//! [`super::decimator`] primitives and wired their internal `restart`
//! signal between them. That forced this widget to work out which of
//! the upstream outputs was the first post-restart one — and get it
//! wrong for one restart in `R1`, because on the cycle a restart
//! arrives the upstream output register still holds the *previous*
//! window's sample. Passing framing instead of internals deletes the
//! problem rather than fixing it, and the widget shrank to wiring.
//!
//! The general rule, worth remembering the next time two widgets need
//! to agree about something: **an internal is not an interface.** If
//! two widgets have to coordinate, the coordination belongs in the
//! framing they already exchange.
//!
//! # It is a decimator too
//!
//! Same `In` and `Out` as [`super::stream::StreamDecimator`], so a
//! cascade *is* a framed decimator — it nests, and it drops into
//! anything that takes one.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/cic_cascaded.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/cic_cascaded.md")]

use rhdl::prelude::*;

use super::stream;
use crate::dsp::iq::Real;
use crate::dsp::sync::SyncMark;
use crate::rcstream::RCStream;

/// Two framed decimators in series, presenting one.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct CascadedDecimator<const W_IN: usize, const W_MID: usize, const W_OUT: usize, A, B>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    A: SynchronousIO<I = stream::In<W_IN>, O = stream::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    B: SynchronousIO<I = stream::In<W_MID>, O = stream::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// The fast stage, at the input rate.
    first: A,
    /// The slow stage, at the first stage's output rate.
    second: B,
}

impl<const W_IN: usize, const W_MID: usize, const W_OUT: usize, A, B>
    CascadedDecimator<W_IN, W_MID, W_OUT, A, B>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    A: SynchronousIO<I = stream::In<W_IN>, O = stream::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    B: SynchronousIO<I = stream::In<W_MID>, O = stream::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// Compose two framed decimators.
    pub fn new(first: A, second: B) -> Self {
        Self { first, second }
    }
}

impl<const W_IN: usize, const W_MID: usize, const W_OUT: usize, A, B> Default
    for CascadedDecimator<W_IN, W_MID, W_OUT, A, B>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    A: SynchronousIO<I = stream::In<W_IN>, O = stream::Out<W_MID>>
        + Synchronous
        + Default
        + Clone
        + std::fmt::Debug,
    B: SynchronousIO<I = stream::In<W_MID>, O = stream::Out<W_OUT>>
        + Synchronous
        + Default
        + Clone
        + std::fmt::Debug,
{
    fn default() -> Self {
        Self::new(A::default(), B::default())
    }
}

impl<const W_IN: usize, const W_MID: usize, const W_OUT: usize, A, B> SynchronousIO
    for CascadedDecimator<W_IN, W_MID, W_OUT, A, B>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    A: SynchronousIO<I = stream::In<W_IN>, O = stream::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    B: SynchronousIO<I = stream::In<W_MID>, O = stream::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    type I = stream::In<W_IN>;
    type O = stream::Out<W_OUT>;
    type Kernel = cascaded_decimator_kernel<W_IN, W_MID, W_OUT, A, B>;
}

#[kernel]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn cascaded_decimator_kernel<const W_IN: usize, const W_MID: usize, const W_OUT: usize, A, B>(
    cr: ClockReset,
    i: stream::In<W_IN>,
    q: Q<W_IN, W_MID, W_OUT, A, B>,
) -> (stream::Out<W_OUT>, D<W_IN, W_MID, W_OUT, A, B>)
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    A: SynchronousIO<I = stream::In<W_IN>, O = stream::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    B: SynchronousIO<I = stream::In<W_MID>, O = stream::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    let mut d = D::<W_IN, W_MID, W_OUT, A, B>::dont_care();

    d.first = stream::In::<W_IN> {
        stream: i.stream,
        downstream_ready: i.downstream_ready,
    };

    // The whole composition. The second stage restarts because the
    // first stage's output carries a mark -- nothing here arranges
    // that, and nothing here needs to know the first stage exists.
    d.second = stream::In::<W_MID> {
        stream: q.first.stream.data,
        downstream_ready: i.downstream_ready,
    };

    let mut o = stream::Out::<W_OUT> {
        stream: q.second.stream,
        overrun: q.first.overrun || q.second.overrun,
        saturated: q.first.saturated || q.second.saturated,
    };

    if cr.reset.any() {
        d.first = stream::In::<W_IN> {
            stream: None,
            downstream_ready: false,
        };
        d.second = stream::In::<W_MID> {
            stream: None,
            downstream_ready: false,
        };
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
    use super::super::stream::StreamDecimator;
    use super::super::{CicDecimate, accumulator_width, counter_width, dc_gain};
    use super::*;
    use crate::rcstream::Item;
    use expect_test::expect;

    // /4 then /8 = /32 overall.
    const WI: usize = 10;
    const R1: usize = 4;
    const R2: usize = 8;
    const N1: usize = 2;
    const N2: usize = 3;
    const M: usize = 1;
    const WMID: usize = accumulator_width(WI, N1, R1, M);
    const WOUT: usize = accumulator_width(WMID, N2, R2, M);

    type First = StreamDecimator<WI, WMID, CicDecimate<WI, WMID, N1, R1, M, { counter_width(R1) }>>;
    type Second =
        StreamDecimator<WMID, WOUT, CicDecimate<WMID, WOUT, N2, R2, M, { counter_width(R2) }>>;
    type Uut = CascadedDecimator<WI, WMID, WOUT, First, Second>;

    fn item(v: i128, sync: bool) -> stream::In<WI> {
        stream::In::<WI> {
            stream: Some(Item::<Real<WI>, SyncMark> {
                data: Real::<WI> { v: signed::<WI>(v) },
                frame: SyncMark { sync },
            }),
            downstream_ready: true,
        }
    }

    fn idle() -> stream::In<WI> {
        stream::In::<WI> {
            stream: None,
            downstream_ready: true,
        }
    }

    /// `(value, sync)` per emitted sample.
    fn run(seq: Vec<stream::In<WI>>) -> Vec<(i128, bool)> {
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
    fn it_decimates_by_the_product() {
        let n = 32 * 12;
        let mut seq: Vec<stream::In<WI>> =
            (0..n as i128).map(|k| item((k % 13) - 6, false)).collect();
        seq.extend(std::iter::repeat_n(idle(), 8));
        let out = run(seq);
        let want = n / (R1 * R2);
        assert!(
            out.len() == want || out.len() == want - 1,
            "expected about {want}, got {}",
            out.len()
        );
    }

    #[test]
    fn dc_gain_is_the_product_of_the_stages() {
        let mut seq: Vec<stream::In<WI>> = (0..32 * 20).map(|_| item(7, false)).collect();
        seq.extend(std::iter::repeat_n(idle(), 8));
        let out = run(seq);
        let want = 7 * dc_gain(N1, R1, M) as i128 * dc_gain(N2, R2, M) as i128;
        assert_eq!(out.last().unwrap().0, want, "cascade DC gain");
    }

    /// **A mark propagates the whole way through, exactly once.**
    ///
    /// The first stage discards `R1 - 1` of every `R1` frames and the
    /// second discards `R2 - 1` of every `R2`; between them that is 31
    /// of every 32. The mark survives because each stage latches it,
    /// and nothing in this widget arranges that.
    #[test]
    fn a_mark_reaches_the_output_from_any_offset() {
        let period = R1 * R2;
        for offset in 0..period {
            let mut seq: Vec<stream::In<WI>> = (0..(period * 4)).map(|_| item(50, false)).collect();
            for k in 0..(period * 6) {
                seq.push(item(50, k == offset));
            }
            seq.extend(std::iter::repeat_n(idle(), 8));
            let out = run(seq);
            let marks = out.iter().filter(|(_, s)| *s).count();
            assert_eq!(
                marks, 1,
                "offset {offset}: expected exactly one marked output, got {marks}"
            );
        }
    }

    /// **The marked output excludes pre-trigger data, at any offset.**
    ///
    /// This is the property the earlier `restart`-wiring version got
    /// wrong for one offset in `R1`. Here nothing coordinates the two
    /// stages, so there is no offset for it to be wrong at — but the
    /// sweep stays, because that is how the bug was found.
    #[test]
    fn the_marked_output_excludes_pre_trigger_data_at_any_offset() {
        let period = R1 * R2;
        let after: Vec<i128> = (0..(period as i128 * 10))
            .map(|k| (k * 29) % 301 - 150)
            .collect();
        for offset in 0..period {
            let go = |hist: i128| -> Vec<(i128, bool)> {
                let mut seq: Vec<stream::In<WI>> = (0..(period * 4 + offset) as i128)
                    .map(|k| item((k * hist) % 251 - 125, false))
                    .collect();
                for (n, v) in after.iter().enumerate() {
                    seq.push(item(*v, n == 0));
                }
                seq.extend(std::iter::repeat_n(idle(), 8));
                run(seq)
            };
            let a = go(3);
            let b = go(41);
            let ma = a.iter().position(|(_, s)| *s).expect("a mark in a");
            let mb = b.iter().position(|(_, s)| *s).expect("a mark in b");
            // From the marked output onward, the two runs must agree:
            // the mark names a boundary built only from post-trigger
            // samples.
            let n = (a.len() - ma).min(b.len() - mb);
            assert!(n >= 3, "offset {offset}: not enough outputs after the mark");
            assert_eq!(
                a[ma..ma + n],
                b[mb..mb + n],
                "offset {offset}: post-mark output depends on pre-trigger history"
            );
        }
    }

    #[test]
    fn an_unmarked_stream_produces_no_marks() {
        let mut seq: Vec<stream::In<WI>> = (0..32 * 8).map(|_| item(11, false)).collect();
        seq.extend(std::iter::repeat_n(idle(), 8));
        let out = run(seq);
        assert!(!out.is_empty());
        assert!(out.iter().all(|(_, s)| !*s), "no mark in, none out");
    }

    #[test]
    fn an_idle_cycle_holds_both_stages() {
        let x: Vec<i128> = (0..32 * 8).map(|k: i128| (k * 53) % 199 - 99).collect();
        let mut dense: Vec<stream::In<WI>> = x.iter().map(|v| item(*v, false)).collect();
        dense.extend(std::iter::repeat_n(idle(), 8));
        let mut sparse: Vec<stream::In<WI>> = Vec::new();
        for v in &x {
            sparse.push(item(*v, false));
            sparse.push(idle());
        }
        sparse.extend(std::iter::repeat_n(idle(), 8));
        assert_eq!(
            run(dense),
            run(sparse),
            "a gap must not advance either stage"
        );
    }

    /// The cascade equals the two stages run separately.
    #[test]
    fn the_cascade_equals_the_stages_in_series() {
        let x: Vec<i128> = (0..32 * 14).map(|k: i128| (k * 37) % 401 - 200).collect();
        let mut seq: Vec<stream::In<WI>> = x.iter().map(|v| item(*v, false)).collect();
        seq.extend(std::iter::repeat_n(idle(), 8));
        let together = run(seq.clone());

        let mid: Vec<(i128, bool)> = First::default()
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| {
                s.output
                    .stream
                    .data
                    .map(|it| (it.data.v.raw(), it.frame.sync))
            })
            .collect();
        let mut b_in: Vec<stream::In<WMID>> = mid
            .iter()
            .map(|(v, sync)| stream::In::<WMID> {
                stream: Some(Item::<Real<WMID>, SyncMark> {
                    data: Real::<WMID> {
                        v: signed::<WMID>(*v),
                    },
                    frame: SyncMark { sync: *sync },
                }),
                downstream_ready: true,
            })
            .collect();
        b_in.extend(std::iter::repeat_n(
            stream::In::<WMID> {
                stream: None,
                downstream_ready: true,
            },
            8,
        ));
        let separate: Vec<i128> = Second::default()
            .run(b_in.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.stream.data.map(|it| it.data.v.raw()))
            .collect();

        let n = together.len().min(separate.len());
        assert!(n >= 3);
        let t: Vec<i128> = together[..n].iter().map(|(v, _)| *v).collect();
        assert_eq!(t, separate[..n], "cascade != stages in series");
    }

    // ---- Tier 3 / 4 / 5 --------------------------------------------

    fn hdl_stimulus() -> Vec<stream::In<WI>> {
        let mut v: Vec<stream::In<WI>> = (0..80)
            .map(|k: i128| item((k * 71) % 187 - 93, k == 9))
            .collect();
        v[15].downstream_ready = false;
        v.extend(std::iter::repeat_n(idle(), 6));
        v
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let hdl = Uut::default()
            .descriptor("top".into())?
            .hdl()?
            .modules
            .pretty();
        assert!(hdl.contains("module top_first"), "no first stage");
        assert!(hdl.contains("module top_second"), "no second stage");
        Ok(())
    }

    #[test]
    fn test_cic_cascaded_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let tb = uut
            .run(hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100))
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_cic_cascaded_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let vcd = uut
            .run(hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100))
            .collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("cic_cascaded");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["6d7bf5feb709653914b98401d824a7baea1a37c0359b4a2e2249b4b642e3e5cf"];
        let digest = vcd.dump_to_file(root.join("cic_cascaded.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
