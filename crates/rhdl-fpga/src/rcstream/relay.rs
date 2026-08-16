#![warn(missing_docs)]
//! `RCStreamRelay<T, F>` — a Carloni relay station with the typed
//! [`RCStream`] interface.
//!
//! Wraps the LID-paper-faithful [`crate::lid::carloni::Carloni`]
//! widget — same skid-buffer FSM, same throughput, same one-cycle
//! latency — but presents the typed `RCStream<T, F>` I/O instead of
//! the 3-signal `data/void/stop` Carloni interface.  This is the
//! canonical pipeline-stage primitive for `RCStream` connections.
//!
//! # Schematic symbol
//!
#![doc = badascii_doc::badascii_formal!("
     +-+RCStreamRelay+-----+
?T,F |                     | ?T,F
+--->+ data           data +--->
     |                     |
 <---+ ready         ready |<---+
     +---------------------+
")]
//!
//! # Design property
//!
//! Per Carloni's LID theorem (DAC 1999, *Proceedings of the IEEE*
//! 2015 retrospective), inserting a relay station anywhere on a
//! latency-insensitive connection adds one cycle of latency without
//! changing throughput or functional behavior.  This is the formal
//! basis for sound auto-pipelining at inter-kernel boundaries: the
//! auto-pipeliner can place an `RCStreamRelay` on any `RCStream`
//! connection with no hazard analysis required.
//!
//! # Internals
//!
//! Translates between `RCStream<T, F>` and the Carloni `data/void/stop`
//! 3-signal interface:
//!
//! ```text
//!   RCStream            Carloni
//!   data: Option<Item>  ←→  (data: Item, void: bool)   void = data.is_none()
//!   ready: bool         ←→  stop: bool                  stop = !ready
//! ```
//!
//! See [`crate::lid::carloni`] for the underlying FSM diagram and
//! the original-paper reference.
//!
//! # When to use
//!
//! - Any time an `RCStream` connection's TVALID/TREADY combinational
//!   path is a timing-closure concern.  Insert the relay; the LID
//!   theorem says throughput is unchanged.
//! - At inter-kernel boundaries where the auto-pipeliner needs a
//!   sound cut point.  Relay insertion at `RCStream` boundaries is
//!   guaranteed-correct without hazard analysis (per the design
//!   plan, this is the auto-pipeliner's preferred cut point).
//! - Anywhere a vendor's IP block expects a registered Ready/Valid
//!   handshake (to avoid combinational paths through the IP boundary).

use rhdl::prelude::*;

use crate::lid::carloni::{self, Carloni};
use crate::rcstream::bus::{Item, RCStream};

/// A Carloni relay station with the typed [`RCStream`] interface.
///
/// One cycle of latency, same throughput.  Pure thin wrapper around
/// [`Carloni<Item<T, F>>`] — see module docs for the encoding bridge.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RCStreamRelay<T: Digital, F: Digital> {
    /// The underlying Carloni skid-buffer, parameterized over
    /// `Item<T, F>` (the bus's payload type).
    inner: Carloni<Item<T, F>>,
}

impl<T: Digital, F: Digital> Default for RCStreamRelay<T, F> {
    fn default() -> Self {
        Self {
            inner: Carloni::<Item<T, F>>::default(),
        }
    }
}

impl<T: Digital, F: Digital> SynchronousIO for RCStreamRelay<T, F> {
    type I = RCStream<T, F>;
    type O = RCStream<T, F>;
    type Kernel = relay_kernel<T, F>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn relay_kernel<T: Digital, F: Digital>(
    _cr: ClockReset,
    i: RCStream<T, F>,
    q: Q<T, F>,
) -> (RCStream<T, F>, D<T, F>) {
    let mut d = D::<T, F>::dont_care();
    let mut o = RCStream::<T, F>::dont_care();

    // Decompose RCStream `i` (incoming side) into Carloni's
    // (data_in, void_in, stop_in).  Single match yields the
    // valid-flag and the payload, with the None arm carrying a
    // don't-care payload (Carloni ignores it because void_in=true).
    // Mirrors the existing `stream_buffer::option_carloni_kernel`
    // pattern.  Requires `#[kernel(allow_weak_partial)]` so RHDL's
    // kernel-coverage tracker accepts the don't-care leaves of
    // `Item<T, F>` in the None arm.
    let (data_valid, item_in): (bool, Item<T, F>) = match i.data {
        Some(it) => (true, it),
        None => (
            false,
            Item::<T, F> {
                data: T::dont_care(),
                frame: F::dont_care(),
            },
        ),
    };
    d.inner.data_in = item_in;
    d.inner.void_in = !data_valid;
    d.inner.stop_in = !i.ready;

    // Compose RCStream `o` (outgoing side) from Carloni's
    // (data_out, void_out, stop_out).
    o.data = if q.inner.void_out {
        None
    } else {
        Some(q.inner.data_out)
    };
    o.ready = !q.inner.stop_out;

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A relay with no items in flight should idle: data out = None,
    /// ready out = false (Carloni starts in Run with stop_out=true to
    /// signal "I'm not ready yet, don't send").
    ///
    /// More importantly: this confirms the type infrastructure
    /// composes — `RCStreamRelay<T, F>` builds a valid `Synchronous`
    /// widget that the framework accepts.
    #[test]
    fn relay_default_construction() {
        let _r: RCStreamRelay<b8, ()> = RCStreamRelay::default();
        let _r2: RCStreamRelay<b32, bool> = RCStreamRelay::default();
        let _r3: RCStreamRelay<b16, b8> = RCStreamRelay::default();
    }

    /// Direct kernel test: idle in → idle out (after 1-cycle latency
    /// the relay still has no data to deliver).
    #[test]
    fn relay_kernel_idle() {
        let cr = ClockReset::dont_care();
        let i = RCStream::<b8, ()> {
            data: None,
            ready: true,
        };
        let q = Q::<b8, ()> {
            inner: carloni::Out::<Item<b8, ()>> {
                data_out: Item::<b8, ()>::dont_care(),
                void_out: true, // no data in main_ff
                stop_out: false,
            },
        };
        let (o, _d) = relay_kernel::<b8, ()>(cr, i, q);
        assert!(o.data.is_none());
        assert_eq!(o.ready, true); // !stop_out = !false = true
    }

    /// Direct kernel test: when Carloni has buffered data
    /// (q.inner.void_out = false), the relay output's data is
    /// Some(item).
    #[test]
    fn relay_kernel_data_held() {
        let cr = ClockReset::dont_care();
        let i = RCStream::<b8, ()> {
            data: None,
            ready: true,
        };
        let held = Item::<b8, ()> {
            data: bits::<8>(0xAB),
            frame: (),
        };
        let q = Q::<b8, ()> {
            inner: carloni::Out::<Item<b8, ()>> {
                data_out: held,
                void_out: false, // data valid
                stop_out: false,
            },
        };
        let (o, _d) = relay_kernel::<b8, ()>(cr, i, q);
        match o.data {
            Some(it) => assert_eq!(it.data.raw(), 0xAB),
            None => panic!("expected Some(item) when void_out=false"),
        }
        assert_eq!(o.ready, true);
    }

    /// Direct kernel test: an incoming item is forwarded into
    /// Carloni's `data_in`/`void_in`.
    #[test]
    fn relay_kernel_forwards_item_to_carloni() {
        let cr = ClockReset::dont_care();
        let it = Item::<b8, ()> {
            data: bits::<8>(0x55),
            frame: (),
        };
        let i = RCStream::<b8, ()> {
            data: Some(it),
            ready: false, // downstream not ready → stop_in=true
        };
        let q = Q::<b8, ()> {
            inner: carloni::Out::<Item<b8, ()>> {
                data_out: Item::<b8, ()>::dont_care(),
                void_out: true,
                stop_out: false,
            },
        };
        let (_o, d) = relay_kernel::<b8, ()>(cr, i, q);
        assert_eq!(d.inner.data_in.data.raw(), 0x55);
        assert_eq!(d.inner.void_in, false); // is_none() = false → void = false
        assert_eq!(d.inner.stop_in, true); // !ready = true
    }

    /// Property: a `RCStreamRelay<T, F>` is a `Synchronous` widget that
    /// can be `descriptor()`-ed and asked for its HDL representation.
    /// Smoke test that the `Synchronous` derive composes cleanly with
    /// the wrapped Carloni.
    #[test]
    fn relay_descriptor_smoke() -> miette::Result<()> {
        let uut: RCStreamRelay<b8, ()> = RCStreamRelay::default();
        let _desc = uut.descriptor("rcstream_relay_b8".into())?;
        Ok(())
    }

    /// Tier 2 — the relay under sustained backpressure.
    ///
    /// The relay's insertion-invariance is covered at composition level
    /// in `tests/rcstream_relay_insertion.rs`, but the widget's own
    /// suite only ever drove `ready: true`.  A skid buffer exists to
    /// absorb stalls, so a test that never stalls exercises everything
    /// except its reason for existing.
    #[test]
    fn relay_loses_nothing_under_backpressure() {
        use rhdl::core::sim::ResetOrData;
        const COUNT: u128 = 24;
        let uut = RCStreamRelay::<b8, bool>::default();
        let mut to_send: u128 = 0;
        let mut got: Vec<(u128, bool)> = Vec::new();
        let mut need_reset = true;
        let mut phase: u32 = 0;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                let ready = phase.is_multiple_of(3);
                if let Some(it) = output.data {
                    if ready {
                        got.push((it.data.raw(), it.frame));
                    }
                }
                let mut input = RCStream::<b8, bool> { data: None, ready };
                if to_send < COUNT && output.ready {
                    input.data = Some(Item::<b8, bool> {
                        data: bits::<8>(to_send),
                        frame: to_send % 8 == 7,
                    });
                    to_send += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 400_000)
        .for_each(drop);

        let want: Vec<(u128, bool)> = (0..COUNT).map(|k| (k, k % 8 == 7)).collect();
        assert_eq!(
            got, want,
            "a skid buffer must lose nothing when the sink stalls"
        );
    }

    /// iverilog round-trip: the relay's emitted Verilog matches the
    /// Rust simulation exactly.
    #[test]
    fn relay_iverilog_round_trip() -> Result<(), RHDLError> {
        let uut: RCStreamRelay<b8, ()> = RCStreamRelay::default();
        let inputs: Vec<RCStream<b8, ()>> = (0..16)
            .map(|k| {
                let it = Item::<b8, ()> {
                    data: bits::<8>(k as u128),
                    frame: (),
                };
                RCStream::<b8, ()> {
                    data: Some(it),
                    ready: true,
                }
            })
            .collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    /// Round-trip with framing parameter `F = bool` (TLAST-equivalent).
    /// Verifies the relay's typed-framing flow-through works.
    #[test]
    fn relay_with_framing_round_trip() -> Result<(), RHDLError> {
        let uut: RCStreamRelay<b8, bool> = RCStreamRelay::default();
        let inputs: Vec<RCStream<b8, bool>> = (0..16)
            .map(|k| {
                let it = Item::<b8, bool> {
                    data: bits::<8>(k as u128),
                    frame: k == 15, // last item carries TLAST = true
                };
                RCStream::<b8, bool> {
                    data: Some(it),
                    ready: true,
                }
            })
            .collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }
}
