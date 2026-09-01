// A stream source that emits one fixed item, every cycle, forever.
//
// What to look for:
//
//   - `data` is `Some` on every cycle including the first post-reset
//     one, and never changes. There is no state to initialise, so there
//     is no start-up transient.
//   - Reset does not change the value. The value lives in a
//     `Constant`, not a register, so a reset has nothing to clear --
//     which is what makes this usable as the quiescent input to a
//     widget under test.
//   - `ready` from downstream is ignored, deliberately. An infinite
//     source is always able to present its item, so backpressure has
//     nothing to act on: the item is simply still there next cycle.
//
// Its use is as a test and bring-up source, and as the tie-off for a
// stream input a design does not drive.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::{
    doc::write_svg_as_markdown, dsp::sync::SyncMark, rcstream::util::constant::RCStreamConstant,
};

fn main() -> Result<(), RHDLError> {
    // Marked, to show that the framing is part of the constant rather
    // than something a consumer has to supply.
    let uut = RCStreamConstant::<b8, SyncMark>::new(b8(0x5A), SyncMark { sync: true });

    let vcd = uut
        .run((0..12).map(|_| ()).with_reset(2).clock_pos_edge(100))
        .collect::<SvgFile>();

    write_svg_as_markdown(
        vcd,
        "rcstream_constant.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
