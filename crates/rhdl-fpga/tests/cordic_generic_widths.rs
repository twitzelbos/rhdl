//! The CORDIC widgets work at widths other than the validated default.
//!
//! # Why this file exists
//!
//! Both widgets are generic, but genericity that is only ever
//! instantiated at one configuration is a claim, not a property. The
//! module's own tests all run at `W = 18, INT_W = 22`; passing them
//! proves the default still works and says nothing about any other
//! width.
//!
//! So these exercise the parameters that actually vary, end to end,
//! including through `iverilog` — a width-dependent mistake in the
//! sign-extension or the gain correction would show up as an accuracy
//! failure here and nowhere else.
//!
//! # What is *not* generic, and why
//!
//! `ANGLE_W = 18` and `ITERATIONS = 16` are fixed on purpose.
//!
//! `ANGLE_W` exists to match `dsp::nco`'s phase convention so an angle
//! from the CORDIC can drive the oscillator without rescaling; making it
//! a free parameter would break that coupling silently.
//!
//! `ITERATIONS` is not independently meaningful. In turn units at
//! `ANGLE_W = 18`, `atan(2^-i)` rounds to zero at `i = 17`:
//!
//! ```text
//! i=15  1.2732 -> 1
//! i=16  0.6366 -> 1
//! i=17  0.3183 -> 0      <- and every stage after
//! ```
//!
//! so stages past sixteen are pure latency and area. The count is
//! determined by the angle width, not chosen.

use rhdl::prelude::*;
use rhdl_fpga::dsp::cordic::rotation::{CordicRotation, In as RotIn};
use rhdl_fpga::dsp::cordic::vectoring::{CordicVectoring, In as VecIn};
use rhdl_fpga::dsp::cordic::{CordicConsts, int_width_is_sufficient, iterations_for};
use rhdl_fpga::dsp::iq::Iq;
use std::f64::consts::TAU;

/// Feed a circle of vectors through vectoring and return the measured
/// magnitude for each, at an arbitrary `(W, INT_W)`.
fn magnitudes<const W: usize, const INT_W: usize, const AW: usize, const N: usize>(
    radius: f64,
    n: usize,
) -> Vec<i128>
where
    rhdl::bits::W<W>: BitWidth,
    rhdl::bits::W<INT_W>: BitWidth,
    rhdl::bits::W<AW>: BitWidth,
{
    let uut = CordicVectoring::<W, INT_W, AW, N>::default();
    let mut seq: Vec<VecIn<W>> = (0..n)
        .map(|k| {
            let t = TAU * k as f64 / n as f64;
            VecIn::<W> {
                sample: Some(Iq::<W> {
                    re: signed::<W>((radius * t.cos()) as i128),
                    im: signed::<W>((radius * t.sin()) as i128),
                }),
            }
        })
        .collect();
    seq.extend(std::iter::repeat_n(VecIn::<W> { sample: None }, N + 2));
    uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .filter(|s| s.output.valid)
        .map(|s| s.output.magnitude.raw())
        .collect()
}

/// **A constant-radius sweep must come back at a constant magnitude.**
///
/// The single most sensitive check on a CORDIC: a gain, quadrant or
/// sign-extension error that depends on the width shows up here as a
/// magnitude that wobbles around the circle.
fn assert_flat_magnitude(label: &str, mags: &[i128], expected: f64, tol_frac: f64) {
    assert!(!mags.is_empty(), "{label}: no samples came out");
    for (k, m) in mags.iter().enumerate() {
        let err = (*m as f64 - expected).abs() / expected;
        assert!(
            err <= tol_frac,
            "{label}: sample {k} magnitude {m} vs expected {expected:.0} \
             ({:.3}% error, tolerance {:.3}%)",
            err * 100.0,
            tol_frac * 100.0
        );
    }
}

#[test]
fn the_default_configuration_is_still_accurate() {
    let mags = magnitudes::<18, 22, 18, 16>(90_000.0, 32);
    assert_flat_magnitude("W=18 INT_W=22", &mags, 90_000.0, 0.005);
}

/// A narrower sample width.
#[test]
fn a_narrow_configuration_is_accurate() {
    let mags = magnitudes::<12, 16, 18, 16>(1_400.0, 32);
    assert_flat_magnitude("W=12 INT_W=16", &mags, 1_400.0, 0.01);
}

/// A width between the two, on a non-multiple-of-four boundary.
#[test]
fn an_odd_configuration_is_accurate() {
    let mags = magnitudes::<15, 19, 18, 16>(11_000.0, 32);
    assert_flat_magnitude("W=15 INT_W=19", &mags, 11_000.0, 0.006);
}

/// Extra internal headroom beyond the minimum must not change the
/// answer — it is headroom, not precision.
#[test]
fn extra_headroom_does_not_change_the_result() {
    let tight = magnitudes::<12, 16, 18, 16>(1_400.0, 16);
    let roomy = magnitudes::<12, 20, 18, 16>(1_400.0, 16);
    assert_eq!(
        tight, roomy,
        "widening the internal datapath should not alter the result"
    );
}

/// **Round-trip at a non-default width.**
///
/// Vectoring then rotation must return the original vector. This is the
/// composition the widgets exist for, and until now it was only ever
/// checked at the default.
#[test]
fn vectoring_then_rotation_round_trips_at_a_narrow_width() {
    const W: usize = 12;
    const INT_W: usize = 16;
    const AW: usize = 18;
    const N: usize = 16;
    const R: f64 = 1_400.0;
    const COUNT: usize = 16;

    let originals: Vec<(i128, i128)> = (0..COUNT)
        .map(|k| {
            let t = TAU * k as f64 / COUNT as f64;
            ((R * t.cos()) as i128, (R * t.sin()) as i128)
        })
        .collect();

    let vec_uut = CordicVectoring::<W, INT_W, AW, N>::default();
    let mut vseq: Vec<VecIn<W>> = originals
        .iter()
        .map(|(re, im)| VecIn::<W> {
            sample: Some(Iq::<W> {
                re: signed::<W>(*re),
                im: signed::<W>(*im),
            }),
        })
        .collect();
    vseq.extend(std::iter::repeat_n(VecIn::<W> { sample: None }, N + 2));
    let polar: Vec<(i128, i128)> = vec_uut
        .run(vseq.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .filter(|s| s.output.valid)
        .map(|s| (s.output.magnitude.raw(), s.output.phase.raw()))
        .collect();
    assert_eq!(polar.len(), COUNT, "the forward pass dropped samples");

    let rot_uut = CordicRotation::<INT_W, AW, N>::default();
    let mut rseq: Vec<RotIn<INT_W, AW>> = polar
        .iter()
        .map(|(m, p)| RotIn::<INT_W, AW> {
            magnitude: Some(signed::<INT_W>(*m)),
            phase: signed::<AW>(*p),
        })
        .collect();
    rseq.extend(std::iter::repeat_n(
        RotIn::<INT_W, AW> {
            magnitude: None,
            phase: signed::<AW>(0),
        },
        N + 2,
    ));
    let back: Vec<(i128, i128)> = rot_uut
        .run(rseq.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .filter(|s| s.output.valid)
        .map(|s| (s.output.sample.re.raw(), s.output.sample.im.raw()))
        .collect();
    assert_eq!(back.len(), COUNT, "the reverse pass dropped samples");

    // Tolerance scaled to the radius: a 12-bit sample carries fewer bits
    // than the default, so the absolute error is correspondingly larger.
    let tol = R * 0.02;
    for (k, ((ore, oim), (bre, bim))) in originals.iter().zip(back.iter()).enumerate() {
        assert!(
            (*ore as f64 - *bre as f64).abs() <= tol && (*oim as f64 - *bim as f64).abs() <= tol,
            "sample {k}: ({ore}, {oim}) came back as ({bre}, {bim}), tolerance {tol:.0}"
        );
    }
}

/// **Both directions survive `iverilog` at a non-default width.**
///
/// The Rust simulator and the emitted Verilog must agree at whatever
/// width the widget is instantiated at, not just the one the module's
/// own tests use.
#[test]
fn both_directions_round_trip_through_iverilog_at_a_narrow_width() -> miette::Result<()> {
    const W: usize = 12;
    const INT_W: usize = 16;
    const AW: usize = 18;
    const N: usize = 16;

    let vec_uut = CordicVectoring::<W, INT_W, AW, N>::default();
    let vseq: Vec<VecIn<W>> = (0..8)
        .map(|k| VecIn::<W> {
            sample: Some(Iq::<W> {
                re: signed::<W>(1000 - 200 * k),
                im: signed::<W>(300 * k - 600),
            }),
        })
        .collect();
    let tb = vec_uut
        .run(vseq.into_iter().with_reset(1).clock_pos_edge(100))
        .collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&vec_uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&vec_uut, &Default::default())?.run_iverilog()?;

    let rot_uut = CordicRotation::<INT_W, AW, N>::default();
    let rseq: Vec<RotIn<INT_W, AW>> = (0..8)
        .map(|k| RotIn::<INT_W, AW> {
            magnitude: Some(signed::<INT_W>(1200)),
            phase: signed::<AW>(k * 8192),
        })
        .collect();
    let tb = rot_uut
        .run(rseq.into_iter().with_reset(1).clock_pos_edge(100))
        .collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&rot_uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&rot_uut, &Default::default())?.run_iverilog()?;
    Ok(())
}

/// The headroom rule is stated as a predicate rather than a comment.
#[test]
fn the_headroom_rule_is_checkable() {
    assert!(int_width_is_sufficient(18, 22));
    assert!(int_width_is_sufficient(12, 16));
    assert!(
        !int_width_is_sufficient(18, 21),
        "one bit short must not pass"
    );
    assert!(
        !int_width_is_sufficient(12, 12),
        "no headroom must not pass"
    );
}

// ---- the angle width is generic too ------------------------------------

/// **The generated constants reproduce the hand-written reference.**
///
/// The default configuration's table, gain and half-turn were literals
/// in the source. If the generic builder disagrees with them by even one
/// unit, every accuracy claim the module makes was measured against a
/// different widget than the one that now ships.
#[test]
fn the_builder_reproduces_the_reference_constants() {
    use rhdl_fpga::dsp::cordic::{ATAN_TABLE, INV_GAIN_Q17, ITERATIONS};
    let c = CordicConsts::<18, 16>::build();
    let table: Vec<i128> = c.atan.iter().map(|v| v.raw()).collect();
    assert_eq!(table, ATAN_TABLE.to_vec(), "generated arctangent table");
    assert_eq!(c.inv_gain_q17.raw(), INV_GAIN_Q17, "generated gain");
    assert_eq!(c.half_turn.raw(), -131_072, "generated half turn");
    assert_eq!(
        iterations_for(18),
        ITERATIONS,
        "iteration rule vs the default"
    );
}

/// No generated table entry rounds to zero — the iteration rule's whole
/// purpose.
#[test]
fn no_generated_stage_is_a_no_op() {
    macro_rules! check {
        ($aw:literal, $n:literal) => {{
            assert_eq!(iterations_for($aw), $n);
            let c = CordicConsts::<$aw, $n>::build();
            for (i, v) in c.atan.iter().enumerate() {
                assert!(v.raw() > 0, "angle_w={} stage {i} rounds to zero", $aw);
            }
        }};
    }
    check!(12, 10);
    check!(14, 12);
    check!(16, 14);
    check!(18, 16);
    check!(20, 18);
}

/// **A coarse angle width works end to end.**
///
/// The point of the parameter: 12-bit angles for an application that
/// does not need 18. Phase resolution drops with the width, so the
/// tolerance is stated relative to one LSB of the angle rather than
/// carried over from the default.
#[test]
fn a_coarse_angle_width_is_accurate() {
    let mags = magnitudes::<12, 16, 12, 10>(1_400.0, 32);
    assert_flat_magnitude("W=12 INT_W=16 AW=12 N=10", &mags, 1_400.0, 0.02);
}

/// A finer angle width than the default.
#[test]
fn a_fine_angle_width_is_accurate() {
    let mags = magnitudes::<18, 22, 20, 18>(90_000.0, 32);
    assert_flat_magnitude("W=18 INT_W=22 AW=20 N=18", &mags, 90_000.0, 0.005);
}

/// And a coarse-angle configuration survives `iverilog` in both
/// directions.
#[test]
fn a_coarse_angle_configuration_round_trips_through_iverilog() -> miette::Result<()> {
    const W: usize = 12;
    const INT_W: usize = 16;
    const AW: usize = 12;
    const N: usize = 10;

    let vec_uut = CordicVectoring::<W, INT_W, AW, N>::default();
    let vseq: Vec<VecIn<W>> = (0..8)
        .map(|k| VecIn::<W> {
            sample: Some(Iq::<W> {
                re: signed::<W>(1000 - 200 * k),
                im: signed::<W>(300 * k - 600),
            }),
        })
        .collect();
    let tb = vec_uut
        .run(vseq.into_iter().with_reset(1).clock_pos_edge(100))
        .collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&vec_uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&vec_uut, &Default::default())?.run_iverilog()?;

    let rot_uut = CordicRotation::<INT_W, AW, N>::default();
    let rseq: Vec<RotIn<INT_W, AW>> = (0..8)
        .map(|k| RotIn::<INT_W, AW> {
            magnitude: Some(signed::<INT_W>(1200)),
            phase: signed::<AW>(k * 256),
        })
        .collect();
    let tb = rot_uut
        .run(rseq.into_iter().with_reset(1).clock_pos_edge(100))
        .collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&rot_uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&rot_uut, &Default::default())?.run_iverilog()?;
    Ok(())
}

/// A mismatched iteration count is refused at construction.
///
/// `N` is a separate const generic only because Rust cannot compute an
/// array length from another const generic without `generic_const_exprs`.
/// The assert is what keeps it honest.
#[test]
#[should_panic(expected = "iteration count")]
fn a_wrong_iteration_count_is_rejected() {
    let _ = CordicVectoring::<12, 16, 18, 12>::default();
}

/// As is a too-narrow internal datapath.
#[test]
#[should_panic(expected = "INT_W")]
fn a_too_narrow_datapath_is_rejected() {
    let _ = CordicVectoring::<18, 21, 18, 16>::default();
}
