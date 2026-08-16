//! Filter Map Stream Core
//!
//!# Purpose
//!
//! A [FilterMap] Core takes a sequence of elements of type `T`
//! and a function `fn(T) -> Option<S>`, and keeps only those
//! items which are `Some`.  This is particularly handy for
//! processing streams of `enum` values, and then extracting
//! a particular variant, for example.
//!
//!# Schematic Symbol
//!
//! Here is the schematic symbol for the [FilterMap] stream
//!
#![doc = badascii_formal!("
     ++FilterMap+---+        
 ?T  |              | ?S    
+--->+ data   data  +---->
 R<T>|              | R<S>       
<----+ ready  ready |<---+
     +--------------+       
")]
//!
//!# Internals
//!
//! Unlike [FlattenPipe] or [ChunkedPipe], the [FilterMap] does not
//! impose any flow control on the upstream pipe.  Because it can
//! at most produce as many items as the source pipe, it can be
//! implemented with simple [StreamBuffer] buffers at the input
//! and output, which are needed to isolate the combinatorial
//! filter-map function from the remaining parts of the pipeline.  
//! Note that if you need a more expensive filter-map function (i.e., one
//! that itself is pipelined), then you cannot use this construct.
//!
#![doc = badascii!(r"
                                    ++func++   +          
                                    |      |?S |\         
                                  +>|in out+-->|1+  data  
     +-+Input Buf++     +unpack+  | +------+   | +------->
 ?T  |            | ?T  |      |T |     None+->|0+        
+--->|data    data+---->|in out+--+            |/         
 R<T>|            |     |      |               +^   R<S> 
<----+ready  ready|<-+  |   tag+----------------+  +-----+
     +------------+  |  +------+                   |      
                     +-----------------------------+      
")]
//!# Example
//!
//! Here is an example of running the pipeline filter map.  It is
//! interesting, because it demonstrates the use of `enum` values
//! to thar are filtered and the payload stripped for further
//! processing.
//!
//!```
#![doc = include_str!("../../examples/filter_map.rs")]
//!```
//!
//! with a trace file like this:
//!
#![doc = include_str!("../../doc/filter_map.md")]
//!

use badascii_doc::{badascii, badascii_formal};

use rhdl::prelude::*;

use crate::stream::ready;

use super::{stream_buffer::StreamBuffer, StreamIO};

#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// The FilterMap Core
///
/// Here `T` is the input type, and `S` is the
/// output type.  A provided (combinatorial) function
/// performs the mapping function.  It must have a
/// signature of `fn(T) -> Option<S>`.
pub struct FilterMap<T: Digital, S: Digital> {
    input_buffer: StreamBuffer<T>,
    func: Func<T, Option<S>>,
}

impl<T, S> FilterMap<T, S>
where
    T: Digital,
    S: Digital,
{
    /// Construct a Filter Map Stream
    ///
    /// The argument to the filter map
    /// `try_new` function is a synthesizable function
    /// (i.e., one marked with the `#[kernel]` attribute).
    /// It must have a signature `fn(T) -> Option<S>`.
    pub fn try_new<K>() -> Result<Self, RHDLError>
    where
        K: DigitalFn,
        K: DigitalFn2<A0 = ClockReset, A1 = T, O = Option<S>>,
    {
        Ok(Self {
            input_buffer: StreamBuffer::default(),
            func: Func::try_new::<K>()?,
        })
    }
}

/// The input for the [FilterMap]
pub type In<T, S> = StreamIO<T, S>;

/// The output type for the [FilterMap]
pub type Out<T, S> = StreamIO<S, T>;

impl<T, S> SynchronousIO for FilterMap<T, S>
where
    T: Digital,
    S: Digital,
{
    type I = In<T, S>;
    type O = Out<T, S>;
    type Kernel = kernel<T, S>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn kernel<T, S>(_cr: ClockReset, i: In<T, S>, q: Q<T, S>) -> (Out<T, S>, D<T, S>)
where
    T: Digital,
    S: Digital,
{
    let mut d = D::<T, S>::dont_care();
    d.input_buffer.data = i.data;
    d.func = T::dont_care();
    let mut have = false;
    if let Some(data) = q.input_buffer.data {
        d.func = data;
        have = true;
    }
    // The function's verdict only means anything when we had an item to
    // hand it.
    let produced = match q.func {
        Some(_) => true,
        None => false,
    };
    let emit = have && produced;
    // A DROPPED item must be consumed by us, not by the sink — see
    // `super::filter` for the full argument.  A sink may gate its
    // `ready` on seeing data, and a dropped item shows it none, so
    // waiting for `i.ready` here deadlocks the stream.
    let dropping = have && !produced;
    d.input_buffer.ready = ready::<T>(i.ready.raw || dropping);
    let o = Out::<T, S> {
        data: if emit { q.func } else { None },
        ready: q.input_buffer.ready,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use std::iter::repeat_n;

    use crate::{
        core::slice::lsbs,
        rng::xorshift::XorShift128,
        stream::testing::{single_stage::single_stage, utils::stalling},
    };

    use super::*;
    use crate::stream::testing::sink_from_fn::SinkView;

    #[kernel]
    fn filter_map_item(_cr: ClockReset, t: b4) -> Option<b2> {
        if (t & bits(1)).any() {
            None
        } else {
            Some(lsbs::<2, 4>(t))
        }
    }

    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let map = FilterMap::try_new::<filter_map_item>()?;
        drc::no_combinatorial_paths(&map)?;
        Ok(())
    }

    #[kernel]
    fn halve_even_regression(_cr: ClockReset, t: b4) -> Option<b4> {
        if !(t & bits(1)).any() {
            Some(t >> 1)
        } else {
            None
        }
    }

    /// **Regression: deadlock against a data-gated sink.**
    ///
    /// The AXI Ready/Valid contract this module implements permits a
    /// sink to withhold `ready` until it sees data. A dropped item
    /// produces `data = None` downstream, so such a sink never asserts
    /// `ready` — and this widget used to gate its input buffer on
    /// `i.ready` alone, leaving the dropped item stuck forever. The
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
        let uut = FilterMap::<b4, b4>::try_new::<halve_even_regression>()?;
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
        let want: Vec<u128> = (0..COUNT).filter(|k| k % 2 == 0).map(|k| k >> 1).collect();
        assert_eq!(got, want, "every surviving item must be delivered");
        Ok(())
    }

    #[test]
    fn test_operation() -> Result<(), RHDLError> {
        let a_rng = XorShift128::default().map(|x| b4((x & 0xF) as u128));
        let mut b_rng = a_rng.clone().filter_map(|x| {
            if (x & bits(1)).any() {
                None
            } else {
                Some(lsbs::<2, 4>(x))
            }
        });
        let a_rng = stalling(a_rng, 0.23);
        let consume = move |v: SinkView<b2>| {
            if let Some(data) = v.accepted {
                let orig = b_rng.next().unwrap();
                assert_eq!(data, orig);
            }
            rand::random::<f64>() > 0.2
        };
        let map = FilterMap::try_new::<filter_map_item>()?;
        let uut = single_stage(map, a_rng, consume);
        // Run a few samples through
        let input = repeat_n((), 10_000).with_reset(1).clock_pos_edge(100);
        uut.run(input).for_each(drop);
        Ok(())
    }
}
