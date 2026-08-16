// Split a stream of pairs into two independent streams.  The `b` sink
// drains only every third cycle, so the whole widget backpressures and
// the two outputs stay index-aligned.
//
// Deterministic (no RNG).

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    rcstream::{
        bus::Item,
        tee::{In, RCStreamTee},
    },
};

fn main() -> Result<(), RHDLError> {
    let uut = RCStreamTee::<b8, bool, b4, ()>::default();
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
                let mut input = In::<b8, bool, b4, ()> {
                    data: None,
                    a_ready: !phase.is_multiple_of(5),
                    b_ready: phase.is_multiple_of(3),
                };
                if output.ready {
                    input.data = Some(Item::<(b8, b4), (bool, ())> {
                        data: (b8(sent % 256), b4(sent % 16)),
                        frame: (sent % 4 == 3, ()),
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
        "rcstream_tee.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
