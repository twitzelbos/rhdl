// A two-stage CIC interpolator, run at two different rates.
//
// What to look for:
//
//   - `input_ready` pulses once every R cycles. That is the widget
//     asking for its next low-rate sample, and it is the only flow
//     control on the input side: an upstream that ignores it will be
//     sampled on the widget's grid, not its own.
//   - `sample` on the output is present on *every* cycle. An
//     interpolator emits continuously -- that is what it is for -- so
//     the output is a plain value rather than an `Option`.
//   - The first stretch runs at R = 4 with a constant input of 20. The
//     output climbs through the cascade's fill and then settles at
//     exactly 80 = 20 * (R*M)^N / R = 20 * 4. Note the `/R`: the
//     transfer function's DC gain is (R*M)^N = 16, but zero-stuffing
//     divides the signal by R on the way in, so the factor a caller has
//     to undo is 4, not 16.
//   - Then the rate changes to 8 *with a restart on the same cycle*.
//     The output drops to zero -- the restart clearing the cascade --
//     and refills to 160 = 20 * 8.
//   - The restart is not optional here, and this is the trace's real
//     lesson. Changing `rate` alone would have left the output sitting
//     at 80 forever: the integrators only move when the comb section
//     feeds them, the comb section computes the Nth difference of the
//     input, and the Nth difference of a constant is zero. With nothing
//     arriving, the integrators hold whatever the old rate left them.
//   - `starved` stays low throughout, because a sample is presented on
//     every cycle. It would fire if an `input_ready` cycle found
//     `None`, and the widget would feed zero -- decaying to silence
//     rather than freezing a DC offset onto the DAC.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::cic::interpolator::{CicInterpolate, In};

const WI: usize = 8;
const WA: usize = 11;
const S: usize = 2;
const RMAX: usize = 8;
const M: usize = 1;
const CW: usize = 4;

/// Cycles spent at the first rate before switching.
const AT_FOUR: usize = 28;

fn main() -> Result<(), RHDLError> {
    let uut = CicInterpolate::<WI, WA, S, RMAX, M, CW>::default();

    let seq: Vec<In<WI, CW>> = (0..AT_FOUR + 40)
        .map(|n| {
            let rate = if n < AT_FOUR { 4 } else { 8 };
            In::<WI, CW> {
                sample: Some(signed::<WI>(20)),
                rate: bits::<CW>(rate),
                // The restart is what makes the new rate take effect;
                // see the note above.
                restart: n == AT_FOUR,
                downstream_ready: true,
            }
        })
        .collect();

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "cic_interpolate.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
