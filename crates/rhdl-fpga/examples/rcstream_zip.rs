// Combine two streams into one stream of pairs.  The `b` source offers
// items only every third cycle, so `a` is backpressured until its
// partner catches up — the pairs stay index-aligned regardless.
//
// Deterministic (no RNG).

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    rcstream::{
        bus::Item,
        zip::{In, RCStreamZip},
    },
};

fn main() -> Result<(), RHDLError> {
    let uut = RCStreamZip::<b8, bool, b4, ()>::default();
    let mut a_sent: u128 = 0;
    let mut b_sent: u128 = 0;
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
                    a_data: None,
                    b_data: None,
                    ready: !phase.is_multiple_of(4),
                };
                if output.a_ready {
                    input.a_data = Some(Item::<b8, bool> {
                        data: b8(a_sent % 256),
                        frame: a_sent % 4 == 3,
                    });
                    a_sent += 1;
                }
                if output.b_ready && phase.is_multiple_of(3) {
                    input.b_data = Some(Item::<b4, ()> {
                        data: b4(b_sent % 16),
                        frame: (),
                    });
                    b_sent += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 1500)
        .collect::<SvgFile>();

    write_svg_as_markdown(
        vcd,
        "rcstream_zip.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
