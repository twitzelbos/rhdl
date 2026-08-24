#![warn(missing_docs)]
//! `Fir` — an arbitrary-tap FIR filter.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+Fir+--------------------+
      |                          |
+---->+ sample                   |
      |   Option<SignedBits<WI>> |
      |                   sample |
      |   Option<SignedBits<WO>> +----->
+---->+ downstream_ready         |
      |                saturated |
      |                          +----->
      +--------------------------+
")]
//!
//! The general case: any impulse response, of any length, symmetric or
//! not. One multiplier per tap.
//!
//! # When to use this and when not to
//!
//! [`super::SymmetricFir`] is the better choice whenever the taps
//! qualify — odd length and symmetric — because folding halves the
//! multipliers for identical arithmetic. A CIC compensator's taps
//! qualify by construction.
//!
//! Reach for `Fir` when they do not: a matched filter, a fractional
//! delay, a deliberately asymmetric equaliser, or a tap set arriving
//! from outside the library that you would rather not have to prove
//! symmetric. It costs `TAPS` multipliers instead of `TAPS/2 + 1` and
//! makes no claim about phase.
//!
//! # Internals
#![doc = badascii_doc::badascii!(r"
  x[n] -> +--+--+--+ ... +--+   delay line, TAPS deep
          |  |  |        |
          h0 h1 h2  ...  h(T-1)  one multiply per tap
          |  |  |        |
          +--+--+---sum--+ -> >>SHIFT -> saturate -> y[n]
")]
//!
//! # Same interface as the folded filter
//!
//! [`super::In`] and [`super::Out`], so the two are interchangeable.
//! Swapping a symmetric filter for a general one is a type change and
//! nothing else.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/fir.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/fir.md")]

use rhdl::prelude::*;

use super::{In, Out, accumulator_width_is_sufficient};
use crate::core::constant::Constant;
use crate::core::dff;
use crate::dsp::sign_extend;

/// An arbitrary-tap FIR filter.
///
/// `SHIFT` is the coefficients' fractional bit count, applied as a
/// right shift after accumulation.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct Fir<
    const W_IN: usize,
    const W_C: usize,
    const W_ACC: usize,
    const TAPS: usize,
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
    const SHIFT: usize,
    const W_OUT: usize,
> Fir<W_IN, W_C, W_ACC, TAPS, SHIFT, W_OUT>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_C>: BitWidth,
    rhdl::bits::W<W_ACC>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
{
    /// Build a filter from a tap set.
    ///
    /// `taps[0]` multiplies the newest sample, so the array is the
    /// impulse response in time order — which is what
    /// [`crate::dsp::cic::compensator`] and every filter-design table
    /// produce, and what the widget's own impulse-response test
    /// asserts.
    ///
    /// No symmetry requirement and no odd-length requirement: that is
    /// the whole difference from [`super::SymmetricFir`].
    pub fn new(taps: [SignedBits<W_C>; TAPS]) -> Self {
        assert!(TAPS >= 1, "a filter needs at least one tap");
        assert!(
            accumulator_width_is_sufficient(W_IN, W_C, TAPS, W_ACC),
            "W_ACC is too narrow for W_IN + W_C + ceil(log2(TAPS)) + 1"
        );
        assert!(W_OUT <= W_ACC, "W_OUT cannot exceed the accumulator");
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
    const SHIFT: usize,
    const W_OUT: usize,
> SynchronousIO for Fir<W_IN, W_C, W_ACC, TAPS, SHIFT, W_OUT>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_C>: BitWidth,
    rhdl::bits::W<W_ACC>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
{
    type I = In<W_IN>;
    type O = Out<W_OUT>;
    type Kernel = fir_kernel<W_IN, W_C, W_ACC, TAPS, SHIFT, W_OUT>;
}

#[kernel]
#[doc(hidden)]
// `acc = acc + x` rather than `acc += x`: compound assignment is not in
// the subset `#[kernel]` accepts.
#[allow(clippy::type_complexity, clippy::assign_op_pattern)]
pub fn fir_kernel<
    const W_IN: usize,
    const W_C: usize,
    const W_ACC: usize,
    const TAPS: usize,
    const SHIFT: usize,
    const W_OUT: usize,
>(
    cr: ClockReset,
    i: In<W_IN>,
    q: Q<W_IN, W_C, W_ACC, TAPS, SHIFT, W_OUT>,
) -> (Out<W_OUT>, D<W_IN, W_C, W_ACC, TAPS, SHIFT, W_OUT>)
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_C>: BitWidth,
    rhdl::bits::W<W_ACC>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
{
    let mut d = D::<W_IN, W_C, W_ACC, TAPS, SHIFT, W_OUT>::dont_care();
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

        // One multiply per tap. No folding: the taps are not assumed
        // symmetric, so no two of them share a multiplier.
        let mut acc = signed::<W_ACC>(0);
        for k in 0..TAPS {
            acc = acc + sign_extend::<W_IN, W_ACC>(line[k]) * sign_extend::<W_C, W_ACC>(q.coeff[k]);
        }

        let scaled = acc >> bits::<8>(SHIFT as u128);

        // Clamp rather than wrap: a filter with gain above one can
        // exceed the output range on signal that was already large,
        // and wrapping turns a large positive sample into a large
        // negative one.
        //
        // `lo` is built by subtraction rather than written as a
        // negative literal, which trips a known signedness defect in
        // the kernel compiler -- see `crate::dsp::sign_extend`.
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
    use super::super::{SymmetricFir, accumulator_width};
    use super::*;
    use expect_test::expect;

    const WI: usize = 18;
    const WC: usize = 12;
    const TAPS: usize = 6;
    const SHIFT: usize = 10;
    const WO: usize = 18;
    const WACC: usize = accumulator_width(WI, WC, TAPS);
    type Uut = Fir<WI, WC, WACC, TAPS, SHIFT, WO>;

    /// **Deliberately asymmetric and even-length** — the whole point of
    /// this widget is the tap sets `SymmetricFir` refuses.
    const T: [i128; TAPS] = [512, -300, 180, -90, 40, -10];

    fn taps() -> [SignedBits<WC>; TAPS] {
        let mut t = [SignedBits::<WC>::default(); TAPS];
        for (k, v) in T.iter().enumerate() {
            t[k] = signed::<WC>(*v);
        }
        t
    }

    fn uut() -> Uut {
        Uut::new(taps())
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

    /// Direct convolution, from the definition.
    fn model(x: &[i128], t: &[i128], shift: usize, w_out: usize) -> Vec<i128> {
        let hi = (1i128 << (w_out - 1)) - 1;
        let lo = -(1i128 << (w_out - 1));
        (0..x.len())
            .map(|k| {
                let acc: i128 = t
                    .iter()
                    .enumerate()
                    .map(|(j, tj)| if k >= j { x[k - j] * tj } else { 0 })
                    .sum();
                (acc >> shift).clamp(lo, hi)
            })
            .collect()
    }

    #[test]
    fn an_impulse_returns_the_taps_in_order() {
        // For an asymmetric filter this also pins the *direction*:
        // `taps[0]` multiplies the newest sample, so the impulse
        // response comes out in tap order. A reversed delay line would
        // pass a symmetric test and fail this one.
        let mut x = vec![0i128; 16];
        x[0] = 1 << SHIFT;
        let got = run(&x);
        assert_eq!(&got[..TAPS], &T[..], "impulse response must be the taps");
        assert!(got[TAPS..].iter().all(|v| *v == 0), "must be finite");
    }

    #[test]
    fn an_even_length_asymmetric_filter_is_accepted() {
        // `SymmetricFir::new` panics on both of these properties; this
        // widget exists to accept them.
        assert_eq!(TAPS % 2, 0, "the fixture must be even length");
        assert_ne!(T[0], T[TAPS - 1], "and asymmetric");
        let _ = uut();
    }

    #[test]
    fn matches_direct_convolution_bit_exactly() {
        let mut seed = 0x51ee_d001u64;
        let x: Vec<i128> = (0..300)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((seed >> 33) as i128 % 200_001) - 100_000
            })
            .collect();
        assert_eq!(run(&x), model(&x, &T, SHIFT, WO));
    }

    /// The two implementations must agree on taps they both accept.
    ///
    /// This is the load-bearing test for the shared interface: if the
    /// folded and unfolded datapaths disagree on a symmetric tap set,
    /// one of them is wrong, and swapping one for the other would
    /// silently change the filter.
    #[test]
    fn the_two_implementations_agree_where_they_overlap() {
        const ST: usize = 7;
        const SH: usize = 3;
        const SW: usize = accumulator_width(WI, WC, ST);
        let v = [-20i128, 60, 180, 584, 180, 60, -20];
        let mut t = [SignedBits::<WC>::default(); ST];
        for (k, x) in v.iter().enumerate() {
            t[k] = signed::<WC>(*x);
        }
        let x: Vec<i128> = (0..200)
            .map(|k: i128| (k * 5779) % 60_001 - 30_000)
            .collect();

        let general: Vec<i128> = Fir::<WI, WC, SW, ST, SHIFT, WO>::new(t)
            .run(feed(&x).into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.sample.map(|z| z.raw()))
            .collect();
        let folded: Vec<i128> = SymmetricFir::<WI, WC, SW, ST, SH, SHIFT, WO>::new(t)
            .run(feed(&x).into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.sample.map(|z| z.raw()))
            .collect();
        assert_eq!(general, folded, "folding must be an identity");
    }

    #[test]
    fn an_idle_cycle_holds_the_window() {
        let x: Vec<i128> = (0..12).map(|k: i128| (k * 977) % 4001 - 2000).collect();
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
        }
        let got: Vec<i128> = uut()
            .run(sparse.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.sample.map(|v| v.raw()))
            .collect();
        assert_eq!(dense, got);
    }

    #[test]
    fn saturation_clamps_and_reports() {
        let mut t = [SignedBits::<WC>::default(); TAPS];
        t[0] = signed::<WC>(2000); // gain ~1.95
        let uut = Fir::<WI, WC, WACC, TAPS, SHIFT, WO>::new(t);
        let fs = (1i128 << (WI - 1)) - 1;
        let out: Vec<(Option<i128>, bool)> = uut
            .run(
                feed(&[fs; 10])
                    .into_iter()
                    .with_reset(1)
                    .clock_pos_edge(100),
            )
            .synchronous_sample()
            .map(|s| (s.output.sample.map(|v| v.raw()), s.output.saturated))
            .collect();
        let hi = (1i128 << (WO - 1)) - 1;
        assert!(out.iter().any(|(v, _)| *v == Some(hi)), "must clamp");
        assert!(out.iter().any(|(_, sat)| *sat), "and report it");
    }

    #[test]
    #[should_panic(expected = "W_ACC is too narrow")]
    fn a_narrow_accumulator_is_rejected() {
        let _ = Fir::<WI, WC, 20, TAPS, SHIFT, WO>::new(taps());
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
        // One multiply per tap: no folding, unlike SymmetricFir.
        let muls = hdl.matches('*').count();
        assert_eq!(muls, TAPS, "expected {TAPS} multiplies, saw {muls}");
        Ok(())
    }

    #[test]
    fn test_fir_hdl_works() -> miette::Result<()> {
        let uut = uut();
        let tb = uut
            .run(hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100))
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_fir_trace() -> miette::Result<()> {
        let uut = uut();
        let vcd = uut
            .run(hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100))
            .collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("fir");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["c10b114411f0bc83a2f44e9b2e0a3f023e8f149bdf15796eaf2e8a34339cb63c"];
        let digest = vcd.dump_to_file(root.join("fir.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
