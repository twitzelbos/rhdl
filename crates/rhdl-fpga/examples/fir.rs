// A six-tap FIR with deliberately asymmetric taps.
//
// `SymmetricFir` would refuse this filter twice over -- it is even
// length and it is not symmetric -- which is exactly what the general
// `Fir` is for.
//
// What to look for:
//
//   - The first stretch is a single impulse of 1024 (= 2^SHIFT, the
//     coefficients' scale). What comes out is the tap set in order:
//     512, -300, 180, -90, 40, -10. An FIR's impulse response *is* its
//     coefficients, and because these taps are asymmetric the trace
//     also pins the direction: `taps[0]` multiplies the newest sample.
//     A reversed delay line would look identical on a symmetric filter
//     and wrong here.
//   - Then the output returns to exactly zero. Six taps, six nonzero
//     outputs, no tail -- the "finite" in finite impulse response.
//   - The second stretch is a step. The output settles at the input
//     times the tap sum (332/1024, so about a third), and gets there
//     through the cumulative sum of the taps, which for an alternating
//     tap set overshoots and rings down rather than ramping.
//   - `sample: None` cycles hold the window: a FIR's state is over
//     samples, not cycles.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::fir::{Fir, In, accumulator_width};

const WI: usize = 18;
const WC: usize = 12;
const TAPS: usize = 6;
const SHIFT: usize = 10;
const WO: usize = 18;
const WACC: usize = accumulator_width(WI, WC, TAPS);

fn main() -> Result<(), RHDLError> {
    let mut t = [SignedBits::<WC>::default(); TAPS];
    for (k, v) in [512i128, -300, 180, -90, 40, -10].iter().enumerate() {
        t[k] = signed::<WC>(*v);
    }
    let uut = Fir::<WI, WC, WACC, TAPS, SHIFT, WO>::new(t);

    let sample = |v: i128| In::<WI> {
        sample: Some(signed::<WI>(v)),
        downstream_ready: true,
    };
    let idle = In::<WI> {
        sample: None,
        downstream_ready: true,
    };

    let mut seq: Vec<In<WI>> = Vec::new();
    seq.push(sample(1 << SHIFT));
    for _ in 0..9 {
        seq.push(sample(0));
    }
    seq.push(idle);
    seq.push(idle);
    for _ in 0..10 {
        seq.push(sample(3000));
    }
    seq.push(idle);

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(vcd, "fir.md", SvgOptions::default().with_io_filter())?;
    Ok(())
}
