#![warn(missing_docs)]
//! `IqSplit` — one [`Iq`] stream into a [`Real`] stream and an [`Imag`]
//! stream.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+IqSplit+------------+
      |                      |
+---->+ stream          real +----->
      |                      |
      |                 imag +----->
      +----------------------+
")]
//!
//! # Pure rewiring, zero latency
//!
//! An [`Iq<W>`](Iq) is two `SignedBits<W>` laid end to end; a
//! [`Real<W>`](Real) and an [`Imag<W>`](Imag) are one each. So this
//! widget contains no logic at all — it renames bits. It is
//! combinational and adds **nothing** to the scheduler's latency
//! arithmetic.
//!
//! The value is entirely in the type system: without split and combine
//! the sample types are decorative, because routing a complex stream
//! into a widget that wants a real one would not be expressible, and
//! the `Real × Iq` instantiation of a mixer could never be reached from
//! an `Iq` source.
//!
//! # Validity is propagated, not invented
//!
//! Both outputs are valid exactly when the input is. Splitting cannot
//! create data, and an implementation that emitted `Some` on an idle
//! cycle would be inventing samples.

use rhdl::prelude::*;

use crate::dsp::iq::{Imag, Iq, Real};
use crate::rcstream::bus::{Item, RCStream};

/// Splits an `Iq` stream into its real and imaginary parts.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct IqSplit<const W: usize, F: Digital>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// Carries the framing type; see the note on `PhantomData` in
    /// [`super::constant::RCStreamConstant`].
    marker: crate::core::constant::Constant<F>,
}

impl<const W: usize, F: Digital> Default for IqSplit<W, F>
where
    rhdl::bits::W<W>: BitWidth,
{
    fn default() -> Self {
        Self {
            marker: crate::core::constant::Constant::new(F::dont_care()),
        }
    }
}

/// Inputs to [`IqSplit`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const W: usize, F: Digital>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The complex stream.
    pub stream: Option<Item<Iq<W>, F>>,
    /// Ready from the real-side consumer.
    pub real_ready: bool,
    /// Ready from the imaginary-side consumer.
    pub imag_ready: bool,
}

/// Outputs from [`IqSplit`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const W: usize, F: Digital>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The real part.
    pub real: RCStream<Real<W>, F>,
    /// The imaginary part.
    pub imag: RCStream<Imag<W>, F>,
    /// Ready toward the upstream source: both consumers must be ready,
    /// since one item becomes two and neither can be held back
    /// independently without buffering.
    pub ready: bool,
}

impl<const W: usize, F: Digital> SynchronousIO for IqSplit<W, F>
where
    rhdl::bits::W<W>: BitWidth,
{
    type I = In<W, F>;
    type O = Out<W, F>;
    type Kernel = iq_split_kernel<W, F>;
}

#[kernel]
#[doc(hidden)]
pub fn iq_split_kernel<const W: usize, F: Digital>(
    _cr: ClockReset,
    i: In<W, F>,
    q: Q<W, F>,
) -> (Out<W, F>, D<W, F>)
where
    rhdl::bits::W<W>: BitWidth,
{
    let mut d = D::<W, F>::dont_care();
    d.marker = ();

    let mut real_data = None;
    let mut imag_data = None;

    if let Some(item) = i.stream {
        real_data = Some(Item::<Real<W>, F> {
            data: Real::<W> { v: item.data.re },
            frame: item.frame,
        });
        imag_data = Some(Item::<Imag<W>, F> {
            data: Imag::<W> { v: item.data.im },
            frame: item.frame,
        });
    }

    let o = Out::<W, F> {
        real: RCStream::<Real<W>, F> {
            data: real_data,
            ready: i.real_ready,
        },
        imag: RCStream::<Imag<W>, F> {
            data: imag_data,
            ready: i.imag_ready,
        },
        // One item becomes two, so the source may only advance when
        // both consumers can take theirs.
        ready: i.real_ready && i.imag_ready,
    };
    let _ = q;
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 18;
    type Uut = IqSplit<W, ()>;

    fn item(re: i128, im: i128) -> In<W, ()> {
        In::<W, ()> {
            stream: Some(Item::<Iq<W>, ()> {
                data: Iq::<W> {
                    re: signed::<W>(re),
                    im: signed::<W>(im),
                },
                frame: (),
            }),
            real_ready: true,
            imag_ready: true,
        }
    }

    fn idle() -> In<W, ()> {
        In::<W, ()> {
            stream: None,
            real_ready: true,
            imag_ready: true,
        }
    }

    fn run(seq: Vec<In<W, ()>>) -> Vec<Out<W, ()>> {
        let uut = Uut::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect()
    }

    /// Each half goes to the right output, and the halves are not
    /// swapped.
    ///
    /// Distinct values per component, so a transposition shows up
    /// rather than cancelling.
    #[test]
    fn the_components_are_not_swapped() {
        let out = run(vec![item(1234, -5678); 3]);
        let s = &out[2];
        match (s.real.data, s.imag.data) {
            (Some(r), Some(i)) => {
                assert_eq!(r.data.v.raw(), 1234, "real side carries the real part");
                assert_eq!(
                    i.data.v.raw(),
                    -5678,
                    "imag side carries the imaginary part"
                );
            }
            _ => panic!("both outputs must be valid when the input is"),
        }
    }

    /// Validity is propagated, not invented: an idle input gives two
    /// idle outputs.
    #[test]
    fn an_idle_input_gives_idle_outputs() {
        let mut seq = vec![item(100, 200); 2];
        seq.extend(vec![idle(); 3]);
        let out = run(seq);
        let tail = out.last().unwrap();
        assert!(
            tail.real.data.is_none() && tail.imag.data.is_none(),
            "splitting cannot create data"
        );
    }

    /// The source may advance only when both consumers can take their
    /// half — one item becomes two.
    #[test]
    fn ready_requires_both_consumers() {
        let uut = Uut::default();
        let mut a = item(1, 2);
        a.imag_ready = false;
        let out: Vec<Out<W, ()>> = uut
            .run(vec![a; 3].into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect();
        assert!(
            !out[2].ready,
            "upstream must not be told to advance while one consumer is stalled"
        );
    }

    /// Tier 4 — emitted Verilog agrees with the Rust simulation.
    #[test]
    fn test_iq_split_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let seq: Vec<In<W, ()>> = (0..16i128)
            .map(|k| item((k - 8) * 900, (8 - k) * 700))
            .collect();
        let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }
}
