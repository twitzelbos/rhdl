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
//! # Saturation at the rails is load-bearing
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
//! | tuning word | wrapping | saturating |
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
//! Two fixes are standard. **Saturate**, as here — two comparators on an
//! 18-bit path, no amplitude loss, and correct however the widths later
//! change. Or **scale the table one LSB below full scale**, which costs
//! no logic at all and only 0.000066 dB of amplitude. Saturation is
//! chosen because it stays correct if `TBL_W`, `FINE_W`, or `AMP_W` are
//! ever retuned, whereas the scaling margin would have to be
//! re-derived — and silently under-scaling reintroduces exactly this
//! failure.
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
    let scale = ((1i128 << (AMP_W - 1)) - 1) as f64;
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
    let coarse = (i.phase >> 12u128).resize::<TBL_W>();
    let quadrant = (i.phase >> 20u128).resize::<2>();
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
    let corr_sin = ((c0.resize::<48>() * delta * k) >> 32u128).resize::<AMP_W>();
    let corr_cos = ((s0.resize::<48>() * delta * k) >> 32u128).resize::<AMP_W>();

    // Saturate rather than wrap.  Near |sin| or |cos| = 1 the table
    // value is already at full scale, and the interpolation can push the
    // sum one LSB past the 18-bit signed range — where wrapping flips it
    // to the opposite rail, an error of the full 2^18. It happens for
    // roughly 1 phase in 2000, which is exactly the density that hides
    // behind an average and only shows in a worst-case sweep.
    let lim = signed::<20>(131071);
    let sum_sin = s0.resize::<20>() + corr_sin.resize::<20>();
    let sum_cos = c0.resize::<20>() - corr_cos.resize::<20>();
    let sat_sin = if sum_sin > lim {
        lim
    } else if sum_sin < -lim {
        -lim
    } else {
        sum_sin
    };
    let sat_cos = if sum_cos > lim {
        lim
    } else if sum_cos < -lim {
        -lim
    } else {
        sum_cos
    };

    let o = Out {
        sin: sat_sin.resize::<AMP_W>(),
        cos: sat_cos.resize::<AMP_W>(),
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_construction() {
        let _a = SinCosLinearInterp::default();
    }

    /// Scratch: hold one phase constant and watch the pipeline settle.
    #[test]
    #[ignore]
    fn scratch_settle() {
        let uut = SinCosLinearInterp::default();
        let full = 1u128 << TOTAL_W;
        let scale = ((1i128 << (AMP_W - 1)) - 1) as f64;
        let ph = 700_000u128;
        let th = 2.0 * std::f64::consts::PI * ph as f64 / full as f64;
        println!(
            "\n  phase {ph} -> want sin {:.0} cos {:.0}",
            th.sin() * scale,
            th.cos() * scale
        );
        let stream = std::iter::repeat_n(
            In {
                phase: bits::<TOTAL_W>(ph),
            },
            8,
        )
        .with_reset(1)
        .clock_pos_edge(100);
        println!("  cycle   got sin    got cos");
        for (k, s) in uut.run(stream).synchronous_sample().enumerate() {
            println!(
                "  {k:>5}  {:>9}  {:>9}",
                s.output.sin.raw(),
                s.output.cos.raw()
            );
        }
    }

    /// Scratch: drive the kernel directly with a hand-built Q, so the
    /// BRAM and its latency are out of the picture entirely.
    #[test]
    #[ignore]
    fn scratch_kernel_direct() {
        let tbl = quarter_table();
        let scale = ((1i128 << (AMP_W - 1)) - 1) as f64;
        let two_pi = 2.0 * std::f64::consts::PI;
        println!("\n  quad idx  fine    want sin   want cos    got sin    got cos");
        for (quad, idx, fine) in [
            (0u128, 128u128, 2048u128),
            (0, 128, 0),
            (0, 128, 4095),
            (1, 64, 2048),
            (2, 200, 2048),
            (3, 10, 2048),
        ] {
            // Addresses the kernel would have issued for this phase.
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
            // True angle for this (quadrant, index, fine).
            let coarse_angle = two_pi * ((quad * 256 + idx) as f64 + 0.5) / 1024.0;
            let delta = (fine as f64 / 4096.0 - 0.5) * (two_pi / 1024.0);
            let th = coarse_angle + delta;
            println!(
                "  {quad:>4} {idx:>4} {fine:>5}  {:>10.0} {:>10.0}  {:>10} {:>10}",
                th.sin() * scale,
                th.cos() * scale,
                o.sin.raw(),
                o.cos.raw()
            );
        }
    }

    /// Scratch: print actual vs expected for a few phases.
    #[test]
    #[ignore]
    fn scratch_dump() {
        let uut = SinCosLinearInterp::default();
        let full = 1u128 << TOTAL_W;
        let scale = ((1i128 << (AMP_W - 1)) - 1) as f64;
        let phases: Vec<u128> = (0..12u128).map(|k| k * (full / 12)).collect();
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
        println!("\n  idx  phase      want sin   want cos   got sin    got cos");
        for (i, ph) in phases.iter().enumerate() {
            let th = 2.0 * std::f64::consts::PI * *ph as f64 / full as f64;
            let (ws, wc) = ((th.sin() * scale) as i128, (th.cos() * scale) as i128);
            let (gs, gc) = if i < out.len() { out[i] } else { (0, 0) };
            println!("  {i:>3}  {ph:>9}  {ws:>9}  {wc:>9}  {gs:>9}  {gc:>9}");
        }
        println!("  (out len {})", out.len());
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

    /// Bit-exact software model of the datapath, so the overflow
    /// question can be answered spectrally without running 65536 cycles
    /// of simulation.  Mirrors the kernel line for line.
    fn model_pair(tbl: &[i128], phase: u128, saturate: bool) -> (i128, i128) {
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
