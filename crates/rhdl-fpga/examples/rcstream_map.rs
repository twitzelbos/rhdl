// Map a stream of `b8` payloads down to `b4`, preserving the framing
// marker on every item.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::{
    core::slice::lsbs,
    doc::write_svg_as_markdown,
    rcstream::{RCStream, bus::Item, map::RCStreamMap},
};

#[kernel]
fn narrow(_cr: ClockReset, t: b8) -> b4 {
    lsbs::<4, 8>(t)
}

fn main() -> Result<(), RHDLError> {
    let uut = RCStreamMap::<b8, bool, b4>::try_new::<narrow>()?;
    let mut to_send: u128 = 0;
    let mut need_reset = true;
    let mut phase: u32 = 0;

    let vcd = uut
        .run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                let sink_ready = !phase.is_multiple_of(3);
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
        "rcstream_map.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
