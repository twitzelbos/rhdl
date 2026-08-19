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

    // The imaginary half first, so the real half's `frame` never has to
    // leave the scope it is bound in.
    //
    // Hoisting it as `let mut frame = F::dont_care()` and filling it in
    // a branch triggers an RHDL internal compile error, so the framing
    // is read inside the `if let` that binds the item.
    let mut im = signed::<W>(0);
    let mut have_im = false;
    if let Some(item) = i.imag {
        im = item.data.v;
        have_im = true;
    }

    let mut out_data = None;
    let mut starved = false;
    if let Some(item) = i.real {
        if have_im {
            out_data = Some(Item::<Iq<W>, F> {
                data: Iq::<W> {
                    re: item.data.v,
                    im,
                },
                // Framing comes from the real side; the type system
                // requires both inputs to carry the same `F`.
                frame: item.frame,
            });
        } else {
            starved = true;
        }
    } else if have_im {
        starved = true;
    }

    let o = Out::<W, F> {
        stream: RCStream::<Iq<W>, F> {
            data: out_data,
            ready: i.downstream_ready,
        },
        starved,
    };
    let _ = q;
    (o, d)
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
