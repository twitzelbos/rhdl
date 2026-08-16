//! Zip Stream Core
//!
//!# Purpose
//!
//! A [Zip] Core takes 2 streams as inputs and yields a
//! single pipeline of outputs consisting of tuples formed
//! from the two input pipelines.  It is roughly equivalent to
//! the `.zip()` method on iterators.  The [Zip] propogates
//! backpressure up the two source pipes.
//!
//!# Schematic Symbol
//!
//! Here is the schematic symbol for the [Zip] core
//!
#![doc = badascii_formal!("
        +--+Zip+---------+        
  ?S    |                |        
+------>|a.data          |        
 R<S>   |                | ?(S,T) 
 <------+a.ready     data+------> 
  ?T    |                | R<(S,T)>       
+------>|b.data     ready|<------+
 R<T>   |                |        
 <------+b.ready         |        
        +----------------+        
")]
//!
//!# Internals
//!
//! The [Zip] uses input FIFOs to buffer incoming data elements
//! on each of the two upstream pipes, and then advances them to the
//! output FIFO when both are ready.  Otherwise, the control logic
//! is straightfoward, and purely combinatorial.
//!
#![doc = badascii!(r"
      ++Stm2FIFO+--+    ++unpck+-+     +conct++                                        
  ?S  |            | ?S |        |  S  |      |      +-+pack+-+      ++FIFO2Stm+       
+---->| data  data +--->|in   out+---->|.0    |      |        |?(S,T)|         | ?(S,T)
 R<S> |            |    |        |     |   out+----->|data out+----->|in    out+------>
 <----+ ready next |<+  |     tag+-+ +>|.1    |      |        |      |         | R<(S,T)>      
      |            | |  |        | | | |      |   +->|tag     |   +--+full  rdy|<-----+
      +------------+ |  +--------+ | | +------+   |  |        |   |  |         |       
                     |             +-+---------+  |  +--------+   |  +---------+       
      ++Stm2FIFO+--+ +  ++unpck+-+   |         v  |               |                    
  ?T  |            | ?T |        | T |    +-------+-------+       |                    
+---->| data  data +--->|in   out+---+    |               |       |                    
 R<T> |            | +  |        |        |    Control    |<------+                    
 <----+ ready next |<+  |     tag+------->|               |                            
      |            | |  |        |        |               |                            
      +------------+ |  +--------+        +-+-------------+                            
                     |                      |                                          
                     +----------------------+                                          
")]
//!
//!# Example
//!
//! An example of using a [Zip] is here.
//!
//!```
#![doc = include_str!("../../examples/zip.rs")]
//!```
//!
//! With the resulting trace.
//!
#![doc = include_str!("../../doc/zip.md")]

use badascii_doc::{badascii, badascii_formal};
use rhdl::prelude::*;

use crate::stream::{fifo_to_stream::FIFOToStream, stream_to_fifo::StreamToFIFO};

use super::Ready;

#[derive(Debug, Clone, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
/// The [Zip] Core
///
/// This core takes two streams.  One of type
/// `S`, and one of type `T`, and generates a stream
/// of `(S,T)` elements.
pub struct Zip<S: Digital, T: Digital> {
    a_buffer: StreamToFIFO<S>,
    b_buffer: StreamToFIFO<T>,
    out_buffer: FIFOToStream<(S, T)>,
}

#[derive(PartialEq, Clone, Copy, Digital)]
/// Input struct for the [Zip]
pub struct In<S: Digital, T: Digital> {
    /// Input data for the `a` stream
    pub a_data: Option<S>,
    /// Input data for the `b` stream
    pub b_data: Option<T>,
    /// Ready signal for the downstream
    pub ready: Ready<(S, T)>,
}

#[derive(PartialEq, Clone, Copy, Digital)]
/// Output struct for the [Zip]
pub struct Out<S: Digital, T: Digital> {
    /// Ready signal for the `a`` stream
    pub a_ready: Ready<S>,
    /// Ready signal for the `b` stream
    pub b_ready: Ready<T>,
    /// Output data containing the tuples
    pub data: Option<(S, T)>,
}

impl<S: Digital, T: Digital> SynchronousIO for Zip<S, T> {
    type I = In<S, T>;
    type O = Out<S, T>;
    type Kernel = kernel<S, T>;
}

#[kernel]
#[doc(hidden)]
pub fn kernel<S: Digital, T: Digital>(
    _cr: ClockReset,
    i: In<S, T>,
    q: Q<S, T>,
) -> (Out<S, T>, D<S, T>) {
    let mut d = D::<S, T>::dont_care();
    d.a_buffer.data = i.a_data;
    d.b_buffer.data = i.b_data;
    let mut out_data = None;
    let mut next = false;
    if !q.out_buffer.full {
        if let Some::<S>(data_a) = q.a_buffer.data {
            if let Some::<T>(data_b) = q.b_buffer.data {
                out_data = Some((data_a, data_b));
                next = true;
            }
        }
    }
    d.a_buffer.next = next;
    d.b_buffer.next = next;
    d.out_buffer.data = out_data;
    d.out_buffer.ready = i.ready;
    let o = Out::<S, T> {
        a_ready: q.a_buffer.ready,
        b_ready: q.b_buffer.ready,
        data: q.out_buffer.data,
    };
    (o, d)
}

#[cfg(test)]
mod tests {

    /// Both inputs data-gated **and** skewed against each other.
    ///
    /// `Zip` holds one side while waiting for the other, so it presents
    /// `None` downstream while still needing to accept — the
    /// absorb-without-emitting shape that deadlocked `stream::filter`.
    /// A sink withholding `ready` when it sees nothing is legal, and
    /// pairs must still come out index-aligned however skewed the
    /// arrivals are.
    #[test]
    fn data_gated_sink_does_not_stall_or_shear_the_zip() -> Result<(), RHDLError> {
        use crate::stream::ready;
        use rhdl::core::sim::ResetOrData;

        const COUNT: u128 = 12;
        let uut = Zip::<b4, b4>::default();
        let (mut a_sent, mut b_sent) = (0u128, 0u128);
        let mut got: Vec<(u128, u128)> = Vec::new();
        let mut need_reset = true;
        let mut phase: u32 = 0;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                let sink_ready = output.data.is_some();
                if let Some((x, y)) = output.data {
                    got.push((x.raw(), y.raw()));
                }
                let mut input = super::In::<b4, b4> {
                    a_data: None,
                    b_data: None,
                    ready: ready::<(b4, b4)>(sink_ready),
                };
                if a_sent < COUNT && output.a_ready.raw {
                    input.a_data = Some(b4(a_sent % 16));
                    a_sent += 1;
                }
                // `b` offers on a different cadence, so the sides skew.
                if b_sent < COUNT && output.b_ready.raw && !phase.is_multiple_of(3) {
                    input.b_data = Some(b4((15 - b_sent) % 16));
                    b_sent += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 300_000)
        .for_each(drop);

        let want: Vec<(u128, u128)> = (0..COUNT).map(|k| (k % 16, (15 - k) % 16)).collect();
        assert_eq!(
            got, want,
            "pairs must stay index-aligned and none may be lost"
        );
        Ok(())
    }
    use std::iter::repeat_n;

    use super::*;
    use crate::{
        rng::xorshift::XorShift128,
        stream::testing::{
            sink_from_fn::SinkFromFn, source_from_fn::SourceFromFn, utils::stalling,
        },
    };

    #[derive(Clone, Synchronous, SynchronousDQ)]
    #[rhdl(dq_no_prefix)]
    struct TestFixture {
        a_source: SourceFromFn<b4>,
        b_source: SourceFromFn<b6>,
        zip: Zip<b4, b6>,
        sink: SinkFromFn<(b4, b6)>,
    }

    impl SynchronousIO for TestFixture {
        type I = ();
        type O = ();
        type Kernel = kernel;
    }

    #[kernel]
    pub fn kernel(_cr: ClockReset, _i: (), q: Q) -> ((), D) {
        let mut d = D::dont_care();
        d.zip.a_data = q.a_source;
        d.zip.b_data = q.b_source;
        d.sink = q.zip.data;
        d.zip.ready = q.sink;
        d.a_source = q.zip.a_ready;
        d.b_source = q.zip.b_ready;
        ((), d)
    }

    #[test]
    fn test_operation() -> Result<(), RHDLError> {
        let a_rng = XorShift128::default().map(|x| b4((x & 0xF) as u128));
        let b_rng = XorShift128::default().map(|x| b6(((x >> 8) & 0x3F) as u128));
        let c_rng = a_rng.clone().zip(b_rng.clone());
        let a_rng = stalling(a_rng, 0.23);
        let b_rng = stalling(b_rng, 0.15);
        let uut = TestFixture {
            a_source: SourceFromFn::new(a_rng),
            b_source: SourceFromFn::new(b_rng),
            zip: Zip::default(),
            sink: SinkFromFn::new_from_iter(c_rng, 0.2),
        };
        // Run a few samples through
        let input = repeat_n((), 10_000).with_reset(1).clock_pos_edge(100);
        uut.run(input).for_each(drop);
        Ok(())
    }
}
