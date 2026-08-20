// Rectangular to polar and back: a vector swept once around the circle
// at constant radius, converted to magnitude and phase, and converted
// straight back.
//
// What to look for:
//
//   - `magnitude` is flat. The input radius is constant, so a varying
//     magnitude would mean the gain correction or the iteration is
//     wrong. It is the most sensitive single trace here.
//   - `phase` ramps linearly and wraps once per revolution. The wrap is
//     at the signed boundary, because a full turn is 2^18 and a half
//     turn is the most negative representable angle.
//   - `re` and `im` come back out in quadrature, matching what went in.
//   - Everything is 16 cycles late, and `valid` tracks the sample
//     through the pipeline rather than being asserted continuously.
//
// Note what this costs: 101 adders and 553 registers for one
// arithmetic operation. See the module docs before deciding to build
// one -- in a receiver the usual answer is to decimate first and
// convert in software.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::cordic::vectoring::{CordicVectoring, In};
use rhdl_fpga::dsp::iq::Iq;

const W: usize = 18;

fn main() -> Result<(), RHDLError> {
    let uut = CordicVectoring::<W>::default();

    const N: usize = 48;
    const R: f64 = 100_000.0;
    let mut seq: Vec<In<W>> = (0..N)
        .map(|k| {
            let t = std::f64::consts::TAU * k as f64 / N as f64;
            In::<W> {
                sample: Some(Iq::<W> {
                    re: signed::<W>((R * t.cos()) as i128),
                    im: signed::<W>((R * t.sin()) as i128),
                }),
            }
        })
        .collect();
    // Let the pipeline drain, so `valid` falling is visible.
    seq.extend(std::iter::repeat_n(In::<W> { sample: None }, 20));

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "cordic_magphase.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
