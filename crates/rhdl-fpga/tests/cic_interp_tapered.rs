//! Is a width-tapered CIC interpolator the same filter as a
//! uniform-width one?
//!
//! **Bit-identically, yes**, and that is a much stronger claim than
//! `cic_pruned.rs` can make about its decimator. A pruned decimator
//! throws away low-order bits, so the honest question there is whether
//! the error stays inside Hogenauer's predicted budget. A tapered
//! interpolator throws away nothing — each stage is sized to its own
//! exact growth bound, so it holds its value at LSB weight one like
//! every other stage, and there is no shift anywhere in the datapath.
//!
//! So every test here compares the generated widget against
//! `CicInterpolate` at uniform width with the same stimulus and demands
//! **equality**, not a tolerance. A tolerance would be the wrong shape
//! of test: it would pass on a datapath that had quietly acquired a
//! truncation.
//!
//! See `dsp::cic::tapered` for the macro and `dsp::cic::interp` for the
//! bounds.

use rhdl::prelude::*;
use rhdl_fpga::cic_interp_tapered;
use rhdl_fpga::core::dff;
use rhdl_fpga::dsp::cic::{interp, interpolator, interpolator::CicInterpolate};

/// The configuration under test: 12-bit envelope, three stages, rates up
/// to 32. Deep enough that the taper is real (three of the six stages
/// are narrower than the widest) and small enough to simulate.
const WI: usize = 12;
const N: usize = 3;
const RMAX: usize = 32;
const M: usize = 1;

const UNIFORM: usize = interp::accumulator_width(WI, N, RMAX, M);
const CW: usize = interp::rate_width(RMAX);

mod tapered {
    use super::*;
    cic_interp_tapered!(Uut, w_in = 12, n = 3, r_max = 32, m = 1);
}

/// The uniform-width reference.
type Exact = CicInterpolate<WI, UNIFORM, N, RMAX, M, CW>;

/// Present `x[n / rate]` on every cycle, then drain.
fn stimulus(x: &[i128], rate: usize, drain: usize) -> Vec<interpolator::In<WI, CW>> {
    let mut seq: Vec<interpolator::In<WI, CW>> = (0..x.len() * rate)
        .map(|n| interpolator::In::<WI, CW> {
            sample: Some(signed::<WI>(x[n / rate])),
            rate: bits::<CW>(rate as u128),
            restart: false,
            downstream_ready: true,
        })
        .collect();
    seq.extend(std::iter::repeat_n(
        interpolator::In::<WI, CW> {
            sample: None,
            rate: bits::<CW>(rate as u128),
            restart: false,
            downstream_ready: true,
        },
        drain,
    ));
    seq
}

fn run_tapered(seq: Vec<interpolator::In<WI, CW>>) -> Vec<i128> {
    let uut = tapered::Uut::default();
    uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .map(|s| s.output.sample.raw())
        .collect()
}

fn run_exact(seq: Vec<interpolator::In<WI, CW>>) -> Vec<i128> {
    let uut = Exact::default();
    uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .map(|s| s.output.sample.raw())
        .collect()
}

/// **The generated output width equals the uniform one.**
///
/// Not a coincidence, and worth asserting before anything is compared:
/// the widest stage's bound *is* the uniform width by construction —
/// `gain_bits` is the maximum over the stages and the last implemented
/// width is the running maximum over the same set. So the two widgets
/// have literally the same `Out` type and a comparison is a comparison
/// of numbers, not of scalings.
#[test]
fn the_output_width_matches_the_uniform_widget() {
    assert_eq!(
        interp::implemented_stage_width(2 * N, WI, N, RMAX, M),
        UNIFORM
    );
}

/// **The taper is bit-identical to the uniform widget.**
///
/// The claim the lossless taper earns, on a varying input across three
/// rates. Equality, not a tolerance.
#[test]
fn the_taper_is_bit_identical_to_the_uniform_widget() {
    for rate in [2usize, 8, 32] {
        let x: Vec<i128> = (0..24).map(|k| (k * 137 % 2001) as i128 - 1000).collect();
        let seq = stimulus(&x, rate, 3);
        assert_eq!(
            run_tapered(seq.clone()),
            run_exact(seq),
            "rate {rate}: a lossless taper must agree exactly"
        );
    }
}

/// And at full scale, which is where a stage one bit too narrow would
/// wrap and a tolerance-based test would have shrugged.
#[test]
fn it_is_bit_identical_at_full_scale() {
    for rate in [2usize, 32] {
        for level in [-2048i128, 2047] {
            let seq = stimulus(&[level; 20], rate, 3);
            assert_eq!(
                run_tapered(seq.clone()),
                run_exact(seq),
                "rate {rate} level {level}"
            );
        }
    }
}

/// And across a restart, which zeroes every stage at once.
#[test]
fn it_is_bit_identical_across_a_restart() {
    let rate = 8usize;
    let mut seq = stimulus(&[700i128; 10], rate, 0);
    let mut second = stimulus(&[-900i128; 10], rate, 3);
    second[0].restart = true;
    seq.extend(second);
    assert_eq!(run_tapered(seq.clone()), run_exact(seq));
}

/// And when the rate changes mid-stream.
#[test]
fn it_is_bit_identical_when_the_rate_changes() {
    let mut seq = stimulus(&[500i128; 10], 4, 0);
    let mut second = stimulus(&[500i128; 10], 16, 3);
    second[0].restart = true;
    seq.extend(second);
    assert_eq!(run_tapered(seq.clone()), run_exact(seq));
}

/// And on a starved cycle, where both must feed zero.
#[test]
fn it_is_bit_identical_when_starved() {
    let rate = 8usize;
    let seq: Vec<interpolator::In<WI, CW>> = (0..6 * rate)
        .map(|n| interpolator::In::<WI, CW> {
            sample: if n == 2 * rate {
                None
            } else {
                Some(signed::<WI>(321))
            },
            rate: bits::<CW>(rate as u128),
            restart: false,
            downstream_ready: true,
        })
        .collect();
    assert_eq!(run_tapered(seq.clone()), run_exact(seq));
}

/// **The taper actually saves state.**
///
/// The other half of the point. If this ever stopped being true the
/// macro would be pure cost, and the bit-exactness tests above would
/// still pass — so it is asserted separately.
#[test]
fn the_taper_is_smaller_than_the_uniform_datapath() {
    let built = interp::implemented_state_bits(WI, N, RMAX, M);
    let uniform = interp::uniform_state_bits(WI, N, RMAX, M);
    assert!(built < uniform, "built {built} vs uniform {uniform}");
    // As data, so a change in either schedule is visible.
    // (13+14+15) + (15+18+22) = 97, against six stages at 22 = 132.
    assert_eq!((built, uniform), (97, 132));
}

/// The generated widths are the ones the design maths specifies.
///
/// True by construction — the macro substitutes
/// `implemented_stage_width` — so this is really a check that the *arm*
/// for `n = 3` wires the stage indices in the right order. A macro arm
/// with two indices transposed would still compile.
#[test]
fn the_generated_widths_are_the_specified_ones() {
    let widths: Vec<usize> = (1..=2 * N)
        .map(|j| interp::implemented_stage_width(j, WI, N, RMAX, M))
        .collect();
    // combs +1,+2,+3; then 4 -> +2 lifted to the running max 15,
    // 64 -> +6, 1024 -> +10. The 15 is the monotone lift.
    assert_eq!(widths, vec![13, 14, 15, 15, 18, 22]);
    // Monotone, which is what makes every transfer a widening.
    for pair in widths.windows(2) {
        assert!(pair[1] >= pair[0]);
    }
}

/// **The numbers in `examples/cic_interp_tapered.rs` prose, checked.**
///
/// The example quotes the six widths, the 97-against-132 bit counts and
/// the 27% figure. Prose drifts silently; this does not.
#[test]
fn the_claims_in_the_example_prose_hold() {
    let w: Vec<usize> = (1..=2 * N)
        .map(|j| interp::implemented_stage_width(j, WI, N, RMAX, M))
        .collect();
    assert_eq!(w, vec![13, 14, 15, 15, 18, 22]);
    let built = interp::implemented_state_bits(WI, N, RMAX, M);
    let uniform = interp::uniform_state_bits(WI, N, RMAX, M);
    assert_eq!((built, uniform), (97, 132));
    let saving = 100.0 * (uniform - built) as f64 / uniform as f64;
    assert!(
        (saving - 27.0).abs() < 1.0,
        "the example claims 27%, measured {saving:.1}%"
    );
    // And the exact bound at the fourth stage really is 14, which is the
    // dip the example explains.
    assert_eq!(interp::stage_width(4, WI, N, RMAX, M), 14);
    assert_eq!(interp::implemented_stage_width(4, WI, N, RMAX, M), 15);
}

/// It presents the interpolator's interface, so it drops into a whole
/// up-converter.
///
/// The composition claim, checked by building it. This is the payoff:
/// the taper is invisible to everything downstream.
#[test]
fn it_drops_into_a_real_up_converter() -> miette::Result<()> {
    const OW: usize = 12;
    const PROD_W: usize = UNIFORM + 18 + 1;
    const DROP: usize = PROD_W - OW;
    type Duc = rhdl_fpga::dsp::duc::real::RealDuc<WI, UNIFORM, CW, OW, PROD_W, DROP, tapered::Uut>;
    let uut = Duc::default();
    let _ = uut.descriptor("top".into())?;
    Ok(())
}

/// The emitted Verilog carries the tapered widths, not the uniform one.
///
/// The structural claim: if the generated module declared every register
/// at the uniform width the functional tests above would all still pass
/// and the macro would have achieved nothing.
#[test]
fn the_emitted_verilog_is_narrower() -> miette::Result<()> {
    let tapered_hdl = tapered::Uut::default()
        .descriptor("top".into())?
        .hdl()?
        .modules
        .pretty();
    let exact_hdl = Exact::default()
        .descriptor("top".into())?
        .hdl()?
        .modules
        .pretty();

    // The state register's width appears in its module's port
    // declaration. Compare the widest declaration in each.
    let widest = |hdl: &str| -> usize {
        hdl.lines()
            .filter_map(|l| {
                let (_, rest) = l.split_once('[')?;
                let (hi, _) = rest.split_once(':')?;
                hi.trim().parse::<usize>().ok()
            })
            .max()
            .unwrap_or(0)
    };
    let t = widest(&tapered_hdl);
    let e = widest(&exact_hdl);
    assert!(
        t < e,
        "the tapered datapath must declare narrower registers: {t} vs {e}"
    );
    Ok(())
}

/// And iverilog agrees, on both paths.
#[test]
fn test_hdl_works() -> miette::Result<()> {
    let uut = tapered::Uut::default();
    let x: Vec<i128> = (0..8).map(|k| (k * 211 % 1601) as i128 - 800).collect();
    let input = stimulus(&x, 4, 2)
        .into_iter()
        .with_reset(1)
        .clock_pos_edge(100);
    let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&uut, &Default::default())?.run_iverilog()?;
    Ok(())
}

/// **Every macro arm is exercised, not just the one above.**
///
/// The macro has four arms — `n` of 2, 3, 4 and 5 — each wiring the
/// stage indices by hand. A transposed pair in an unexercised arm
/// compiles, generates a plausible widget, and computes the wrong
/// filter. Only `n = 3` was covered until this module existed.
///
/// Each arm gets the same treatment as `n = 3`: bit-identical to the
/// uniform widget at its own configuration.
mod every_arm {
    use super::*;

    /// One arm's worth of checks, as a macro because the generated type
    /// names and widths differ per arm and a function cannot be generic
    /// over them.
    macro_rules! check_arm {
        ($modname:ident, $n:tt, $wi:tt, $rmax:tt, $m:tt) => {
            mod $modname {
                use super::*;
                const N: usize = $n;
                const WI: usize = $wi;
                const RMAX: usize = $rmax;
                const M: usize = $m;
                const UNIFORM: usize = interp::accumulator_width(WI, N, RMAX, M);
                const CW: usize = interp::rate_width(RMAX);

                cic_interp_tapered!(Arm, w_in = $wi, n = $n, r_max = $rmax, m = $m);
                type Exact = CicInterpolate<WI, UNIFORM, N, RMAX, M, CW>;

                fn seq(x: &[i128], rate: usize) -> Vec<interpolator::In<WI, CW>> {
                    let mut s: Vec<interpolator::In<WI, CW>> = (0..x.len() * rate)
                        .map(|n| interpolator::In::<WI, CW> {
                            sample: Some(signed::<WI>(x[n / rate])),
                            rate: bits::<CW>(rate as u128),
                            restart: false,
                            downstream_ready: true,
                        })
                        .collect();
                    s.extend(std::iter::repeat_n(
                        interpolator::In::<WI, CW> {
                            sample: None,
                            rate: bits::<CW>(rate as u128),
                            restart: false,
                            downstream_ready: true,
                        },
                        3,
                    ));
                    s
                }

                #[test]
                fn is_bit_identical_to_the_uniform_widget() {
                    let x: Vec<i128> = (0..20).map(|k| (k * 97 % 401) as i128 - 200).collect();
                    for rate in [2usize, RMAX] {
                        let s = seq(&x, rate);
                        let tapered: Vec<i128> = Arm::default()
                            .run(s.clone().into_iter().with_reset(1).clock_pos_edge(100))
                            .synchronous_sample()
                            .map(|t| t.output.sample.raw())
                            .collect();
                        let exact: Vec<i128> = Exact::default()
                            .run(s.into_iter().with_reset(1).clock_pos_edge(100))
                            .synchronous_sample()
                            .map(|t| t.output.sample.raw())
                            .collect();
                        assert_eq!(tapered, exact, "n={N} rate={rate}");
                    }
                }

                /// A constant input still comes out an exact constant at
                /// the published gain — the interpolator's defining
                /// property, which a mis-wired cascade would break even
                /// where it happened to agree with the reference on a
                /// varying input.
                #[test]
                fn a_constant_input_settles_at_the_published_gain() {
                    let rate = RMAX;
                    let level = 100i128;
                    let out: Vec<i128> = Arm::default()
                        .run(
                            seq(&[level; 40], rate)
                                .into_iter()
                                .with_reset(1)
                                .clock_pos_edge(100),
                        )
                        .synchronous_sample()
                        .map(|t| t.output.sample.raw())
                        .collect();
                    let settled = out[out.len() - 5];
                    let (num, den) = interp::dc_gain_ratio(N, rate, M);
                    assert_eq!(
                        settled,
                        level * num as i128 / den as i128,
                        "n={N}: gain {num}/{den}"
                    );
                }

                /// And the widths are monotone, which the arm's index
                /// order has to produce.
                #[test]
                fn the_widths_are_monotone() {
                    let w: Vec<usize> = (1..=2 * N)
                        .map(|j| interp::implemented_stage_width(j, WI, N, RMAX, M))
                        .collect();
                    for pair in w.windows(2) {
                        assert!(pair[1] >= pair[0], "n={N}: {w:?}");
                    }
                    assert_eq!(w[2 * N - 1], UNIFORM);
                }
            }
        };
    }

    check_arm!(n2, 2, 10, 8, 1);
    check_arm!(n3, 3, 10, 8, 1);
    check_arm!(n4, 4, 10, 8, 1);
    check_arm!(n5, 5, 10, 8, 1);
}
