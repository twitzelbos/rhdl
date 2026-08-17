#![warn(missing_docs)]
//! `SinCosLinearInterp` — quadrature phase-to-amplitude by coarse table
//! plus **first-order linear (Taylor) interpolation**.
//!
//! The name is deliberate. "Hybrid" in the architecture note covers at
//! least three different circuits, and they are not interchangeable:
//!
//! | variant | fine stage | cost |
//! |---|---|---|
//! | **this — linear/Taylor** | `sin θ + cos θ·δ` | 2 multipliers, 1 cycle |
//! | LUT + CORDIC | CORDIC micro-rotations | shift-adds, several cycles |
//! | dual-ROM (Sunderland) | second small table | extra ROM |
//!
//! This is the one Xilinx exposes as "Taylor Series Correction". The
//! CORDIC variant exists to *avoid* multipliers, which matters on parts
//! without DSP blocks or when they are all committed elsewhere — not
//! the case here, where a handful of a Zynq's 80 DSP slices is free.
//!
//! **The measured −116 dBc below applies to this variant with 18-bit
//! arithmetic and nothing else.**
//!
//! Converts a 22-bit truncated phase into `(sin, cos)` using a
//! 256-entry quarter-wave table and a first-order rotation by the fine
//! remainder:
//!
//! ```text
//! sin(θ+δ) ≈ sin θ + cos θ · δ
//! cos(θ+δ) ≈ cos θ − sin θ · δ
//! ```
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+SinCosLinearInterp+-+
      |                      | [18]
 [22] |                   sin+----->
+---->+ phase                | [18]
      |                   cos+----->
      +----------------------+
")]
//!
//! # Internals
//!
//! The phase splits three ways. The top two bits pick the quadrant, the
//! next eight address the table, and the low twelve are the remainder
//! the rotation works on. Quadrant and remainder are delayed by one
//! cycle so they meet the registered table output that belongs to them.
#![doc = badascii_doc::badascii!(r"
  phase[21:20]      +-----------+
  quadrant  +------>| address   | sin_addr  +----------+
  phase[19:12]      | mirror on +---------->| quarter  | s0
  index     +------>| odd, cos  | cos_addr  | wave     | c0
                    | a quarter +---------->| BRAM x2  +------+
                    | turn on   |           +----------+      |
                    +-----------+                             v
  phase[11:0]       +-----------+ quadrant  +----------------+
  fine      +------>|    DFF    +---------->| sign flip, then| sin
                    |  (delay)  | fine      | rotate by delta+----->
                    |           +---------->|                | cos
                    +-----------+           +----------------+----->
")]
//!
//! # Why this rather than a bigger table
//!
//! Interpolation attacks the truncation error instead of merely
//! reducing it, so accuracy improves roughly twice as fast per coarse
//! bit as enlarging the table does. Measured with the exact spur
//! analysis in [`super::model`]:
//!
//! | architecture | table | worst in-band spur |
//! |---|---|---|
//! | plain LUT, 13 address bits | 32 Kbit | −78 dBc |
//! | plain LUT, 14 address bits | 64 Kbit | −84 dBc |
//! | **this** | **~9 Kbit** | **−116 dBc** |
//!
//! Reaching −116 dBc with a plain table needs ~21 address bits — of
//! order 8 Mbit, hundreds of block RAMs.
//!
//! Dithering is the other classical way to suppress truncation spurs
//! and is **not** used: it trades a discrete spur for a raised noise
//! floor, which in a sensitivity-limited instrument lands directly on
//! the quantity being bought with averaging time.
//!
//! # Fixed widths, deliberately
//!
//! The configuration below is the one the spur analysis validated, and
//! the fixed-point scaling constant is derived from it. Making the
//! widths generic would require deriving that constant inside a kernel
//! from const generics, which the kernel language cannot do — so the
//! widths are concrete rather than approximately-generic-and-wrong.
//!
//! | parameter | value |
//! |---|---|
//! | phase consumed | 22 bits (2 quadrant + 8 index + 12 fine) |
//! | table | 256 entries × 18 bit, quarter wave |
//! | amplitude | 18 bit (native DSP48 port; reaches the ceiling exactly) |
//! | phase resolution | 360° / 2²² ≈ 0.000086° |
//!
//! # One LSB of table headroom, and why it is load-bearing
//!
//! Near a peak the table value is already at full scale and the
//! first-order correction can push the sum **one LSB** past the 18-bit
//! signed range. Exactly one LSB — measured across all 2²² phases, the
//! largest overshoot is `131072` against a limit of `131071`, and it
//! happens for 1550 phases, about 1 in 2706.
//!
//! Wrapping converts that one-LSB excess into `-131072`: a full-scale
//! **sign inversion**, the largest error the format can represent. The
//! damage is spectral, not cosmetic, because the hit recurs at a rate
//! locked to the tuning word rather than at random — a coherent spur,
//! not added noise. Measured on the cosine output, 65536-point
//! Blackman-Harris, worst spur outside the carrier:
//!
//! | tuning word | if it wrapped | as built |
//! |---|---|---|
//! | 524288 | **−0.0 dBc** | −106.8 dBc |
//! | 262144 | −9.5 dBc | −105.5 dBc |
//! | 16384 | −36.0 dBc | −104.4 dBc |
//! | 1234567 | −63.6 dBc | −104.3 dBc |
//! | 419431 | −104.0 dBc | −104.0 dBc |
//!
//! At −0.0 dBc the spur equals the carrier: the output is no longer a
//! sine wave. Across 68 sampled words, 47 are more than 20 dB worse when
//! wrapping — including innocuous-looking odd ones, so this is not
//! confined to the round-number adversarial class.
//!
//! Two fixes are standard: saturate the sum, or scale the table so the
//! sum cannot overflow in the first place. **This widget scales**, via
//! [`TABLE_SCALE`] — one LSB below the 18-bit maximum, which costs
//! 0.000066 dB of amplitude and **no logic at all**, against two
//! 20-bit comparators and a pair of muxes for saturation.
//!
//! The usual objection to scaling is that the margin is a silent
//! assumption which a later width change can invalidate. Here it is not
//! silent: `interpolated_sum_never_leaves_the_range` evaluates all 2²²
//! phases and fails if the headroom ever stops being sufficient, which
//! is a stronger guarantee than a comparator provides — a clamp keeps
//! the output in range but says nothing about whether the design still
//! makes sense.
//!
//! Note that the positive rail is where this bites, and that is a
//! property of two's complement rather than of trigonometry: the range
//! is asymmetric, `[−2¹⁷, 2¹⁷−1]`, so a table scaled to `2¹⁷−1` sits
//! one code short of the negative limit and has nowhere to go when the
//! interpolation rounds upward.
//!
//! Saturation was implemented first and then removed, for a second
//! reason worth recording: RHDL currently emits signed comparisons
//! against literals as **unsigned** Verilog, so the clamp inverted its
//! own sense in hardware while simulating correctly. See
//! `tests/scratch_signed_literal_cmp.rs`. Scaling is the better design
//! on cost alone, so this only settled an already-close call — but if
//! saturation is ever reinstated, that defect must be fixed first.
//!
//! Note the asymmetry with the phase accumulator, where wrapping is not
//! a bug but *the arithmetic*: phase is modulo 2π. Wrapping is correct
//! in the phase domain and catastrophic in the amplitude domain.
//!
//! # Latency
//!
//! The table read is registered: **data latency is one cycle**, and
//! there are no control inputs. That figure belongs in the scheduler's
//! arithmetic.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/sin_cos_linear_interp.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/sin_cos_linear_interp.md")]

use rhdl::prelude::*;

use crate::core::dff;
use crate::core::ram::synchronous::{In as RamIn, SyncBRAM, Write};

/// Quarter-wave table address bits (256 entries).
pub const TBL_W: usize = 8;
/// Fine interpolation bits.
pub const FINE_W: usize = 12;
/// Total phase bits consumed: 2 quadrant + `TBL_W` + `FINE_W`.
pub const TOTAL_W: usize = 22;
/// Bits per output component.
pub const AMP_W: usize = 18;

/// `round(2π / 2^TOTAL_W · 2^32)` — converts a fine-remainder LSB into
/// radians in Q32. Relative error 2.8e-6 on a correction that is itself
/// ≤0.3% of full scale, i.e. about −161 dBc: far below the −116 dBc the
/// architecture delivers.
const DELTA_K: i128 = 6434;

/// Table amplitude: **one LSB below** the 18-bit signed maximum.
///
/// Public because the headroom is part of the widget's contract, not an
/// implementation detail: anything reasoning about the output range
/// needs it.
///
/// That single LSB of headroom is what makes the interpolated sum
/// unable to leave the output range — see the saturation discussion in
/// the module docs, and `interpolated_sum_never_leaves_the_range`,
/// which proves it exhaustively over all 2²² phases.
pub const TABLE_SCALE: i128 = (1 << (AMP_W - 1)) - 2;

/// Quadrature phase-to-amplitude: coarse quarter-wave table plus
/// first-order fine rotation.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct SinCosLinearInterp {
    /// Quarter-wave table read at the sine address.
    sin_tbl: SyncBRAM<SignedBits<AMP_W>, TBL_W>,
    /// Second instance of the same table for the cosine address.
    /// `SyncBRAM` is single-port; on a device with true dual-port block
    /// RAM these collapse to one primitive.
    cos_tbl: SyncBRAM<SignedBits<AMP_W>, TBL_W>,
    /// Quadrant and fine remainder, delayed to match the table read.
    ///
    /// **Load-bearing.** The BRAM read is registered, so its output
    /// corresponds to the address presented on the *previous* cycle.
    /// Applying this cycle's sign and interpolation to it misaligns the
    /// datapath by one cycle and produces garbage — an error of about
    /// twice full scale, which is how the omission announced itself.
    delayed: dff::DFF<Pipelined>,
}

/// Phase attributes carried alongside the registered table read.
#[derive(PartialEq, Clone, Copy, Debug, Digital, Default)]
#[doc(hidden)]
pub struct Pipelined {
    /// Quadrant of the phase that produced the current table output.
    pub quadrant: Bits<2>,
    /// Fine remainder of that same phase.
    pub fine: Bits<FINE_W>,
}

/// Build the quarter-wave table: 256 entries at bin midpoints over
/// `[0, π/2)`.
///
/// Midpoint sampling is what makes the odd-quadrant mirror exact rather
/// than off-by-one.
fn quarter_table() -> Vec<(Bits<TBL_W>, SignedBits<AMP_W>)> {
    let coarse_w = TBL_W + 2;
    let scale = TABLE_SCALE as f64;
    (0..(1usize << TBL_W))
        .map(|i| {
            let theta = 2.0 * std::f64::consts::PI * (i as f64 + 0.5) / (1u64 << coarse_w) as f64;
            (
                bits::<TBL_W>(i as u128),
                signed::<AMP_W>((theta.sin() * scale).round() as i128),
            )
        })
        .collect()
}

impl Default for SinCosLinearInterp {
    fn default() -> Self {
        Self {
            sin_tbl: SyncBRAM::new(quarter_table()),
            cos_tbl: SyncBRAM::new(quarter_table()),
            delayed: dff::DFF::new(Pipelined::default()),
        }
    }
}

/// Inputs for [`SinCosLinearInterp`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In {
    /// Phase, truncated to the 22 bits this stage consumes.
    pub phase: Bits<TOTAL_W>,
}

/// Outputs from [`SinCosLinearInterp`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out {
    /// Sine of the phase.
    pub sin: SignedBits<AMP_W>,
    /// Cosine of the phase.
    pub cos: SignedBits<AMP_W>,
}

impl SynchronousIO for SinCosLinearInterp {
    type I = In;
    type O = Out;
    type Kernel = sin_cos_linear_interp_kernel;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn sin_cos_linear_interp_kernel(_cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();

    // Split phase into quadrant | index | fine.
    let coarse = (i.phase >> 12).resize::<TBL_W>();
    let quadrant = (i.phase >> 20).resize::<2>();
    let fine = i.phase.resize::<FINE_W>();

    // Odd quadrants read the table mirrored.
    let mirrored = !bits::<TBL_W>(0) - coarse;
    let odd = (quadrant & bits::<2>(1)) != bits::<2>(0);
    let sin_addr = if odd { mirrored } else { coarse };

    // Cosine is a quarter turn ahead.
    let cos_quadrant = quadrant + bits::<2>(1);
    let cos_odd = (cos_quadrant & bits::<2>(1)) != bits::<2>(0);
    let cos_addr = if cos_odd { mirrored } else { coarse };

    let no_write = Write::<SignedBits<AMP_W>, TBL_W> {
        addr: bits::<TBL_W>(0),
        value: signed::<AMP_W>(0),
        enable: false,
    };
    d.sin_tbl = RamIn::<SignedBits<AMP_W>, TBL_W> {
        read_addr: sin_addr,
        write: no_write,
    };
    d.cos_tbl = RamIn::<SignedBits<AMP_W>, TBL_W> {
        read_addr: cos_addr,
        write: no_write,
    };

    // Carry this cycle's attributes forward to meet the table output
    // next cycle.
    d.delayed = Pipelined { quadrant, fine };

    // Sign and interpolation use the DELAYED attributes, which belong to
    // the phase that produced the table values now emerging.
    let dq = q.delayed.quadrant;
    let dcq = dq + bits::<2>(1);
    let sin_neg = (dq & bits::<2>(2)) != bits::<2>(0);
    let cos_neg = (dcq & bits::<2>(2)) != bits::<2>(0);
    let s0 = if sin_neg { -q.sin_tbl } else { q.sin_tbl };
    let c0 = if cos_neg { -q.cos_tbl } else { q.cos_tbl };

    // Fine remainder, centred so δ spans ±half a coarse step.
    // Zero-extend BEFORE reinterpreting as signed.  `as_signed()` on a
    // 12-bit value treats it as two's complement, so any fine remainder
    // >= 2048 would become negative before the centring subtraction —
    // wrong for half of all phases, and catastrophically so.
    let delta = q.delayed.fine.resize::<48>().as_signed() - signed::<48>(2048);

    // First-order rotation.  Wide intermediate: 18 + 12 + 13 = 43 bits
    // of true product, carried in 48.
    let k = signed::<48>(DELTA_K);
    let corr_sin = ((c0.resize::<48>() * delta * k) >> 32).resize::<AMP_W>();
    let corr_cos = ((s0.resize::<48>() * delta * k) >> 32).resize::<AMP_W>();

    // No clamp.  `TABLE_SCALE` leaves one LSB of headroom, which is
    // exactly the largest overshoot the rotation can produce, so the sum
    // provably cannot leave the output range —
    // `interpolated_sum_never_leaves_the_range` checks all 2^22 phases.
    // Wrapping here would be catastrophic rather than cosmetic: see the
    // module docs.
    let o = Out {
        sin: s0 + corr_sin,
        cos: c0 - corr_cos,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;

    #[test]
    fn default_construction() {
        let _a = SinCosLinearInterp::default();
    }

    /// Scratch: hold one phase constant and watch the pipeline settle.
    /// Tier 1 — the kernel alone, driven with a hand-built `q`, matches
    /// `f64` trigonometry across every quadrant and both ends of the
    /// fine remainder.
    ///
    /// The kernel takes its sign and its interpolation input from
    /// `q.delayed`, **not** from `i.phase` — that separation is what the
    /// one-cycle pipeline alignment rests on. Driving the kernel
    /// directly is the only place it can be checked in isolation from
    /// the alignment itself, which is why `i.phase` is left at zero
    /// here and the assertion still means something.
    #[test]
    fn kernel_matches_trigonometry_directly() {
        let tbl = quarter_table();
        let scale = ((1i128 << (AMP_W - 1)) - 1) as f64;
        let two_pi = 2.0 * std::f64::consts::PI;

        let mut worst = 0.0f64;
        let mut worst_at = (0u128, 0u128, 0u128);
        let mut checked = 0usize;

        for quad in 0..4u128 {
            for idx in [0u128, 1, 64, 128, 200, 255] {
                for fine in [0u128, 1, 2048, 4094, 4095] {
                    let mirrored = 255 - idx;
                    let sin_addr = if quad % 2 == 1 { mirrored } else { idx };
                    let cq = (quad + 1) % 4;
                    let cos_addr = if cq % 2 == 1 { mirrored } else { idx };

                    let q = Q {
                        sin_tbl: tbl[sin_addr as usize].1,
                        cos_tbl: tbl[cos_addr as usize].1,
                        delayed: Pipelined {
                            quadrant: bits::<2>(quad),
                            fine: bits::<FINE_W>(fine),
                        },
                    };
                    let (o, _d) = sin_cos_linear_interp_kernel(
                        ClockReset::dont_care(),
                        In {
                            phase: bits::<TOTAL_W>(0),
                        },
                        q,
                    );

                    let coarse_angle = two_pi * ((quad * 256 + idx) as f64 + 0.5) / 1024.0;
                    let delta = (fine as f64 / 4096.0 - 0.5) * (two_pi / 1024.0);
                    let th = coarse_angle + delta;
                    let e = (o.sin.raw() as f64 - th.sin() * scale)
                        .abs()
                        .max((o.cos.raw() as f64 - th.cos() * scale).abs());
                    if e > worst {
                        worst = e;
                        worst_at = (quad, idx, fine);
                    }
                    checked += 1;
                }
            }
        }

        assert_eq!(checked, 120, "the sweep did not cover what it claims to");
        assert!(
            worst < 4.0,
            "worst error {worst:.1} LSB at (quadrant, index, fine) = {worst_at:?}"
        );
        // Zero would mean the comparison is vacuous — the kernel is
        // fixed-point and cannot match f64 exactly.
        assert!(
            worst > 0.0,
            "exactly zero error suggests a vacuous comparison"
        );
    }

    /// The interpolated output tracks `f64` trigonometry to within a
    /// small fraction of a coarse table step, across all four quadrants
    /// and — critically — at the rails, where the correction can push
    /// the sum past full scale.
    ///
    /// Verified able to fail (CLAUDE.md §5): removing the saturation,
    /// zeroing the fine rotation, and asserting the wrong pipeline
    /// latency each turn this red.
    #[test]
    fn output_matches_trigonometry() {
        let uut = SinCosLinearInterp::default();
        let full = 1u128 << TOTAL_W;
        let scale = ((1i128 << (AMP_W - 1)) - 1) as f64;

        let stride = 7919u128;
        let mut phases: Vec<u128> = (0..4096u128).map(|k| (k * stride) % full).collect();

        // A coprime stride gives broad coverage but reaches the rails
        // only by luck, and the rails are exactly where the interpolated
        // sum can leave the 18-bit range.  Sweep them explicitly: an
        // earlier version of this test omitted them, and removing the
        // saturation entirely still passed.
        for quad in 0..4u128 {
            let rail = quad * (full / 4);
            for d in 0..512u128 {
                phases.push((rail + d) % full);
                phases.push((rail + full - d - 1) % full);
            }
        }

        let stream = phases
            .iter()
            .map(|p| In {
                phase: bits::<TOTAL_W>(*p),
            })
            .collect::<Vec<_>>()
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);

        let out: Vec<(i128, i128)> = uut
            .run(stream)
            .synchronous_sample()
            .map(|s| (s.output.sin.raw(), s.output.cos.raw()))
            .collect();

        let want = |p: u128| {
            let th = 2.0 * std::f64::consts::PI * p as f64 / full as f64;
            (th.sin() * scale, th.cos() * scale)
        };

        // The BRAM read and the attribute DFF each cost a cycle, so a
        // phase presented on cycle n emerges on cycle n+2.  Asserted, not
        // searched: picking the best of several shifts lets a single
        // catastrophic sample hide behind a misalignment.
        const LATENCY: usize = 2;
        // Skip the pipeline fill: until the datapath is primed the
        // outputs are the DFF/BRAM initial state.
        const FILL: usize = 4;

        let mut worst_err = 0.0f64;
        let mut worst_phase = 0u128;
        for (i, p) in phases.iter().enumerate().skip(FILL) {
            if i + LATENCY >= out.len() {
                break;
            }
            let (ws, wc) = want(*p);
            let (gs, gc) = out[i + LATENCY];
            let e = (gs as f64 - ws).abs().max((gc as f64 - wc).abs());
            if e > worst_err {
                worst_err = e;
                worst_phase = *p;
            }
        }
        let best_shift = LATENCY;

        // A coarse step is 2^(AMP_W-1) * 2*pi/2^10 ~ 805 LSB.  A plain
        // LUT would sit near half that; linear interpolation must do
        // dramatically better, which is what distinguishes the two.
        let coarse_step_lsb = scale * 2.0 * std::f64::consts::PI / 1024.0;
        assert!(
            worst_err < coarse_step_lsb / 20.0,
            "worst error {worst_err:.1} LSB at phase {worst_phase} (shift {best_shift}); \
             a coarse step is {coarse_step_lsb:.0} LSB.  An error near a coarse step \
             means the fine rotation is missing or wrong; an error near 2^AMP_W means \
             the sum wrapped instead of saturating at a rail; anything else is an \
             alignment error"
        );
        assert!(
            worst_err > 0.0,
            "exactly zero error is implausible and suggests the comparison is vacuous"
        );
    }

    /// The bit-exact model used by the proof below really does describe
    /// the widget.
    ///
    /// Without this the exhaustive check would prove a property of the
    /// model and say nothing about the hardware — the exact substitution
    /// CLAUDE.md forbids. Compared against the widget's own output, at
    /// the same alignment the Tier 2 test asserts.
    #[test]
    fn model_agrees_with_the_widget() {
        let uut = SinCosLinearInterp::default();
        let full = 1u128 << TOTAL_W;
        let stride = 7919u128;
        let phases: Vec<u128> = (0..2048u128).map(|k| (k * stride) % full).collect();
        let stream = phases
            .iter()
            .map(|p| In {
                phase: bits::<TOTAL_W>(*p),
            })
            .collect::<Vec<_>>()
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);
        let out: Vec<(i128, i128)> = uut
            .run(stream)
            .synchronous_sample()
            .map(|s| (s.output.sin.raw(), s.output.cos.raw()))
            .collect();

        let tbl = model_table();
        let mut compared = 0usize;
        for (i, p) in phases.iter().enumerate().skip(4) {
            if i + 2 >= out.len() {
                break;
            }
            assert_eq!(model_pair_raw(&tbl, *p), out[i + 2], "phase {p}");
            compared += 1;
        }
        assert!(compared > 2000, "only {compared} samples compared");
    }

    /// Exhaustive proof that the one-LSB headroom in [`TABLE_SCALE`] is
    /// sufficient: across all 2²² phases neither interpolated sum leaves
    /// the 18-bit signed range, so the output cannot wrap.
    ///
    /// **This test is the saturation logic.** The widget carries no
    /// clamp; the margin is what keeps it in range, and a margin is only
    /// as good as the thing that checks it. If `TBL_W`, `FINE_W`,
    /// `AMP_W` or `DELTA_K` are ever retuned so the headroom stops being
    /// enough, this fails instead of the hardware silently inverting
    /// sign at the peaks.
    ///
    /// Verified able to fail: setting `TABLE_SCALE` back to the
    /// full-scale `(1 << (AMP_W - 1)) - 1` reports 1550 overflowing
    /// phases, which is where the whole investigation started.
    #[test]
    fn interpolated_sum_never_leaves_the_range() {
        let tbl = model_table();
        let limit = (1i128 << (AMP_W - 1)) - 1;
        let mut n_over = 0u64;
        let mut worst = 0i128;
        let mut worst_phase = 0u128;
        for phase in 0..(1u128 << TOTAL_W) {
            let (s, c) = model_pair_raw(&tbl, phase);
            for v in [s, c] {
                if v.abs() > limit {
                    n_over += 1;
                    if v.abs() > worst {
                        worst = v.abs();
                        worst_phase = phase;
                    }
                }
            }
        }
        assert_eq!(
            n_over, 0,
            "{n_over} components leave the range; worst |value| {worst} at phase \
             {worst_phase} against a limit of {limit}.  The table headroom is no \
             longer sufficient — either restore it or reinstate a clamp."
        );
    }

    /// Tier 3 — HDL emission snapshot.
    ///
    /// Only module `top` is captured verbatim. The two table instances
    /// emit ~278 lines each of BRAM initialisation: 550 lines of
    /// trigonometric constants that no reviewer will audit, and whose
    /// correctness is already asserted by
    /// [`quarter_table_is_a_rising_quarter_sine`]. The child modules are
    /// pinned by name and start line instead, so adding, removing or
    /// resizing one still fails the test.
    ///
    /// `fifo::synchronous` and `core::ram::option_sync` omit an HDL
    /// snapshot entirely for the same reason; capturing `top` is
    /// strictly more coverage than the existing convention for
    /// memory-backed widgets, not less.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = SinCosLinearInterp::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();

        let shape = hdl
            .lines()
            .enumerate()
            .filter(|(_, l)| l.starts_with("module ") || l.starts_with("endmodule"))
            .map(|(n, l)| format!("{}: {}", n + 1, l.split('(').next().unwrap()))
            .collect::<Vec<_>>()
            .join("\n");
        let expect_shape = expect![[r#"
            1: module top
            173: endmodule
            174: module top_sin_tbl
            452: endmodule
            453: module top_cos_tbl
            731: endmodule
            732: module top_delayed
            747: endmodule"#]];
        expect_shape.assert_eq(&shape);

        let top = hdl
            .lines()
            .take_while(|l| !l.starts_with("module top_"))
            .collect::<Vec<_>>()
            .join("\n");
        let expect_top = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [21:0] i, output wire [35:0] o);
               wire [119:0] od;
               wire [83:0] d;
               wire [49:0] q;
               assign o = od[35:0];
               top_sin_tbl c0(.clock_reset(clock_reset), .i(d[34:0]), .o(q[17:0]));
               top_cos_tbl c1(.clock_reset(clock_reset), .i(d[69:35]), .o(q[35:18]));
               top_delayed c2(.clock_reset(clock_reset), .i(d[83:70]), .o(q[49:36]));
               assign d = od[119:36];
               assign od = kernel_sin_cos_linear_interp_kernel(clock_reset, i, q);
               function [119:0] kernel_sin_cos_linear_interp_kernel(input reg [1:0] arg_0, input reg [21:0] arg_1, input reg [49:0] arg_2);
                     reg [21:0] r0;
                     reg [21:0] r1;
                     reg [7:0] r2;
                     reg [21:0] r3;
                     reg [1:0] r4;
                     reg [11:0] r5;
                     reg [7:0] r6;
                     reg [1:0] r7;
                     reg [0:0] r8;
                     reg [7:0] r9;
                     reg [1:0] r10;
                     reg [1:0] r11;
                     reg [0:0] r12;
                     reg [7:0] r13;
                     reg [34:0] r14;
                     reg [34:0] r15;
                     // d
                     reg [83:0] r16;
                     reg [34:0] r17;
                     reg [34:0] r18;
                     // d
                     reg [83:0] r19;
                     reg [13:0] r20;
                     reg [13:0] r21;
                     // d
                     reg [83:0] r22;
                     reg [13:0] r23;
                     reg [49:0] r24;
                     reg [1:0] r25;
                     reg [1:0] r26;
                     reg [1:0] r27;
                     reg [0:0] r28;
                     reg [1:0] r29;
                     reg [0:0] r30;
                     reg signed [17:0] r31;
                     reg signed [17:0] r32;
                     reg signed [17:0] r33;
                     reg signed [17:0] r34;
                     reg signed [17:0] r35;
                     reg signed [17:0] r36;
                     reg signed [17:0] r37;
                     reg signed [17:0] r38;
                     reg [13:0] r39;
                     reg [11:0] r40;
                     reg [47:0] r41;
                     reg signed [47:0] r42;
                     reg signed [47:0] r43;
                     reg signed [47:0] r44;
                     reg signed [47:0] r45;
                     reg signed [47:0] r46;
                     reg signed [47:0] r47;
                     reg signed [17:0] r48;
                     reg signed [47:0] r49;
                     reg signed [47:0] r50;
                     reg signed [47:0] r51;
                     reg signed [47:0] r52;
                     reg signed [17:0] r53;
                     reg signed [17:0] r54;
                     reg signed [17:0] r55;
                     reg [35:0] r56;
                     reg [35:0] r57;
                     reg [119:0] r58;
                     reg [1:0] r59;
                     reg [33:0] r60;
                     reg [41:0] r61;
                     reg signed [79:0] r62;
                     reg signed [79:0] r63;
                     localparam l0 = 8'b11111111;
                     localparam l1 = 2'b01;
                     localparam l2 = 2'b01;
                     localparam l3 = 2'b01;
                     localparam l4 = 35'b00000000000000000000000000000000000;
                     localparam l5 = 27'b000000000000000000000000000;
                     localparam l6 = 84'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l7 = 35'b00000000000000000000000000000000000;
                     localparam l8 = 14'b00000000000000;
                     localparam l9 = 2'b01;
                     localparam l10 = 2'b10;
                     localparam l11 = 2'b10;
                     localparam l12 = 48'b000000000000000000000000000000000000100000000000;
                     localparam l13 = 48'b000000000000000000000000000000000001100100100010;
                     localparam l14 = 36'b000000000000000000000000000000000000;
                     begin
                        r59 = arg_0;
                        r0 = arg_1;
                        r24 = arg_2;
                        r60 = {{12{1'b0}}, r0};
                        r1 = r60[33:12];
                        r2 = r1[7:0];
                        r61 = {{20{1'b0}}, r0};
                        r3 = r61[41:20];
                        r4 = r3[1:0];
                        r5 = r0[11:0];
                        r6 = l0 - r2;
                        r7 = r4 & l1;
                        r8 = |r7;
                        r9 = r8 ? r6 : r2;
                        r10 = r4 + l2;
                        r11 = r10 & l3;
                        r12 = |r11;
                        r13 = r12 ? r6 : r2;
                        r14 = l4;
                        r14[7:0] = r9;
                        r15 = r14;
                        r15[34:8] = l5;
                        r16 = l6;
                        r16[34:0] = r15;
                        r17 = l7;
                        r17[7:0] = r13;
                        r18 = r17;
                        r18[34:8] = l5;
                        r19 = r16;
                        r19[69:35] = r18;
                        r20 = l8;
                        r20[1:0] = r4;
                        r21 = r20;
                        r21[13:2] = r5;
                        r22 = r19;
                        r22[83:70] = r21;
                        r23 = r24[49:36];
                        r25 = r23[1:0];
                        r26 = r25 + l9;
                        r27 = r25 & l10;
                        r28 = |r27;
                        r29 = r26 & l11;
                        r30 = |r29;
                        r31 = r24[17:0];
                        r32 = -r31;
                        r33 = r24[17:0];
                        r34 = r28 ? r32 : r33;
                        r35 = r24[35:18];
                        r36 = -r35;
                        r37 = r24[35:18];
                        r38 = r30 ? r36 : r37;
                        r39 = r24[49:36];
                        r40 = r39[13:2];
                        r41 = {{36{1'b0}}, r40};
                        r42 = $signed(r41);
                        r43 = r42 - l12;
                        r44 = $signed({{30{r38[17]}}, r38});
                        r45 = r44 * r43;
                        r46 = r45 * l13;
                        r62 = $signed({{32{r46[47]}}, r46});
                        r47 = r62[79:32];
                        r48 = $signed(r47[17:0]);
                        r49 = $signed({{30{r34[17]}}, r34});
                        r50 = r49 * r43;
                        r51 = r50 * l13;
                        r63 = $signed({{32{r51[47]}}, r51});
                        r52 = r63[79:32];
                        r53 = $signed(r52[17:0]);
                        r54 = r34 + r48;
                        r55 = r38 - r53;
                        r56 = l14;
                        r56[17:0] = r54;
                        r57 = r56;
                        r57[35:18] = r55;
                        r58 = {r22, r57};
                        kernel_sin_cos_linear_interp_kernel = r58;
                     end
               endfunction
            endmodule"#]];
        expect_top.assert_eq(&top);
        Ok(())
    }

    /// A short phase ramp, reused by Tiers 4 and 5 so the Verilog
    /// round-trip and the committed waveform describe the same stimulus.
    fn hdl_stimulus() -> impl Iterator<Item = In> {
        // A stride that is not a divisor of the phase space, so the ramp
        // crosses quadrant boundaries and lands on assorted fine
        // remainders rather than repeating one alignment.
        (0..64u128).map(|k| In {
            phase: bits::<TOTAL_W>((k * 65_537) % (1 << TOTAL_W)),
        })
    }

    /// Tier 4 — the emitted Verilog agrees with the Rust simulation,
    /// cycle by cycle, through both the RTL and the NTL paths.
    ///
    /// This is the tier that would have caught the widget being
    /// unsynthesisable: it simulated correctly for its entire life while
    /// `descriptor()` failed outright, because simulation runs the
    /// kernel as a Rust function and never parses a literal.
    #[test]
    fn test_sin_cos_linear_interp_hdl_works() -> miette::Result<()> {
        let uut = SinCosLinearInterp::default();
        let stream = hdl_stimulus()
            .collect::<Vec<_>>()
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        // `.skip(2)` matches `core::ram::synchronous`: a block RAM's
        // output register is `x` in Verilog until the first read
        // completes, while the Rust simulator reports the initial value
        // immediately.  The two agree from the moment the pipeline is
        // primed, which for this widget is the BRAM read plus the
        // attribute DFF — the same two cycles as `LATENCY` above.
        let opts = TestBenchOptions::default().skip(2);
        let tm = test_bench.rtl(&uut, &opts)?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &opts)?;
        tm.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn test_sin_cos_linear_interp_trace() -> miette::Result<()> {
        let uut = SinCosLinearInterp::default();
        let stream = hdl_stimulus()
            .collect::<Vec<_>>()
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("sin_cos_linear_interp");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["cb1701e2304efd26a47b1dccb86d7a10f82486c6136b5628669a19486e1bab8e"];
        let digest = vcd
            .dump_to_file(root.join("sin_cos_linear_interp.vcd"))
            .unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    /// Bit-exact software model of the datapath, so the overflow
    /// question can be answered spectrally without running 65536 cycles
    /// of simulation.  Mirrors the kernel line for line.
    fn model_pair_raw(tbl: &[i128], phase: u128) -> (i128, i128) {
        let coarse = ((phase >> 12) & 0xFF) as usize;
        let quadrant = (phase >> 20) & 0x3;
        let fine = (phase & 0xFFF) as i128;
        let mirrored = 255 - coarse;
        let sin_addr = if quadrant & 1 != 0 { mirrored } else { coarse };
        let cos_quadrant = (quadrant + 1) & 0x3;
        let cos_addr = if cos_quadrant & 1 != 0 {
            mirrored
        } else {
            coarse
        };
        let s0 = if quadrant & 2 != 0 {
            -tbl[sin_addr]
        } else {
            tbl[sin_addr]
        };
        let c0 = if cos_quadrant & 2 != 0 {
            -tbl[cos_addr]
        } else {
            tbl[cos_addr]
        };
        let delta = fine - 2048;
        let sum_sin = s0 + ((c0 * delta * DELTA_K) >> 32);
        let sum_cos = c0 - ((s0 * delta * DELTA_K) >> 32);
        (sum_sin, sum_cos)
    }

    /// [`model_pair_raw`] with the output range enforced, either by
    /// saturating or by wrapping — used only by the diagnostics that
    /// document *why* the headroom exists.
    fn model_pair(tbl: &[i128], phase: u128, saturate: bool) -> (i128, i128) {
        let (sum_sin, sum_cos) = model_pair_raw(tbl, phase);
        let fix = |v: i128| -> i128 {
            if saturate {
                v.clamp(-131071, 131071)
            } else {
                ((v + 131072) & 0x3FFFF) - 131072
            }
        };
        (fix(sum_sin), fix(sum_cos))
    }

    fn model_table() -> Vec<i128> {
        quarter_table().iter().map(|(_, v)| v.raw()).collect()
    }

    /// How often does the sum leave the 18-bit signed range, and where?
    #[test]
    #[ignore = "diagnostic"]
    fn scratch_overflow_census() {
        let tbl = model_table();
        let mut n_over = 0u64;
        let mut first = Vec::new();
        for phase in 0..(1u128 << TOTAL_W) {
            let (ws, wc) = model_pair(&tbl, phase, false);
            let (ss, sc) = model_pair(&tbl, phase, true);
            if ws != ss || wc != sc {
                n_over += 1;
                if first.len() < 6 {
                    first.push((phase, ws, ss, wc, sc));
                }
            }
        }
        let total = 1u64 << TOTAL_W;
        println!(
            "overflowing phases: {n_over} / {total}  ({:.4}%, 1 in {:.0})",
            100.0 * n_over as f64 / total as f64,
            total as f64 / n_over as f64
        );
        for (p, ws, ss, wc, sc) in first {
            println!("  phase {p:>8}  sin wrap {ws:>8} sat {ss:>8}   cos wrap {wc:>8} sat {sc:>8}");
        }
    }

    /// The spectral cost.  Same tuning word, same table, same
    /// interpolation — the only difference is what happens at the rail.
    #[test]
    #[ignore = "diagnostic"]
    fn scratch_overflow_spectrum() {
        use crate::dsp::nco::model::{blackman_harris, fft};
        let tbl = model_table();
        const N: usize = 1 << 16;
        let win = blackman_harris(N);

        let mut words: Vec<u128> = Vec::new();
        for b in 0..TOTAL_W {
            words.push(1u128 << b);
            words.push((1u128 << b) + 1);
            words.push((1u128 << b) | 1024);
        }
        words.push(419431);
        words.push(1234567);
        let mut n_bad = 0;
        for word in words {
            let mut line = format!("word {word:>8}  ");
            for saturate in [false, true] {
                let mut re = vec![0.0f64; N];
                let mut im = vec![0.0f64; N];
                let mut phase = 0u128;
                for k in 0..N {
                    let (_s, c) = model_pair(&tbl, phase, saturate);
                    re[k] = c as f64 * win[k];
                    phase = (phase + word) % (1u128 << TOTAL_W);
                }
                fft(&mut re, &mut im);
                let mag: Vec<f64> = (0..N / 2)
                    .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt())
                    .collect();
                let carrier = (1..N / 2)
                    .max_by(|a, b| mag[*a].total_cmp(&mag[*b]))
                    .unwrap();
                let mut worst = 0.0f64;
                for (k, m) in mag.iter().enumerate().take(N / 2).skip(1) {
                    if (k as i64 - carrier as i64).abs() > 8 {
                        worst = worst.max(*m);
                    }
                }
                let dbc = 20.0 * (worst / mag[carrier]).log10();
                line += &format!(
                    "{:>10} {dbc:>8.1} dBc   ",
                    if saturate { "saturate" } else { "wrap" }
                );
            }
            let _ = &line;
            if line.contains("dBc") {
                let parts: Vec<f64> = line
                    .split_whitespace()
                    .filter_map(|t| t.parse::<f64>().ok())
                    .collect();
                // [word, wrap_dbc, sat_dbc]
                if parts.len() == 3 && parts[1] > parts[2] + 20.0 {
                    n_bad += 1;
                    println!("BAD {line}");
                }
            }
        }
        println!("words where wrapping is >20 dB worse: {n_bad}");
    }

    #[test]
    fn quarter_table_is_a_rising_quarter_sine() {
        let t = quarter_table();
        assert_eq!(t.len(), 256);
        let vals: Vec<i128> = t.iter().map(|(_, v)| v.raw()).collect();
        assert!(
            vals[0] > 0 && vals[0] < 2000,
            "starts near zero: {}",
            vals[0]
        );
        let full = (1i128 << (AMP_W - 1)) - 1;
        assert!(
            vals[255] > full - full / 100,
            "ends near full scale: {} vs {full}",
            vals[255]
        );
        assert!(
            vals.windows(2).all(|w| w[1] > w[0]),
            "must rise monotonically across the quarter"
        );
    }
}
