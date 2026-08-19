#![warn(missing_docs)]
//! `RCStreamConstant` — a source that emits the same item every cycle.
//!
//! For a fixed envelope (continuous-wave transmit), for filling an
//! input a design does not use, and as a stimulus in tests.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+RCStreamConstant+---+
      |                      |
      |               stream +----->
      |                      |
      +----------------------+
")]
//!
//! # Why there is no `ready` input and no overrun flag
//!
//! Contrast [`crate::dsp::nco::composite::Nco`], which reports
//! `overrun` when downstream is not ready: its samples are **specific
//! to a moment**, because phase represents absolute elapsed time, so a
//! sample downstream failed to take is a sample lost.
//!
//! A constant has no such property. If downstream does not accept this
//! cycle, the identical value is still there next cycle — nothing was
//! lost and there is nothing to report. So the widget ignores
//! backpressure, takes no input at all, and drives its outgoing `ready`
//! vacuously true.
//!
//! That difference is worth stating rather than leaving implied: it is
//! the distinction between a stream whose samples carry time and one
//! whose samples do not.

use rhdl::prelude::*;

use crate::core::constant::Constant;
use crate::rcstream::bus::{Item, RCStream};

/// Emits a fixed [`Item`] on every cycle.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RCStreamConstant<T: Digital, F: Digital> {
    /// The value, held in a [`Constant`] so the type parameters are
    /// carried by a real sub-circuit.
    ///
    /// `PhantomData` would be fatal here: `SynchronousDQ` treats every
    /// field as a child circuit and `PhantomData` has no HDL, so a
    /// derived widget carrying one fails at `descriptor()` — for itself
    /// and for any design containing it. See CLAUDE.md §4.
    value: Constant<Item<T, F>>,
}

impl<T: Digital, F: Digital> RCStreamConstant<T, F> {
    /// Create a source emitting `data` with framing `frame`.
    pub fn new(data: T, frame: F) -> Self {
        Self {
            value: Constant::new(Item { data, frame }),
        }
    }
}

impl<T: Digital, F: Digital> Default for RCStreamConstant<T, F> {
    fn default() -> Self {
        Self::new(T::dont_care(), F::dont_care())
    }
}

impl<T: Digital, F: Digital> SynchronousIO for RCStreamConstant<T, F> {
    type I = ();
    type O = RCStream<T, F>;
    type Kernel = rcstream_constant_kernel<T, F>;
}

#[kernel]
#[doc(hidden)]
pub fn rcstream_constant_kernel<T: Digital, F: Digital>(
    _cr: ClockReset,
    _i: (),
    q: Q<T, F>,
) -> (RCStream<T, F>, D<T, F>) {
    let mut d = D::<T, F>::dont_care();
    d.value = ();

    let o = RCStream::<T, F> {
        data: Some(q.value),
        // Vacuous: a source has no upstream to backpressure.
        ready: true,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::iq::Real;

    type Uut = RCStreamConstant<Real<16>, ()>;

    fn run(cycles: usize) -> Vec<RCStream<Real<16>, ()>> {
        let uut = Uut::new(
            Real::<16> {
                v: signed::<16>(4321),
            },
            (),
        );
        uut.run(
            std::iter::repeat_n((), cycles)
                .with_reset(1)
                .clock_pos_edge(100),
        )
        .synchronous_sample()
        .map(|s| s.output)
        .collect()
    }

    /// The value appears, unchanged, on every cycle.
    ///
    /// Asserts on *every* sample rather than on one: a source that
    /// emitted correctly once and then went idle would pass a
    /// single-sample check.
    #[test]
    fn the_value_is_emitted_every_cycle() {
        let out = run(12);
        let mut seen = 0;
        for s in out.iter().skip(2) {
            match s.data {
                Some(item) => {
                    assert_eq!(item.data.v.raw(), 4321);
                    seen += 1;
                }
                None => panic!("a constant source must never be idle"),
            }
        }
        assert!(seen >= 10, "only {seen} samples were checked");
    }

    /// Reset does not disturb it — a constant has no state to lose.
    #[test]
    fn reset_does_not_change_the_value() {
        let out = run(6);
        // Sample 0 is the reset cycle; the value is combinational from
        // the Constant, so it is already correct there.
        match out[0].data {
            Some(item) => assert_eq!(item.data.v.raw(), 4321),
            None => panic!("idle during reset"),
        }
    }

    /// Tier 4 — emitted Verilog agrees with the Rust simulation.
    #[test]
    fn test_rcstream_constant_hdl_works() -> miette::Result<()> {
        let uut = Uut::new(
            Real::<16> {
                v: signed::<16>(4321),
            },
            (),
        );
        let stream = std::iter::repeat_n((), 8).with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }
}
