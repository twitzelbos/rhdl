use rhdl::prelude::*;

use crate::{
    axi4lite::{
        core::controller::write::WriteController,
        register::single::AxiRegister,
        types::{ReadMOSI, StrobedData, WriteCommand, WriteResult},
    },
    core::dff::DFF,
    rng::xorshift::{XorShift, XorShift128},
    stream::testing::{
        sink_from_fn::{AcceptCount, SinkFromFn, SinkView},
        source_from_fn::SourceFromFn,
        utils::stalling,
    },
};

#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct Fixture {
    write_source: SourceFromFn<WriteCommand>,
    write: WriteController,
    write_sink: SinkFromFn<WriteResult>,
    reg: AxiRegister,
    xor: XorShift,
    valid: DFF<bool>,
    prev_value: DFF<b32>,
}

impl Fixture {
    /// Build the fixture together with a live count of write results
    /// the sink accepted.
    ///
    /// The acceptor's `assert_eq!(res, Ok(()))` fires only when a result
    /// arrives, so a register path that completed no writes would run
    /// zero assertions and the test would pass. Assert on the count.
    // Kept for tests that want the accept counter; unused today.
    #[allow(dead_code)]
    fn new_counted() -> (Self, AcceptCount) {
        let count = AcceptCount::default();
        let sink_count = count.clone();
        (Self::build(sink_count), count)
    }
}

impl Default for Fixture {
    fn default() -> Self {
        Self::build(AcceptCount::default())
    }
}

impl Fixture {
    fn build(sink_count: AcceptCount) -> Self {
        // get a set of write commands
        let cmd = XorShift128::default().map(|x| WriteCommand {
            addr: bits(0),
            strobed_data: StrobedData {
                data: bits(x as u128),
                strobe: bits(0b1111),
            },
        });
        let cmd = stalling(cmd, 0.23);
        // For the write sink, we expect all writes to succeed
        let acceptor = move |v: SinkView<WriteResult>| {
            if let Some(res) = v.accepted {
                assert_eq!(res, Ok(()));
                sink_count.record();
            }
            rand::random_bool(0.85)
        };
        Self {
            write_source: SourceFromFn::new(cmd),
            write: WriteController::default(),
            write_sink: SinkFromFn::new(acceptor),
            reg: AxiRegister::new(bits(0), bits(0)),
            xor: XorShift::default(),
            valid: DFF::new(true),
            prev_value: DFF::new(bits(0)),
        }
    }
}

impl SynchronousIO for Fixture {
    type I = ();
    type O = ();
    type Kernel = kernel;
}

#[kernel]
#[doc(hidden)]
pub fn kernel(_cr: ClockReset, _i: (), q: Q) -> ((), D) {
    let mut d = D::dont_care();
    // Pair the source interfaces
    d.write.req_data = q.write_source;
    d.write_source = q.write.req_ready;
    // Pair the sink interfaces
    d.write.resp_ready = q.write_sink;
    d.write_sink = q.write.resp_data;
    // Pair the AXI interface to the register
    d.reg.write_axi = q.write.axi;
    d.write.axi = q.reg.write_axi;
    // Nothing on the read interface in this test case
    d.reg.read_axi = ReadMOSI::default();
    // Nothing on the core write interface in this case
    d.reg.data = None;
    d.valid = q.valid;
    d.xor = false;
    d.prev_value = q.prev_value;
    if q.reg.data != q.prev_value {
        // Register value has changed
        d.prev_value = q.reg.data;
        // Update the valid flag comparing with the XOR sequence
        d.valid = q.valid & (q.reg.data == q.xor);
        // Advance the XOR generator
        d.xor = true;
    }
    ((), d)
}

#[cfg(test)]
mod tests {
    use std::iter::repeat_n;

    use super::*;

    #[test]
    fn synth_works() -> miette::Result<()> {
        let input = repeat_n((), 100).with_reset(1).clock_pos_edge(100);
        let (uut, writes_acked) = Fixture::new_counted();
        let vcd = uut.run(input).collect::<VcdFile>();
        vcd.dump_to_file("thing.vcd").unwrap();
        // This test asserted nothing at all: its only check was the
        // acceptor's `assert_eq!(res, Ok(()))`, which fires only on
        // delivery. A register path that completed no writes would have
        // passed while dumping an empty waveform.
        writes_acked.assert_at_least(10);
        Ok(())
    }
}
