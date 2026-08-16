// Keep only even payloads, while never dropping an item that carries an
// end-of-frame marker — the framing-safe filtering idiom.
//
// The sink here is *data-gated*: it only asserts `ready` when it can see
// an item.  A filter that waited for downstream before discarding a
// rejected item would deadlock against this sink.
//
// Deterministic (no RNG).

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    rcstream::{bus::Item, filter::RCStreamFilter, RCStream},
};

#[kernel]
fn keep_even(_cr: ClockReset, it: Item<b8, bool>) -> bool {
    it.frame || ((it.data & b8(1)) == b8(0))
}

fn main() -> Result<(), RHDLError> {
    let uut = RCStreamFilter::<b8, bool>::try_new::<keep_even>()?;
    let mut to_send: u128 = 0;
    let mut need_reset = true;

    let vcd = uut
        .run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                let sink_ready = output.data.is_some();
                let mut input = RCStream::<b8, bool> {
                    data: None,
                    ready: sink_ready,
                };
                if output.ready {
                    input.data = Some(Item::<b8, bool> {
                        data: b8(to_send % 256),
                        frame: to_send % 8 == 7,
                    });
                    to_send += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 1500)
        .collect::<SvgFile>();

    write_svg_as_markdown(
        vcd,
        "rcstream_filter.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
