// A sample-synchronous modulation stream contributing to the frequency
// word, and what happens when it stops.
//
// What to look for:
//
//   - `word` follows the stream: a two's-complement contribution that
//     goes negative for negative samples, so it lowers the frequency
//     when the composer adds it.
//   - Partway through, the stream stops. `absent` asserts and the
//     contribution returns to **zero**, not to the last value. A
//     compensation value is specific to a moment -- eddy-current decay
//     is a function of time since the gradient event -- so a held-over
//     correction is confidently wrong rather than merely stale.
//   - `stale` latches once the stream has stopped after having started,
//     distinguishing a dead stream from one that never began. Only the
//     first is a fault.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::nco::modulation::{In, MOD_W, ModulationInput};
use rhdl_fpga::rcstream::bus::Item;

fn main() -> Result<(), RHDLError> {
    let uut = ModulationInput::default();

    let sample = |v: i128| In {
        stream: Some(Item::<SignedBits<MOD_W>, ()> {
            data: signed::<MOD_W>(v),
            frame: (),
        }),
    };
    let gap = In { stream: None };

    // A slow sweep through zero, then the stream stops.
    let mut seq: Vec<In> = (0..24i128).map(|k| sample((k - 12) * 2000)).collect();
    seq.extend(std::iter::repeat_n(gap, 12));

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "nco_modulation.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
