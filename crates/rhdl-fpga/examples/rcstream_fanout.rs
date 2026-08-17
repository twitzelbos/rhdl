// Broadcasting one `RCStream` to three sinks that drain at different
// rates.
//
// Every branch receives every item. The interesting part of the trace is
// what happens between acceptances: once a fast branch has taken the
// item, its `data` goes back to `None` while the slower branches are
// still being offered it. That per-branch delivery state is the whole
// point of the widget — without it the fast branch would be handed the
// same item on every cycle until the slowest one caught up.
//
// The three branches accept on 1-in-2, 1-in-3 and 1-in-5 cycles. The
// cadences are coprime on purpose: equal rates would let all three
// retire together on every item and the hold would never be exercised.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    rcstream::{bus::Item, fanout::RCStreamFanout},
};

fn main() -> Result<(), RHDLError> {
    let uut: RCStreamFanout<b8, (), 3> = RCStreamFanout::default();

    let stream = (0..36u128)
        .map(|k| rhdl_fpga::rcstream::fanout::In::<b8, (), 3> {
            data: if k.is_multiple_of(4) {
                None
            } else {
                Some(Item::<b8, ()> {
                    data: b8(k % 256),
                    frame: (),
                })
            },
            ready: [
                k.is_multiple_of(2),
                k.is_multiple_of(3),
                k.is_multiple_of(5),
            ],
        })
        .with_reset(1)
        .clock_pos_edge(100);

    let vcd = uut
        .run(stream)
        .take_while(|t| t.time < 1500)
        .collect::<SvgFile>();

    write_svg_as_markdown(
        vcd,
        "rcstream_fanout.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
