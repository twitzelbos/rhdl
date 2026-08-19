// The full complex multiply: an Iq carrier times an Iq operand.
//
// The companion to `complex_real_mixer`. Reading the two together is the
// point -- they show the difference a real operand makes.
//
// What to look for:
//
//   - Both operands rotate. `a` advances one cycle per 16 samples and
//     `b` one per 24, in opposite senses.
//   - The output rotates at the DIFFERENCE of the two rates, because
//     multiplying complex exponentials adds their angles and `b` turns
//     backwards: 1/16 - 1/24 = 1/48 turn per sample, so the product
//     advances 7.5 degrees each cycle and completes exactly one turn
//     across the 48 samples shown. That frequency shift is what a
//     complex multiply does and what a real one cannot -- an
//     `Iq x Real` product can only be scaled, never rotated.
//   - The output magnitude is the product of the two magnitudes, held
//     constant here so the rotation is easy to read on its own. It
//     narrows to 110000^2 / 2^19 = 23079, comfortably inside the 18-bit
//     range, so nothing here is clipping.
//   - Output lags the inputs by one cycle; the product is registered.
//   - `starved` and `overrun` stay low throughout, which is the correct
//     operating condition: both inputs present a sample every cycle and
//     downstream holds `ready`. Both are fault reports rather than flow
//     control -- this mixer cannot stall, so a low `downstream_ready`
//     loses the sample instead of delaying it. See
//     `a_lost_sample_is_reported` in the module's tests for that case.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::iq::Iq;
use rhdl_fpga::dsp::mixer::complex::{ComplexMixer, In};
use rhdl_fpga::rcstream::bus::Item;

// Both operands 18 bits, output 18. Each partial product is 36 bits and
// the result sums two of them, so the natural width is 37 -- one more
// than the real-operand case. The narrowing therefore drops 19.
const A: usize = 18;
const B: usize = 18;
const O: usize = 18;
const P: usize = 37;
const DR: usize = 19;

const _: () = assert!(P == A + B + 1 && DR == P - O);

fn main() -> Result<(), RHDLError> {
    let uut = ComplexMixer::<A, B, O, P, DR>::default();

    // Two counter-rotating carriers at constant magnitude.
    let n = 48i128;
    let seq: Vec<In<A, B>> = (0..n)
        .map(|k| {
            let amp = 110_000.0;
            let theta_a = 2.0 * std::f64::consts::PI * (k as f64) / 16.0;
            let theta_b = -2.0 * std::f64::consts::PI * (k as f64) / 24.0;
            In::<A, B> {
                a: Some(Item::<Iq<A>, ()> {
                    data: Iq::<A> {
                        re: signed::<A>((theta_a.cos() * amp) as i128),
                        im: signed::<A>((theta_a.sin() * amp) as i128),
                    },
                    frame: (),
                }),
                b: Some(Item::<Iq<B>, ()> {
                    data: Iq::<B> {
                        re: signed::<B>((theta_b.cos() * amp) as i128),
                        im: signed::<B>((theta_b.sin() * amp) as i128),
                    },
                    frame: (),
                }),
                downstream_ready: true,
            }
        })
        .collect();

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "complex_mixer.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
