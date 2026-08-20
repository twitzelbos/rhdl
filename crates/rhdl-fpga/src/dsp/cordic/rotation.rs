#![warn(missing_docs)]
//! `CordicRotation` — magnitude and phase back to `Iq`.
//!
//! **Read [`super`] before using this.** The same caveats apply: on an
//! FPGA this is usually the wrong thing to build, and for a *carrier*
//! specifically it is definitely the wrong thing — a quarter-wave table
//! with linear interpolation ([`crate::dsp::nco::sin_cos_linear_interp`])
//! reaches −116 dBc with two multipliers and one cycle, where an
//! 8-stage CORDIC managed −103.6 dBc for sixteen adders and eight
//! cycles. That comparison was measured, not assumed.
//!
//! This exists for the inverse of [`super::vectoring`] — turning a
//! magnitude and phase that came *from* somewhere back into
//! rectangular — not for generating sinusoids.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+CordicRotation+-----+
      |                      |
+---->+ magnitude         re +----->
      |                      |
+---->+ phase             im +----->
      |                      |
      |                valid +----->
      +----------------------+
")]
//!
//! # Quadrant handling
//!
//! Rotation converges only for angles within a quarter turn of the real
//! axis. Angles outside that are folded by half a turn and both outputs
//! negated afterwards. Without the fold the second and third quadrants
//! converge to plausible-looking wrong answers.
//!
//! # Gain
//!
//! The algorithm multiplies its input by `K = 1.6468…`, so the incoming
//! magnitude is pre-scaled by [`super::INV_GAIN_Q17`] rather than
//! correcting afterwards. Pre-scaling keeps the correction out of the
//! critical path and applies it once instead of to both outputs.

use rhdl::prelude::*;

use crate::core::dff;
use crate::dsp::iq::Iq;

use super::{ANGLE_W, ATAN_TABLE, HALF_TURN, INT_W, INV_GAIN_Q17, ITERATIONS, sign_extend};

/// One pipeline stage's state.
#[derive(PartialEq, Clone, Copy, Debug, Digital, Default)]
#[doc(hidden)]
pub struct Stage {
    /// Real component.
    pub x: SignedBits<INT_W>,
    /// Imaginary component.
    pub y: SignedBits<INT_W>,
    /// Angle remaining to rotate.
    pub z: SignedBits<ANGLE_W>,
    /// Outputs must be negated at the end (second/third quadrant).
    pub negate: bool,
    /// This slot holds a real sample.
    pub valid: bool,
}

/// Magnitude and phase to `Iq`.
#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
pub struct CordicRotation {
    /// One slot per iteration; see the note in
    /// [`super::vectoring::CordicVectoring`] on why these are bundled.
    pipe: dff::DFF<[Stage; ITERATIONS]>,
}

/// Inputs to [`CordicRotation`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In {
    /// Magnitude, or `None` for an idle cycle.
    pub magnitude: Option<SignedBits<INT_W>>,
    /// Phase, a full turn being `2^ANGLE_W`.
    pub phase: SignedBits<ANGLE_W>,
}

/// Outputs from [`CordicRotation`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out {
    /// The rectangular sample.
    pub sample: Iq<INT_W>,
    /// The output corresponds to a real input sample.
    pub valid: bool,
}

impl SynchronousIO for CordicRotation {
    type I = In;
    type O = Out;
    type Kernel = cordic_rotation_kernel;
}

#[kernel]
#[doc(hidden)]
pub fn cordic_rotation_kernel(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut next = q.pipe;

    let mut seed = Stage {
        x: signed::<INT_W>(0),
        y: signed::<INT_W>(0),
        z: signed::<ANGLE_W>(0),
        negate: false,
        valid: false,
    };

    if let Some(mag) = i.magnitude {
        // Pre-scale by 1/K so the algorithm's own gain lands on the
        // right answer, rather than correcting both outputs afterwards.
        let scaled =
            ((sign_extend::<INT_W, 48>(mag) * signed::<48>(INV_GAIN_Q17)) >> 17).resize::<INT_W>();

        let mut z = i.phase;
        let mut negate = false;
        // Fold to within a quarter turn of the real axis.
        let quarter = signed::<ANGLE_W>(1 << (ANGLE_W - 2));
        // Half a turn: the most negative representable angle, the same
        // point as +half modulo a full turn.  A literal, not
        // `-(1 << ...)` -- see the note in `super::vectoring`.
        let half = signed::<ANGLE_W>(HALF_TURN);
        let z_neg = (z.as_unsigned() & bits::<ANGLE_W>(1 << (ANGLE_W - 1))) != bits::<ANGLE_W>(0);
        if z_neg {
            if z < -quarter {
                z += half;
                negate = true;
            }
        } else if z > quarter {
            z -= half;
            negate = true;
        }

        seed = Stage {
            x: scaled,
            y: signed::<INT_W>(0),
            z,
            negate,
            valid: true,
        };
    }

    // Unrolled; see the note in `super::vectoring` on the compiler
    // panic that a dynamic array index provokes.
    next[0] = rotation_step(seed, bits::<8>(0), signed::<ANGLE_W>(ATAN_TABLE[0]));
    next[1] = rotation_step(q.pipe[0], bits::<8>(1), signed::<ANGLE_W>(ATAN_TABLE[1]));
    next[2] = rotation_step(q.pipe[1], bits::<8>(2), signed::<ANGLE_W>(ATAN_TABLE[2]));
    next[3] = rotation_step(q.pipe[2], bits::<8>(3), signed::<ANGLE_W>(ATAN_TABLE[3]));
    next[4] = rotation_step(q.pipe[3], bits::<8>(4), signed::<ANGLE_W>(ATAN_TABLE[4]));
    next[5] = rotation_step(q.pipe[4], bits::<8>(5), signed::<ANGLE_W>(ATAN_TABLE[5]));
    next[6] = rotation_step(q.pipe[5], bits::<8>(6), signed::<ANGLE_W>(ATAN_TABLE[6]));
    next[7] = rotation_step(q.pipe[6], bits::<8>(7), signed::<ANGLE_W>(ATAN_TABLE[7]));
    next[8] = rotation_step(q.pipe[7], bits::<8>(8), signed::<ANGLE_W>(ATAN_TABLE[8]));
    next[9] = rotation_step(q.pipe[8], bits::<8>(9), signed::<ANGLE_W>(ATAN_TABLE[9]));
    next[10] = rotation_step(q.pipe[9], bits::<8>(10), signed::<ANGLE_W>(ATAN_TABLE[10]));
    next[11] = rotation_step(q.pipe[10], bits::<8>(11), signed::<ANGLE_W>(ATAN_TABLE[11]));
    next[12] = rotation_step(q.pipe[11], bits::<8>(12), signed::<ANGLE_W>(ATAN_TABLE[12]));
    next[13] = rotation_step(q.pipe[12], bits::<8>(13), signed::<ANGLE_W>(ATAN_TABLE[13]));
    next[14] = rotation_step(q.pipe[13], bits::<8>(14), signed::<ANGLE_W>(ATAN_TABLE[14]));
    next[15] = rotation_step(q.pipe[14], bits::<8>(15), signed::<ANGLE_W>(ATAN_TABLE[15]));
    d.pipe = next;

    let last = q.pipe[ITERATIONS - 1];
    let mut re = last.x;
    let mut im = last.y;
    if last.negate {
        // See the note in `vectoring`: subtraction from zero rather
        // than unary negation.
        let zero = signed::<INT_W>(0);
        re = zero - re;
        im = zero - im;
    }

    let o = Out {
        sample: Iq::<INT_W> { re, im },
        valid: last.valid,
    };

    if cr.reset.any() {
        d.pipe = [Stage {
            x: signed::<INT_W>(0),
            y: signed::<INT_W>(0),
            z: signed::<ANGLE_W>(0),
            negate: false,
            valid: false,
        }; ITERATIONS];
    }
    (o, d)
}

/// One CORDIC rotation iteration.
#[kernel]
#[doc(hidden)]
pub fn rotation_step(s: Stage, k: Bits<8>, angle: SignedBits<ANGLE_W>) -> Stage {
    // Direction from the sign of the remaining angle: drive z to zero.
    let z_neg = (s.z.as_unsigned() & bits::<ANGLE_W>(1 << (ANGLE_W - 1))) != bits::<ANGLE_W>(0);

    let xs = s.x >> k;
    let ys = s.y >> k;

    let mut out = s;
    if z_neg {
        out.x = s.x + ys;
        out.y = s.y - xs;
        out.z = s.z + angle;
    } else {
        out.x = s.x - ys;
        out.y = s.y + xs;
        out.z = s.z - angle;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::f64::consts::TAU;

    type Uut = CordicRotation;

    fn polar(mag: i128, turns: f64) -> In {
        let phase = (turns * (1i128 << ANGLE_W) as f64).round() as i128;
        // Wrap into the signed range: a full turn is 2^ANGLE_W.
        let full = 1i128 << ANGLE_W;
        let mut p = phase % full;
        if p >= full / 2 {
            p -= full;
        }
        if p < -full / 2 {
            p += full;
        }
        In {
            magnitude: Some(signed::<INT_W>(mag)),
            phase: signed::<ANGLE_W>(p),
        }
    }

    fn convert(points: &[(i128, f64)]) -> Vec<(f64, f64)> {
        let uut = Uut::default();
        let mut seq: Vec<In> = points.iter().map(|(m, t)| polar(*m, *t)).collect();
        seq.extend(std::iter::repeat_n(
            In {
                magnitude: None,
                phase: signed::<ANGLE_W>(0),
            },
            ITERATIONS + 2,
        ));
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter(|s| s.output.valid)
            .map(|s| {
                (
                    s.output.sample.re.raw() as f64,
                    s.output.sample.im.raw() as f64,
                )
            })
            .collect()
    }

    #[test]
    fn default_construction() {
        let _uut = Uut::default();
    }

    /// **Accuracy over the whole circle**, with the gain compensated.
    ///
    /// The gain correction is the part most easily got wrong: an
    /// uncompensated CORDIC is 64.7% too large, which is obvious, but a
    /// *mis*-compensated one is subtly wrong in a way that looks like
    /// quantisation.
    #[test]
    fn accurate_over_the_whole_circle() {
        const R: i128 = 100_000;
        const N: usize = 64;
        let points: Vec<(i128, f64)> = (0..N).map(|k| (R, k as f64 / N as f64)).collect();
        let got = convert(&points);
        assert_eq!(got.len(), N, "the pipeline dropped samples");

        let mut worst = 0.0f64;
        for (k, (re, im)) in got.iter().enumerate() {
            let t = TAU * k as f64 / N as f64;
            let (wr, wi) = (R as f64 * t.cos(), R as f64 * t.sin());
            worst = worst.max((re - wr).abs()).max((im - wi).abs());
        }
        assert!(
            worst < 300.0,
            "worst component error {worst:.1} of {R}; an uncompensated gain \
             would be about 64700 out"
        );
    }

    /// The axes, where the quadrant fold is most likely to be wrong.
    #[test]
    fn the_axes_are_correct() {
        let r = 80_000i128;
        let got = convert(&[(r, 0.0), (r, 0.25), (r, 0.5), (r, 0.75)]);
        let want = [
            (r as f64, 0.0),
            (0.0, r as f64),
            (-(r as f64), 0.0),
            (0.0, -(r as f64)),
        ];
        for (k, (re, im)) in got.iter().enumerate() {
            assert!(
                (re - want[k].0).abs() < 300.0 && (im - want[k].1).abs() < 300.0,
                "axis {k}: got ({re:.0}, {im:.0}), want ({:.0}, {:.0})",
                want[k].0,
                want[k].1
            );
        }
    }

    /// **Vectoring then rotation is the identity.**
    ///
    /// The strongest statement about the pair: whatever each does
    /// internally, together they return the original vector. A gain
    /// error, a quadrant error, or a table error in *either* breaks
    /// this, and a test of one direction alone would not catch a
    /// consistent mistake made in both.
    #[test]
    fn vectoring_then_rotation_is_the_identity() {
        use crate::dsp::cordic::vectoring::{CordicVectoring, In as VecIn};
        use crate::dsp::iq::Iq;

        const W: usize = 18;
        const R: f64 = 90_000.0;
        const N: usize = 32;

        let originals: Vec<(i128, i128)> = (0..N)
            .map(|k| {
                let t = TAU * k as f64 / N as f64;
                ((R * t.cos()) as i128, (R * t.sin()) as i128)
            })
            .collect();

        // Forward: rectangular -> polar.
        let vec_uut = CordicVectoring::<W>::default();
        let mut vseq: Vec<VecIn<W>> = originals
            .iter()
            .map(|(re, im)| VecIn::<W> {
                sample: Some(Iq::<W> {
                    re: signed::<W>(*re),
                    im: signed::<W>(*im),
                }),
            })
            .collect();
        vseq.extend(std::iter::repeat_n(
            VecIn::<W> { sample: None },
            ITERATIONS + 2,
        ));

        let polar_pairs: Vec<(i128, i128)> = vec_uut
            .run(vseq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter(|s| s.output.valid)
            .map(|s| (s.output.magnitude.raw(), s.output.phase.raw()))
            .collect();
        assert_eq!(polar_pairs.len(), N, "the forward pass dropped samples");

        // Reverse: polar -> rectangular.  The magnitude from `vectoring`
        // is already gain-corrected, and `rotation` compensates its own
        // gain internally, so the value passes straight through.
        //
        // This composition is why the correction belongs in the widgets
        // rather than in the caller: with the gain left on the output,
        // this test failed by 58212 of 90000 -- which is exactly
        // 90000*(K-1), the gain applied twice.
        let rot_uut = Uut::default();
        let mut rseq: Vec<In> = polar_pairs
            .iter()
            .map(|(m, p)| In {
                magnitude: Some(signed::<INT_W>(*m)),
                phase: signed::<ANGLE_W>(*p),
            })
            .collect();
        rseq.extend(std::iter::repeat_n(
            In {
                magnitude: None,
                phase: signed::<ANGLE_W>(0),
            },
            ITERATIONS + 2,
        ));

        let back: Vec<(i128, i128)> = rot_uut
            .run(rseq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter(|s| s.output.valid)
            .map(|s| (s.output.sample.re.raw(), s.output.sample.im.raw()))
            .collect();
        assert_eq!(back.len(), N, "the reverse pass dropped samples");

        let mut worst = 0i128;
        for (k, (re, im)) in back.iter().enumerate() {
            worst = worst.max((re - originals[k].0).abs());
            worst = worst.max((im - originals[k].1).abs());
        }
        // Two 16-iteration passes plus two gain quantisations.
        assert!(
            worst < 1200,
            "round trip worst component error {worst} of {R}; the two \
             directions do not invert each other"
        );
    }

    /// Tier 3 — HDL emission shape.
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
    fn test_cordic_rotation_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let seq: Vec<In> = (0..24i128)
            .map(|k| polar(70_000, k as f64 / 24.0))
            .collect();
        let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("cordic_rotation");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["1914f560697d499e6d1e89160cdc85f8428e547b21d210b7ab690719ecdce117"];
        let digest = vcd.dump_to_file(root.join("cordic_rotation.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    /// Tier 4 — emitted Verilog agrees with the Rust simulation.
    #[test]
    fn test_cordic_rotation_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let seq: Vec<In> = (0..24i128)
            .map(|k| polar(70_000, k as f64 / 24.0))
            .collect();
        let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }
}
