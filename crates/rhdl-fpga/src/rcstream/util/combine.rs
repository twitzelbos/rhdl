#![warn(missing_docs)]
//! `IqCombine` — a [`Real`] stream and an [`Imag`] stream into one
//! [`Iq`] stream.
//!
//! The inverse of [`super::split::IqSplit`], and pure rewiring for the
//! same reason: combinational, zero latency, no logic.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+IqCombine+----------+
      |                      |
+---->+ real          stream +----->
      |                      |
+---->+ imag         starved +----->
      +----------------------+
")]
//!
//! # Both sides must present data
//!
//! A complex sample needs both halves, so an item is emitted only when
//! both inputs are valid. A cycle where exactly one side has data sets
//! `starved`.
//!
//! Reported rather than buffered, for the same reason as
//! [`crate::dsp::mixer::ComplexRealMixer`]: holding one side to wait
//! for the other is an elastic buffer with data-dependent occupancy,
//! which makes the path's latency data-dependent and breaks the
//! scheduler's arithmetic. In the timed domain both sides are
//! isochronous, so a one-sided cycle is a design error rather than a
//! condition to handle.

use rhdl::prelude::*;

use crate::dsp::iq::{Imag, Iq, Real};
use crate::rcstream::bus::{Item, RCStream};

/// Combines real and imaginary streams into one `Iq` stream.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct IqCombine<const W: usize, F: Digital>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// Carries the framing type.
    marker: crate::core::constant::Constant<F>,
}

impl<const W: usize, F: Digital> Default for IqCombine<W, F>
where
    rhdl::bits::W<W>: BitWidth,
{
    fn default() -> Self {
        Self {
            marker: crate::core::constant::Constant::new(F::dont_care()),
        }
    }
}

/// Inputs to [`IqCombine`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const W: usize, F: Digital>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The real part.
    pub real: Option<Item<Real<W>, F>>,
    /// The imaginary part.
    pub imag: Option<Item<Imag<W>, F>>,
    /// Ready from the downstream consumer.
    pub downstream_ready: bool,
}

/// Outputs from [`IqCombine`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const W: usize, F: Digital>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The combined complex stream.
    pub stream: RCStream<Iq<W>, F>,
    /// Exactly one side presented data on some cycle.
    pub starved: bool,
    /// **The two sides presented data whose framing disagreed.**
    ///
    /// The type system requires both inputs to carry the same framing
    /// *type*; it cannot require the same *value*. Two paths that were
    /// split from one stream should carry identical frames, so a
    /// disagreement means they have drifted — a dropped item on one
    /// side, or two decimators that fell out of step. That is a fault
    /// in the composition, not a condition to paper over, so it is
    /// reported rather than resolved by silently preferring one side.
    pub frame_mismatch: bool,
}

impl<const W: usize, F: Digital> SynchronousIO for IqCombine<W, F>
where
    rhdl::bits::W<W>: BitWidth,
{
    type I = In<W, F>;
    type O = Out<W, F>;
    type Kernel = iq_combine_kernel<W, F>;
}

#[kernel]
#[doc(hidden)]
pub fn iq_combine_kernel<const W: usize, F: Digital>(
    _cr: ClockReset,
    i: In<W, F>,
    q: Q<W, F>,
) -> (Out<W, F>, D<W, F>)
where
    rhdl::bits::W<W>: BitWidth,
{
    let mut d = D::<W, F>::dont_care();
    d.marker = ();

    // Presence is tracked with plain `bool`s, and the two framing
    // values are compared inside *nested* `if let`s so that neither
    // ever leaves the scope that binds it.
    //
    // This shape is not a preference. Hoisting a generic `F` as
    // `let mut frame = F::dont_care()` and filling it in a branch
    // trips an RHDL partial-initialisation error -- the original
    // version of this kernel carried a comment saying so, and the
    // first attempt at frame comparison did it anyway and broke the
    // build. Nesting keeps `F` inside its binding and needs no
    // placeholder value at all.
    let mut have_re = false;
    if let Some(_item) = i.real {
        have_re = true;
    }
    let mut have_im = false;
    if let Some(_item) = i.imag {
        have_im = true;
    }

    let mut out_data = None;
    let mut frame_mismatch = false;
    if let Some(re_item) = i.real {
        if let Some(im_item) = i.imag {
            // The two frames must agree. Both sides came from one
            // stream, so a disagreement is drift, not a choice to be
            // made -- see `Out::frame_mismatch`. The real side's frame
            // is carried so the item is still well formed, and the
            // mismatch is reported alongside it.
            if re_item.frame != im_item.frame {
                frame_mismatch = true;
            }
            out_data = Some(Item::<Iq<W>, F> {
                data: Iq::<W> {
                    re: re_item.data.v,
                    im: im_item.data.v,
                },
                frame: re_item.frame,
            });
        }
    }
    // Exactly one side presented data.
    let starved = have_re != have_im;

    let o = Out::<W, F> {
        stream: RCStream::<Iq<W>, F> {
            data: out_data,
            ready: i.downstream_ready,
        },
        starved,
        frame_mismatch,
    };
    let _ = q;
    (o, d)
}

#[cfg(test)]
mod frame_alignment_tests {
    use super::*;

    /// Agreeing frames pass through and raise nothing.
    #[test]
    fn aligned_frames_do_not_flag() {
        let uut = IqCombine::<8, bool>::default();
        let seq = vec![In::<8, bool> {
            real: Some(Item::<Real<8>, bool> {
                data: Real::<8> { v: signed::<8>(3) },
                frame: true,
            }),
            imag: Some(Item::<Imag<8>, bool> {
                data: Imag::<8> { v: signed::<8>(-4) },
                frame: true,
            }),
            downstream_ready: true,
        }];
        let out: Vec<(bool, bool)> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| {
                s.output
                    .stream
                    .data
                    .map(|it| (it.frame, s.output.frame_mismatch))
            })
            .collect();
        assert!(!out.is_empty());
        assert!(
            out.iter().all(|(f, m)| *f && !*m),
            "aligned frames must pass through unflagged: {out:?}"
        );
    }

    /// **Disagreeing frames are reported, not silently resolved.**
    ///
    /// Before `frame_mismatch` existed, the real side's frame was taken
    /// and the imaginary side's discarded without comment — so two
    /// paths that had drifted produced a confident, wrong answer.
    #[test]
    fn disagreeing_frames_are_flagged() {
        let uut = IqCombine::<8, bool>::default();
        let seq = vec![In::<8, bool> {
            real: Some(Item::<Real<8>, bool> {
                data: Real::<8> { v: signed::<8>(3) },
                frame: true,
            }),
            imag: Some(Item::<Imag<8>, bool> {
                data: Imag::<8> { v: signed::<8>(-4) },
                frame: false,
            }),
            downstream_ready: true,
        }];
        let flagged: Vec<bool> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.frame_mismatch)
            .collect();
        assert!(
            flagged.iter().any(|m| *m),
            "a frame disagreement must be reported"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 18;
    type Uut = IqCombine<W, ()>;

    fn both(re: i128, im: i128) -> In<W, ()> {
        In::<W, ()> {
            real: Some(Item::<Real<W>, ()> {
                data: Real::<W> { v: signed::<W>(re) },
                frame: (),
            }),
            imag: Some(Item::<Imag<W>, ()> {
                data: Imag::<W> { v: signed::<W>(im) },
                frame: (),
            }),
            downstream_ready: true,
        }
    }

    fn only_real(re: i128) -> In<W, ()> {
        In::<W, ()> {
            real: Some(Item::<Real<W>, ()> {
                data: Real::<W> { v: signed::<W>(re) },
                frame: (),
            }),
            imag: None,
            downstream_ready: true,
        }
    }

    fn run(seq: Vec<In<W, ()>>) -> Vec<Out<W, ()>> {
        let uut = Uut::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect()
    }

    /// The halves land in the right components.
    #[test]
    fn the_components_are_not_swapped() {
        let out = run(vec![both(4321, -8765); 3]);
        match out[2].stream.data {
            Some(item) => {
                assert_eq!(item.data.re.raw(), 4321);
                assert_eq!(item.data.im.raw(), -8765);
            }
            None => panic!("both inputs were valid, so an item must be emitted"),
        }
    }

    /// One-sided cycles are reported and emit nothing.
    #[test]
    fn a_one_sided_cycle_is_reported() {
        let mut seq = vec![both(10, 20); 2];
        seq.push(only_real(30));
        seq.extend(vec![both(10, 20); 2]);
        let out = run(seq);
        assert!(
            out.iter().any(|o| o.starved),
            "a one-sided cycle was not reported"
        );
        assert!(out.iter().any(|o| !o.starved), "starved is stuck high");
        let starved_at = out.iter().position(|o| o.starved).unwrap();
        assert!(
            out[starved_at].stream.data.is_none(),
            "a half sample must not be emitted as if it were whole"
        );
    }

    /// **Split then combine is the identity.**
    ///
    /// The strongest statement about the pair: whatever routing the
    /// type algebra makes possible, it does not alter the data. A
    /// transposition or a dropped component in *either* widget breaks
    /// this, which a test of one widget alone would not catch.
    #[test]
    fn split_then_combine_is_the_identity() {
        use crate::rcstream::util::split::{In as SplitIn, IqSplit};

        let values: Vec<(i128, i128)> = (0..24i128)
            .map(|k| ((k - 12) * 1100, (12 - k) * 900))
            .collect();

        // Stage 1: split.
        let split = IqSplit::<W, ()>::default();
        let halves: Vec<(i128, i128)> = split
            .run(
                values
                    .iter()
                    .map(|(r, i)| SplitIn::<W, ()> {
                        stream: Some(Item::<Iq<W>, ()> {
                            data: Iq::<W> {
                                re: signed::<W>(*r),
                                im: signed::<W>(*i),
                            },
                            frame: (),
                        }),
                        real_ready: true,
                        imag_ready: true,
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .with_reset(1)
                    .clock_pos_edge(100),
            )
            .synchronous_sample()
            .filter_map(|s| match (s.output.real.data, s.output.imag.data) {
                (Some(r), Some(i)) => Some((r.data.v.raw(), i.data.v.raw())),
                _ => None,
            })
            .collect();

        assert_eq!(halves.len(), values.len(), "the split dropped samples");

        // Stage 2: combine.
        let recombined: Vec<(i128, i128)> =
            run(halves.iter().map(|(r, i)| both(*r, *i)).collect::<Vec<_>>())
                .into_iter()
                .filter_map(|o| o.stream.data.map(|it| (it.data.re.raw(), it.data.im.raw())))
                .collect();

        assert_eq!(
            recombined, values,
            "split followed by combine must return exactly what went in"
        );
    }

    /// Tier 4 — emitted Verilog agrees with the Rust simulation.
    #[test]
    fn test_iq_combine_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let seq: Vec<In<W, ()>> = (0..16i128)
            .map(|k| both((k - 8) * 800, (8 - k) * 600))
            .collect();
        let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }
}
