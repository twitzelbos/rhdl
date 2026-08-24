// The same two-stage, decimate-by-four CIC as `cic_decimate`, but with
// a Hogenauer-pruned datapath.
//
// What to look for:
//
//   - Compare against the `cic_decimate` trace. Same stimulus, same
//     shape, same nulls -- pruning changes the arithmetic, not the
//     filter.
//   - The settled DC value is 400, not 1600. Both are the same answer:
//     the gain is still (R*M)^N = 16, but this datapath's output LSB
//     weighs four, because the last comb discarded two low-order bits.
//     A pruned CIC returns a coarser number, not a different one.
//   - Then the input switches to a tone at fs/4, the first null of the
//     sinc^2 response, and the output collapses toward zero exactly as
//     the full-width version does. The nulls are a property of the
//     structure and pruning does not move them.
//   - The whole taper for this configuration is 12, 11, 11, 10 bits
//     against a uniform 12 -- 44 bits of state instead of 48. The
//     saving grows sharply with depth and rate: at N = 5, R = 1024 it
//     is 517 bits instead of 680.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::cic_pruned;
use rhdl_fpga::core::dff;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::cic::decimator::In;

cic_pruned!(PrunedCic, w_in = 8, n = 2, r = 4, m = 1, b_out = 4);

const WI: usize = 8;

fn main() -> Result<(), RHDLError> {
    let uut = PrunedCic::default();

    let seq: Vec<In<WI>> = (0..48i128)
        .map(|k| {
            // First half: DC.  Second half: a tone at the first null.
            let v = if k < 24 {
                100
            } else {
                match k % 4 {
                    0 => 100,
                    2 => -100,
                    _ => 0,
                }
            };
            In::<WI> {
                sample: Some(signed::<WI>(v)),
                restart: false,
                downstream_ready: true,
            }
        })
        .collect();

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(vcd, "cic_pruned.md", SvgOptions::default().with_io_filter())?;
    Ok(())
}
