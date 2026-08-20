#![warn(missing_docs)]
//! `CordicVectoring` — `Iq` to magnitude and phase.
//!
//! **Read [`super`] before using this.** On an FPGA a CORDIC is usually
//! the wrong answer; this exists for the cases where polar samples are
//! genuinely needed in hardware at rate.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+CordicVectoring+----+
      |                      |
+---->+ sample     magnitude +----->
      |                      |
      |                phase +----->
      |                      |
      |                valid +----->
      +----------------------+
")]
//!
//! # How it works
//!
//! Vectoring mode rotates the input vector onto the positive real axis
//! by a fixed sequence of ever-smaller rotations, accumulating the
//! angle it removed. What remains on the real axis is the magnitude
//! (scaled by the gain `K`), and the accumulated angle is `atan2`.
//!
//! Each iteration is a shift, an add and a subtract — no multiplier —
//! which is the algorithm's whole appeal. The price is one stage per
//! bit: [`super::ITERATIONS`] cycles of latency and that many sets of
//! registers.
//!
//! # Quadrant handling
//!
//! The rotations only converge for vectors in the right half plane, so
//! a vector with negative real part is pre-rotated by half a turn and
//! the angle corrected afterwards. Without this, everything in the left
//! half plane converges to the wrong answer *quietly* — the magnitude
//! still looks plausible.
//!
//! # Sign tests use the sign bit, not a comparison
//!
//! Each iteration chooses its direction from the sign of `y`. That test
//! is written as a bit mask rather than `y < 0`, which is deliberate:
//! signed comparison against a literal has been a source of codegen
//! defects in this tree, and a bit test does not depend on the
//! operand's declared signedness at all.

//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/cordic_magphase.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/cordic_magphase.md")]

use rhdl::prelude::*;

use crate::core::dff;
use crate::dsp::iq::Iq;

use super::{ANGLE_W, ATAN_TABLE, HALF_TURN, INT_W, INV_GAIN_Q17, ITERATIONS, sign_extend};

/// One pipeline stage's state.
#[derive(PartialEq, Clone, Copy, Debug, Digital, Default)]
#[doc(hidden)]
pub struct Stage {
    /// Real component, rotating toward the magnitude.
    pub x: SignedBits<INT_W>,
    /// Imaginary component, rotating toward zero.
    pub y: SignedBits<INT_W>,
    /// Accumulated angle.
    pub z: SignedBits<ANGLE_W>,
    /// This slot holds a real sample.
    pub valid: bool,
}

/// `Iq` to magnitude and phase.
#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
pub struct CordicVectoring<const W: usize>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// One slot per iteration. Bundled in a single DFF rather than as
    /// sibling fields: sixteen `DFF`s would blow the twelve-element
    /// ceiling on the derived `Q`/`D` tuples (CLAUDE.md §3.1).
    pipe: dff::DFF<[Stage; ITERATIONS]>,
}

/// Inputs to [`CordicVectoring`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const W: usize>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The rectangular sample, or `None` for an idle cycle.
    pub sample: Option<Iq<W>>,
}

/// Outputs from [`CordicVectoring`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out {
    /// Magnitude, **gain-corrected**.
    ///
    /// The algorithm's own gain `K = 1.6468…` is removed inside the
    /// widget, so this is the magnitude and not a scaled version of it.
    /// Leaving the correction to the caller makes the outputs
    /// uncomposable: feeding an uncorrected magnitude to
    /// [`super::rotation`] applies `K` twice.
    pub magnitude: SignedBits<INT_W>,
    /// Phase, a full turn being `2^ANGLE_W`.
    pub phase: SignedBits<ANGLE_W>,
    /// The outputs correspond to a real input sample.
    pub valid: bool,
}

impl<const W: usize> SynchronousIO for CordicVectoring<W>
where
    rhdl::bits::W<W>: BitWidth,
{
    type I = In<W>;
    type O = Out;
    type Kernel = cordic_vectoring_kernel<W>;
}

#[kernel]
#[doc(hidden)]
pub fn cordic_vectoring_kernel<const W: usize>(cr: ClockReset, i: In<W>, q: Q<W>) -> (Out, D<W>)
where
    rhdl::bits::W<W>: BitWidth,
{
    let mut d = D::<W>::dont_care();
    let mut next = q.pipe;

    // ---- stage 0: quadrant fold, then iteration 0 ----
    let mut seed = Stage {
        x: signed::<INT_W>(0),
        y: signed::<INT_W>(0),
        z: signed::<ANGLE_W>(0),
        valid: false,
    };
    if let Some(s) = i.sample {
        // Explicit sign extension -- see `super::sign_extend`.
        let mut x = sign_extend::<W, INT_W>(s.re);
        let mut y = sign_extend::<W, INT_W>(s.im);
        let mut z = signed::<ANGLE_W>(0);
        // Left half plane: rotate by half a turn first, since the
        // iteration only converges for x >= 0.
        let x_neg = (x.as_unsigned() & bits::<INT_W>(1 << (INT_W - 1))) != bits::<INT_W>(0);
        if x_neg {
            // `zero - v` rather than `-v`: unary negation on a value
            // derived from an `Option` payload trips a dynamic type
            // error in the VM ("cannot negate unsigned value"), part of
            // the same signedness-through-aggregates family documented
            // on `super::sign_extend`.
            let zero = signed::<INT_W>(0);
            x = zero - x;
            y = zero - y;
            // Half a turn.
            //
            // In a signed angle of ANGLE_W bits this is the *most
            // negative* value, not +2^(ANGLE_W-1) which does not fit --
            // and it is the same point, since angle arithmetic is modulo
            // a full turn.
            //
            // Written as a literal rather than `-(1 << (ANGLE_W - 1))`:
            // the negation there applies to an *unsigned* shift result
            // before `signed()` converts it, and the kernel compiler
            // rejects that with "cannot negate unsigned value 20000_b64"
            // -- 0x20000 being this constant in hex.
            z = signed::<ANGLE_W>(HALF_TURN);
        }
        seed = Stage {
            x,
            y,
            z,
            valid: true,
        };
    }

    // Unrolled rather than a loop over a dynamic index.
    //
    // `for k in 1..ITERATIONS { next[k] = step(q.pipe[k-1], k) }` is the
    // natural spelling and panics the compiler: `lower_rhif_to_rtl.rs`
    // computes `array.size.min(1 << slot_bits)` for a dynamic index, and
    // with a `usize` index `slot_bits` is 64, so `1 << 64` overflows
    // before the `.min()` can clamp it.  Unrolling uses only constant
    // indices, which take a different path.  Filed as compiler work.
    next[0] = cordic_step(seed, bits::<8>(0), signed::<ANGLE_W>(ATAN_TABLE[0]));
    next[1] = cordic_step(q.pipe[0], bits::<8>(1), signed::<ANGLE_W>(ATAN_TABLE[1]));
    next[2] = cordic_step(q.pipe[1], bits::<8>(2), signed::<ANGLE_W>(ATAN_TABLE[2]));
    next[3] = cordic_step(q.pipe[2], bits::<8>(3), signed::<ANGLE_W>(ATAN_TABLE[3]));
    next[4] = cordic_step(q.pipe[3], bits::<8>(4), signed::<ANGLE_W>(ATAN_TABLE[4]));
    next[5] = cordic_step(q.pipe[4], bits::<8>(5), signed::<ANGLE_W>(ATAN_TABLE[5]));
    next[6] = cordic_step(q.pipe[5], bits::<8>(6), signed::<ANGLE_W>(ATAN_TABLE[6]));
    next[7] = cordic_step(q.pipe[6], bits::<8>(7), signed::<ANGLE_W>(ATAN_TABLE[7]));
    next[8] = cordic_step(q.pipe[7], bits::<8>(8), signed::<ANGLE_W>(ATAN_TABLE[8]));
    next[9] = cordic_step(q.pipe[8], bits::<8>(9), signed::<ANGLE_W>(ATAN_TABLE[9]));
    next[10] = cordic_step(q.pipe[9], bits::<8>(10), signed::<ANGLE_W>(ATAN_TABLE[10]));
    next[11] = cordic_step(q.pipe[10], bits::<8>(11), signed::<ANGLE_W>(ATAN_TABLE[11]));
    next[12] = cordic_step(q.pipe[11], bits::<8>(12), signed::<ANGLE_W>(ATAN_TABLE[12]));
    next[13] = cordic_step(q.pipe[12], bits::<8>(13), signed::<ANGLE_W>(ATAN_TABLE[13]));
    next[14] = cordic_step(q.pipe[13], bits::<8>(14), signed::<ANGLE_W>(ATAN_TABLE[14]));
    next[15] = cordic_step(q.pipe[14], bits::<8>(15), signed::<ANGLE_W>(ATAN_TABLE[15]));
    d.pipe = next;

    let last = q.pipe[ITERATIONS - 1];

    // Correct the CORDIC gain here rather than leaving it to the
    // caller.  An uncorrected magnitude is 64.7% too large, and a
    // widget that returns "the magnitude, times a constant you have to
    // know about" is one whose outputs cannot be composed -- feeding it
    // straight to `rotation` would apply K twice, which is exactly the
    // 58212-of-90000 error the round-trip test caught when this
    // correction lived in the test instead.
    let corrected =
        ((sign_extend::<INT_W, 48>(last.x) * signed::<48>(INV_GAIN_Q17)) >> 17).resize::<INT_W>();

    let o = Out {
        magnitude: corrected,
        phase: last.z,
        valid: last.valid,
    };

    if cr.reset.any() {
        d.pipe = [Stage {
            x: signed::<INT_W>(0),
            y: signed::<INT_W>(0),
            z: signed::<ANGLE_W>(0),
            valid: false,
        }; ITERATIONS];
    }
    (o, d)
}

/// One CORDIC vectoring iteration.
#[kernel]
#[doc(hidden)]
pub fn cordic_step(s: Stage, k: Bits<8>, angle: SignedBits<ANGLE_W>) -> Stage {
    // Direction from the sign of y: rotate toward y = 0. A bit test
    // rather than `y < 0`, so it cannot depend on how signedness
    // survives codegen.
    let y_neg = (s.y.as_unsigned() & bits::<INT_W>(1 << (INT_W - 1))) != bits::<INT_W>(0);

    let xs = s.x >> k;
    let ys = s.y >> k;

    let mut out = s;
    if y_neg {
        // y < 0: rotate counter-clockwise.
        out.x = s.x - ys;
        out.y = s.y + xs;
        out.z = s.z - angle;
    } else {
        out.x = s.x + ys;
        out.y = s.y - xs;
        out.z = s.z + angle;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::f64::consts::TAU;

    const W: usize = 18;
    type Uut = CordicVectoring<W>;

    /// The widget corrects its own gain, so this is a plain
    /// conversion.
    fn corrected(raw: i128) -> f64 {
        raw as f64
    }

    fn turns(phase: i128) -> f64 {
        phase as f64 / (1i128 << ANGLE_W) as f64
    }

    /// Run a set of vectors through and return (magnitude, phase) for
    /// each, aligned to its input.
    fn convert(points: &[(i128, i128)]) -> Vec<(f64, f64)> {
        let uut = Uut::default();
        let mut seq: Vec<In<W>> = points
            .iter()
            .map(|(re, im)| In::<W> {
                sample: Some(Iq::<W> {
                    re: signed::<W>(*re),
                    im: signed::<W>(*im),
                }),
            })
            .collect();
        // Flush the pipeline.
        seq.extend(std::iter::repeat_n(
            In::<W> { sample: None },
            ITERATIONS + 2,
        ));

        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter(|s| s.output.valid)
            .map(|s| {
                (
                    corrected(s.output.magnitude.raw()),
                    turns(s.output.phase.raw()),
                )
            })
            .collect()
    }

    #[test]
    fn default_construction() {
        let _uut = Uut::default();
    }

    /// **Accuracy across all four quadrants.**
    ///
    /// A ring of vectors at constant radius, so magnitude error and
    /// phase error are both visible and neither can hide behind a
    /// varying amplitude. Quadrant coverage matters because the left
    /// half plane goes through the pre-rotation path, which converges to
    /// a plausible-looking wrong answer if it is missing.
    #[test]
    fn accurate_over_the_whole_circle() {
        const R: f64 = 100_000.0;
        const N: usize = 64;
        let points: Vec<(i128, i128)> = (0..N)
            .map(|k| {
                let t = TAU * k as f64 / N as f64;
                ((R * t.cos()) as i128, (R * t.sin()) as i128)
            })
            .collect();
        let got = convert(&points);
        assert_eq!(got.len(), N, "the pipeline dropped samples");

        let mut worst_mag = 0.0f64;
        let mut worst_phase = 0.0f64;
        for (k, (m, p)) in got.iter().enumerate() {
            let want_t = (k as f64 / N as f64).fract();
            let want_m = ((points[k].0 * points[k].0 + points[k].1 * points[k].1) as f64).sqrt();
            worst_mag = worst_mag.max((m - want_m).abs());
            // Phase is modulo one turn; compare on the circle.
            let mut e = (p - want_t).abs();
            if e > 0.5 {
                e = 1.0 - e;
            }
            worst_phase = worst_phase.max(e);
        }
        // Magnitude to a handful of LSB out of 100000, phase to well
        // under a thousandth of a turn.
        assert!(
            worst_mag < 200.0,
            "worst magnitude error {worst_mag:.1} of {R} -- more than quantisation"
        );
        assert!(
            worst_phase < 1e-3,
            "worst phase error {worst_phase:.2e} turns ({:.4} deg)",
            worst_phase * 360.0
        );
    }

    /// The axes, where quadrant folding is most likely to be off by a
    /// half turn.
    #[test]
    fn the_axes_are_correct() {
        let r = 80_000i128;
        let got = convert(&[(r, 0), (0, r), (-r, 0), (0, -r)]);
        let want_phase = [0.0, 0.25, 0.5, 0.75];
        for (k, (m, p)) in got.iter().enumerate() {
            assert!(
                (m - r as f64).abs() < 200.0,
                "axis {k}: magnitude {m:.0} should be {r}"
            );
            let mut e = (p - want_phase[k]).abs();
            if e > 0.5 {
                e = 1.0 - e;
            }
            assert!(
                e < 1e-3,
                "axis {k}: phase {p:.5} turns should be {:.5}",
                want_phase[k]
            );
        }
    }

    /// A zero vector must not produce a spurious magnitude.
    #[test]
    fn the_origin_has_zero_magnitude() {
        let got = convert(&[(0, 0)]);
        assert!(
            got[0].0.abs() < 4.0,
            "the origin gave magnitude {:.1}",
            got[0].0
        );
    }

    /// Validity tracks the sample through all sixteen stages, so an
    /// idle input does not emit a stale conversion.
    #[test]
    fn latency_is_the_iteration_count() {
        let uut = Uut::default();
        let mut seq = vec![In::<W> {
            sample: Some(Iq::<W> {
                re: signed::<W>(50_000),
                im: signed::<W>(50_000),
            }),
        }];
        seq.extend(std::iter::repeat_n(
            In::<W> { sample: None },
            ITERATIONS + 4,
        ));
        let out: Vec<bool> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.valid)
            .collect();
        let first = out.iter().position(|v| *v).expect("nothing was ever valid");
        // One reset cycle, then ITERATIONS stages.
        assert_eq!(
            first,
            1 + ITERATIONS,
            "latency should be {ITERATIONS} cycles after the reset cycle"
        );
        assert_eq!(
            out.iter().filter(|v| **v).count(),
            1,
            "exactly one sample went in, so exactly one result should come out"
        );
    }

    /// **What this costs**, measured from the emitted Verilog.
    ///
    /// The module docs claim a CORDIC is expensive; this puts numbers
    /// on it rather than leaving it as an assertion. Printed rather
    /// than bounded, because the point is to inform a reader deciding
    /// whether to instantiate one, not to freeze an implementation
    /// detail.
    #[test]
    fn report_the_resource_cost() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let adds = hdl.matches(" + ").count();
        let subs = hdl.matches(" - ").count();
        let muls = hdl.matches(" * ").count();
        let regs = hdl
            .lines()
            .filter(|l| l.trim_start().starts_with("reg "))
            .count();
        println!(
            "\n  CordicVectoring<{W}> at {ITERATIONS} iterations:\n\
             \x20   {adds} adds, {subs} subtracts, {muls} multiplies\n\
             \x20   {regs} register declarations, {ITERATIONS} cycles of latency\n\
             \x20   (the single multiply is the gain correction)"
        );
        assert_eq!(muls, 1, "only the gain correction should need a multiplier");
        assert!(
            adds + subs >= ITERATIONS,
            "expected at least one add/subtract per iteration"
        );
        Ok(())
    }

    /// Tier 3 — HDL emission shape.
    ///
    /// Shape only: 553 register declarations make a full snapshot
    /// unreviewable, and the resource counts are asserted separately by
    /// `report_the_resource_cost`.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let shape = hdl
            .lines()
            .filter(|l| l.starts_with("module "))
            .map(|l| l.split('(').next().unwrap().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let expect = expect![[r#"
            module top
            module top_pipe"#]];
        expect.assert_eq(&shape);
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn test_cordic_vectoring_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let seq: Vec<In<W>> = (0..24i128)
            .map(|k| In::<W> {
                sample: Some(Iq::<W> {
                    re: signed::<W>((k - 12) * 7000),
                    im: signed::<W>((12 - k) * 5000),
                }),
            })
            .collect();
        let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("cordic_vectoring");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["e95f2751c7ed7423be6e12d2008aae58d887461113270e03c0ef225d649ec366"];
        let digest = vcd.dump_to_file(root.join("cordic_vectoring.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    /// Tier 4 — emitted Verilog agrees with the Rust simulation.
    #[test]
    fn test_cordic_vectoring_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let seq: Vec<In<W>> = (0..24i128)
            .map(|k| In::<W> {
                sample: Some(Iq::<W> {
                    re: signed::<W>((k - 12) * 7000),
                    im: signed::<W>((12 - k) * 5000),
                }),
            })
            .collect();
        let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }
}
