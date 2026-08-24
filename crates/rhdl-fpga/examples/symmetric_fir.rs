// A seven-tap symmetric FIR, shown by its impulse response.
//
// What to look for:
//
//   - The first stretch is a single impulse of 1024 (= 2^SHIFT, the
//     coefficients' scale). What comes out is the tap set itself:
//     -20, 60, 180, 584, 180, 60, -20. An FIR's impulse response *is*
//     its coefficients, so this trace is the filter's definition made
//     visible -- and it is symmetric, which is the linear-phase
//     property the widget is named for.
//   - Then the output returns to exactly zero and stays there. That is
//     the "finite" in finite impulse response: seven taps, seven
//     nonzero outputs, no tail.
//   - The second stretch is a step. The output settles at exactly the
//     input, 2000, because these taps sum to 2^SHIFT and so have unity
//     DC gain. The ramp up to it traces the cumulative sum of the taps
//     -- and it overshoots to 2039 on the way, which is not an error:
//     the partial sums of a mildly high-boosting tap set exceed the
//     total. A compensator overshoots a step by construction, because
//     lifting the band edge back up is exactly what it is for.
//   - `sample: None` cycles appear between the two stretches and the
//     output holds. A FIR's window is over samples, not over cycles,
//     so a gap must not be read as a zero -- which is what lets this
//     sit behind a CIC that only produces a sample once every R.
//   - `saturated` stays low throughout: nothing here exceeds the
//     output range.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::fir::{In, SymmetricFir, accumulator_width};

const WI: usize = 18;
const WC: usize = 12;
const TAPS: usize = 7;
const HALF: usize = 3;
const SHIFT: usize = 10;
const WO: usize = 18;
const WACC: usize = accumulator_width(WI, WC, TAPS);

fn main() -> Result<(), RHDLError> {
    let mut t = [SignedBits::<WC>::default(); TAPS];
    for (k, v) in [-20i128, 60, 180, 584, 180, 60, -20].iter().enumerate() {
        t[k] = signed::<WC>(*v);
    }
    let uut = SymmetricFir::<WI, WC, WACC, TAPS, HALF, SHIFT, WO>::new(t);

    let mut seq: Vec<In<WI>> = Vec::new();
    let sample = |v: i128| In::<WI> {
        sample: Some(signed::<WI>(v)),
        downstream_ready: true,
    };
    let idle = In::<WI> {
        sample: None,
        downstream_ready: true,
    };

    // An impulse at the coefficient scale, then quiet.
    seq.push(sample(1 << SHIFT));
    for _ in 0..9 {
        seq.push(sample(0));
    }
    // A gap, to show the window holding.
    seq.push(idle);
    seq.push(idle);
    // A step.
    for _ in 0..10 {
        seq.push(sample(2000));
    }
    seq.push(idle);

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "symmetric_fir.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
