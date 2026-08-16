// Gather a stream of elements into 4-element chunks.
//
// The chunk's framing is the array of its elements' markers, positionally
// aligned with the payload — nothing is discarded, so `chunked` followed
// by `flatten` is lossless.
//
// Deterministic (no RNG).

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    rcstream::{bus::Item, chunked::RCStreamChunked, RCStream},
};

fn main() -> Result<(), RHDLError> {
    let uut = RCStreamChunked::<b8, bool, 3, 4>::default();
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
                let mut input = RCStream::<b8, bool> {
                    data: None,
                    ready: !phase.is_multiple_of(4),
                };
                if output.ready {
                    input.data = Some(Item::<b8, bool> {
                        data: b8(sent % 256),
                        frame: sent.is_multiple_of(3),
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
        "rcstream_chunked.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
