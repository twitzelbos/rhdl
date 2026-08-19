// The transmit modulator: a complex carrier scaled by a real envelope.
//
// What to look for:
//
//   - The carrier rotates (re and im in quadrature) while the envelope
//     ramps up and back down. The output is the product, so it is the
//     carrier with the envelope's shape impressed on it -- which is
//     what pulse modulation is.
//   - Both components scale together. A real envelope cannot rotate the
//     carrier, only scale it, which is exactly why this case needs two
//     multiplies rather than four.
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
use rhdl_fpga::dsp::iq::{Iq, Real};
use rhdl_fpga::dsp::mixer::complex_real::{ComplexRealMixer, In};
use rhdl_fpga::rcstream::bus::Item;

const A: usize = 18;
const B: usize = 16;
const O: usize = 18;
const P: usize = 34;
const DR: usize = 16;

fn main() -> Result<(), RHDLError> {
    let uut = ComplexRealMixer::<A, B, O, P, DR>::default();

    // A carrier at one cycle per 16 samples, and a triangular envelope.
    let n = 48i128;
    let seq: Vec<In<A, B>> = (0..n)
        .map(|k| {
            let theta = 2.0 * std::f64::consts::PI * (k as f64) / 16.0;
            let amp = 120_000.0;
            let env = if k < n / 2 { k } else { n - k } * 1200;
            In::<A, B> {
                carrier: Some(Item::<Iq<A>, ()> {
                    data: Iq::<A> {
                        re: signed::<A>((theta.cos() * amp) as i128),
                        im: signed::<A>((theta.sin() * amp) as i128),
                    },
                    frame: (),
                }),
                envelope: Some(Item::<Real<B>, ()> {
                    data: Real::<B> {
                        v: signed::<B>(env),
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
        "complex_real_mixer.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
