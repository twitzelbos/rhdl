// Halve even payloads and drop odd ones in a single pass, keeping frame
// markers attached.
//
// Deterministic (no RNG).

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::{
    core::slice::lsbs,
    doc::write_svg_as_markdown,
    rcstream::{bus::Item, filter_map::RCStreamFilterMap, RCStream},
};

#[kernel]
fn halve_even(_cr: ClockReset, it: Item<b8, bool>) -> Option<Item<b4, bool>> {
    if it.frame || ((it.data & b8(1)) == b8(0)) {
        Some(Item::<b4, bool> {
            data: lsbs::<4, 8>(it.data >> 1),
            frame: it.frame,
        })
    } else {
        None
    }
}

fn main() -> Result<(), RHDLError> {
    let uut = RCStreamFilterMap::<b8, bool, b4>::try_new::<halve_even>()?;
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
        "rcstream_filter_map.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
