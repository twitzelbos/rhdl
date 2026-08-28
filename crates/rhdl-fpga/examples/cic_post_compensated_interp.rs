// A CIC interpolator with its compensator on the far side, at the
// converter rate, where it can suppress the images.
//
// The pre-compensated form (`cic_compensated_interp`) puts the FIR before
// the rate change. That is cheap -- the filter runs at the envelope rate
// -- and it cannot touch the images at all: its response is periodic
// with period one in envelope-rate units, so it lifts each image exactly
// as much as it lifts the signal.
//
// Move the same filter to the converter rate and the periodicity goes
// away. Now the signal and its images are different frequencies to the
// filter, and it can pass one and stop the others.
//
// What to look for:
//
//   - The envelope is a tone at 0.15 cycles per envelope sample, not a
//     constant. A constant envelope's images sit exactly on the sinc^N
//     nulls and are already gone, so a trace with one would show nothing
//     about this widget.
//   - `input_ready` still pulses once per rate, and `sample` is present
//     every cycle. The widget presents the bare interpolator's In/Out,
//     so it drops into a StreamInterpolator or either up-converter
//     unchanged.
//   - The output is visibly a cleaner sinusoid than `cic_interpolate`'s
//     at the same input. Measured through the hardware, images go from
//     29 dB below the signal to 69 dB -- which is what the design maths
//     predicts for this configuration, to within a tenth of a dB.
//   - There is latency, and it is the FIR's group delay -- twelve
//     converter cycles for these 25 taps, plus its output register. Note
//     that is twelve *converter* cycles, where the pre-compensated form
//     costs its group delay in *envelope* samples: a factor of R
//     difference in absolute time, and the one respect in which
//     post-compensation is the cheaper arrangement.
//
// The price is the tap count, and it scales with R. Twenty-five taps at
// R = 4 is fine. At R = 125 the same requirement needs about 755 taps at
// 125 MHz, which is not a filter anybody builds -- so a post-compensator
// belongs between chain stages, where the local R is small, rather than
// after a whole interpolation. `interp_chain::post_compensator_taps`
// gives the number before you commit to it.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::cic::interpolator::{self, CicInterpolate};
use rhdl_fpga::dsp::cic::post_compensated_interp::PostCompensatedInterp;
use rhdl_fpga::dsp::cic::{interp, interp_chain};
use rhdl_fpga::dsp::fir::{SymmetricFir, accumulator_width as fir_acc};

const WI: usize = 14;
const S: usize = 2;
const RMAX: usize = 4;
const M: usize = 1;
const WMID: usize = interp::accumulator_width(WI, S, RMAX, M);
const CW: usize = interp::rate_width(RMAX);

const TAPS: usize = 25;
const HALF: usize = 12;
const WC: usize = 18;
const WACC: usize = fir_acc(WMID, WC, TAPS);
const SHIFT: usize = 14;
const WOUT: usize = 18;

/// The band the filter is designed for, as a fraction of the envelope
/// Nyquist.
const PASSBAND: f64 = 0.4;

type Fir = SymmetricFir<WMID, WC, WACC, TAPS, HALF, SHIFT, WOUT>;
type Core = CicInterpolate<WI, WMID, S, RMAX, M, CW>;

fn compensator() -> Fir {
    let shapes = vec![rhdl_fpga::dsp::cic::compensator::CicShape {
        decimate: RMAX,
        stages: S,
        delay: M,
    }];
    // 60 dB is a *composite* requirement -- cascade and filter together
    // -- and the cascade alone already gives 24, so asking for 30 would
    // buy almost nothing.
    let q = interp_chain::post_compensator(&shapes, PASSBAND, RMAX, TAPS, 60.0, WC)
        .expect("designable at R = 4");
    let scale = (1u64 << q.shift) as f64;
    let unity = (1i128 << SHIFT) as f64;
    let mut taps = [SignedBits::<WC>::default(); TAPS];
    for (k, v) in q.taps.iter().enumerate() {
        taps[k] = signed::<WC>(((*v as f64 / scale) * unity).round() as i128);
    }
    SymmetricFir::new(taps)
}

fn main() -> Result<(), RHDLError> {
    let uut =
        PostCompensatedInterp::<WI, WMID, WOUT, CW, Core, Fir>::new(Core::default(), compensator());

    let envelope: Vec<i128> = (0..24)
        .map(|m| {
            let t = std::f64::consts::TAU * 0.15 * m as f64;
            (6000.0 * t.cos()) as i128
        })
        .collect();

    let seq: Vec<interpolator::In<WI, CW>> = (0..envelope.len() * RMAX)
        .map(|n| interpolator::In::<WI, CW> {
            sample: Some(signed::<WI>(envelope[n / RMAX])),
            rate: bits::<CW>(RMAX as u128),
            restart: false,
            downstream_ready: true,
        })
        .collect();

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "cic_post_compensated_interp.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
