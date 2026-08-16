//! Filter Stream Core
//!
//!# Purpose
//!
//! A [Filter] Core takes a stream of elements of type `T`
//! and a function `fn(T) -> bool`, and keeps only those items for
//! which the function evaluates to `true`.  The filter function is
//! provided in the form of a synthesizable function.  This is
//! equivalent to using `.filter()` on an interator.
//!
//!# Schematic Symbol
//!
//! Here is the schematic symbol for the [Filter] core
//!
#![doc = badascii_formal!("
      +-+Filter+-----+        
 ?T   |              | ?T    
+---->+data     data +----->
 R<T> |              | R<T>       
<-----+ready    ready|<----+
      +--------------+       
")]
//!
//!# Internals
//!
//! Unlike [Flatten] or [Chunked], the [FilterPipe] does not
//! impose any flow control on the upstream.  Because it can
//! at most produce as many items as the source, it can be
//! implemented with a [StreamBuffer] buffers at the input
//! which is needed to isolate the combinatorial
//! filter function from the remaining parts of the stream.  
//! Note that if you need a more expensive filter function (i.e., one
//! that itself is pipelined), then you cannot use this construct.
//!
#![doc = badascii!(r"
                                      +-+Func+--+                        
                                      |         |                        
                                    +>|in   keep+--+                     
     +-+Input Buf++     +-+upck+-+  | +---------+  |   +-+pck+-+         
 ?T  |            | ?T  |        |T |              |   |       |?T data  
+--->|data    data+---->|in   out+--+-------------+|+->|in  out+-------->
R<T> |            |     |        |                 +   |       |   Ready<T>
<----+ready  ready|<-+  |     tag+---------------> &+->|tag    |  +-----+
     +------------+  |  +--------+                     +-------+  |      
                     |                                            |      
                     +--------------------------------------------+      
")]
//!# Example
//!
//! Here is an example of filtering a stream.
//!
//!```
#![doc = include_str!("../../examples/filter.rs")]
//!```
//!
//! with a trace file like this:
//!
#![doc = include_str!("../../doc/filter.md")]
//!
use badascii_doc::{badascii, badascii_formal};
use rhdl::prelude::*;

use super::{ready, stream_buffer::StreamBuffer, StreamIO};

#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// The [Filter] Stream Core
///
/// Here `T` is the type flowing in the stream.
/// At construction time, you provide a synthesizable
/// function to filter the contents of the stream.
/// Only items for which `fn(T)` returns `true` will
/// be passed on downstream.
pub struct Filter<T: Digital> {
    input_buffer: StreamBuffer<T>,
    func: Func<T, bool>,
}

impl<T> Filter<T>
where
    T: Digital,
{
    /// Construct a [Filter] Stream
    ///
    /// The argument to the filter `try_new` function
    /// is a synthesizable function (i.e., one marked with the
    /// `#[kernel]` attribute).  It must have a signature of
    /// `fn(ClockReset, T) -> bool`.
    pub fn try_new<S>() -> Result<Self, RHDLError>
    where
        S: DigitalFn,
        S: DigitalFn2<A0 = ClockReset, A1 = T, O = bool>,
    {
        Ok(Self {
            input_buffer: StreamBuffer::default(),
            func: Func::try_new::<S>()?,
        })
    }
}

/// The input for the [Filter]
pub type In<T> = StreamIO<T, T>;

/// The output of the [Filter]
pub type Out<T> = StreamIO<T, T>;

impl<T> SynchronousIO for Filter<T>
where
    T: Digital,
{
    type I = In<T>;
    type O = Out<T>;
    type Kernel = kernel<T>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn kernel<T: Digital>(_cr: ClockReset, i: In<T>, q: Q<T>) -> (Out<T>, D<T>) {
    let mut d = D::<T>::dont_care();
    d.input_buffer.data = i.data;
    d.func = T::dont_care();
    let mut have = false;
    if let Some(data) = q.input_buffer.data {
        d.func = data;
        have = true;
    }
    let keep = have && q.func;
    // A REJECTED item must be consumed by us, not by the sink.
    //
    // The handshake here is the AXI Ready/Valid contract, under which a
    // sink may legitimately withhold `ready` until it sees data.  A
    // rejected item produces `data = None` downstream, so such a sink
    // never asserts `ready` — and if we gated the buffer on `i.ready`
    // alone the rejected item would sit there forever and the stream
    // would deadlock.  Consuming rejections ourselves is what makes this
    // widget correct against every conforming sink rather than only
    // against unconditionally-ready ones.
    let dropping = have && !q.func;
    d.input_buffer.ready = ready::<T>(i.ready.raw || dropping);
    let mut o = Out::<T> {
        data: None,
        ready: q.input_buffer.ready,
    };
    if let Some(data) = q.input_buffer.data {
        if keep {
            o.data = Some(data);
        }
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use std::iter::repeat_n;

    use crate::{
        rng::xorshift::XorShift128,
        stream::testing::{single_stage::single_stage, utils::stalling},
    };

    use super::*;
    use crate::stream::testing::sink_from_fn::SinkView;

    #[kernel]
    fn keep_even(_cr: ClockReset, t: b4) -> bool {
        !(t & bits(1)).any()
    }

    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let filter = Filter::try_new::<keep_even>()?;
        drc::no_combinatorial_paths(&filter)?;
        Ok(())
    }

    /// **Regression: deadlock against a data-gated sink.**
    ///
    /// The AXI Ready/Valid contract this module implements permits a
    /// sink to withhold `ready` until it sees data. A rejected item
    /// produces `data = None` downstream, so such a sink never asserts
    /// `ready` — and this widget used to gate its input buffer on
    /// `i.ready` alone, leaving the rejected item stuck forever. The
    /// stream deadlocked after the first one, with everything behind it
    /// silently lost.
    ///
    /// The pre-existing `test_operation` missed it twice over: its sink
    /// returns `rand::random::<f64>() > 0.2`, which is independent of
    /// whether data was presented, and it only asserts a property of the
    /// values that *do* arrive rather than that all of them do.
    #[test]
    fn no_deadlock_against_a_data_gated_sink() -> Result<(), RHDLError> {
        use rhdl::core::sim::ResetOrData;
        const COUNT: u128 = 16;
        let uut = Filter::<b4>::try_new::<keep_even>()?;
        let mut to_send: u128 = 0;
        let mut got: Vec<u128> = Vec::new();
        let mut need_reset = true;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                // The sink only asserts ready when it can see an item.
                let sink_ready = output.data.is_some();
                if let Some(d) = output.data {
                    got.push(d.raw());
                }
                let mut input = StreamIO::<b4, b4> {
                    data: None,
                    ready: ready::<b4>(sink_ready),
                };
                if to_send < COUNT && output.ready.raw {
                    input.data = Some(b4(to_send % 16));
                    to_send += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 200_000)
        .for_each(drop);

        assert_eq!(to_send, COUNT, "the source must not be stalled forever");
        let want: Vec<u128> = (0..COUNT).filter(|k| k % 2 == 0).collect();
        assert_eq!(got, want, "every surviving item must be delivered");
        Ok(())
    }

    /// The same deadlock, caught **through the shared `single_stage`
    /// harness** rather than a hand-written `run_fn` loop.
    ///
    /// Two changes were needed before this test could exist at all:
    ///
    /// 1. [`SinkView`] — so the closure can see what is *offered*, not
    ///    just what it accepted. Without it, readiness cannot be
    ///    correlated with data presence.
    /// 2. [`SinkFromFn::new_combinational`] — so `ready` is computed
    ///    from the live offer rather than registered from the previous
    ///    cycle. The registered form's one-cycle lag is exactly enough
    ///    slack for the buggy filter to escape, so a *registered*
    ///    data-gated sink does **not** catch this. That was measured,
    ///    not assumed.
    ///
    /// Verified to fail when the reject-consuming term is removed.
    #[test]
    fn data_gated_sink_through_the_shared_harness() -> Result<(), RHDLError> {
        use crate::stream::testing::{
            single_stage::single_stage_with_sink, sink_from_fn::SinkFromFn,
        };
        use std::{cell::RefCell, rc::Rc};

        const COUNT: u128 = 16;
        let got = Rc::new(RefCell::new(Vec::<u128>::new()));
        let collector = got.clone();
        let sink = SinkFromFn::<b4>::new_combinational(
            // Ready only while something is actually on the wire.
            |offer| offer.is_some(),
            move |accepted| {
                if let Some(t) = accepted {
                    collector.borrow_mut().push(t.raw());
                }
            },
        );
        let src = (0..COUNT).map(|k| Some(b4(k % 16)));

        let uut = single_stage_with_sink(Filter::try_new::<keep_even>()?, src, sink);
        let input = repeat_n((), 4_000).with_reset(1).clock_pos_edge(100);
        uut.run(input).for_each(drop);

        let want: Vec<u128> = (0..COUNT).filter(|k| k % 2 == 0).collect();
        assert_eq!(
            *got.borrow(),
            want,
            "a data-gated sink must still receive every kept item"
        );
        Ok(())
    }

    #[test]
    fn test_operation() -> Result<(), RHDLError> {
        let a_rng = XorShift128::default().map(|x| b4((x & 0xF) as u128));
        let a_rng = stalling(a_rng, 0.23);
        let consume = move |v: SinkView<b4>| {
            if let Some(data) = v.accepted {
                // Only even values kept
                assert!(data.raw() & 1 == 0);
            }
            rand::random::<f64>() > 0.2
        };
        let filter = Filter::try_new::<keep_even>()?;
        let uut = single_stage(filter, a_rng, consume);
        // Run a few samples through
        let input = repeat_n((), 10_000).with_reset(1).clock_pos_edge(100);
        uut.run(input).for_each(drop);
        Ok(())
    }
}
