// A width-tapered CIC interpolator: each stage only as wide as it needs
// to be.
//
// What to look for:
//
//   - The output is *bit-identical* to `cic_interpolate`'s at the same
//     configuration. That is the whole claim, and it is the difference
//     from `cic_pruned`: a pruned decimator discards low-order bits and
//     trades noise for area, so its correctness argument is an error
//     budget. A tapered interpolator discards nothing -- each stage is
//     sized to its own exact growth bound, holds its value at LSB weight
//     one, and there is no shift anywhere in the datapath.
//   - `input_ready` still pulses once per rate, and the rate is still an
//     input. The taper is invisible from outside: the widget presents
//     the same In/Out as `CicInterpolate`, so it drops into a
//     `StreamInterpolator`, an `EnvelopeUpsampler`, or either
//     up-converter without any of them knowing.
//   - The envelope steps partway through, marked with a restart, which
//     clears every stage at once -- the comb delay lines as well as the
//     integrators.
//
// The saving is in the emitted Verilog rather than in the waveform. At
// w_in = 12, N = 3, R_MAX = 32 the six stages are 13, 14, 15, 15, 18 and
// 22 bits: 97 register bits against 132 for a uniform datapath, a 27%
// reduction for no error at all.
//
// Note the fourth stage. The exact bounds are not monotonic -- the last
// comb needs 15 bits and the first integrator only 14, because
// zero-stuffing divides the signal by R faster than one integrator
// re-grows it by R*M. The generated widths take the running maximum, so
// the 14 becomes a 15 and every inter-stage transfer is a widening. That
// costs one bit and removes an entire class of scaling error.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::cic_interp_tapered;
use rhdl_fpga::core::dff;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::cic::{interp, interpolator};

const WI: usize = 12;
const RMAX: usize = 32;
const CW: usize = interp::rate_width(RMAX);

/// The rate used in the trace: small enough to read.
const RATE: usize = 4;

/// Cycle the second, restarted, envelope begins on.
const SWITCH: usize = 24;

cic_interp_tapered!(TxInterp, w_in = 12, n = 3, r_max = 32, m = 1);

fn main() -> Result<(), RHDLError> {
    let uut = TxInterp::default();

    let seq: Vec<interpolator::In<WI, CW>> = (0..SWITCH + 28)
        .map(|n| interpolator::In::<WI, CW> {
            sample: Some(signed::<WI>(if n < SWITCH { 200 } else { -400 })),
            rate: bits::<CW>(RATE as u128),
            restart: n == SWITCH,
            downstream_ready: true,
        })
        .collect();

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "cic_interp_tapered.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
