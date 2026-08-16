// Expand a stream of 4-element arrays into a stream of elements.
//
// Each emitted element carries the group's original framing marker plus
// a flag that is true only on the last element of that group, so no
// framing information is invented or lost.
//
// Deterministic (no RNG).

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    rcstream::{bus::Item, flatten::RCStreamFlatten, RCStream},
};

fn main() -> Result<(), RHDLError> {
    let uut = RCStreamFlatten::<b8, bool, 3, 4>::default();
    let mut sent: u128 = 0;
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
                let mut input = RCStream::<[b8; 4], bool> {
                    data: None,
                    ready: !phase.is_multiple_of(3),
                };
                if output.ready {
                    let b = sent * 4;
                    input.data = Some(Item::<[b8; 4], bool> {
                        data: [
                            b8(b % 256),
                            b8((b + 1) % 256),
                            b8((b + 2) % 256),
                            b8((b + 3) % 256),
                        ],
                        frame: sent.is_multiple_of(2),
                    });
                    sent += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 1500)
        .collect::<SvgFile>();

    write_svg_as_markdown(
        vcd,
        "rcstream_flatten.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
