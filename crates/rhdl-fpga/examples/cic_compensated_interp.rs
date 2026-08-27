// A CIC interpolator with its droop pre-corrected: the compensator runs
// before the rate change, at the envelope rate.
//
// The taps come from `dsp::cic::compensator::design` rather than being
// picked by hand -- which is the workflow to copy, and also how the
// first version of this example was found to have the sign of its outer
// tap pair wrong.
//
// What to look for:
//
//   - `input_ready` pulses once every four cycles, unchanged by the
//     compensator. Wrapping the interpolator must not disturb its
//     request; the compensator is gated to that request rather than
//     driving it.
//   - `sample` on the output is present every cycle, as a bare
//     interpolator's is.
//   - The envelope is a tone near the band edge, where the droop is
//     worst. Compare the output amplitude here with `cic_interpolate`'s:
//     the CIC alone loses about 3 dB at this frequency and the
//     pre-compensator puts it back.
//   - There is latency, and it is not one sample. A five-tap symmetric
//     FIR is linear-phase about its centre, so it delays by two envelope
//     samples whatever its coefficients are, and the handover register
//     between the FIR and the interpolator adds a third. The first three
//     envelope periods of output are the silence the handover register
//     starts with.
//   - `saturated` is worth watching in a real design and stays low here.
//     A bare interpolator can never assert it -- its widths are exact --
//     but inverting a droop means gain above one, so a near-full-scale
//     envelope at the band edge can exceed the compensator's output
//     width. That is a headroom budget the caller owns.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::cic::compensated_interp::CompensatedInterp;
use rhdl_fpga::dsp::cic::interpolator::{self, CicInterpolate};
use rhdl_fpga::dsp::cic::{compensator, interp};
use rhdl_fpga::dsp::fir::SymmetricFir;

const WI: usize = 8;
const WM: usize = 12;
const S: usize = 2;
const RMAX: usize = 8;
const M: usize = 1;
const CW: usize = 4;
const WO: usize = interp::accumulator_width(WM, S, RMAX, M);
const RATE: usize = 4;

const TAPS: usize = 5;
const HALF: usize = 2;
const WC: usize = 12;
const WACC: usize = 28;
const SHIFT: usize = 10;

/// The band the compensator is designed for, as a fraction of Nyquist.
const PASSBAND: f64 = 0.7;

type Fir = SymmetricFir<WI, WC, WACC, TAPS, HALF, SHIFT, WM>;
type Core = CicInterpolate<WM, WO, S, RMAX, M, CW>;

fn compensator_taps() -> Fir {
    let d = compensator::design(compensator::Spec {
        cics: vec![compensator::CicShape {
            decimate: RATE,
            stages: S,
            delay: M,
        }],
        passband: PASSBAND,
        taps: TAPS,
        stopband_edge: 1.0,
        // A pre-compensator's stopband cannot affect image rejection, so
        // constraining it would spend taps on nothing.
        min_stopband_db: 0.0,
        max_ripple_db: 1.0,
        method: compensator::Method::LeastSquares,
    })
    .expect("designable");
    let scale = (1i128 << SHIFT) as f64;
    let q: Vec<i128> = d.taps.iter().map(|x| (x * scale).round() as i128).collect();
    SymmetricFir::new([
        signed::<WC>(q[0]),
        signed::<WC>(q[1]),
        signed::<WC>(q[2]),
        signed::<WC>(q[3]),
        signed::<WC>(q[4]),
    ])
}

fn main() -> Result<(), RHDLError> {
    let uut =
        CompensatedInterp::<WI, WM, WO, CW, Fir, Core>::new(compensator_taps(), Core::default());

    // A tone near the band edge, where the droop is worst.
    let f = 0.5 * PASSBAND * 0.85;
    let envelope: Vec<i128> = (0..16)
        .map(|m| {
            let t = std::f64::consts::TAU * f * m as f64;
            (70.0 * t.cos()) as i128
        })
        .collect();

    let seq: Vec<interpolator::In<WI, CW>> = (0..envelope.len() * RATE)
        .map(|n| interpolator::In::<WI, CW> {
            sample: Some(signed::<WI>(envelope[n / RATE])),
            rate: bits::<CW>(RATE as u128),
            restart: false,
            downstream_ready: true,
        })
        .collect();

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "cic_compensated_interp.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
