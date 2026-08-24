//! A down-converter running on a Hogenauer-pruned decimator.
//!
//! This is what `cic_pruned!` was for and what the DQ-derive fix
//! unblocked. `Ddc` is generic over its decimator, so the same
//! down-converter source hosts either the uniform `CicDecimate` or a
//! generated pruned one, and nothing about the mixing, the oscillator
//! or the acquisition marker changes.
//!
//! The tests check the two agree. Not that they produce identical
//! numbers — they cannot, the pruned output has a coarser LSB — but
//! that they measure the same signal to within the schedule's error.

use rhdl::prelude::*;
use rhdl_fpga::cic_pruned;
use rhdl_fpga::core::dff;
use rhdl_fpga::dsp::cic::{accumulator_width, counter_width, prune};
use rhdl_fpga::dsp::ddc::{Ddc, In, UniformDdc};
use rhdl_fpga::dsp::iq::Iq;
use rhdl_fpga::dsp::nco::config::PHASE_W;
use rhdl_fpga::dsp::sync::SyncMark;
use rhdl_fpga::rcstream::Item;
use std::f64::consts::TAU;

const W: usize = 18;
const S: usize = 2;
const R: usize = 16;
const M: usize = 1;
const BO: usize = 10;
const PROD_W: usize = W + 18 + 1;
const FS: f64 = 125_000_000.0;

/// Full accumulator width for this configuration — what the uniform
/// decimator spends in every stage.
const WA: usize = accumulator_width(W, S, R, M);
const CW: usize = counter_width(R);
/// The pruned decimator's output width.
const WP: usize = prune::stage_width(2 * S, W, S, R, M, BO);

mod pruned_cic {
    use super::*;
    cic_pruned!(Cic, w_in = 18, n = 2, r = 16, m = 1, b_out = 10);
}

type Uniform = UniformDdc<W, WA, S, R, M, CW, PROD_W>;
type Pruned = Ddc<W, WP, PROD_W, pruned_cic::Cic>;

fn tune(hz: f64) -> u128 {
    let full = (1u128 << PHASE_W) as f64;
    ((hz / FS * full).rem_euclid(full)) as u128
}

fn stimulus(f_in: f64, f_lo: f64, n: usize) -> Vec<In<W>> {
    let amp = 100_000.0;
    (0..n)
        .map(|k| {
            let t = k as f64;
            let re = (amp * (TAU * f_in / FS * t).cos()).round() as i128;
            let im = (amp * (TAU * f_in / FS * t).sin()).round() as i128;
            In::<W> {
                sample: Some(Item {
                    data: Iq {
                        re: signed::<W>(re),
                        im: signed::<W>(im),
                    },
                    frame: SyncMark { sync: k == 0 },
                }),
                frequency: bits(tune(f_lo)),
                phase: bits(0),
                downstream_ready: true,
            }
        })
        .collect()
}

macro_rules! run {
    ($ty:ty, $f_in:expr, $f_lo:expr, $n:expr) => {{
        let uut = <$ty>::default();
        let input = stimulus($f_in, $f_lo, $n)
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);
        let out: Vec<(f64, f64)> = uut
            .run(input)
            .synchronous_sample()
            .filter_map(|t| {
                t.output
                    .sample
                    .map(|s| (s.data.re.raw() as f64, s.data.im.raw() as f64))
            })
            .collect();
        out
    }};
}

/// Root-mean-square magnitude over the back half, so start-up
/// transients do not dominate.
fn rms(v: &[(f64, f64)]) -> f64 {
    let tail = &v[v.len() / 2..];
    (tail.iter().map(|(r, i)| r * r + i * i).sum::<f64>() / tail.len() as f64).sqrt()
}

#[test]
fn the_pruned_datapath_is_actually_smaller() {
    let widths: Vec<usize> = (1..=2 * S)
        .map(|j| prune::stage_width(j, W, S, R, M, BO))
        .collect();
    let uniform_bits = WA * 2 * S;
    let pruned_bits: usize = widths.iter().sum();
    println!("full {WA}, widths {widths:?}, {pruned_bits} bits vs {uniform_bits}");
    assert!(
        pruned_bits < uniform_bits,
        "{pruned_bits} vs {uniform_bits}"
    );
    // Two arms, so the saving lands twice in the down-converter.
    //
    // Both are consts, so this is a compile-time claim about the
    // schedule rather than a runtime check -- which is the point.
    #[allow(clippy::assertions_on_constants)]
    {
        assert!(WP < WA, "the pruned output is narrower: {WP} vs {WA}");
    }
}

#[test]
fn both_down_convert_a_tone_at_the_oscillator_to_dc() {
    // On tune: a large output. Off tune by a quarter of the input rate,
    // far outside the decimated band: rejected. Both must hold for the
    // pruned arm, or the pruning has broken the filter rather than
    // coarsened it.
    let f = 3_000_000.0;
    let on_u = rms(&run!(Uniform, f, f, 1200));
    let off_u = rms(&run!(Uniform, f, f + FS / 8.0, 1200));
    let on_p = rms(&run!(Pruned, f, f, 1200));
    let off_p = rms(&run!(Pruned, f, f + FS / 8.0, 1200));
    println!("uniform on {on_u:.1} off {off_u:.1}; pruned on {on_p:.1} off {off_p:.1}");
    assert!(on_u > 1000.0 * off_u, "uniform rejection {on_u} / {off_u}");
    assert!(on_p > 1000.0 * off_p, "pruned rejection {on_p} / {off_p}");
}

#[test]
fn the_pruned_arm_measures_the_same_amplitude() {
    // The pruned output is the same number at a coarser LSB, so the
    // comparison is of magnitudes referred to a common scale.
    let f = 3_000_000.0;
    let u = rms(&run!(Uniform, f, f, 1200));
    let p = rms(&run!(Pruned, f, f, 1200));
    let scaled = p * (1u64 << (WA - WP)) as f64;
    let rel = (scaled - u).abs() / u;
    println!("uniform {u:.1}, pruned {p:.1} -> {scaled:.1} (rel {rel:.2e})");
    // One output LSB of the pruned arm is 2^(WA-WP) uniform LSBs, so
    // agreement to within a few of those is the most that can be asked.
    let one_lsb_rel = (1u64 << (WA - WP)) as f64 / u;
    assert!(
        rel < 8.0 * one_lsb_rel,
        "amplitudes disagree by {rel}, more than the quantisation allows ({one_lsb_rel})"
    );
}

#[test]
fn iverilog_agrees_with_the_simulator() -> miette::Result<()> {
    let uut = Pruned::default();
    let input = stimulus(3_000_000.0, 3_000_000.0, 80)
        .into_iter()
        .with_reset(1)
        .clock_pos_edge(100);
    let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&uut, &Default::default())?.run_iverilog()?;
    Ok(())
}

/// A down-converter whose decimators are *compensated*.
///
/// The point of the whole compensation exercise. `Ddc` is generic over
/// its decimator and `CompensatedCic` presents the decimator
/// interface, so this composes with no changes to the down-converter
/// at all — and both arms are the same type, so the I/Q symmetry a
/// phase-sensitive measurement depends on stays unrepresentable to
/// break.
mod compensated {
    use super::*;
    use rhdl_fpga::dsp::cic::CicDecimate;
    use rhdl_fpga::dsp::cic::compensated::CompensatedCic;
    use rhdl_fpga::dsp::cic::compensator;
    use rhdl_fpga::dsp::fir::{SymmetricFir, accumulator_width as fir_acc};

    const CTAPS: usize = 7;
    const CHALF: usize = 3;
    const CWC: usize = 12;
    const CSHIFT: usize = 10;
    const CWACC: usize = fir_acc(WA, CWC, CTAPS);

    type Arm = CompensatedCic<
        W,
        WA,
        WA,
        CicDecimate<W, WA, S, R, M, CW>,
        SymmetricFir<WA, CWC, CWACC, CTAPS, CHALF, CSHIFT, WA>,
    >;
    type CompDdc = Ddc<W, WA, PROD_W, Arm>;

    fn arm() -> Arm {
        let mut spec = compensator::Spec::for_cic(S, R, M);
        spec.taps = CTAPS;
        spec.passband = 0.8;
        let d = compensator::design(spec).expect("design");
        let q = compensator::quantise(&d, CWC);
        assert_eq!(q.shift as usize, CSHIFT, "SHIFT must track quantise()");
        let mut t = [SignedBits::<CWC>::default(); CTAPS];
        for (k, v) in q.taps.iter().enumerate() {
            t[k] = signed::<CWC>(*v as i128);
        }
        CompensatedCic::new(CicDecimate::default(), SymmetricFir::new(t))
    }

    fn uut() -> CompDdc {
        CompDdc::new(arm())
    }

    #[test]
    fn it_elaborates_and_round_trips() -> miette::Result<()> {
        let uut = uut();
        let _ = uut.descriptor("top".into())?;
        let input = stimulus(3_000_000.0, 3_000_000.0, 60)
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn the_passband_is_flatter_than_the_uncompensated_arm() {
        // Measure the down-converter's own response by sweeping the
        // input tone away from the oscillator: the offset lands at
        // baseband, where the decimator's droop lives.
        let f_lo = 3_000_000.0;
        let edge_u = 0.5 * 0.8;
        let probe = |uncomp: bool, u: f64| -> f64 {
            let f = f_lo + u / R as f64 * FS;
            if uncomp {
                rms(&run!(Uniform, f, f_lo, 1200))
            } else {
                let uut = uut();
                let input = stimulus(f, f_lo, 1200)
                    .into_iter()
                    .with_reset(1)
                    .clock_pos_edge(100);
                let out: Vec<(f64, f64)> = uut
                    .run(input)
                    .synchronous_sample()
                    .filter_map(|t| {
                        t.output
                            .sample
                            .map(|s| (s.data.re.raw() as f64, s.data.im.raw() as f64))
                    })
                    .collect();
                rms(&out)
            }
        };
        let span = |uncomp: bool| -> f64 {
            let dc = probe(uncomp, 0.0);
            assert!(dc > 1.0, "degenerate DC reference");
            let mut lo = f64::INFINITY;
            let mut hi = f64::NEG_INFINITY;
            for k in 0..=8 {
                let u = edge_u * k as f64 / 8.0;
                let db = 20.0 * (probe(uncomp, u) / dc).log10();
                lo = lo.min(db);
                hi = hi.max(db);
            }
            hi - lo
        };
        let bare = span(true);
        let comp = span(false);
        println!("uncompensated span {bare:.3} dB, compensated span {comp:.3} dB");
        assert!(
            bare > 2.0,
            "the bare arm must droop, or this proves nothing"
        );
        assert!(
            comp < bare / 3.0,
            "compensation must flatten the down-converter: {comp} vs {bare}"
        );
    }
}
