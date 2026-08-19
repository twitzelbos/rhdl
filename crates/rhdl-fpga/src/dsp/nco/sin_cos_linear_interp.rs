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
//! # Generic widths, and the four validated configurations
//!
//! The widths are const generics. Each validated configuration has a
//! type alias, a measured effective-bit figure, and its own `iverilog`
//! round trip — because a type alias that has never been synthesised is
//! a claim, not a configuration.
//!
//! | alias | TBL/FINE/TOTAL/AMP/INT | table | SFDR | **ENOB** |
//! |---|---|---|---|---|
//! | [`SinCosLinearInterpDefault`] | 8/12/22/18/48 | 9 Kbit | −104.3 dBc | **17.50** |
//! | [`SinCosLinearInterp24`] | 10/14/26/24/56 | 48 Kbit | −140.4 dBc | **23.05** |
//! | [`SinCosLinearInterp28`] | 11/15/28/28/64 | 112 Kbit | −164.5 dBc | **27.02** |
//! | [`SinCosLinearInterp32`] | 12/16/30/32/72 | 256 Kbit | −188.6 dBc | **31.03** |
//!
//! ENOB is derived from SINAD, not from SFDR — the worst single spur
//! flatters the result by ignoring every other one. It lands about a bit
//! below `AMP_W` throughout, so every configuration is
//! **amplitude-quantisation limited**: the interpolation residual is not
//! the bottleneck at any of them, and the widths really are the knob.
//!
//! `AMP_W = 18` is the DSP48's native multiplier port width, which is why
//! the default sits there. The wider configurations buy effective bits
//! with block RAM and with multiplier cascading — see the note on DSP
//! inference below, because that cost is not currently expressed in the
//! emitted Verilog.
//!
//! | parameter | default value |
//! |---|---|
//! | phase consumed | 22 bits (2 quadrant + 8 index + 12 fine) |
//! | table | 256 entries × 18 bit, quarter wave |
//! | amplitude | 18 bit (native DSP48 port) |
//! | phase resolution | 360° / 2²² ≈ 0.000086° |
//!
//! ## What the widths were previously blocked on, and why it was wrong
//!
//! This module used to state that generic widths "would require deriving
//! that constant inside a kernel from const generics, which the kernel
//! language cannot do." **That was false**, and
//! [`crate::dsp::mixer::rounding`] — written two days later, in the same
//! `dsp` tree — already did exactly it: `bits::<PROD_W>(1 << (DROP - 1))`
//! is a const-generic-derived constant inside a kernel, and
//! `>> bits::<8>(DROP as u128)` a const-generic shift that const-folds to
//! a slice.
//!
//! The real obstacle was narrower and is recorded on [`DELTA_K`]: the
//! scaling constant involves 2π, `const fn` cannot do floating point on
//! stable, and `#[kernel]` resolves a call expression as a kernel
//! invocation so a helper function cannot be called from a kernel body
//! either. Both dissolve once the Q-point tracks `TOTAL_W`, because then
//! the constant does not depend on the configuration at all.
//!
//! ## The one-LSB headroom was not scale-invariant
//!
//! The original widget subtracted a fixed 2 LSB from full scale and
//! called the margin "one LSB of headroom". That is correct **only** at
//! 8/18. Linear interpolation overshoots a peak by the second-order term
//! it neglects, which grows with `AMP_W` and falls with the square of the
//! coarse resolution — see [`overshoot_bound`]. At 10/14/26/24 the sum
//! overshoots by 3 LSB, wraps, and the output collapses to −29.8 dBc and
//! 4.7 effective bits.
//!
//! This was found by *validating* the wider configurations rather than
//! merely defining them, which is the argument for per-configuration
//! tests in one sentence.
//!
//! ## DSP48 inference: hoped for, not instantiated
//!
//! **No vendor primitive is instantiated anywhere.** The emitted Verilog
//! is behavioural — `r45 = r44 * r43;` — and DSP48 mapping is left
//! entirely to Vivado's inference. The `Target` / `primitive!` machinery
//! in `vendor-primitive-architecture.md` is a design document; there is
//! no `trait Target` and no `hdl_for` in `rhdl-core` today.
//!
//! Worth knowing, because it undercuts the reason `AMP_W = 18` was
//! chosen: the kernel widens both operands to `INT_W` *before*
//! multiplying, so what is emitted is a **48×48 signed multiply**, not an
//! 18×25 one. A DSP48E1 is 18×25, and a true 48×48 needs six to nine of
//! them. In practice Vivado's bit-width propagation prunes the unused
//! high bits and one of the two multiplies per component is by the
//! constant `DELTA_K` (shift-adds, not a slice) — but that is inference
//! resting on inference, and it is not something this module currently
//! asserts or measures. Narrowing the multiply to natural operand widths
//! is a live follow-up.
//!
//! # Table headroom, and why it is load-bearing
//!
//! Near a peak the table value is already at full scale and the
//! first-order correction can push the sum past the signed range. At the
//! default configuration that overshoot is **one LSB** — measured across
//! all 2²² phases, the largest is `131072` against a limit of `131071`,
//! for 1550 phases, about 1 in 2706. It is *not* one LSB in general:
//! see [`overshoot_bound`], which grows with `AMP_W` and falls with the
//! square of the coarse resolution.
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
//! [`table_scale`] — the signed maximum less [`overshoot_bound`],
//! which at the default configuration is one LSB and costs
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
//! reason worth recording: at the time, RHDL emitted signed comparisons
//! against literals as **unsigned** Verilog, so the clamp inverted its
//! own sense in hardware while simulating correctly. That defect has
//! since been fixed — signed literals now carry Verilog's `s` base
//! specifier — and `tests/signed_literal_comparison.rs` is the
//! regression test.
//!
//! The scaling decision stands regardless: it was the cheaper option on
//! its own merits, and the exhaustive range check is a stronger
//! guarantee than a clamp. The defect only settled an already-close
//! call. Reinstating saturation is now a live option if a future
//! retuning makes the headroom insufficient.
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

/// Quarter-wave table address bits of the default configuration.
pub const TBL_W: usize = 8;
/// Fine interpolation bits of the default configuration.
pub const FINE_W: usize = 12;
/// Total phase bits consumed by the default configuration.
pub const TOTAL_W: usize = 22;
/// Bits per output component in the default configuration.
pub const AMP_W: usize = 18;
/// Intermediate arithmetic width of the default configuration.
pub const INT_W: usize = 48;

/// Bits of guard between the Q-point and `TOTAL_W`. See [`DELTA_K`].
///
/// Ten, which leaves [`DELTA_K`] thirteen significant bits at every
/// configuration.
pub const Q_GUARD: usize = 10;

/// Fine-remainder scaling factor, in Q`(TOTAL_W + Q_GUARD)`.
///
/// Converts a fine-remainder LSB into radians:
///
/// ```text
/// delta_k / 2^(TOTAL_W + Q_GUARD) = 2π / 2^TOTAL_W
/// ```
///
/// # Why this is a constant rather than a function of `TOTAL_W`
///
/// The obvious formulation fixes the Q-point at 32 and lets the factor
/// shrink — `round(2π · 2^32 / 2^TOTAL_W)`, which is 6434 at
/// `TOTAL_W = 22` and **402** at `TOTAL_W = 26`. That loses precision
/// exactly when a wider configuration is asking for more: the factor's
/// relative error grows 2.8e-6 → 3.1e-4, which caps achievable SFDR
/// near −120 dBc no matter how wide `AMP_W` is.
///
/// Letting the Q-point *track* `TOTAL_W` instead makes the factor
/// scale-invariant:
///
/// ```text
/// 2π · 2^(TOTAL_W + 10) / 2^TOTAL_W = 2π · 2^10 = 6434
/// ```
///
/// So one constant serves every configuration with the same 2.8e-6
/// relative error, and the configuration-dependence moves into the
/// shift — where it costs nothing, because a const-generic shift
/// const-folds to a slice. `delta_k_is_scale_invariant` pins this.
///
/// # Why it is a `const`, not a `const fn`
///
/// Two reasons, and they reinforce each other. `const fn` cannot do
/// floating point on stable, which is what defeated the first attempt at
/// making these widths generic — but scale-invariance means there is no
/// function to write, because the value does not depend on the
/// configuration at all.
///
/// It also *has* to be a `const`: `#[kernel]` resolves a call expression
/// as a kernel invocation, so a `const fn` called inside a kernel body
/// fails to compile with "expected type, found function". Constants are
/// substituted before the macro sees them.
pub const DELTA_K: i128 = 6434;

/// π² in Q16, for [`overshoot_bound`]. `const fn` cannot do floating
/// point on stable, so the transcendental is a compile-time integer.
const PI_SQ_Q16: i128 = 646_976;

/// Largest amount, in LSB, by which the interpolated sum can exceed the
/// table amplitude.
///
/// **This is not a constant, and assuming it was is a bug this widget
/// shipped with.** Linear interpolation overshoots a peak by the
/// second-order term it neglects. With a coarse step of
/// `2π/2^(TBL_W+2)` radians and a remainder spanning half a step either
/// side, the overshoot at amplitude `2^(AMP_W-1)` is
///
/// ```text
/// 2^(AMP_W-1) · (π/2^(TBL_W+2))² / 2  =  π² · 2^(AMP_W - 2·TBL_W - 6)
/// ```
///
/// so it grows with `AMP_W` and falls with the *square* of the coarse
/// resolution. Measured against the model, the formula is exact at every
/// validated configuration:
///
/// | TBL_W / AMP_W | predicted | measured |
/// |---|---|---|
/// | 8 / 18 | 0.62 | **1** |
/// | 10 / 24 | 2.47 | **3** |
/// | 11 / 28 | 9.87 | **10** |
/// | 12 / 32 | 39.48 | **40** |
///
/// The original widget subtracted a fixed 2 LSB, which is the correct
/// value *only* at 8/18 — that configuration's bound rounded up to 1,
/// plus the inclusive-range LSB. At 10/24 the sum overshoots by 3 and
/// wraps, which is a full-scale sign inversion at the peaks: measured
/// -29.8 dBc, 4.7 effective bits, against -104.3 dBc for the default.
/// `headroom_holds_at_every_configuration` is what caught it.
pub const fn overshoot_bound(amp_w: usize, tbl_w: usize) -> i128 {
    // e = 16 + (2·tbl_w + 6) - amp_w, the right shift that turns Q16 π²
    // into π²·2^(amp_w - 2·tbl_w - 6).  Rounded UP: the bound must cover
    // the worst case, not approximate it.
    let e = 22 + 2 * tbl_w;
    if e >= amp_w {
        let sh = e - amp_w;
        (PI_SQ_Q16 + (1 << sh) - 1) >> sh
    } else {
        PI_SQ_Q16 << (amp_w - e)
    }
}

/// Table amplitude for a configuration: the signed maximum less the
/// interpolation overshoot the sum can add on top of it.
///
/// Public because the headroom is part of the widget's contract, not an
/// implementation detail: anything reasoning about the output range
/// needs it.
///
/// That headroom is what makes the interpolated sum unable to leave the
/// output range — see the saturation discussion in the module docs, and
/// `headroom_holds_at_every_configuration`, which proves it exhaustively
/// for the default configuration and by dense rail sweep for the wider
/// ones.
///
/// At the default 8/18 this evaluates to `2^17 - 2` — exactly the
/// hand-picked value it replaces, so the default configuration's table,
/// emitted Verilog and committed digests are unchanged.
pub const fn table_scale(amp_w: usize, tbl_w: usize) -> i128 {
    (1 << (amp_w - 1)) - 1 - overshoot_bound(amp_w, tbl_w)
}

/// Smallest intermediate width that cannot overflow, for a given
/// configuration.
///
/// The fine rotation forms `c0 · delta · DELTA_K` as two `xmul`s at
/// natural width. `delta` is `FINE_W + 2` bits after centring and
/// `DELTA_K` is 14, so the chain carries
///
/// ```text
/// AMP_W + (FINE_W + 2) + 14  =  AMP_W + FINE_W + 16
/// ```
///
/// which is wider than the mathematical minimum of `AMP_W + FINE_W + 12`,
/// because a natural-width product carries the operand widths rather than
/// the worst-case magnitude. That is the trade for emitting a narrow
/// multiply, and it is the right way round: `INT_W` is a wire width,
/// while the multiply is silicon. Stated as a `const fn` so
/// an under-sized `INT_W` is a build failure rather than a silent wrap
/// in the fine correction — which would be a full-scale sign inversion
/// at the peaks, the same class of damage the table headroom exists to
/// prevent.
pub const fn min_int_w(amp_w: usize, fine_w: usize) -> usize {
    amp_w + fine_w + 16
}

/// Table amplitude of the default configuration.
pub const TABLE_SCALE: i128 = table_scale(AMP_W, TBL_W);

/// Quadrature phase-to-amplitude: coarse quarter-wave table plus
/// first-order fine rotation.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct SinCosLinearInterp<
    const TBL_W: usize,
    const FINE_W: usize,
    const TOTAL_W: usize,
    const AMP_W: usize,
    const INT_W: usize,
> where
    rhdl::bits::W<TBL_W>: BitWidth,
    rhdl::bits::W<FINE_W>: BitWidth,
    rhdl::bits::W<TOTAL_W>: BitWidth,
    rhdl::bits::W<AMP_W>: BitWidth,
    rhdl::bits::W<INT_W>: BitWidth,
{
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
    delayed: dff::DFF<Pipelined<FINE_W>>,
}

/// The default configuration: 8/12/22/18/48.
///
/// The one the original spur analysis validated, kept as the default so
/// that existing instantiations and every committed snapshot are
/// unchanged by the widths becoming generic. `AMP_W = 18` is the DSP48's
/// native multiplier port width, so this configuration is also the one
/// whose fine rotation fits in a single slice per multiply.
pub type SinCosLinearInterpDefault = SinCosLinearInterp<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W>;

/// **24-bit** configuration: 10/14/26/24, `INT_W = 56`.
///
/// The smallest configuration that clears 18 *effective* bits. See the
/// validated-configuration table in the module docs.
pub type SinCosLinearInterp24 = SinCosLinearInterp<10, 14, 26, 24, 56>;

/// **28-bit** configuration: 11/15/28/28, `INT_W = 56`.
pub type SinCosLinearInterp28 = SinCosLinearInterp<11, 15, 28, 28, 64>;

/// **32-bit** configuration: 12/16/30/32, `INT_W = 64`.
pub type SinCosLinearInterp32 = SinCosLinearInterp<12, 16, 30, 32, 72>;

/// Phase attributes carried alongside the registered table read.
#[derive(PartialEq, Clone, Copy, Debug, Digital, Default)]
#[doc(hidden)]
pub struct Pipelined<const FINE_W: usize>
where
    rhdl::bits::W<FINE_W>: BitWidth,
{
    /// Quadrant of the phase that produced the current table output.
    pub quadrant: Bits<2>,
    /// Fine remainder of that same phase.
    pub fine: Bits<FINE_W>,
}

/// Build the quarter-wave table: `2^TBL_W` entries at bin midpoints over
/// `[0, π/2)`.
///
/// Midpoint sampling is what makes the odd-quadrant mirror exact rather
/// than off-by-one.
///
/// `f64` has a 53-bit mantissa, so the rounded sine is exact for any
/// `AMP_W` this widget can express; `table_generation_is_exact_at_every_width`
/// checks the widest validated configuration against a higher-precision
/// reference.
fn quarter_table<const TBL_W: usize, const AMP_W: usize>() -> Vec<(Bits<TBL_W>, SignedBits<AMP_W>)>
where
    rhdl::bits::W<TBL_W>: BitWidth,
    rhdl::bits::W<AMP_W>: BitWidth,
{
    let coarse_w = TBL_W + 2;
    let scale = table_scale(AMP_W, TBL_W) as f64;
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

impl<
    const TBL_W: usize,
    const FINE_W: usize,
    const TOTAL_W: usize,
    const AMP_W: usize,
    const INT_W: usize,
> Default for SinCosLinearInterp<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W>
where
    rhdl::bits::W<TBL_W>: BitWidth,
    rhdl::bits::W<FINE_W>: BitWidth,
    rhdl::bits::W<TOTAL_W>: BitWidth,
    rhdl::bits::W<AMP_W>: BitWidth,
    rhdl::bits::W<INT_W>: BitWidth,
{
    fn default() -> Self {
        // The configuration is checked here rather than only in a test,
        // so an inconsistent instantiation cannot be constructed at all.
        assert_eq!(
            TOTAL_W,
            TBL_W + FINE_W + 2,
            "TOTAL_W must be 2 quadrant bits + TBL_W + FINE_W"
        );
        assert!(
            INT_W >= min_int_w(AMP_W, FINE_W),
            "INT_W is too small for this configuration: the fine rotation \
             needs at least {} bits and INT_W is {INT_W}.  An under-sized \
             intermediate wraps the correction, which is a full-scale sign \
             inversion at the peaks.",
            min_int_w(AMP_W, FINE_W)
        );
        Self {
            sin_tbl: SyncBRAM::new(quarter_table::<TBL_W, AMP_W>()),
            cos_tbl: SyncBRAM::new(quarter_table::<TBL_W, AMP_W>()),
            delayed: dff::DFF::new(Pipelined::<FINE_W>::default()),
        }
    }
}

/// Inputs for [`SinCosLinearInterp`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const TOTAL_W: usize>
where
    rhdl::bits::W<TOTAL_W>: BitWidth,
{
    /// Phase, truncated to the `TOTAL_W` bits this stage consumes.
    pub phase: Bits<TOTAL_W>,
}

/// Outputs from [`SinCosLinearInterp`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const AMP_W: usize>
where
    rhdl::bits::W<AMP_W>: BitWidth,
{
    /// Sine of the phase.
    pub sin: SignedBits<AMP_W>,
    /// Cosine of the phase.
    pub cos: SignedBits<AMP_W>,
}

impl<
    const TBL_W: usize,
    const FINE_W: usize,
    const TOTAL_W: usize,
    const AMP_W: usize,
    const INT_W: usize,
> SynchronousIO for SinCosLinearInterp<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W>
where
    rhdl::bits::W<TBL_W>: BitWidth,
    rhdl::bits::W<FINE_W>: BitWidth,
    rhdl::bits::W<TOTAL_W>: BitWidth,
    rhdl::bits::W<AMP_W>: BitWidth,
    rhdl::bits::W<INT_W>: BitWidth,
{
    type I = In<TOTAL_W>;
    type O = Out<AMP_W>;
    type Kernel = sin_cos_linear_interp_kernel<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
// Takes `_cr`, not `cr`: this kernel has no `if cr.reset.any()` block,
// which is the one place in `dsp` that departs from CLAUDE.md rule 12.
// Deliberate and safe -- the widget's entire state is the `delayed` DFF
// and the two BRAMs, each of which resets itself, and everything else
// here is combinational. There is no output that reset must force to a
// defined value: at phase zero the table already yields sin=0, cos=full
// scale, which is the correct value rather than a reset artefact.
pub fn sin_cos_linear_interp_kernel<
    const TBL_W: usize,
    const FINE_W: usize,
    const TOTAL_W: usize,
    const AMP_W: usize,
    const INT_W: usize,
>(
    _cr: ClockReset,
    i: In<TOTAL_W>,
    q: Q<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W>,
) -> (Out<AMP_W>, D<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W>)
where
    rhdl::bits::W<TBL_W>: BitWidth,
    rhdl::bits::W<FINE_W>: BitWidth,
    rhdl::bits::W<TOTAL_W>: BitWidth,
    rhdl::bits::W<AMP_W>: BitWidth,
    rhdl::bits::W<INT_W>: BitWidth,
{
    let mut d = D::<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W>::dont_care();

    // Split phase into quadrant | index | fine.  Const-generic shifts
    // const-fold to a slice, so these cost no barrel shifter -- the same
    // property `dsp::mixer::rounding` relies on.
    let coarse = (i.phase >> bits::<8>(FINE_W as u128)).resize::<TBL_W>();
    let quadrant = (i.phase >> bits::<8>((FINE_W + TBL_W) as u128)).resize::<2>();
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
    d.delayed = Pipelined::<FINE_W> { quadrant, fine };

    // Sign and interpolation use the DELAYED attributes, which belong to
    // the phase that produced the table values now emerging.
    let dq = q.delayed.quadrant;
    let dcq = dq + bits::<2>(1);
    let sin_neg = (dq & bits::<2>(2)) != bits::<2>(0);
    let cos_neg = (dcq & bits::<2>(2)) != bits::<2>(0);
    let s0 = if sin_neg { -q.sin_tbl } else { q.sin_tbl };
    let c0 = if cos_neg { -q.cos_tbl } else { q.cos_tbl };

    // Fine remainder, centred so delta spans +/- half a coarse step.
    // Zero-extend BEFORE reinterpreting as signed.  `as_signed()` on a
    // FINE_W-bit value treats it as two's complement, so any fine
    // remainder >= 2^(FINE_W-1) would become negative before the centring
    // subtraction -- wrong for half of all phases, and catastrophically
    // so.
    let fine_ext = q.delayed.fine.dyn_bits().xsgn();
    let half_step = bits::<FINE_W>(1 << (FINE_W - 1)).dyn_bits().xsgn();
    let delta = fine_ext.xsub(half_step);

    // First-order rotation.  The Q-point tracks TOTAL_W, which is what
    // keeps `delta_k()` scale-invariant at 6434 rather than shrinking as
    // the configuration widens -- see its docs.
    // The variable x variable product first, at its NATURAL width.
    //
    // `xmul` forms the product at the sum of the operand widths rather
    // than at INT_W.  That matters for DSP inference: resizing both
    // operands to INT_W first, as this kernel used to, emits a 48x48
    // signed multiply -- six to nine DSP48E1 slices if the synthesiser
    // does not prune it.  At natural width the default emits 30x30.  See
    // the module docs on DSP inference for what RHDL still cannot express.
    let p_sin = c0.dyn_bits().xmul(delta);
    let p_cos = s0.dyn_bits().xmul(delta);

    // ...then the scale, whose operand is a CONSTANT and therefore lowers
    // to shift-adds rather than to a multiplier.  DELTA_K is 14 bits
    // signed at every configuration, which is the point of its
    // scale-invariance.
    let k = signed::<14>(DELTA_K).dyn_bits();
    let q_point = bits::<8>((TOTAL_W + Q_GUARD) as u128);
    let scaled_sin: SignedBits<INT_W> = p_sin.xmul(k).resize::<INT_W>().as_signed_bits();
    let scaled_cos: SignedBits<INT_W> = p_cos.xmul(k).resize::<INT_W>().as_signed_bits();
    let corr_sin = (scaled_sin >> q_point).resize::<AMP_W>();
    let corr_cos = (scaled_cos >> q_point).resize::<AMP_W>();

    // No clamp.  `table_scale(AMP_W, TBL_W)` leaves exactly the overshoot
    // the rotation can produce as headroom, so the sum
    // provably cannot leave the output range --
    // `interpolated_sum_never_leaves_the_range` checks all 2^TOTAL_W
    // phases for the default configuration and sweeps the rails densely
    // for the wider ones.  Wrapping here would be catastrophic rather
    // than cosmetic: see the module docs.
    let o = Out::<AMP_W> {
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
        let _a = SinCosLinearInterpDefault::default();
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
        let tbl = quarter_table::<TBL_W, AMP_W>();
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

                    let q = Q::<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W> {
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
        let uut = SinCosLinearInterpDefault::default();
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
            .map(|p| In::<TOTAL_W> {
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

        // Alignment within *this sample stream*, which is not the
        // widget's hardware latency.  `with_reset(1)` prepends a cycle,
        // so stimulus index k lands at output index k+1; the hardware
        // latency is 1, the registered table read, and the attribute
        // DFF runs concurrently with it rather than after it.  See
        // `super::latency::PHASE_TO_AMPLITUDE`, which measures it.
        //
        // Reading this 2 as a latency put a wrong constant into the
        // scheduler's arithmetic once; do not repeat it.
        //
        // Asserted, not searched: picking the best of several shifts
        // lets a single catastrophic sample hide behind a misalignment.
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
        let uut = SinCosLinearInterpDefault::default();
        let full = 1u128 << TOTAL_W;
        let stride = 7919u128;
        let phases: Vec<u128> = (0..2048u128).map(|k| (k * stride) % full).collect();
        let stream = phases
            .iter()
            .map(|p| In::<TOTAL_W> {
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

    /// Worst in-band spur of the **widget's own output**, per tuning
    /// word.
    ///
    /// 65536-point Blackman-Harris on the cosine, carrier excluded with a
    /// +/-8 bin guard. Mirrors the measurement in the module docs so the
    /// numbers are comparable.
    fn worst_spur_dbc(word: u128) -> f64 {
        use crate::dsp::nco::model::{blackman_harris, fft};
        const N: usize = 1 << 16;

        let full = 1u128 << TOTAL_W;
        let mut phase = 0u128;
        let phases: Vec<u128> = (0..N + 4)
            .map(|_| {
                let p = phase;
                phase = (phase + word) % full;
                p
            })
            .collect();

        let uut = SinCosLinearInterpDefault::default();
        let stream = phases
            .iter()
            .map(|p| In::<TOTAL_W> {
                phase: bits::<TOTAL_W>(*p),
            })
            .collect::<Vec<_>>()
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);
        let cos: Vec<i128> = uut
            .run(stream)
            .synchronous_sample()
            .map(|s| s.output.cos.raw())
            // Skip the reset cycle plus the pipeline fill, so no sample
            // is the DFF/BRAM initial state rather than a real value.
            .skip(4)
            .take(N)
            .collect();
        assert_eq!(cos.len(), N, "not enough samples to transform");

        let win = blackman_harris(N);
        let mut re: Vec<f64> = cos.iter().zip(&win).map(|(c, w)| *c as f64 * w).collect();
        let mut im = vec![0.0f64; N];
        fft(&mut re, &mut im);

        let mag: Vec<f64> = (0..N / 2)
            .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt())
            .collect();
        let carrier = (1..N / 2)
            .max_by(|a, b| mag[*a].total_cmp(&mag[*b]))
            .unwrap();
        let worst = mag
            .iter()
            .enumerate()
            .take(N / 2)
            .skip(1)
            .filter(|(k, _)| (*k as i64 - carrier as i64).abs() > 8)
            .map(|(_, m)| *m)
            .fold(0.0f64, f64::max);
        20.0 * (worst / mag[carrier]).log10()
    }

    /// **The spectral claim, as a test that actually runs.**
    ///
    /// Every spur figure in this module and in [`super::mod`]'s docs came
    /// from `#[ignore]`d sweeps or `scratch_*` diagnostics, so no green
    /// test would have failed if a datapath change cost 20 dB of SFDR.
    /// The spur performance is the entire reason this architecture was
    /// chosen over a bigger table and over CORDIC, so a regression in it
    /// would otherwise surface as a bench measurement months later.
    ///
    /// Measured on the **widget**, not the model. The words are chosen
    /// adversarially: `model`'s docs record that every worst-case word
    /// found has its truncated remainder at a pure power of two or its
    /// complement, because a short remainder period concentrates the
    /// error into few strong spurs. Uniform random sampling lands in the
    /// benign regime and reports 20-30 dB too optimistic.
    ///
    /// The module docs' per-word table gives -104.0 to -106.8 dBc for
    /// this configuration under the same measurement. The threshold sits
    /// at -95 dBc: tight enough to catch a real regression, loose enough
    /// that it is not measuring FFT leakage.
    ///
    /// **Verified able to fail** (measured, not assumed): setting
    /// `TABLE_SCALE` to the full-scale `(1 << (AMP_W - 1)) - 1` makes the
    /// interpolated sum wrap at the peaks, and word 524288 then reports
    /// **-0.00 dBc** -- the spur exactly equal to the carrier, which is
    /// the figure the module docs' wrap table predicts.
    ///
    /// # Two words, not twenty
    ///
    /// Measured across the three words originally tried, the results were
    /// -104.30, -104.30 and -104.27 dBc: **word-independent to within
    /// 0.03 dB.** That is itself informative -- at this configuration the
    /// floor is set by amplitude quantisation and interpolation residual,
    /// not by phase truncation, which is what one expects when the fine
    /// rotation is exact to second order and suppresses truncation spurs
    /// below the quantisation floor.
    ///
    /// So more words buy no discrimination here, only runtime (each is a
    /// 65536-sample simulation). Two are kept rather than one so that a
    /// future retuning which *does* reintroduce word dependence is
    /// visible as a divergence between them.
    #[test]
    fn worst_in_band_spur_is_below_the_threshold() {
        const THRESHOLD_DBC: f64 = -95.0;
        // 1 << 19 is the worst case in the module docs' wrap table.
        // 1234567 is an innocuous-looking odd word, included because the
        // wrapping failure was never confined to round numbers.
        for word in [524_288u128, 1_234_567] {
            let dbc = worst_spur_dbc(word);
            assert!(
                dbc < THRESHOLD_DBC,
                "tuning word {word}: worst in-band spur {dbc:.1} dBc exceeds \
                 {THRESHOLD_DBC:.1} dBc.  The module docs claim -104 dBc or \
                 better for this configuration.  A figure near 0 dBc means \
                 the interpolated sum is wrapping at the peaks (check \
                 TABLE_SCALE); a figure in the -60s means the fine rotation \
                 is degraded (check DELTA_K or the attribute delay)."
            );
            assert!(
                dbc > -200.0,
                "word {word} reported {dbc:.1} dBc, which is not a real \
                 measurement -- the carrier search or the windowing is broken"
            );
        }
    }

    // -----------------------------------------------------------------
    // Per-configuration validation.
    //
    // The widths are generic, so "it works" has to be established per
    // configuration rather than once.  Each of the four validated
    // configurations gets: the headroom property, an `iverilog` round
    // trip proving it is synthesizable, and a measured effective-bit
    // figure.  Naming a type alias is not validation.
    // -----------------------------------------------------------------

    /// Bit-exact model of the datapath, for any configuration.
    ///
    /// Mirrors the kernel exactly, including the `TOTAL_W + Q_GUARD`
    /// Q-point. `model_agrees_with_the_widget_at_every_config` is what
    /// makes it evidence about hardware rather than about itself.
    fn model_pair_cfg<
        const TBL_W: usize,
        const FINE_W: usize,
        const TOTAL_W: usize,
        const AMP_W: usize,
    >(
        tbl: &[i128],
        phase: u128,
    ) -> (i128, i128) {
        let fine_mask = (1u128 << FINE_W) - 1;
        let tbl_mask = (1u128 << TBL_W) - 1;
        let coarse = ((phase >> FINE_W) & tbl_mask) as usize;
        let quadrant = (phase >> (FINE_W + TBL_W)) & 0x3;
        let fine = (phase & fine_mask) as i128;

        let mirrored = (tbl_mask as usize) - coarse;
        let sin_addr = if quadrant & 1 != 0 { mirrored } else { coarse };
        let cos_q = (quadrant + 1) & 0x3;
        let cos_addr = if cos_q & 1 != 0 { mirrored } else { coarse };

        let s0 = if quadrant & 2 != 0 {
            -tbl[sin_addr]
        } else {
            tbl[sin_addr]
        };
        let c0 = if cos_q & 2 != 0 {
            -tbl[cos_addr]
        } else {
            tbl[cos_addr]
        };

        let delta = fine - (1i128 << (FINE_W - 1));
        let shift = TOTAL_W + Q_GUARD;
        (
            s0 + ((c0 * delta * DELTA_K) >> shift),
            c0 - ((s0 * delta * DELTA_K) >> shift),
        )
    }

    fn model_table_cfg<const TBL_W: usize, const AMP_W: usize>() -> Vec<i128>
    where
        rhdl::bits::W<TBL_W>: BitWidth,
        rhdl::bits::W<AMP_W>: BitWidth,
    {
        quarter_table::<TBL_W, AMP_W>()
            .iter()
            .map(|(_, v)| v.raw())
            .collect()
    }

    /// The headroom property for one configuration.
    ///
    /// Returns `(checked, overshoots, worst_abs)`.
    ///
    /// Exhaustive over all `2^TOTAL_W` phases when that is affordable,
    /// and otherwise over a dense sweep of the four rails plus a coprime
    /// stride. **The rails are where this bites** — the overshoot happens
    /// where the table value is already at full scale and the correction
    /// pushes the sum past it — and an earlier version of the Tier-2 test
    /// that omitted them passed with the saturation removed entirely.
    fn headroom_scan<
        const TBL_W: usize,
        const FINE_W: usize,
        const TOTAL_W: usize,
        const AMP_W: usize,
    >(
        exhaustive: bool,
    ) -> (u64, u64, i128)
    where
        rhdl::bits::W<TBL_W>: BitWidth,
        rhdl::bits::W<AMP_W>: BitWidth,
    {
        let tbl = model_table_cfg::<TBL_W, AMP_W>();
        let limit = (1i128 << (AMP_W - 1)) - 1;
        let full = 1u128 << TOTAL_W;

        let phases: Box<dyn Iterator<Item = u128>> = if exhaustive {
            Box::new(0..full)
        } else {
            // Dense around each rail, plus a coprime stride for breadth.
            let rails = (0..4u128).flat_map(move |quad| {
                let rail = quad * (full / 4);
                (0..(1u128 << FINE_W) * 2)
                    .map(move |d| (rail + d) % full)
                    .chain((0..(1u128 << FINE_W) * 2).map(move |d| (rail + full - d - 1) % full))
            });
            let stride = 7_919u128;
            let spread = (0..1u128 << 20).map(move |k| (k * stride) % full);
            Box::new(rails.chain(spread))
        };

        let mut checked = 0u64;
        let mut over = 0u64;
        let mut worst = 0i128;
        for phase in phases {
            let (sn, cs) = model_pair_cfg::<TBL_W, FINE_W, TOTAL_W, AMP_W>(&tbl, phase);
            for v in [sn, cs] {
                checked += 1;
                if v.abs() > limit {
                    over += 1;
                }
                worst = worst.max(v.abs());
            }
        }
        (checked, over, worst)
    }

    /// Worst in-band spur of any configuration's **widget** output, and
    /// the effective bits it implies.
    fn spur_and_enob<
        const TBL_W: usize,
        const FINE_W: usize,
        const TOTAL_W: usize,
        const AMP_W: usize,
        const INT_W: usize,
    >(
        word: u128,
    ) -> (f64, f64)
    where
        rhdl::bits::W<TBL_W>: BitWidth,
        rhdl::bits::W<FINE_W>: BitWidth,
        rhdl::bits::W<TOTAL_W>: BitWidth,
        rhdl::bits::W<AMP_W>: BitWidth,
        rhdl::bits::W<INT_W>: BitWidth,
    {
        use crate::dsp::nco::model::{blackman_harris, fft};
        const N: usize = 1 << 16;

        let full = 1u128 << TOTAL_W;
        let mut phase = 0u128;
        let phases: Vec<u128> = (0..N + 4)
            .map(|_| {
                let p = phase;
                phase = (phase + word) % full;
                p
            })
            .collect();

        let uut = SinCosLinearInterp::<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W>::default();
        let stream = phases
            .iter()
            .map(|p| In::<TOTAL_W> {
                phase: bits::<TOTAL_W>(*p),
            })
            .collect::<Vec<_>>()
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);
        let cos: Vec<i128> = uut
            .run(stream)
            .synchronous_sample()
            .map(|s| s.output.cos.raw())
            .skip(4)
            .take(N)
            .collect();
        assert_eq!(cos.len(), N);

        let win = blackman_harris(N);
        let mut re: Vec<f64> = cos.iter().zip(&win).map(|(c, w)| *c as f64 * w).collect();
        let mut im = vec![0.0f64; N];
        fft(&mut re, &mut im);
        let mag: Vec<f64> = (0..N / 2)
            .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt())
            .collect();
        let carrier = (1..N / 2)
            .max_by(|a, b| mag[*a].total_cmp(&mag[*b]))
            .unwrap();
        // Two different figures, and conflating them overstates the
        // result.  SFDR is the worst SINGLE spur; ENOB must come from
        // SINAD, the ratio of carrier power to ALL non-carrier power,
        // because a converter's effective bits are limited by total noise
        // and distortion rather than by its largest single line.
        //
        // Reporting `(SFDR - 1.76) / 6.02` as effective bits is wrong and
        // flattering: it ignores every spur but one.
        const GUARD: i64 = 8;
        let is_carrier = |k: usize| (k as i64 - carrier as i64).abs() <= GUARD;

        let worst = mag
            .iter()
            .enumerate()
            .take(N / 2)
            .skip(1)
            .filter(|(k, _)| !is_carrier(*k))
            .map(|(_, m)| *m)
            .fold(0.0f64, f64::max);
        let sfdr_dbc = 20.0 * (worst / mag[carrier]).log10();

        // Carrier power gathers the whole main lobe: a Blackman-Harris
        // window spreads it over several bins, and leaving them out of the
        // carrier puts them into the noise instead.
        let carrier_pow: f64 = (1..N / 2)
            .filter(|k| is_carrier(*k))
            .map(|k| mag[k] * mag[k])
            .sum();
        // DC excluded: it is the window's own leakage plus any residual
        // offset, not a property of the datapath under test.
        let noise_pow: f64 = (1..N / 2)
            .filter(|k| !is_carrier(*k))
            .map(|k| mag[k] * mag[k])
            .sum();
        let sinad_db = 10.0 * (carrier_pow / noise_pow).log10();
        (sfdr_dbc, (sinad_db - 1.76) / 6.02)
    }

    /// **The DSP48 claim, made checkable.**
    ///
    /// `AMP_W = 18` is chosen because it is the DSP48E1's native
    /// multiplier port width — but that reason lives in the *widths*, and
    /// what actually reaches the synthesiser is whatever multiply the
    /// kernel emits. Those were different things: the kernel used to
    /// resize both operands to `INT_W` before multiplying and emit a
    /// **48×48** signed multiply, which is six to nine DSP48E1 slices
    /// unless the synthesiser prunes it.
    ///
    /// Forming the product with `xmul` at natural width brings the
    /// variable × variable multiply to `AMP_W + FINE_W + 2` = **32×32**
    /// at the default configuration. This test asserts that, in the
    /// spirit of [`crate::dsp::mixer`]'s `multiplier_count_is_as_claimed`:
    /// a resource claim that cannot be tested is not a resource claim.
    ///
    /// # The emitted operand widths are now exact
    ///
    /// A DSP48E1 is **18×25**, and the operands here are genuinely
    /// `AMP_W` = 18 and `FINE_W + 2` = 14 bits — so the product fits one
    /// slice, and it is now *emitted* that way: `18 × 14`.
    ///
    /// Two changes were needed and neither suffices alone. This widget
    /// stopped resizing its operands to `INT_W` before multiplying, taking
    /// 48×48 down to 32×32 — that removed an explicit `resize` in the
    /// kernel. Then the compiler stopped having `XMul` pre-widen its
    /// operands to the result width in `lower_rhif_to_rtl`, taking 32×32
    /// down to 18×14 — that removed an implicit one in the lowering.
    ///
    /// So this asserts the **exact** widths rather than an upper bound,
    /// which is the strongest form of the claim available without
    /// instantiating a DSP48E1 directly — something RHDL still cannot do;
    /// see the note on DSP inference in the module docs.
    #[test]
    fn emitted_multiply_operands_are_natural_width() -> miette::Result<()> {
        let uut = SinCosLinearInterpDefault::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();

        // Map every declared signed register to its width.
        let mut width = std::collections::HashMap::new();
        for line in hdl.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("reg signed [") {
                if let Some((range, name)) = rest.split_once("] ") {
                    if let Ok(hi) = range.split(':').next().unwrap_or("").parse::<usize>() {
                        width.insert(name.trim_end_matches(';').to_string(), hi + 1);
                    }
                }
            }
        }

        let mut var_mults = Vec::new();
        for line in hdl.lines() {
            let t = line.trim().trim_end_matches(';');
            if let Some((lhs, rhs)) = t.split_once(" * ") {
                let a = lhs.split('=').nth(1).map(str::trim).unwrap_or("");
                let (wa, wb) = (width.get(a).copied(), width.get(rhs.trim()).copied());
                if let (Some(wa), Some(wb)) = (wa, wb) {
                    var_mults.push((wa, wb));
                }
            }
        }
        assert!(
            !var_mults.is_empty(),
            "found no register-by-register multiplies in the emitted \
             Verilog; the parser above has stopped matching:\n{hdl}"
        );
        // Two multiplies per component, and they are not equivalent:
        //
        //   1. `c0 * delta` -- variable x variable, at AMP_W x (FINE_W+2).
        //      This is the one that costs DSP slices.
        //   2. `* DELTA_K` -- variable x CONSTANT. A constant operand
        //      lowers to shift-adds rather than a slice, and its second
        //      operand is a localparam rather than a register, so it does
        //      not appear in `var_mults` at all.
        //
        // The DSP-relevant multiply must sit at exactly the operand widths.
        let want = (AMP_W, FINE_W + 2);
        assert!(
            var_mults.contains(&want),
            "expected a {}x{} multiply -- the natural operand widths, and a \
             single DSP48E1 port pair -- but the register-by-register \
             multiplies emitted were {var_mults:?}.\n\nTwo regressions look \
             like this.  {INT_W}x{INT_W} means this widget is resizing its \
             operands to INT_W before multiplying again.  {}x{} means the \
             compiler has gone back to pre-widening XMul's operands to the \
             result width in lower_rhif_to_rtl.",
            want.0,
            want.1,
            AMP_W + FINE_W + 2,
            AMP_W + FINE_W + 2
        );
        // Nothing is left at the old full intermediate width.
        let widest = var_mults.iter().map(|(a, b)| *a.max(b)).max().unwrap();
        assert!(
            widest < INT_W,
            "a multiply is still at INT_W ({INT_W}) or wider ({widest}), so \
             the narrowing did not take effect everywhere"
        );
        Ok(())
    }

    /// **The answer to "can we get true bitwidth > 18?"**
    ///
    /// Measures each validated configuration's worst in-band spur on the
    /// **widget's own output** and converts to effective bits. This is
    /// the test that distinguishes wider *wires* from more *precision* —
    /// `config`'s docs already record that raising `AMP_W` alone buys
    /// nothing, so a configuration that widened only the amplitude would
    /// pass a type check and fail here.
    ///
    /// The expected figures, and what they cost:
    ///
    /// Measured, on the widget:
    ///
    /// | config | TBL/FINE/TOTAL/AMP | table | SFDR | ENOB |
    /// |---|---|---|---|---|
    /// | [`SinCosLinearInterpDefault`] | 8/12/22/18 | 9 Kbit | −104.3 dBc | **17.50** |
    /// | [`SinCosLinearInterp24`] | 10/14/26/24 | 48 Kbit | −140.4 dBc | **23.05** |
    /// | [`SinCosLinearInterp28`] | 11/15/28/28 | 112 Kbit | −164.5 dBc | **27.02** |
    /// | [`SinCosLinearInterp32`] | 12/16/30/32 | 256 Kbit | −188.6 dBc | **31.03** |
    ///
    /// ENOB lands about one bit below `AMP_W` at every configuration,
    /// which says the datapath is **amplitude-quantisation limited** —
    /// the interpolation residual is no longer the bottleneck anywhere.
    /// That is the useful shape: the widths are the knob, and there is no
    /// hidden phase-domain ceiling being hit first.
    ///
    /// **`AMP_W = 18` is the DSP48's native multiplier port width**, so
    /// the default is the only configuration whose fine rotation fits one
    /// slice per multiply. The wider ones cascade, which is the real
    /// price of the extra bits and the reason all four are validated
    /// rather than the widest simply being adopted.
    ///
    /// Thresholds are set ~1.5 bits below the measured value so this
    /// catches a regression without being a measurement of FFT leakage.
    #[test]
    fn effective_bits_per_configuration() {
        const WORD: u128 = 1 << 19;

        let (d_dbc, d_enob) = spur_and_enob::<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W>(WORD);
        let (a_dbc, a_enob) = spur_and_enob::<10, 14, 26, 24, 56>(WORD);
        let (b_dbc, b_enob) = spur_and_enob::<11, 15, 28, 28, 64>(WORD);
        let (c_dbc, c_enob) = spur_and_enob::<12, 16, 30, 32, 72>(WORD);

        // Printed so the accuracy-versus-DSP-slices tradeoff is legible
        // from a test run rather than only from the docs.
        eprintln!("  config       SFDR dBc   ENOB (from SINAD)");
        eprintln!("  8/12/22/18  {d_dbc:9.1}   {d_enob:6.2}");
        eprintln!(" 10/14/26/24  {a_dbc:9.1}   {a_enob:6.2}");
        eprintln!(" 11/15/28/28  {b_dbc:9.1}   {b_enob:6.2}");
        eprintln!(" 12/16/30/32  {c_dbc:9.1}   {c_enob:6.2}");

        assert!(
            d_enob > 16.0,
            "default configuration gives {d_enob:.2} effective bits ({d_dbc:.1} dBc)"
        );
        // The headline claim: the wider configurations really do exceed 18
        // EFFECTIVE bits, not merely 18 wires.
        assert!(
            a_enob > 21.5,
            "10/14/26/24 gives only {a_enob:.2} effective bits ({a_dbc:.1} dBc); \
             it is supposed to be the smallest configuration that clears 18"
        );
        assert!(
            b_enob > 25.5,
            "11/15/28/28 gives only {b_enob:.2} effective bits ({b_dbc:.1} dBc)"
        );
        assert!(
            c_enob > 29.5,
            "12/16/30/32 gives only {c_enob:.2} effective bits ({c_dbc:.1} dBc)"
        );
        // Monotonic: each step up must actually buy something, or the
        // configuration is paying resources for nothing.
        assert!(
            a_enob > d_enob && b_enob > a_enob && c_enob > b_enob,
            "effective bits are not monotonic in the configuration: \
             {d_enob:.2} -> {a_enob:.2} -> {b_enob:.2} -> {c_enob:.2}"
        );
    }

    /// The table headroom holds at **every** validated configuration.
    ///
    /// The one-LSB margin in [`table_scale`] is the saturation logic, and
    /// a margin is only as good as the thing that checks it. Generic
    /// widths mean it has to be rechecked per configuration: nothing
    /// guarantees a priori that one LSB still suffices when `FINE_W`
    /// grows and the correction gets finer-grained.
    ///
    /// **Exhaustive at every configuration** — all `2^TOTAL_W` phases,
    /// about 1.4 billion in total across the four.
    ///
    /// Sampling was considered and rejected. The overshoot is a
    /// second-order effect concentrated at the four rails, so a sampled
    /// sweep has to *know* where to look, and this property is precisely
    /// the one whose violation is catastrophic and silent: a one-LSB
    /// excess becomes a full-scale sign inversion, recurring at a rate
    /// locked to the tuning word — a coherent spur, not noise. An earlier
    /// version of the Tier-2 test that omitted the rails passed with the
    /// saturation removed entirely.
    ///
    /// It is slow. That is the correct trade for the one property with no
    /// runtime guard behind it.
    #[test]
    fn headroom_holds_at_every_configuration() {
        let cases = [
            (
                "8/12/22/18",
                headroom_scan::<TBL_W, FINE_W, TOTAL_W, AMP_W>(true),
                (1i128 << (AMP_W - 1)) - 1,
            ),
            (
                "10/14/26/24",
                headroom_scan::<10, 14, 26, 24>(true),
                (1i128 << 23) - 1,
            ),
            (
                "11/15/28/28",
                headroom_scan::<11, 15, 28, 28>(true),
                (1i128 << 27) - 1,
            ),
            (
                "12/16/30/32",
                headroom_scan::<12, 16, 30, 32>(true),
                (1i128 << 31) - 1,
            ),
        ];
        for (name, (checked, over, worst), limit) in cases {
            assert_eq!(
                over, 0,
                "{name}: {over} of {checked} components leave the range; \
                 worst |value| {worst} against a limit of {limit}.  The table \
                 headroom is not sufficient at this configuration -- either \
                 widen it or reinstate a clamp."
            );
            assert!(
                worst > limit / 2,
                "{name}: worst |value| is only {worst} against a limit of \
                 {limit}, so the scan never approached the rails and proves \
                 nothing.  Check the sweep covers the peaks."
            );
        }
    }

    /// The bit-exact model describes the **widget** at every
    /// configuration.
    ///
    /// Without this, `headroom_holds_at_every_configuration` would prove
    /// a property of `model_pair_cfg` and say nothing about hardware —
    /// the substitution CLAUDE.md forbids. The default configuration is
    /// already covered by `model_agrees_with_the_widget`; this extends
    /// the guarantee to the three wider ones, which is where the model
    /// could plausibly diverge because the Q-point shift differs.
    #[test]
    fn model_agrees_with_the_widget_at_every_config() {
        fn check<
            const TBL_W: usize,
            const FINE_W: usize,
            const TOTAL_W: usize,
            const AMP_W: usize,
            const INT_W: usize,
        >(
            name: &str,
        ) where
            rhdl::bits::W<TBL_W>: BitWidth,
            rhdl::bits::W<FINE_W>: BitWidth,
            rhdl::bits::W<TOTAL_W>: BitWidth,
            rhdl::bits::W<AMP_W>: BitWidth,
            rhdl::bits::W<INT_W>: BitWidth,
        {
            let full = 1u128 << TOTAL_W;
            let stride = 7_919u128;
            let phases: Vec<u128> = (0..1024u128).map(|k| (k * stride) % full).collect();
            let uut = SinCosLinearInterp::<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W>::default();
            let out: Vec<(i128, i128)> = uut
                .run(
                    phases
                        .iter()
                        .map(|p| In::<TOTAL_W> {
                            phase: bits::<TOTAL_W>(*p),
                        })
                        .collect::<Vec<_>>()
                        .into_iter()
                        .with_reset(1)
                        .clock_pos_edge(100),
                )
                .synchronous_sample()
                .map(|s| (s.output.sin.raw(), s.output.cos.raw()))
                .collect();

            let tbl = model_table_cfg::<TBL_W, AMP_W>();
            let mut compared = 0usize;
            for (i, p) in phases.iter().enumerate().skip(4) {
                if i + 2 >= out.len() {
                    break;
                }
                assert_eq!(
                    model_pair_cfg::<TBL_W, FINE_W, TOTAL_W, AMP_W>(&tbl, *p),
                    out[i + 2],
                    "{name}: model and widget disagree at phase {p}"
                );
                compared += 1;
            }
            assert!(compared > 1000, "{name}: only {compared} samples compared");
        }
        check::<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W>("8/12/22/18");
        check::<10, 14, 26, 24, 56>("10/14/26/24");
        check::<11, 15, 28, 28, 64>("11/15/28/28");
        check::<12, 16, 30, 32, 72>("12/16/30/32");
    }

    /// Every validated configuration is synthesizable: **both**
    /// `iverilog` round trips at all four widths.
    ///
    /// A type alias that has never been through `iverilog` is a claim,
    /// not a configuration. This is also where an under-sized `INT_W`
    /// would surface as a Verilog/Rust divergence rather than as a
    /// plausible-looking number.
    #[test]
    fn every_configuration_round_trips_through_iverilog() -> miette::Result<()> {
        fn check<
            const TBL_W: usize,
            const FINE_W: usize,
            const TOTAL_W: usize,
            const AMP_W: usize,
            const INT_W: usize,
        >() -> miette::Result<()>
        where
            rhdl::bits::W<TBL_W>: BitWidth,
            rhdl::bits::W<FINE_W>: BitWidth,
            rhdl::bits::W<TOTAL_W>: BitWidth,
            rhdl::bits::W<AMP_W>: BitWidth,
            rhdl::bits::W<INT_W>: BitWidth,
        {
            let uut = SinCosLinearInterp::<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W>::default();
            let full = 1u128 << TOTAL_W;
            // Cover the rails, where the interpolated sum is largest, plus
            // a coprime spread.
            let mut phases: Vec<u128> = (0..4u128)
                .flat_map(|quad| {
                    let rail = quad * (full / 4);
                    (0..8u128).map(move |d| (rail + full - d - 1) % full)
                })
                .collect();
            phases.extend((0..32u128).map(|k| (k * 7_919) % full));
            let stream = phases
                .iter()
                .map(|p| In::<TOTAL_W> {
                    phase: bits::<TOTAL_W>(*p),
                })
                .collect::<Vec<_>>()
                .into_iter()
                .with_reset(1)
                .clock_pos_edge(100);
            let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
            // `.skip(2)` for the same reason as
            // `test_sin_cos_linear_interp_hdl_works`: a block RAM's output
            // register is `x` in Verilog until the first read completes,
            // while the Rust simulator reports the initial value
            // immediately.  Omitting it fails at time 0 with an all-`x`
            // expected value, which is the testbench working correctly.
            let opts = TestBenchOptions::default().skip(2);
            tb.rtl(&uut, &opts)?.run_iverilog()?;
            tb.ntl(&uut, &opts)?.run_iverilog()?;
            Ok(())
        }
        check::<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W>()?;
        check::<10, 14, 26, 24, 56>()?;
        check::<11, 15, 28, 28, 64>()?;
        check::<12, 16, 30, 32, 72>()?;
        Ok(())
    }

    /// `DELTA_K` is scale-invariant, which is the whole reason one
    /// constant serves every configuration.
    ///
    /// With the Q-point fixed at 32 the factor would be
    /// `round(2π·2^32 / 2^TOTAL_W)` — 6434 at `TOTAL_W = 22` but 402 at
    /// 26, losing precision exactly where a wider configuration is asking
    /// for more. With the Q-point at `TOTAL_W + Q_GUARD` the factor is
    /// `2π·2^Q_GUARD` regardless of `TOTAL_W`.
    #[test]
    fn delta_k_is_scale_invariant() {
        let exact = std::f64::consts::TAU * (1u64 << Q_GUARD) as f64;
        assert_eq!(
            DELTA_K,
            exact.round() as i128,
            "DELTA_K must be round(2^Q_GUARD * tau)"
        );
        let rel = (DELTA_K as f64 - exact).abs() / exact;
        assert!(
            rel < 1e-5,
            "DELTA_K relative error {rel:.2e} is too large; it must stay far \
             below the -116 dBc the architecture delivers"
        );
        // The relative error does not depend on the configuration, which
        // is the property the fixed-Q-point formulation lacked.
        for total_w in [22usize, 26, 28, 30] {
            let implied = DELTA_K as f64 / (1u64 << Q_GUARD) as f64 / (1u128 << total_w) as f64;
            let want = std::f64::consts::TAU / (1u128 << total_w) as f64;
            assert!(
                (implied - want).abs() / want < 1e-5,
                "at TOTAL_W={total_w} the implied radians-per-LSB is wrong"
            );
        }
    }

    /// `min_int_w` really is the minimum: the declared `INT_W` of every
    /// validated configuration is sufficient, and one bit less would not
    /// be.
    #[test]
    fn declared_int_w_is_sufficient_and_tight() {
        for (name, amp_w, fine_w, int_w) in [
            ("8/12/22/18", AMP_W, FINE_W, INT_W),
            ("10/14/26/24", 24, 14, 56),
            ("11/15/28/28", 28, 15, 64),
            ("12/16/30/32", 32, 16, 72),
        ] {
            let need = min_int_w(amp_w, fine_w);
            assert!(
                int_w >= need,
                "{name}: INT_W={int_w} is below the required {need}"
            );
            // Confirm the formula against the actual worst-case product,
            // so `min_int_w` is not merely self-consistent.
            let worst = (1i128 << (amp_w - 1)) * (1i128 << (fine_w - 1)) * DELTA_K;
            let bits = 128 - worst.leading_zeros() as usize + 1;
            assert!(
                need >= bits,
                "{name}: min_int_w says {need} but the worst-case product \
                 needs {bits} bits"
            );
        }
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
        let uut = SinCosLinearInterpDefault::default();
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
            181: endmodule
            182: module top_sin_tbl
            460: endmodule
            461: module top_cos_tbl
            739: endmodule
            740: module top_delayed
            755: endmodule"#]];
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
                     reg signed [12:0] r41;
                     reg [12:0] r42;
                     reg signed [13:0] r43;
                     reg signed [13:0] r44;
                     reg signed [13:0] r45;
                     reg signed [13:0] r46;
                     reg signed [13:0] r47;
                     reg signed [31:0] r48;
                     reg signed [31:0] r49;
                     reg signed [45:0] r50;
                     reg signed [47:0] r51;
                     reg signed [45:0] r52;
                     reg signed [47:0] r53;
                     reg signed [47:0] r54;
                     reg signed [17:0] r55;
                     reg signed [47:0] r56;
                     reg signed [17:0] r57;
                     reg signed [17:0] r58;
                     reg signed [17:0] r59;
                     reg [35:0] r60;
                     reg [35:0] r61;
                     reg [119:0] r62;
                     reg [1:0] r63;
                     reg [33:0] r64;
                     reg [41:0] r65;
                     reg signed [79:0] r66;
                     reg signed [79:0] r67;
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
                     localparam l12 = 14'sb01100100100010;
                     localparam l13 = 36'b000000000000000000000000000000000000;
                     localparam l14 = 14'sb00100000000000;
                     begin
                        r63 = arg_0;
                        r0 = arg_1;
                        r24 = arg_2;
                        r64 = {{12{1'b0}}, r0};
                        r1 = r64[33:12];
                        r2 = r1[7:0];
                        r65 = {{20{1'b0}}, r0};
                        r3 = r65[41:20];
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
                        r42 = {{1{1'b0}}, r40};
                        r41[12:0] = $signed(r42);
                        r44 = $signed({{1{r41[12]}}, r41});
                        r45 = l14;
                        r46[13:0] = $signed(r44);
                        r47[13:0] = $signed(r45);
                        r43 = r46 - r47;
                        r48 = r38 * r43;
                        r49 = r34 * r43;
                        r50 = r48 * l12;
                        r51 = $signed({{2{r50[45]}}, r50});
                        r52 = r49 * l12;
                        r53 = $signed({{2{r52[45]}}, r52});
                        r66 = $signed({{32{r51[47]}}, r51});
                        r54 = r66[79:32];
                        r55 = $signed(r54[17:0]);
                        r67 = $signed({{32{r53[47]}}, r53});
                        r56 = r67[79:32];
                        r57 = $signed(r56[17:0]);
                        r58 = r34 + r55;
                        r59 = r38 - r57;
                        r60 = l13;
                        r60[17:0] = r58;
                        r61 = r60;
                        r61[35:18] = r59;
                        r62 = {r22, r61};
                        kernel_sin_cos_linear_interp_kernel = r62;
                     end
               endfunction
            endmodule"#]];
        expect_top.assert_eq(&top);
        Ok(())
    }

    /// A short phase ramp, reused by Tiers 4 and 5 so the Verilog
    /// round-trip and the committed waveform describe the same stimulus.
    fn hdl_stimulus() -> impl Iterator<Item = In<TOTAL_W>> {
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
        let uut = SinCosLinearInterpDefault::default();
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
        let uut = SinCosLinearInterpDefault::default();
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

    /// Which narrowing rule harms the signal least?
    ///
    /// Narrows the 18-bit oscillator output to the 14-bit DAC four
    /// ways and reports the two things that matter separately: the
    /// worst *discrete* spur, and the broadband floor. They trade
    /// against each other, so a single number cannot answer it.
    #[test]
    #[ignore = "diagnostic"]
    fn scratch_narrowing_rules() {
        use crate::dsp::nco::model::{blackman_harris, fft};
        let tbl = model_table();
        const N: usize = 1 << 16;
        const OUT_W: usize = 14;
        const DROP: usize = AMP_W - OUT_W; // 4 bits
        let half = 1i128 << (DROP - 1);
        let win = blackman_harris(N);

        // Deterministic xorshift, so the dither row reproduces.
        let mut st: u32 = 0x1234_5678;
        let mut rnd = move || {
            st ^= st << 13;
            st ^= st >> 17;
            st ^= st << 5;
            (st & ((1 << DROP) - 1) as u32) as i128
        };

        println!("\n  narrowing to {OUT_W} bits, worst discrete spur / broadband floor / DC");
        println!(
            "  {:<18} {:>10} {:>12} {:>10}",
            "rule", "spur dBc", "floor dBc", "DC dBc"
        );

        for rule in ["truncate", "round-half-up", "convergent", "dither"] {
            let mut re = vec![0.0f64; N];
            let mut im = vec![0.0f64; N];
            let mut phase = 0u128;
            let word = 419_431u128; // odd, so truncation spurs are mild
            for k in 0..N {
                let (_s, c) = model_pair_raw(&tbl, phase);
                let narrowed = match rule {
                    "truncate" => c >> DROP,
                    "round-half-up" => (c + half) >> DROP,
                    "convergent" => {
                        let q = (c + half) >> DROP;
                        // ties (exact half) go to even
                        if (c - half) & ((1 << DROP) - 1) == 0 && q % 2 != 0 {
                            q - 1
                        } else {
                            q
                        }
                    }
                    _ => (c + rnd()) >> DROP,
                };
                re[k] = narrowed as f64 * win[k];
                phase = (phase + word) % (1u128 << TOTAL_W);
            }
            fft(&mut re, &mut im);
            let mag: Vec<f64> = (0..N / 2)
                .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt())
                .collect();
            let carrier = (1..N / 2)
                .max_by(|a, b| mag[*a].total_cmp(&mag[*b]))
                .unwrap();
            let c_mag = mag[carrier];

            let mut others: Vec<f64> = mag
                .iter()
                .enumerate()
                .filter(|(k, _)| *k > 0 && (*k as i64 - carrier as i64).abs() > 8)
                .map(|(_, m)| *m)
                .collect();
            let worst = others.iter().cloned().fold(0.0f64, f64::max);
            others.sort_by(f64::total_cmp);
            let floor = others[others.len() / 2];
            let dc = mag[0];

            println!(
                "  {:<18} {:>10.1} {:>12.1} {:>10.1}",
                rule,
                20.0 * (worst / c_mag).log10(),
                20.0 * (floor / c_mag).log10(),
                20.0 * (dc / c_mag).log10()
            );
        }
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
        quarter_table::<TBL_W, AMP_W>()
            .iter()
            .map(|(_, v)| v.raw())
            .collect()
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
        let t = quarter_table::<TBL_W, AMP_W>();
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
