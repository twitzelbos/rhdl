//! Map Stream Core
//!
//!# Purpose
//!
//! A [Map] Core takes a stream of elements of type `T` and
//! a synthesizable function `fn(T) -> S`, and feeds a stream
//! that carries type `S`.  This is equivalent to using `.map()` on
//! an interator.
//!
//!# Schematic Symbol
//!
//! Here is the schematic symbol for the [Map] buffer
//!
#![doc = badascii_formal!("
     +--+Map+---------+        
 ?T  |                | ?S   
+--->+ data     data  +---->
Ry<T>|                | Ry<S>       
<----+ ready    ready |<---+
     +----------------+       
")]
//!
//!# Internals
//!
//! Unlike [Flatten] or [Chunked], the [Map] does not
//! impose any flow control on the upstream.  Because it can
//! at most produce as many items as the source stream, it can be
//! implemented with simple [StreamBuffer] buffers at the input
//! and output, which are needed to isolate the combinatorial
//! `map` function from the remaining parts of the stream.  
//! Note that if you need a more expensive `map` function (i.e., one
//! that itself is pipelined), then you cannot use this construct.
//!
#![doc = badascii!(r"
                                      +-+Func+--+                       
                                      |         | S                     
                                    +>|in    out+--+                    
     +-+Buffer+---+     +-+upck+-+  | +---------+  |   +-+pck+-+        
 ?T  |            | ?T  |        |T |              |   |       |   ?S
+--->|data    data+---->|in   out+--+              +-->|in  out+------->
Ry<T>|            |     |        |                     |       |   Ry<S>
<----+ready  ready|<-+  |     tag+-------------------->|tag    |  +----+
     +------------+  |  +--------+                     +-------+  |     
                     |                                            |     
                     +--------------------------------------------+     
")]
//!# Example
//!
//! Here is an example of mapping a stream, transforming
//! elements.
//!
//!```
#![doc = include_str!("../../examples/stream_map.rs")]
//!```
//!
//! with a trace file like this:
//!
#![doc = include_str!("../../doc/stream_map.md")]
//!

use badascii_doc::{badascii, badascii_formal};
use rhdl::{
    core::{ClockReset, DigitalFn, DigitalFn2, RHDLError},
    prelude::*,
};

use super::{ready_cast, stream_buffer::StreamBuffer, StreamIO};

#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// The Map Core (Stream Version)
///
/// Here `T` is the input type, and `S` is the
/// output type.  A provided (combinatorial) function
/// performs the mapping function.
pub struct Map<T: Digital, S: Digital> {
    input_buffer: StreamBuffer<T>,
    func: Func<T, S>,
}

impl<T, S> Map<T, S>
where
    T: Digital,
    S: Digital,
{
    /// Construct a Map Stream
    ///
    /// The argument to the map stream `try_new` function
    /// is a synthesizable function (i.e., one marked with the
    /// `#[kernel]` attribute).  It must have a signature of
    /// `fn(ClockReset, T) -> S`.
    pub fn try_new<K>() -> Result<Self, RHDLError>
    where
        K: DigitalFn,
        K: DigitalFn2<A0 = ClockReset, A1 = T, O = S>,
    {
        Ok(Self {
            input_buffer: StreamBuffer::default(),
            func: Func::try_new::<K>()?,
        })
    }
}

/// The input for the [Map]
pub type In<T, S> = StreamIO<T, S>;

/// The output for the [Map]
pub type Out<T, S> = StreamIO<S, T>;

impl<T, S> SynchronousIO for Map<T, S>
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
    d.input_buffer.ready = ready_cast::<T, S>(i.ready);
    let o_data = if let Some(data) = q.input_buffer.data {
        d.func = data;
        Some(q.func)
    } else {
        d.func = T::dont_care();
        None
    };
    let o = Out::<T, S> {
        data: o_data,
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

    #[kernel]
    fn map_item(_cr: ClockReset, t: b4) -> b2 {
        lsbs::<2, 4>(t)
    }

    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let uut = Map::try_new::<map_item>()?;
        drc::no_combinatorial_paths(&uut)?;
        Ok(())
    }

    /// `map` wires its input buffer's `ready` straight to the
    /// downstream — the same shape that deadlocked `filter` and
    /// `filter_map`. It is safe only because it never drops: its output
    /// is `None` exactly when the buffer is empty, and then there is
    /// nothing to consume.
    ///
    /// That is safety **by accident of the operation**, not by design,
    /// so it is worth pinning down. If `map` ever gains a path that
    /// withholds output while holding an item, this test fails.
    ///
    /// Driven with `run_fn` rather than `single_stage`, because
    /// `SinkFromFn` hands its closure an acceptance report rather than
    /// the offered value and so cannot express a data-gated sink at all
    /// — see `stream::testing::sinks` for why that matters.
    #[test]
    fn map_survives_a_data_gated_sink() -> Result<(), RHDLError> {
        use crate::stream::{ready, StreamIO};
        use rhdl::core::sim::ResetOrData;

        const COUNT: u128 = 16;
        let uut = Map::<b4, b2>::try_new::<map_item>()?;
        let mut to_send: u128 = 0;
        let mut got: Vec<u128> = Vec::new();
        let mut need_reset = true;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                // Ready only when something is actually presented.
                let sink_ready = output.data.is_some();
                if let Some(d) = output.data {
                    got.push(d.raw());
                }
                let mut input = StreamIO::<b4, b2> {
                    data: None,
                    ready: ready::<b2>(sink_ready),
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

        assert_eq!(to_send, COUNT, "the source must not stall forever");
        let want: Vec<u128> = (0..COUNT).map(|k| k & 0x3).collect();
        assert_eq!(
            got, want,
            "map must deliver every item to a data-gated sink"
        );
        Ok(())
    }

    #[test]
    fn test_operation() -> Result<(), RHDLError> {
        let a_rng = XorShift128::default().map(|x| b4((x & 0xF) as u128));
        let mut b_rng = a_rng.clone();
        let a_rng = stalling(a_rng, 0.23);
        let consume = move |data: Option<b2>| {
            if let Some(data) = data {
                let orig = b_rng.next().unwrap();
                let orig_lsb = lsbs::<2, 4>(orig);
                assert_eq!(data, orig_lsb);
            }
            rand::random::<f64>() > 0.2
        };
        let map = Map::try_new::<map_item>()?;
        let uut = single_stage(map, a_rng, consume);
        // Run a few samples through
        let input = repeat_n((), 10_000).with_reset(1).clock_pos_edge(100);
        uut.run(input).for_each(drop);
        Ok(())
    }
}
