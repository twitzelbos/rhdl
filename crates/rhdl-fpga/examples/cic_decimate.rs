// A two-stage CIC decimating by four.
//
// What to look for:
//
//   - `sample` on the output is present on one cycle in four. That is
//     the decimation: the filter consumes at the input rate and emits
//     at a quarter of it.
//   - The input is a constant 100 for the first stretch. The output
//     climbs and then settles at exactly 1600 = 100 * (R*M)^N =
//     100 * 4^2. A CIC does not normalise its gain, and the settled
//     value is how you confirm the cascade depth is what you think.
//   - Then the input switches to a tone at fs/4 -- exactly the first
//     null of the sinc^2 response. The output collapses toward zero.
//     That null is the whole reason a CIC is the right filter in front
//     of a decimator: it puts its zeros precisely where the decimation
//     would otherwise fold energy back into the band.
//   - `overrun` stays low. It is a fault report, not flow control: this
//     filter cannot stall, because its state is a running sum tied to
//     the input stream.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::cic::decimator::{CicDecimate, In};

const WI: usize = 8;
const WA: usize = 12;
const S: usize = 2;
const R: usize = 4;
const M: usize = 1;
const CW: usize = 2;

fn main() -> Result<(), RHDLError> {
    let uut = CicDecimate::<WI, WA, S, R, M, CW>::default();

    let seq: Vec<In<WI>> = (0..48i128)
        .map(|k| {
            // First half: DC.  Second half: a tone at the first null.
            let v = if k < 24 {
                100
            } else {
                // cos(2*pi*k/4) * 100 -> +100, 0, -100, 0, ...
                match k % 4 {
                    0 => 100,
                    2 => -100,
                    _ => 0,
                }
            };
            In::<WI> {
                sample: Some(signed::<WI>(v)),
                downstream_ready: true,
            }
        })
        .collect();

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "cic_decimate.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
