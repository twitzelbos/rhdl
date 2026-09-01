// Split one complex stream into two real ones, carrying the framing
// marker onto both halves.
//
// What to look for:
//
//   - `real` and `imag` carry the two components of the same input
//     item, and the *same* `SyncMark`. The marker is replicated, not
//     divided: both halves describe the same instant, so both must be
//     able to say so.
//   - `ready` to the source is the AND of the two consumers'. One item
//     becomes two, so the source may advance only when both halves can
//     be taken. Watch the stretch where the imaginary consumer stalls:
//     `ready` goes low even though the real consumer is willing.
//   - An absent input gives two absent outputs. Splitting cannot invent
//     data.
//
// The pairing with `IqCombine` is the point: split, do the same thing to
// both arms, recombine, and `IqCombine::frame_mismatch` checks that the
// two arms stayed aligned. `dsp::ddc` is that pattern.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    dsp::{iq::Iq, sync::SyncMark},
    rcstream::{
        bus::Item,
        util::split::{In, IqSplit},
    },
};

const W: usize = 12;

/// Cycles over which the imaginary consumer refuses to take its half.
const STALL: std::ops::Range<u128> = 9..14;

fn main() -> Result<(), RHDLError> {
    let uut = IqSplit::<W, SyncMark>::default();
    let mut n: u128 = 0;
    let mut need_reset = true;

    let vcd = uut
        .run_fn(
            |_output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                let k = n as i128;
                n += 1;
                Some(ResetOrData::Data(In::<W, SyncMark> {
                    // Absent for two cycles, to show that splitting
                    // cannot create data.
                    stream: if (5..7).contains(&(n - 1)) {
                        None
                    } else {
                        Some(Item::<Iq<W>, SyncMark> {
                            data: Iq::<W> {
                                re: signed::<W>(100 * k),
                                im: signed::<W>(-70 * k),
                            },
                            // Marked once, on the item a downstream
                            // measurement would anchor to.
                            frame: SyncMark { sync: n - 1 == 3 },
                        })
                    },
                    real_ready: true,
                    imag_ready: !STALL.contains(&(n - 1)),
                }))
            },
            100,
        )
        .take_while(|t| t.time < 2000)
        .collect::<SvgFile>();

    write_svg_as_markdown(vcd, "iq_split.md", SvgOptions::default().with_io_filter())?;
    Ok(())
}
