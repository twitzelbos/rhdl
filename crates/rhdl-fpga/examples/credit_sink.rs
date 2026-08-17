// A `CreditSink` buffering an incoming credit-based stream.
//
// The sink holds items in an internal `SyncFIFO` and hands credits back
// upstream as slots free.  Two things are worth watching in the trace:
//
//  * `credit_grant` dribbles out the initial pool over the first cycles
//    after reset — that pool is the FIFO's *usable* capacity,
//    `2^FIFO_N - 1`, not `2^FIFO_N`.  Granting one token too many is a
//    real bug this widget shipped with: the source spends it, the write
//    lands on a full FIFO, and the item is dropped with no error.
//  * A grant reappears each time an item is popped, so over time the
//    source's credit equals the number of free slots.
//
// `downstream_ready` is withheld on one cycle in three.  With a
// permanently-ready downstream the buffer drains as fast as it fills
// and never approaches capacity, which is exactly why that off-by-one
// survived its original test suite.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    rcstream::{bus::Item, credit::sink::CreditSink},
};

const CW: usize = 5;
const FIFO_N: usize = 3;

fn main() -> Result<(), RHDLError> {
    let uut: CreditSink<b8, (), CW, FIFO_N> = CreditSink::default();

    let stream = (0..28u128)
        .map(|k| rhdl_fpga::rcstream::credit::sink::In {
            // Offer items for the first stretch, then go quiet so the
            // buffer drains and the credit pool refills visibly.
            upstream_data: if k < 16 {
                Some(Item::<b8, ()> {
                    data: b8(k % 256),
                    frame: (),
                })
            } else {
                None
            },
            downstream_ready: !k.is_multiple_of(3),
        })
        .with_reset(1)
        .clock_pos_edge(100);

    let vcd = uut
        .run(stream)
        .take_while(|t| t.time < 1500)
        .collect::<SvgFile>();

    write_svg_as_markdown(
        vcd,
        "credit_sink.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
