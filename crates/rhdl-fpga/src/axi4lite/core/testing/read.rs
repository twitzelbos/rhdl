//! Test harness for Read Controller and Read Endpoint
//!
#![doc = badascii!(r"

                      ++ReadController++                  ++ReadEndpoint++      +ReqSink+       
+ReqSource+           |  sink          | araddr    araddr |      source  |?Axil |       |       
|         |?AxilAddr  |                +--------->------->|              | Addr |       |       
|         +---------->| req.data       | arvalid  arvalid |    req.data  +----->|       |       
|         |           |                +--------->------->|              |      |       |       
|         |<----------+ req.ready      | arready  arready |    req.ready |<----+|       |       
|         |           |                |<--------+--------+              |      +-------+       
+---------+           |  - - - - - -   |                  |  - - - - - - |                      
                      |                | rdata    rdata   |              |                      
                      |                |<--------+--------+              |                      
                      |  source        | rresp    rresp   |      sink    |            +ReplySrc+
+ReplySink+?ReadResult|                |<--------+--------+              | ?ReadResult|        |
|         |<----------+ resp.data      | rvalid   rvalid  |   resp.data  |<-----------+        |
|         |           |                |<--------+--------+              |            |        |
|         +---------->| resp.ready     | rready   rready  |   resp.ready +----------->|        |
+---------+           |                +--------->------->+              |            +--------+
                      +----------------+                  +--------------+                      
")]

use badascii_doc::badascii;
use rhdl::prelude::*;

use crate::{
    axi4lite::{
        core::{controller::read::ReadController, endpoint::read::ReadEndpoint},
        types::ReadResult,
    },
    stream::testing::{sink_from_fn::SinkFromFn, source_from_fn::SourceFromFn},
};

#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct TestFixture {
    req_source: SourceFromFn<b32>,
    controller: ReadController,
    endpoint: ReadEndpoint,
    req_sink: SinkFromFn<b32>,
    reply_source: SourceFromFn<ReadResult>,
    reply_sink: SinkFromFn<ReadResult>,
}

impl SynchronousIO for TestFixture {
    type I = ();
    type O = ();
    type Kernel = kernel;
}

#[kernel]
pub fn kernel(_cr: ClockReset, _i: (), q: Q) -> ((), D) {
    let mut d = D::dont_care();
    // Wire the request source to the read controller
    d.controller.req_data = q.req_source;
    d.req_source = q.controller.req_ready;
    // Wire the request sink to the read endpoint
    d.req_sink = q.endpoint.req_data;
    d.endpoint.req_ready = q.req_sink;
    // Wire the controller to the reply sink
    d.reply_sink = q.controller.resp_data;
    d.controller.resp_ready = q.reply_sink;
    // Wire the endpoint to the reply source
    d.endpoint.resp_data = q.reply_source;
    d.reply_source = q.endpoint.resp_ready;
    // Wire the AXI busses together.
    d.controller.axi = q.endpoint.axi;
    d.endpoint.axi = q.controller.axi;
    ((), d)
}

#[cfg(test)]
mod tests {
    use std::iter::repeat_n;

    use crate::{rng::xorshift::XorShift128, stream::testing::utils::stalling_with_seed};

    use super::*;

    #[test]
    fn test_controller_endpoint() -> Result<(), RHDLError> {
        let rng = XorShift128::default().map(|x| bits(x as u128));
        let address_sink = rng.clone();
        // Address and reply stall at the same rate, so they need
        // explicit, distinct seeds: sharing one would stall both channels
        // on identical cycles and never test a blocked request against a
        // flowing response.
        let address = stalling_with_seed(rng.clone(), 0.23, 0x00A1_5E01);
        let reply = rng.clone().map(ReadResult::Ok);
        let reply_sink = reply.clone();
        let reply = stalling_with_seed(reply, 0.23, 0x00B2_5E02);
        let uut = TestFixture {
            req_source: SourceFromFn::new(address),
            controller: ReadController::default(),
            endpoint: ReadEndpoint::default(),
            // Same rate on both sinks — distinct seeds, same reasoning as
            // the sources above.
            req_sink: SinkFromFn::new_from_iter_with_seed(address_sink, 0.1, 0x00C3_5E03),
            reply_source: SourceFromFn::new(reply),
            reply_sink: SinkFromFn::new_from_iter_with_seed(reply_sink, 0.1, 0x00D4_5E04),
        };
        let input = repeat_n((), 250);
        let input = input.with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        vcd.dump_to_file("axi_read.vcd")?;
        Ok(())
    }
}
