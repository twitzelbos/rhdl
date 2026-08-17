// Quadrature phase-to-amplitude conversion: one full turn of phase in,
// sine and cosine out.
//
// The phase increment is chosen so that 2^22 / 65536 = 64 samples make
// exactly one revolution, which is what puts a whole cycle of both
// components in the window. Two turns are shown so the periodicity is
// visible rather than inferred.
//
// What to look for:
//
//   - `sin` and `cos` are a quarter cycle apart. That separation is not
//     a second table: one quarter-wave table is read at two addresses,
//     because cos(t) = sin(t + pi/2) and pi/2 is a constant offset in
//     phase units.
//   - Both reach the rails without wrapping. The table is scaled one LSB
//     below full scale precisely so the interpolated sum cannot leave
//     the 18-bit range at the peaks -- see the module docs, where
//     wrapping costs up to 96 dB of SFDR.
//   - Outputs lag `phase` by two cycles: the block RAM read is
//     registered, and the quadrant/fine attributes are delayed to match
//     it.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    dsp::nco::sin_cos_linear_interp::{In, SinCosLinearInterp, TOTAL_W},
};

fn main() -> Result<(), RHDLError> {
    let uut = SinCosLinearInterp::default();

    // 64 samples per revolution, two revolutions.
    const STEP: u128 = 65536;
    let stream = (0..128u128)
        .map(|k| In {
            phase: bits::<TOTAL_W>((k * STEP) % (1 << TOTAL_W)),
        })
        .with_reset(1)
        .clock_pos_edge(100);

    let vcd = uut.run(stream).collect::<SvgFile>();

    write_svg_as_markdown(
        vcd,
        "sin_cos_linear_interp.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
