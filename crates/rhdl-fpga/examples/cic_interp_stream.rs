// A CIC interpolator behind an RCStream front end, interpolating by
// four.
//
// What to look for:
//
//   - `stream.ready` on the output pulses once every four cycles. This
//     is the half of the RCStream contract that a decimator does not
//     really use: `StreamDecimator` passes `downstream_ready` straight
//     through, because a decimator never refuses a sample. An
//     interpolator consumes one sample per R cycles, so its `ready` is
//     a genuine request and this widget is the rate-controlling element
//     of a transmit chain.
//   - `stream.data` is present on every cycle. An interpolator emits
//     continuously; the `Option` is there because `RCStream` requires
//     the shape, not because there are idle cycles.
//   - The envelope steps from 10 to -30 partway through. Watch the
//     output move between the two levels smoothly rather than in one
//     jump: two stages of CIC is linear interpolation, so the
//     transition takes a whole window.
//   - The second half is marked on its first sample, and the mark rides
//     out on the first output of the new window. The output drops to
//     zero on that cycle -- the restart clearing the cascade -- and
//     refills.
//   - The mark is presented for several cycles before it is taken,
//     because upstream holds a sample until `ready` comes up. It
//     restarts the window exactly once, on the cycle it is consumed.
//     Restarting on every cycle the mark was visible would re-clear the
//     cascade continuously and the output would never leave zero.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::cic::interp_stream::{In, StreamInterpolator};
use rhdl_fpga::dsp::cic::interpolator::CicInterpolate;
use rhdl_fpga::dsp::iq::Real;
use rhdl_fpga::dsp::sync::SyncMark;
use rhdl_fpga::rcstream::Item;

const WI: usize = 8;
const WA: usize = 11;
const S: usize = 2;
const RMAX: usize = 8;
const M: usize = 1;
const CW: usize = 4;
const RATE: usize = 4;

/// Cycle the second, marked, burst begins on.
const SWITCH: usize = 24;

type Core = CicInterpolate<WI, WA, S, RMAX, M, CW>;

fn main() -> Result<(), RHDLError> {
    let uut = StreamInterpolator::<WI, WA, CW, Core>::default();

    let seq: Vec<In<WI, CW>> = (0..SWITCH + 28)
        .map(|n| {
            let v = if n < SWITCH { 10 } else { -30 };
            In::<WI, CW> {
                stream: Some(Item::<Real<WI>, SyncMark> {
                    data: Real::<WI> { v: signed::<WI>(v) },
                    // Held high for a whole window, the way a real
                    // upstream holds a sample until `ready`. It is
                    // consumed once.
                    frame: SyncMark {
                        sync: (SWITCH..SWITCH + RATE).contains(&n),
                    },
                }),
                rate: bits::<CW>(RATE as u128),
                downstream_ready: true,
            }
        })
        .collect();

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "cic_interp_stream.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
