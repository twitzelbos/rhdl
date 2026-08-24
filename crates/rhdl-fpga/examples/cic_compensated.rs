// A two-stage CIC decimating by four, with its compensating FIR.
//
// This is the intended workflow end to end: ask `compensator` for a
// tap set that inverts this decimator's droop, quantise it to real
// coefficients, and hand it to the filter that sits behind the CIC.
//
// What to look for:
//
//   - `sample` on the output appears once every four input samples,
//     as it does for the bare CIC. Adding the compensator changes the
//     amplitude response, not the rate.
//   - The input is a constant 100 for the first stretch, and the
//     output settles at exactly 1600 = 100 * 4^2, the CIC's DC gain.
//     `quantise` trims the centre tap so the filter's DC gain is
//     exactly one, so the compensator does not move the settled value
//     at all -- it only changes what happens away from DC, which is
//     the entire point. An inexact DC gain here would be a systematic
//     amplitude error on every sample, which is worse than the ripple
//     the design exists to remove.
//   - Note how long it takes to settle. The CIC's window is N*R
//     samples and the FIR's is TAPS *output* samples, so the pair
//     needs roughly (N + TAPS) * R inputs before it means anything.
//   - Then the input switches to a tone near the passband edge, where
//     the bare CIC would be several dB down. Compare the amplitude
//     here against `cic_decimate`'s trace at the same stimulus: the
//     compensated output is larger, because the droop has been undone.
//   - `saturated` stays low. A compensator has gain above one, so it
//     can clamp on near-full-scale input; there is headroom here.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::cic::compensated::CompensatedCic;
use rhdl_fpga::dsp::cic::decimator::In;
use rhdl_fpga::dsp::cic::{CicDecimate, accumulator_width, compensator, counter_width};
use rhdl_fpga::dsp::fir::{SymmetricFir, accumulator_width as fir_acc};

const WI: usize = 8;
const N: usize = 2;
const R: usize = 4;
const M: usize = 1;
const WA: usize = accumulator_width(WI, N, R, M);
const CW: usize = counter_width(R);

const TAPS: usize = 7;
const HALF: usize = 3;
const WC: usize = 12;
const SHIFT: usize = 10;
const WACC: usize = fir_acc(WA, WC, TAPS);

type Cic = CicDecimate<WI, WA, N, R, M, CW>;
type Fir = SymmetricFir<WA, WC, WACC, TAPS, HALF, SHIFT, WA>;
type Uut = CompensatedCic<WI, WA, WA, Cic, Fir>;

fn main() -> Result<(), RHDLError> {
    // Design the compensator for exactly this decimator.
    let mut spec = compensator::Spec::for_cic(N, R, M);
    spec.taps = TAPS;
    spec.passband = 0.8;
    let design = compensator::design(spec.clone()).expect("a sane spec must design");
    let quant = compensator::quantise(&design, WC);
    assert_eq!(
        quant.shift as usize, SHIFT,
        "SHIFT must match what quantise() picked"
    );
    println!(
        "taps {:?} shift {} ripple {:.4} dB (droop was {:.2} dB)",
        quant.taps,
        quant.shift,
        quant.ripple_db,
        rhdl_fpga::dsp::cic::response::passband_droop_db(spec.passband, N, R, M)
    );

    let mut t = [SignedBits::<WC>::default(); TAPS];
    for k in 0..TAPS {
        t[k] = signed::<WC>(quant.taps[k] as i128);
    }
    // The pair has no `Default`: a filter with no taps is a filter
    // that outputs zero, and that should not be reachable by accident.
    let uut = Uut::new(Cic::default(), Fir::new(t));

    let seq: Vec<In<WI>> = (0..96i128)
        .map(|k| {
            // First half DC, then a tone near the passband edge.
            let v = if k < 56 {
                100
            } else {
                // u ~ 0.4 of output Nyquist is f = 0.05 of input rate.
                (100.0 * (std::f64::consts::TAU * 0.05 * k as f64).cos()).round() as i128
            };
            In::<WI> {
                sample: Some(signed::<WI>(v)),
                restart: k == 0,
                downstream_ready: true,
            }
        })
        .collect();

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "cic_compensated.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
