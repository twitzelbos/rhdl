#![warn(missing_docs)]
//! `SymmetricFir` — a linear-phase FIR at a decimated rate.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+SymmetricFir+-----------+
      |                          |
+---->+ sample                   |
      |   Option<SignedBits<WI>> |
      |                   sample |
      |   Option<SignedBits<WO>> +----->
+---->+ downstream_ready         |
      |                 saturated|
      |                          +----->
      +--------------------------+
")]
//!
//! Built to sit behind [`crate::dsp::cic::CicDecimate`] and undo its
//! passband droop, which is what [`crate::dsp::cic::compensator`]
//! designs the taps for. Nothing here is CIC-specific though — it
//! executes whatever symmetric tap set it is given.
//!
//! # Internals
#![doc = badascii_doc::badascii!(r"
  x[n] +--+--+--+--+--+--+--+  delay line, TAPS deep
       |  |  |  |  |  |  |  |
       +--|--|--|--|--|--|--+  fold: pairs equidistant from the centre
          +--|--|--|--|--+     are added before the multiply
             +--|--|--+
                +--+
                 |
              h[0..HALF]  HALF+1 multiplies, not TAPS
                 |
                sum -> >>SHIFT -> saturate -> y[n]
")]
//!
//! # Why the taps are folded
//!
//! A linear-phase FIR has `h[k] == h[L-1-k]`, so the two samples that
//! meet a shared coefficient can be added *before* the multiply. That
//! is `(L+1)/2` multipliers instead of `L` — for the fifteen-tap
//! compensator, eight instead of fifteen — with identical arithmetic
//! results, not an approximation.
//!
//! # Why the adder tree is not pipelined
//!
//! [`crate::dsp::cic::CicDecimate`]'s integrator cascade *is*
//! pipelined, because it runs at the full converter rate and its depth
//! set fmax. This filter runs after decimation, so its timing budget
//! is `R` times larger — at `R = 32` there are 32 converter clocks
//! between output samples for a path that is one multiply and a
//! `log2(TAPS)`-deep add.
//!
//! That is a deliberate reading of where the budget is, not an
//! oversight, and it has a limit: at small `R`, long tap sets, or a
//! high converter rate it stops being true. When RHDL's auto-pipelining
//! lands (see `auto-pipelining-plan.md`) this is a natural cut point.
//!
//! # Saturation, not wrapping
//!
//! A compensator has gain above one — the whole point is to lift the
//! band edge back up — so its output can exceed the input's range on
//! signal that was already near full scale. Wrapping there turns a
//! large positive sample into a large negative one, which in a
//! phase-sensitive receiver is not a small error but a sign flip.
//! The output clamps instead, and reports that it did.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/symmetric_fir.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/symmetric_fir.md")]

use rhdl::prelude::*;

use super::{In, Out, accumulator_width_is_sufficient};
use crate::core::constant::Constant;
use crate::core::dff;
use crate::dsp::sign_extend;

/// A symmetric (linear-phase) FIR filter.
///
/// `TAPS` must be odd and `HALF` must be `TAPS / 2` — Rust cannot
/// derive the second from the first without `generic_const_exprs`, so
/// it is passed and checked. `SHIFT` is the coefficients' fractional
/// bit count, applied as a right shift after accumulation.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct SymmetricFir<
    const W_IN: usize,
    const W_C: usize,
    const W_ACC: usize,
    const TAPS: usize,
    const HALF: usize,
    const SHIFT: usize,
    const W_OUT: usize,
> where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_C>: BitWidth,
    rhdl::bits::W<W_ACC>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
{
    /// The tapped delay line, newest at index zero.
    line: dff::DFF<[SignedBits<W_IN>; TAPS]>,
    /// The coefficients, held as a constant driver.
    coeff: Constant<[SignedBits<W_C>; TAPS]>,
    /// The registered result.
    out: dff::DFF<Option<SignedBits<W_OUT>>>,
}

impl<
    const W_IN: usize,
    const W_C: usize,
    const W_ACC: usize,
    const TAPS: usize,
    const HALF: usize,
    const SHIFT: usize,
    const W_OUT: usize,
> SymmetricFir<W_IN, W_C, W_ACC, TAPS, HALF, SHIFT, W_OUT>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_C>: BitWidth,
    rhdl::bits::W<W_ACC>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
{
    /// Build a filter from a symmetric tap set.
    ///
    /// Symmetry is *checked*, not assumed: the folded datapath computes
    /// a different filter than the taps describe if they are not
    /// symmetric, and it would do so quietly.
    pub fn new(taps: [SignedBits<W_C>; TAPS]) -> Self {
        assert!(TAPS % 2 == 1, "TAPS must be odd so there is a centre tap");
        assert_eq!(HALF, TAPS / 2, "HALF must be TAPS / 2");
        assert!(
            accumulator_width_is_sufficient(W_IN, W_C, TAPS, W_ACC),
            "W_ACC is too narrow for W_IN + W_C + ceil(log2(TAPS)) + 1"
        );
        assert!(W_OUT <= W_ACC, "W_OUT cannot exceed the accumulator");
        for k in 0..TAPS {
            assert_eq!(
                taps[k],
                taps[TAPS - 1 - k],
                "taps must be symmetric: index {k} and {} differ",
                TAPS - 1 - k
            );
        }
        Self {
            line: dff::DFF::new([SignedBits::<W_IN>::default(); TAPS]),
            coeff: Constant::new(taps),
            out: dff::DFF::new(None),
        }
    }
}

impl<
    const W_IN: usize,
    const W_C: usize,
    const W_ACC: usize,
    const TAPS: usize,
    const HALF: usize,
    const SHIFT: usize,
    const W_OUT: usize,
> SynchronousIO for SymmetricFir<W_IN, W_C, W_ACC, TAPS, HALF, SHIFT, W_OUT>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_C>: BitWidth,
    rhdl::bits::W<W_ACC>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
{
    type I = In<W_IN>;
    type O = Out<W_OUT>;
    type Kernel = symmetric_fir_kernel<W_IN, W_C, W_ACC, TAPS, HALF, SHIFT, W_OUT>;
}

#[kernel]
#[doc(hidden)]
// `acc = acc + x` rather than `acc += x`: compound assignment is not in
// the subset `#[kernel]` accepts. Same idiom as `serial_bus::hd44780`.
#[allow(clippy::type_complexity, clippy::assign_op_pattern)]
pub fn symmetric_fir_kernel<
    const W_IN: usize,
    const W_C: usize,
    const W_ACC: usize,
    const TAPS: usize,
    const HALF: usize,
    const SHIFT: usize,
    const W_OUT: usize,
>(
    cr: ClockReset,
    i: In<W_IN>,
    q: Q<W_IN, W_C, W_ACC, TAPS, HALF, SHIFT, W_OUT>,
) -> (Out<W_OUT>, D<W_IN, W_C, W_ACC, TAPS, HALF, SHIFT, W_OUT>)
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_C>: BitWidth,
    rhdl::bits::W<W_ACC>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
{
    let mut d = D::<W_IN, W_C, W_ACC, TAPS, HALF, SHIFT, W_OUT>::dont_care();
    d.line = q.line;
    d.coeff = ();
    d.out = None;

    let mut saturated = false;

    if let Some(s) = i.sample {
        // Shift the window, newest at index zero.
        let mut line = q.line;
        for k in 0..TAPS {
            let idx = TAPS - 1 - k;
            line[idx] = if idx == 0 { s } else { q.line[idx - 1] };
        }
        d.line = line;

        // Fold, then multiply. `h[k] == h[TAPS-1-k]`, so the pair can
        // share one multiplier. Checked in `new`, because a
        // non-symmetric tap set would compute a different filter here
        // without complaining.
        let mut acc = signed::<W_ACC>(0);
        for k in 0..HALF {
            let a = sign_extend::<W_IN, W_ACC>(line[k])
                + sign_extend::<W_IN, W_ACC>(line[TAPS - 1 - k]);
            acc = acc + a * sign_extend::<W_C, W_ACC>(q.coeff[k]);
        }
        // The centre tap has no partner.
        acc =
            acc + sign_extend::<W_IN, W_ACC>(line[HALF]) * sign_extend::<W_C, W_ACC>(q.coeff[HALF]);

        // Drop the coefficients' fractional bits.
        let scaled = acc >> bits::<8>(SHIFT as u128);

        // Clamp rather than wrap -- see the module docs.
        // `lo` is built by subtraction rather than written as a
        // negative literal. A negative constant in a kernel trips the
        // known signedness defect documented on `crate::dsp::sign_extend`
        // -- `descriptor()` rejects it with "cannot negate unsigned
        // value" -- and subtraction reaches the same value without
        // asking the compiler to negate anything.
        let hi = signed::<W_ACC>((1 << (W_OUT - 1)) - 1);
        let lo = signed::<W_ACC>(0) - hi - signed::<W_ACC>(1);
        let mut clamped = scaled;
        if scaled > hi {
            clamped = hi;
            saturated = true;
        }
        if scaled < lo {
            clamped = lo;
            saturated = true;
        }
        d.out = Some(clamped.resize::<W_OUT>());
    }

    let mut o = Out::<W_OUT> {
        sample: q.out,
        saturated,
        overrun: !i.downstream_ready,
    };

    if cr.reset.any() {
        d.line = [signed::<W_IN>(0); TAPS];
        d.out = None;
        o.saturated = false;
        o.overrun = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::super::accumulator_width;
    use super::*;
    use expect_test::expect;

    const WI: usize = 18;
    const WC: usize = 12;
    const TAPS: usize = 7;
    const HALF: usize = 3;
    const SHIFT: usize = 10;
    const WO: usize = 18;
    const WACC: usize = accumulator_width(WI, WC, TAPS);
    type Uut = SymmetricFir<WI, WC, WACC, TAPS, HALF, SHIFT, WO>;

    /// A symmetric tap set summing to 2^SHIFT, so DC gain is one.
    fn taps() -> [SignedBits<WC>; TAPS] {
        // -20, 60, 180, 584, 180, 60, -20  =  1024
        let v = [-20i128, 60, 180, 584, 180, 60, -20];
        let mut t = [SignedBits::<WC>::default(); TAPS];
        for (k, x) in v.iter().enumerate() {
            t[k] = signed::<WC>(*x);
        }
        t
    }

    fn uut() -> Uut {
        Uut::new(taps())
    }

    /// Direct convolution, written from the definition rather than
    /// from the widget: no folding, no shifting tricks.
    ///
    /// The widget folds symmetric pairs before multiplying; this does
    /// not. They must agree bit for bit, which is what makes the fold
    /// an identity rather than an approximation.
    fn model(x: &[i128], t: &[i128], shift: usize, w_out: usize) -> Vec<i128> {
        let hi = (1i128 << (w_out - 1)) - 1;
        let lo = -(1i128 << (w_out - 1));
        let mut out = Vec::new();
        for k in 0..x.len() {
            let mut acc = 0i128;
            for (j, tj) in t.iter().enumerate() {
                // Sample k-j, zero before the start.
                let s = if k >= j { x[k - j] } else { 0 };
                acc += s * tj;
            }
            // Arithmetic shift, then clamp.
            let y = acc >> shift;
            out.push(y.clamp(lo, hi));
        }
        out
    }

    fn feed(x: &[i128]) -> Vec<In<WI>> {
        let mut v: Vec<In<WI>> = x
            .iter()
            .map(|s| In::<WI> {
                sample: Some(signed::<WI>(*s)),
                downstream_ready: true,
            })
            .collect();
        v.push(In::<WI> {
            sample: None,
            downstream_ready: true,
        });
        v
    }

    fn run(x: &[i128]) -> Vec<i128> {
        uut()
            .run(feed(x).into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.sample.map(|v| v.raw()))
            .collect()
    }

    // ---- Tier 1 / 2 ------------------------------------------------

    #[test]
    fn an_impulse_returns_the_taps() {
        // The defining property of an FIR: its impulse response *is*
        // its coefficients. A folding error shows up here immediately
        // and asymmetrically.
        let mut x = vec![0i128; 16];
        x[0] = 1 << SHIFT; // unit impulse at the coefficients' scale
        let got = run(&x);
        let want: Vec<i128> = [-20i128, 60, 180, 584, 180, 60, -20].to_vec();
        assert_eq!(&got[..TAPS], &want[..], "impulse response must be the taps");
        assert!(
            got[TAPS..].iter().all(|v| *v == 0),
            "an FIR must be finite: {:?}",
            &got[TAPS..]
        );
    }

    #[test]
    fn dc_gain_is_the_tap_sum() {
        let got = run(&vec![1000i128; 24]);
        // Taps sum to 2^SHIFT, so the settled output is the input.
        assert_eq!(*got.last().unwrap(), 1000, "unity DC gain by construction");
    }

    #[test]
    fn matches_direct_convolution_bit_exactly() {
        let t = [-20i128, 60, 180, 584, 180, 60, -20];
        let mut seed = 0xa5a5_1234u64;
        let x: Vec<i128> = (0..200)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((seed >> 33) as i128 % 200_001) - 100_000
            })
            .collect();
        assert_eq!(run(&x), model(&x, &t, SHIFT, WO), "fold must be exact");
    }

    #[test]
    fn an_idle_cycle_holds_the_window() {
        // A gap must not be read as a zero sample -- that would be a
        // different filter. Interleaving idles must not change the
        // output sequence at all.
        let x: Vec<i128> = (0..12).map(|k| (k * 977) % 4001 - 2000).collect();
        let dense = run(&x);
        let mut sparse: Vec<In<WI>> = Vec::new();
        for s in &x {
            sparse.push(In::<WI> {
                sample: Some(signed::<WI>(*s)),
                downstream_ready: true,
            });
            sparse.push(In::<WI> {
                sample: None,
                downstream_ready: true,
            });
            sparse.push(In::<WI> {
                sample: None,
                downstream_ready: true,
            });
        }
        let got: Vec<i128> = uut()
            .run(sparse.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.sample.map(|v| v.raw()))
            .collect();
        assert_eq!(dense, got, "idle cycles must not advance the filter");
    }

    #[test]
    fn saturation_clamps_and_reports() {
        // A tap set with gain far above one, driven near full scale.
        let big = {
            let v = [0i128, 0, 0, 2000, 0, 0, 0]; // gain ~1.95
            let mut t = [SignedBits::<WC>::default(); TAPS];
            for (k, x) in v.iter().enumerate() {
                t[k] = signed::<WC>(*x);
            }
            t
        };
        let uut = SymmetricFir::<WI, WC, WACC, TAPS, HALF, SHIFT, WO>::new(big);
        let fs = (1i128 << (WI - 1)) - 1;
        let out: Vec<(Option<i128>, bool)> = uut
            .run(
                feed(&[fs; 12])
                    .into_iter()
                    .with_reset(1)
                    .clock_pos_edge(100),
            )
            .synchronous_sample()
            .map(|s| (s.output.sample.map(|v| v.raw()), s.output.saturated))
            .collect();
        let hi = (1i128 << (WO - 1)) - 1;
        assert!(
            out.iter().any(|(v, _)| *v == Some(hi)),
            "the output must clamp at the positive limit"
        );
        assert!(out.iter().any(|(_, sat)| *sat), "and say that it clamped");
    }

    #[test]
    fn reset_clears_the_window() {
        let x = [50_000i128; 8];
        let out: Vec<Option<i128>> = uut()
            .run(feed(&x).into_iter().with_reset(4).clock_pos_edge(100))
            .synchronous_sample()
            .take(4)
            .map(|s| s.output.sample.map(|v| v.raw()))
            .collect();
        assert!(
            out.iter().all(|v| v.is_none()),
            "no output while reset is asserted: {out:?}"
        );
    }

    #[test]
    #[should_panic(expected = "taps must be symmetric")]
    fn asymmetric_taps_are_rejected() {
        // The folded datapath would silently compute a different
        // filter, so this is checked rather than documented.
        let mut t = taps();
        t[0] = signed::<WC>(999);
        let _ = Uut::new(t);
    }

    #[test]
    #[should_panic(expected = "TAPS must be odd")]
    fn an_even_tap_count_is_rejected() {
        let t = [SignedBits::<WC>::default(); 6];
        let _ = SymmetricFir::<WI, WC, WACC, 6, 3, SHIFT, WO>::new(t);
    }

    #[test]
    #[should_panic(expected = "W_ACC is too narrow")]
    fn a_narrow_accumulator_is_rejected() {
        let _ = SymmetricFir::<WI, WC, 20, TAPS, HALF, SHIFT, WO>::new(taps());
    }

    // ---- Tier 3 / 4 / 5 --------------------------------------------

    fn hdl_stimulus() -> Vec<In<WI>> {
        let mut v = feed(
            &(0..24)
                .map(|k: i128| (k * 7919) % 30011 - 15000)
                .collect::<Vec<_>>(),
        );
        v[5].downstream_ready = false;
        v
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let hdl = uut().descriptor("top".into())?.hdl()?.modules.pretty();
        // Folding is the claim; the emitted module must contain HALF+1
        // multiplies, not TAPS.
        let muls = hdl.matches('*').count();
        assert_eq!(
            muls,
            HALF + 1,
            "expected {} multiplies, saw {muls}",
            HALF + 1
        );
        Ok(())
    }

    #[test]
    fn test_symmetric_fir_hdl_works() -> miette::Result<()> {
        let uut = uut();
        let tb = uut
            .run(hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100))
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_symmetric_fir_trace() -> miette::Result<()> {
        let uut = uut();
        let vcd = uut
            .run(hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100))
            .collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("symmetric_fir");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["93ba3ca4eca42dbba11470275f82f2f1ee49e95c6720a38be772a9db1203352a"];
        let digest = vcd.dump_to_file(root.join("symmetric_fir.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
