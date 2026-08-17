// A single `RCStreamRelay` under intermittent backpressure.
//
// The relay is a Carloni skid buffer wearing the `RCStream` interface:
// one cycle of latency, unchanged throughput.  The trace shows what
// that buys you — when the sink withholds `ready`, the item already in
// flight is absorbed rather than lost, and delivery resumes in order
// once `ready` returns.
//
// `ready` is withheld on one cycle in three.  A relay driven by a
// permanently-ready sink would show a clean pipeline and demonstrate
// nothing, since absorbing stalls is the entire reason the widget
// exists.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    rcstream::{
        bus::{Item, RCStream},
        relay::RCStreamRelay,
    },
};

fn main() -> Result<(), RHDLError> {
    let uut: RCStreamRelay<b8, ()> = RCStreamRelay::default();

    let stream = (0..24u128)
        .map(|k| RCStream::<b8, ()> {
            data: Some(Item::<b8, ()> {
                data: b8(k % 256),
                frame: (),
            }),
            ready: !k.is_multiple_of(3),
        })
        .with_reset(1)
        .clock_pos_edge(100);

    let vcd = uut
        .run(stream)
        .take_while(|t| t.time < 1500)
        .collect::<SvgFile>();

    write_svg_as_markdown(
        vcd,
        "rcstream_relay.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
