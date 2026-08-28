//! Does a Hogenauer-pruned CIC still compute the CIC?
//!
//! The macro substitutes widths from `prune::stage_width`, so *that*
//! the widths match the analysis is true by construction and not worth
//! asserting. What is worth asserting is the thing construction does
//! not give you: that a datapath which throws away low-order bits at
//! every stage still produces the same filter to within the error the
//! analysis predicts.
//!
//! So every test here compares the generated widget against
//! `CicDecimate` at full width, with the same stimulus, and checks the
//! difference against a bound derived from Hogenauer's §V criterion —
//! not from the measurement.

use rhdl::prelude::*;
use rhdl_fpga::cic_pruned;
use rhdl_fpga::core::dff;
use rhdl_fpga::dsp::cic::{accumulator_width, counter_width, decimator::CicDecimate, prune};

/// The configuration under test. Twelve-bit input, three stages,
/// decimate by 32 — small enough to simulate exhaustively, deep enough
/// that the taper is real.
const WI: usize = 12;
const N: usize = 3;
const R: usize = 32;
const M: usize = 1;
const BO: usize = 8;

const FULL: usize = accumulator_width(WI, N, R, M);
const CW: usize = counter_width(R);
const WOUT: usize = prune::stage_width(2 * N, WI, N, R, M, BO);
/// LSBs the pruned output has shed relative to the full-width one.
const SHIFT: usize = FULL - WOUT;

mod pruned {
    use super::*;
    cic_pruned!(Uut, w_in = 12, n = 3, r = 32, m = 1, b_out = 8);
}

type Exact = CicDecimate<WI, FULL, N, R, M, CW>;

/// The error the schedule predicts, in output LSBs.
///
/// Thin wrapper over [`prune::predicted_sigma`], which is the library's
/// own accounting — this used to be duplicated here, and belongs with
/// the schedule it describes now that the chain designer reads it too.
fn predicted_sigma(wi: usize, n: usize, r: usize, m: usize, bo: usize) -> f64 {
    prune::predicted_sigma(wi, n, r, m, bo)
}

/// How far past the predicted sigma a measurement may sit.
///
/// The prediction models truncation error as white and uniform. It is
/// neither — it is a deterministic function of the signal, correlated
/// across stages — so the measured value can land either side of it.
/// A factor of two says "the same size", which is the claim being
/// tested; it is not a fitted constant, and the input-scaling bug this
/// file caught missed by four orders of magnitude.
const SIGMA_SLACK: f64 = 2.0;

fn stim(x: &[i128]) -> Vec<rhdl_fpga::dsp::cic::decimator::In<WI>> {
    use rhdl_fpga::dsp::cic::decimator::In;
    let mut v: Vec<In<WI>> = x
        .iter()
        .map(|s| In::<WI> {
            sample: Some(signed::<WI>(*s)),
            restart: false,
            downstream_ready: true,
        })
        .collect();
    // Let the registered output emerge; idle cycles hold the filter.
    v.extend(std::iter::repeat_n(
        In::<WI> {
            sample: None,
            restart: false,
            downstream_ready: true,
        },
        4,
    ));
    v
}

fn run_exact(x: &[i128]) -> Vec<i128> {
    let uut = Exact::default();
    let input = stim(x).into_iter().with_reset(1).clock_pos_edge(100);
    uut.run(input)
        .synchronous_sample()
        .filter_map(|t| t.output.sample.map(|s| s.raw()))
        .collect()
}

fn run_pruned(x: &[i128]) -> Vec<i128> {
    let uut = pruned::Uut::default();
    let input = stim(x).into_iter().with_reset(1).clock_pos_edge(100);
    uut.run(input)
        .synchronous_sample()
        .filter_map(|t| t.output.sample.map(|s| s.raw()))
        .collect()
}

/// A deterministic in-band signal plus a deterministic dither, so the
/// truncation error is exercised rather than sitting at a fixed phase.
fn signal(n: usize) -> Vec<i128> {
    use std::f64::consts::TAU;
    let mut seed = 0x1234_5678u64;
    (0..n)
        .map(|k| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let dither = ((seed >> 33) % 7) as f64 - 3.0;
            let a = 900.0 * (TAU * 0.003 * k as f64).sin();
            let b = 400.0 * (TAU * 0.011 * k as f64 + 1.1).sin();
            (a + b + dither).round() as i128
        })
        .collect()
}

#[test]
fn the_taper_is_what_the_analysis_says() {
    let widths: Vec<usize> = (1..=2 * N)
        .map(|j| prune::stage_width(j, WI, N, R, M, BO))
        .collect();
    // Non-increasing, or `narrow` would be widening and the whole
    // scheme is incoherent. This is also enforced at const-evaluation
    // time inside the macro, but a clear failure here beats an
    // arithmetic-overflow diagnostic in a const block.
    assert!(
        widths.windows(2).all(|w| w[0] >= w[1]),
        "schedule must be non-increasing, got {widths:?}"
    );
    assert!(widths[0] <= FULL);
    let uniform: usize = FULL * 2 * N;
    let tapered: usize = widths.iter().sum();
    assert!(
        tapered < uniform,
        "pruning that saves nothing is not pruning: {tapered} vs {uniform}"
    );
    println!("widths {widths:?} full {FULL} out {WOUT} shift {SHIFT} bits {tapered}/{uniform}");
}

#[test]
fn pruned_tracks_exact_within_the_predicted_error() {
    let x = signal(4096);
    let a = run_exact(&x);
    let b = run_pruned(&x);
    assert_eq!(a.len(), b.len(), "same stimulus must give the same count");
    assert!(a.len() > 100, "not enough outputs to say anything");

    // Refer the exact result to the pruned output's LSB weight.
    let err: Vec<f64> = a
        .iter()
        .zip(b.iter())
        .map(|(e, p)| (*p - (*e >> SHIFT)) as f64)
        .collect();
    let n = err.len() as f64;
    let mean = err.iter().sum::<f64>() / n;
    let var = err.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / n;
    let std = var.sqrt();
    let peak = err.iter().fold(0.0f64, |m, e| m.max(e.abs()));
    println!(
        "mean {mean:.3} std {std:.3} peak {peak:.1} LSB (n={})",
        a.len()
    );

    // Truncation is biased: each of the 2N+1 truncation points
    // contributes a mean of half its discarded weight, and the schedule
    // equalises those weights at roughly one output LSB. So the bias is
    // bounded by (2N+1)/2 output LSBs; allow twice that.
    assert!(
        mean.abs() <= (2 * N + 1) as f64,
        "bias {mean} exceeds the schedule's budget"
    );
    let pred = predicted_sigma(WI, N, R, M, BO);
    println!("predicted sigma {pred:.3}");
    assert!(
        std <= SIGMA_SLACK * pred,
        "sigma {std} exceeds the schedule's prediction {pred}"
    );
}

#[test]
fn pruning_actually_degrades_when_the_budget_is_spent() {
    // The complement of the test above, and the reason that one means
    // something: if the comparison were insensitive to the datapath,
    // a far more aggressive schedule would also pass it.
    mod loose {
        use super::*;
        cic_pruned!(Uut, w_in = 12, n = 3, r = 32, m = 1, b_out = 16);
    }
    const WOUT_L: usize = prune::stage_width(2 * N, WI, N, R, M, 16);
    let x = signal(2048);
    let a = run_exact(&x);
    let uut = loose::Uut::default();
    let input = stim(&x).into_iter().with_reset(1).clock_pos_edge(100);
    let b: Vec<i128> = uut
        .run(input)
        .synchronous_sample()
        .filter_map(|t| t.output.sample.map(|s| s.raw()))
        .collect();
    let shift = FULL - WOUT_L;
    let err: Vec<f64> = a
        .iter()
        .zip(b.iter())
        .map(|(e, p)| (*p - (*e >> shift)) as f64)
        .collect();
    let n = err.len() as f64;
    let mean = err.iter().sum::<f64>() / n;
    let std = (err.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / n).sqrt();
    println!("loose: out {WOUT_L} shift {shift} mean {mean:.3} std {std:.3}");
    // Same criterion, wider budget -- the *relative* error stays in
    // band because that is what the schedule guarantees at any b_out.
    assert!(mean.abs() <= (2 * N + 1) as f64, "bias {mean}");
    let pred = predicted_sigma(WI, N, R, M, 16);
    assert!(std <= SIGMA_SLACK * pred, "sigma {std} vs predicted {pred}");
    // Both are consts, so this is a compile-time claim about the
    // schedule rather than a runtime check -- which is the point: if a
    // bigger budget stopped buying a narrower datapath, this test's
    // whole premise would be gone.
    #[allow(clippy::assertions_on_constants)]
    {
        assert!(
            WOUT_L < WOUT,
            "a bigger budget must buy a narrower datapath"
        );
    }
}

#[test]
fn dc_gain_survives_the_taper() {
    use rhdl_fpga::dsp::cic::dc_gain;
    let x = vec![300i128; 2048];
    let b = run_pruned(&x);
    let settled = b[b.len() - 4];
    let ideal = (300 * dc_gain(N, R, M) as i128) >> SHIFT;
    // A CIC's DC gain is (R*M)^N; the taper does not change it, it only
    // coarsens the answer.
    let slack = ((2 * N + 1) * 2) as i128;
    assert!(
        (settled - ideal).abs() <= slack,
        "settled {settled} vs ideal {ideal}"
    );
}

#[test]
fn restart_makes_the_output_independent_of_pre_trigger_history() {
    use rhdl_fpga::dsp::cic::decimator::In;
    // Two different histories, the same post-trigger samples. If the
    // restart works, the outputs after the trigger are identical --
    // an invariance property, so it cannot be satisfied by a wrong
    // model of what the value should be.
    let after: Vec<i128> = (0..R as i128 * 6).map(|k| (k * 37) % 611 - 305).collect();
    let build = |hist: i128| {
        let mut v: Vec<In<WI>> = (0..R as i128 * 3)
            .map(|k| In::<WI> {
                sample: Some(signed::<WI>((k * hist) % 501 - 250)),
                restart: false,
                downstream_ready: true,
            })
            .collect();
        for (n, s) in after.iter().enumerate() {
            v.push(In::<WI> {
                sample: Some(signed::<WI>(*s)),
                restart: n == 0,
                downstream_ready: true,
            });
        }
        v.extend(std::iter::repeat_n(
            In::<WI> {
                sample: None,
                restart: false,
                downstream_ready: true,
            },
            4,
        ));
        v
    };
    let go = |hist: i128| -> Vec<i128> {
        let uut = pruned::Uut::default();
        let input = build(hist).into_iter().with_reset(1).clock_pos_edge(100);
        uut.run(input)
            .synchronous_sample()
            .filter_map(|t| t.output.sample.map(|s| s.raw()))
            .collect()
    };
    let a = go(7);
    let b = go(113);
    // Outputs from the pre-trigger window differ; the ones after the
    // restart must not.
    let post = after.len() / R;
    assert!(post >= 5);
    assert_eq!(a[a.len() - post..], b[b.len() - post..]);
    assert_ne!(
        a, b,
        "the histories must actually differ, or this proves nothing"
    );
}

/// The same criterion at other depths and rates.
///
/// One configuration passing proves the schedule works there. These
/// exist because a width-scaling error can easily be invisible at one
/// `(N, R, b_out)` and glaring at another — the input-scaling bug this
/// file was written to catch was invisible at `b_out = 8` for exactly
/// that reason, because that schedule happens not to prune stage one.
mod sweep {
    use super::*;

    /// Generate a full comparison test for one configuration.
    ///
    /// A macro rather than a generic function because the two widgets
    /// have different output widths, so there is no one signature to
    /// write; the run loop is three lines and inlining it costs less
    /// than the trait plumbing to abstract over it would.
    macro_rules! case {
        ($modname:ident, $wi:tt, $n:tt, $r:tt, $bo:tt) => {
            mod $modname {
                use super::*;
                use rhdl_fpga::dsp::cic::decimator::In;

                cic_pruned!(Uut, w_in = $wi, n = $n, r = $r, m = 1, b_out = $bo);

                const FULL: usize = accumulator_width($wi, $n, $r, 1);
                const CW: usize = counter_width($r);
                const WOUT: usize = prune::stage_width(2 * $n, $wi, $n, $r, 1, $bo);
                type Exact = CicDecimate<$wi, FULL, $n, $r, 1, CW>;

                fn feed(x: &[i128]) -> Vec<In<$wi>> {
                    let mut v: Vec<In<$wi>> = x
                        .iter()
                        .map(|s| In::<$wi> {
                            sample: Some(signed::<$wi>(*s)),
                            restart: false,
                            downstream_ready: true,
                        })
                        .collect();
                    v.extend(std::iter::repeat_n(
                        In::<$wi> {
                            sample: None,
                            restart: false,
                            downstream_ready: true,
                        },
                        4,
                    ));
                    v
                }

                #[test]
                fn tracks_exact_within_the_predicted_error() {
                    let scale = (1i128 << ($wi - 1)) - 1;
                    let x = signal_scaled(48 * $r, scale);

                    let input = feed(&x).into_iter().with_reset(1).clock_pos_edge(100);
                    let a: Vec<i128> = Exact::default()
                        .run(input)
                        .synchronous_sample()
                        .filter_map(|t| t.output.sample.map(|s| s.raw()))
                        .collect();

                    let input = feed(&x).into_iter().with_reset(1).clock_pos_edge(100);
                    let b: Vec<i128> = Uut::default()
                        .run(input)
                        .synchronous_sample()
                        .filter_map(|t| t.output.sample.map(|s| s.raw()))
                        .collect();

                    assert_eq!(a.len(), b.len());
                    assert!(a.len() >= 32, "not enough outputs: {}", a.len());

                    let shift = FULL - WOUT;
                    let err: Vec<f64> = a
                        .iter()
                        .zip(b.iter())
                        .map(|(e, p)| (*p - (*e >> shift)) as f64)
                        .collect();
                    let n = err.len() as f64;
                    let mean = err.iter().sum::<f64>() / n;
                    let std = (err.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / n).sqrt();
                    let widths: Vec<usize> = (1..=2 * $n)
                        .map(|j| prune::stage_width(j, $wi, $n, $r, 1, $bo))
                        .collect();
                    println!(
                        "{}: {widths:?} full {FULL} out {WOUT} mean {mean:.3} std {std:.3}",
                        stringify!($modname)
                    );
                    assert!(
                        widths.iter().sum::<usize>() < FULL * 2 * $n,
                        "this configuration prunes nothing, so it tests nothing"
                    );
                    assert!(mean.abs() <= (2 * $n + 1) as f64, "bias {mean}");
                    let pred = predicted_sigma($wi, $n, $r, 1, $bo);
                    println!("  predicted sigma {pred:.3}");
                    assert!(std <= SIGMA_SLACK * pred, "sigma {std} vs predicted {pred}");
                }
            }
        };
    }

    case!(shallow_fast, 10, 2, 8, 6);
    case!(deep_slow, 14, 4, 64, 12);
}

/// Deterministic in-band stimulus scaled to a given amplitude.
fn signal_scaled(n: usize, scale: i128) -> Vec<i128> {
    use std::f64::consts::TAU;
    let mut seed = 0x1234_5678u64;
    (0..n)
        .map(|k| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let dither = ((seed >> 33) % 7) as f64 - 3.0;
            let a = 0.45 * scale as f64 * (TAU * 0.003 * k as f64).sin();
            let b = 0.20 * scale as f64 * (TAU * 0.011 * k as f64 + 1.1).sin();
            (a + b + dither).round() as i128
        })
        .collect()
}

/// Tiers 3-5 for a generated widget.
///
/// A small configuration, so the emitted Verilog is short enough to
/// read. The snapshot is the evidence that the taper survives all the
/// way to hardware — the register declarations carry the per-stage
/// widths, which is the entire claim of this macro.
mod hardware {
    use super::*;
    use expect_test::expect;

    cic_pruned!(Uut, w_in = 8, n = 2, r = 4, m = 1, b_out = 4);

    const FULL: usize = accumulator_width(8, 2, 4, 1);

    fn stream()
    -> impl Iterator<Item = TimedSample<(ClockReset, rhdl_fpga::dsp::cic::decimator::In<8>)>> {
        use rhdl_fpga::dsp::cic::decimator::In;
        let mut v: Vec<In<8>> = (0..64)
            .map(|k: i128| In::<8> {
                sample: Some(signed::<8>((k * 13) % 201 - 100)),
                restart: k == 8,
                downstream_ready: k % 17 != 0,
            })
            .collect();
        v.extend(std::iter::repeat_n(
            In::<8> {
                sample: None,
                restart: false,
                downstream_ready: true,
            },
            4,
        ));
        v.into_iter().with_reset(1).clock_pos_edge(100)
    }

    #[test]
    fn the_taper_reaches_the_verilog() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        // The widths the analysis asked for, in the order the stages
        // appear. If the emitted registers stop matching, either the
        // schedule changed or the bundling did.
        let want: Vec<usize> = (1..=4)
            .map(|j| prune::stage_width(j, 8, 2, 4, 1, 4))
            .collect();
        assert_eq!(want, vec![12, 11, 11, 10], "the schedule for this config");
        assert!(want.iter().sum::<usize>() < FULL * 4);
        // Each stage register must appear at its own width somewhere in
        // the emitted module.
        for w in [12usize, 11, 10] {
            assert!(
                hdl.contains(&format!("[{}:0]", w - 1)),
                "no {w}-bit signal in the emitted Verilog"
            );
        }
        Ok(())
    }

    #[test]
    fn hdl_snapshot_is_stable() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [10:0] i, output wire [12:0] o);
               wire [90:0] od;
               wire [77:0] d;
               wire [77:0] q;
               assign o = od[12:0];
               top_stages c0(.clock_reset(clock_reset), .i(d[64:0]), .o(q[64:0]));
               top_phase c1(.clock_reset(clock_reset), .i(d[66:65]), .o(q[66:65]));
               top_out c2(.clock_reset(clock_reset), .i(d[77:67]), .o(q[77:67]));
               assign d = od[90:13];
               assign od = kernel_cic_pruned_kernel(clock_reset, i, q);
               function [90:0] kernel_cic_pruned_kernel(input reg [1:0] arg_0, input reg [10:0] arg_1, input reg [77:0] arg_2);
                     reg [64:0] r0;
                     reg [77:0] r1;
                     // d
                     reg [77:0] r2;
                     reg [1:0] r3;
                     // d
                     reg [77:0] r4;
                     // d
                     reg [77:0] r5;
                     reg [8:0] r6;
                     reg [10:0] r7;
                     reg [0:0] r8;
                     reg [7:0] r9;
                     reg [7:0] r10;
                     reg [7:0] r11;
                     reg [0:0] r12;
                     reg [11:0] r13;
                     reg [11:0] r14;
                     reg [11:0] r15;
                     reg signed [11:0] r16;
                     reg signed [11:0] r17;
                     // have
                     reg [0:0] r18;
                     // x
                     reg signed [11:0] r19;
                     reg [0:0] r20;
                     reg [64:0] r21;
                     reg [64:0] r22;
                     reg signed [11:0] r23;
                     reg signed [11:0] r24;
                     // st
                     reg [64:0] r25;
                     reg signed [10:0] r26;
                     reg signed [11:0] r27;
                     reg signed [11:0] r28;
                     reg signed [10:0] r29;
                     reg signed [10:0] r30;
                     // st
                     reg [64:0] r31;
                     // d
                     reg [77:0] r32;
                     reg signed [10:0] r33;
                     reg [0:0] r34;
                     reg [1:0] r35;
                     reg [1:0] r36;
                     reg [0:0] r37;
                     reg [1:0] r38;
                     reg [1:0] r39;
                     // d
                     reg [77:0] r40;
                     reg signed [10:0] r41;
                     reg [10:0] r42;
                     // line
                     reg [10:0] r43;
                     // cs
                     reg [64:0] r44;
                     reg [10:0] r45;
                     reg signed [10:0] r46;
                     // cs
                     reg [64:0] r47;
                     reg signed [10:0] r48;
                     reg signed [10:0] r49;
                     reg signed [9:0] r50;
                     reg [9:0] r51;
                     // line
                     reg [9:0] r52;
                     // cs
                     reg [64:0] r53;
                     reg [9:0] r54;
                     reg signed [9:0] r55;
                     // cs
                     reg [64:0] r56;
                     // d
                     reg [77:0] r57;
                     reg signed [9:0] r58;
                     reg [10:0] r59;
                     reg [9:0] r60;
                     // d
                     reg [77:0] r61;
                     // d
                     reg [77:0] r62;
                     // d
                     reg [77:0] r63;
                     reg [10:0] r64;
                     reg [0:0] r65;
                     reg [0:0] r66;
                     reg [12:0] r67;
                     reg [12:0] r68;
                     reg [12:0] r69;
                     reg [0:0] r70;
                     reg [1:0] r71;
                     reg [0:0] r72;
                     // d
                     reg [77:0] r73;
                     // d
                     reg [77:0] r74;
                     // d
                     reg [77:0] r75;
                     // o
                     reg [12:0] r76;
                     // d
                     reg [77:0] r77;
                     // o
                     reg [12:0] r78;
                     reg [90:0] r79;
                     reg signed [11:0] r80;
                     reg signed [12:0] r81;
                     reg signed [10:0] r82;
                     reg signed [11:0] r83;
                     localparam l0 = 78'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l1 = 11'b00000000000;
                     localparam l2 = 8'b10000000;
                     localparam l3 = 12'b111100000000;
                     localparam l4 = 12'b000000000000;
                     localparam l5 = 1'b1;
                     localparam l6 = 1'b1;
                     localparam l7 = 1'b0;
                     localparam l8 = 12'sb000000000000;
                     localparam l9 = 65'b00000000000000000000000000000000000000000000000000000000000000000;
                     localparam l10 = 2'b00;
                     localparam l11 = 2'b11;
                     localparam l12 = 2'b01;
                     localparam l13 = 2'b00;
                     localparam l14 = 1'b1;
                     localparam l15 = 13'b0000000000000;
                     localparam l16 = 1'b0;
                     localparam l17 = 65'b00000000000000000000000000000000000000000000000000000000000000000;
                     localparam l18 = 2'b00;
                     localparam l19 = 11'b00000000000;
                     localparam l20 = 1'b0;
                     begin
                        r71 = arg_0;
                        r7 = arg_1;
                        r1 = arg_2;
                        r0 = r1[64:0];
                        r2 = l0;
                        r2[64:0] = r0;
                        r3 = r1[66:65];
                        r4 = r2;
                        r4[66:65] = r3;
                        r5 = r4;
                        r5[77:67] = l1;
                        r6 = r7[8:0];
                        r8 = r6[8:8];
                        r9 = r6[7:0];
                        r10 = $unsigned(r9);
                        r11 = r10 & l2;
                        r12 = |r11;
                        r13 = {{4{1'b0}}, r10};
                        r14 = r12 ? l3 : l4;
                        r15 = r13 + r14;
                        r16 = $signed(r15);
                        r80 = $signed(r16[11:0]);
                        r17 = $signed(r80[11:0]);
                        case (r8)
                           1'b1 : r18 = l6;
                           default : r18 = l7;
                        endcase
                        case (r8)
                           1'b1 : r19 = r17;
                           default : r19 = l8;
                        endcase
                        r20 = r7[9:9];
                        r21 = r1[64:0];
                        r22 = r20 ? l9 : r21;
                        r23 = r22[11:0];
                        r24 = r23 + r19;
                        r25 = r22;
                        r25[11:0] = r24;
                        r26 = r22[22:12];
                        r27 = r22[11:0];
                        r81 = $signed({{1{r27[11]}}, r27});
                        r28 = r81[12:1];
                        r29 = $signed(r28[10:0]);
                        r30 = r26 + r29;
                        r31 = r25;
                        r31[22:12] = r30;
                        r32 = r5;
                        r32[64:0] = r31;
                        r33 = r31[22:12];
                        r34 = r7[9:9];
                        r35 = r1[66:65];
                        r36 = r34 ? l10 : r35;
                        r37 = r36 == l11;
                        r38 = r36 + l12;
                        r39 = r37 ? l13 : r38;
                        r40 = r32;
                        r40[66:65] = r39;
                        r82 = $signed(r33[10:0]);
                        r41 = $signed(r82[10:0]);
                        r42 = r22[33:23];
                        r43 = r42;
                        r43[10:0] = r41;
                        r44 = r31;
                        r44[33:23] = r43;
                        r45 = r22[33:23];
                        r46 = r41 - r45;
                        r47 = r44;
                        r47[54:44] = r46;
                        r48 = r22[54:44];
                        r83 = $signed({{1{r48[10]}}, r48});
                        r49 = r83[11:1];
                        r50 = $signed(r49[9:0]);
                        r51 = r22[43:34];
                        r52 = r51;
                        r52[9:0] = r50;
                        r53 = r47;
                        r53[43:34] = r52;
                        r54 = r22[43:34];
                        r55 = r50 - r54;
                        r56 = r53;
                        r56[64:55] = r55;
                        r57 = r40;
                        r57[64:0] = r56;
                        r58 = r56[64:55];
                        r60 = $signed(r58[9:0]);
                        r59 = {l14, r60};
                        r61 = r57;
                        r61[77:67] = r59;
                        r62 = r37 ? r61 : r40;
                        r63 = r18 ? r62 : r5;
                        r64 = r1[77:67];
                        r65 = r7[10:10];
                        r66 = ~r65;
                        r67 = l15;
                        r67[10:0] = r64;
                        r68 = r67;
                        r68[12:12] = l16;
                        r69 = r68;
                        r69[11:11] = r66;
                        r70 = r71[1:1];
                        r72 = |r70;
                        r73 = r63;
                        r73[64:0] = l17;
                        r74 = r73;
                        r74[66:65] = l18;
                        r75 = r74;
                        r75[77:67] = l19;
                        r76 = r69;
                        r76[11:11] = l20;
                        r77 = r72 ? r75 : r63;
                        r78 = r72 ? r76 : r69;
                        r79 = {r77, r78};
                        kernel_cic_pruned_kernel = r79;
                     end
               endfunction
            endmodule
            module top_stages(input wire [1:0] clock_reset, input wire [64:0] i, output reg [64:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 65'b00000000000000000000000000000000000000000000000000000000000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 65'b00000000000000000000000000000000000000000000000000000000000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_phase(input wire [1:0] clock_reset, input wire [1:0] i, output reg [1:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 2'b00;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 2'b00;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_out(input wire [1:0] clock_reset, input wire [10:0] i, output reg [10:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 11'b00000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 11'b00000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    #[test]
    fn iverilog_agrees_with_the_simulator() -> miette::Result<()> {
        let uut = Uut::default();
        let tb = uut.run(stream()).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn trace_digest_is_stable() -> miette::Result<()> {
        let uut = Uut::default();
        let vcd = uut.run(stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("cic_pruned");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["07c9e9e876d7fc62d531eaff6088f7759e9c86c3ac5433abcb859c4aa3636d48"];
        let digest = vcd.dump_to_file(root.join("cic_pruned.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
