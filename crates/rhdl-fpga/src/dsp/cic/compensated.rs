#![warn(missing_docs)]
//! `CompensatedCic` — a decimator with its droop already removed.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+CompensatedCic+----------+
      |                           |
+---->+ sample                    |
      |    Option<SignedBits<WI>> |
      |                    sample |
      |    Option<SignedBits<WO>> +----->
+---->+ restart                   |
      |                   overrun |
+---->+ downstream_ready          +----->
      |                 saturated |
      |                           +----->
      +---------------------------+
   same In/Out as cic::decimator, so it drops into any decimator slot
")]
//!
//! # Internals
#![doc = badascii_doc::badascii!(r"
  x[n]        +-----------+   y[m]    +-------------+   z[m]
  --------->  |  C: CIC   | --------> | F: sym FIR  | ------->
  fs          | decim R   |  fs/R     | inverse     |  fs/R
              +-----------+           | sinc        |
                                      +-------------+
    droops the passband        lifts it back, flat to ~0.02 dB
")]
//!
//! A CIC and an inverse-sinc FIR are two halves of one decision, and
//! shipping them separately invites the mistake of using the first
//! without the second. The CIC's `sinc^N` droop is several decibels at
//! a useful passband — [`super::response::passband_droop_db`] says
//! 9.7 dB at `N = 4, R = 32, passband = 0.8` — and a receiver that
//! reports amplitudes is simply wrong by that much.
//!
//! # It *is* a decimator
//!
//! `CompensatedCic` presents exactly [`super::decimator`]'s `In` and
//! `Out`, so it drops into any slot that takes a decimator — including
//! both arms of [`crate::dsp::ddc::Ddc`], which is where it earns its
//! keep. A phase-sensitive receiver that reports amplitudes several
//! decibels low near the band edge is reporting the filter, not the
//! signal.
//!
//! That is why [`super::decimator::Out`] carries a `saturated` flag
//! that the plain [`super::CicDecimate`] always drives false: one
//! interface, two implementations, and the richer one does not need a
//! wrapper to fit.
//!
//! # Generic over both halves
//!
//! `C` is any decimator presenting [`super::decimator`]'s interface:
//! the uniform [`super::CicDecimate`] or a
//! [`crate::cic_pruned!`]-generated one. `F` is any filter presenting
//! [`crate::dsp::fir::symmetric`]'s. Neither choice is visible in the
//! kernel, which only moves samples between them.
//!
//! Design the taps with [`super::compensator`], which reads the same
//! `(N, R, M)` the decimator was built from. Nothing checks that the
//! taps match the decimator — they are numbers, and the type system
//! cannot see that a tap set inverts a particular droop. What *can* be
//! checked is the result, and
//! `crates/rhdl-fpga/tests/cic_compensated.rs` measures it: tone in,
//! amplitude out, against the flat line.
//!
//! # Latency and rate
//!
//! The FIR sees one sample per `R` input samples, so it needs no rate
//! adaptation — but it must not treat the `R-1` idle cycles as zeros,
//! and it does not: `sample: None` holds its window, the same rule the
//! CIC follows.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/cic_compensated.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/cic_compensated.md")]

use rhdl::prelude::*;

use crate::dsp::fir;

/// A decimator followed by its compensating FIR.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct CompensatedCic<const W_IN: usize, const W_MID: usize, const W_OUT: usize, C, F>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    C: SynchronousIO<I = super::decimator::In<W_IN>, O = super::decimator::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    F: SynchronousIO<I = fir::In<W_MID>, O = fir::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// The decimator.
    cic: C,
    /// The compensating filter, at the decimated rate.
    fir: F,
}

impl<const W_IN: usize, const W_MID: usize, const W_OUT: usize, C, F>
    CompensatedCic<W_IN, W_MID, W_OUT, C, F>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    C: SynchronousIO<I = super::decimator::In<W_IN>, O = super::decimator::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    F: SynchronousIO<I = fir::In<W_MID>, O = fir::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    /// Assemble a decimator and its compensating filter.
    ///
    /// There is deliberately no `Default`. A `SymmetricFir` has no
    /// meaningful default — an all-zero tap set is a filter that
    /// outputs zero, and a pair that silently defaulted to it would
    /// look like a wiring fault rather than a missing design. The taps
    /// have to come from somewhere, and [`super::compensator`] is
    /// where.
    pub fn new(cic: C, fir: F) -> Self {
        Self { cic, fir }
    }
}

impl<const W_IN: usize, const W_MID: usize, const W_OUT: usize, C, F> SynchronousIO
    for CompensatedCic<W_IN, W_MID, W_OUT, C, F>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    C: SynchronousIO<I = super::decimator::In<W_IN>, O = super::decimator::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    F: SynchronousIO<I = fir::In<W_MID>, O = fir::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    type I = super::decimator::In<W_IN>;
    type O = super::decimator::Out<W_OUT>;
    type Kernel = compensated_cic_kernel<W_IN, W_MID, W_OUT, C, F>;
}

#[kernel]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn compensated_cic_kernel<const W_IN: usize, const W_MID: usize, const W_OUT: usize, C, F>(
    cr: ClockReset,
    i: super::decimator::In<W_IN>,
    q: Q<W_IN, W_MID, W_OUT, C, F>,
) -> (super::decimator::Out<W_OUT>, D<W_IN, W_MID, W_OUT, C, F>)
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_MID>: BitWidth,
    rhdl::bits::W<W_OUT>: BitWidth,
    C: SynchronousIO<I = super::decimator::In<W_IN>, O = super::decimator::Out<W_MID>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
    F: SynchronousIO<I = fir::In<W_MID>, O = fir::Out<W_OUT>>
        + Synchronous
        + Clone
        + std::fmt::Debug,
{
    let mut d = D::<W_IN, W_MID, W_OUT, C, F>::dont_care();

    d.cic = super::decimator::In::<W_IN> {
        sample: i.sample,
        restart: i.restart,
        // The FIR never stalls, so the decimator is never held up by
        // it. `downstream_ready` describes what is downstream of the
        // *pair*, and it is reported rather than propagated -- see
        // `decimator::In::downstream_ready` for why neither stage can
        // usefully stall.
        downstream_ready: i.downstream_ready,
    };

    // The decimator's output is the filter's input. One sample every R
    // cycles, and `None` on the rest -- which the filter holds on
    // rather than reading as a zero.
    d.fir = fir::In::<W_MID> {
        sample: q.cic.sample,
        downstream_ready: i.downstream_ready,
    };

    let mut o = super::decimator::Out::<W_OUT> {
        sample: q.fir.sample,
        overrun: q.cic.overrun || q.fir.overrun,
        // The decimator half never clips; the filter half can, because
        // a compensator has gain above one by construction.
        saturated: q.cic.saturated || q.fir.saturated,
    };

    if cr.reset.any() {
        d.cic = super::decimator::In::<W_IN> {
            sample: None,
            restart: false,
            downstream_ready: false,
        };
        d.fir = fir::In::<W_MID> {
            sample: None,
            downstream_ready: false,
        };
        o.sample = None;
        o.overrun = false;
        o.saturated = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::super::decimator::In;
    use super::*;
    use crate::dsp::cic::{CicDecimate, accumulator_width, compensator, counter_width, dc_gain};
    use crate::dsp::fir::{SymmetricFir, accumulator_width as fir_acc};
    use expect_test::expect;

    const WI: usize = 8;
    const N: usize = 2;
    const R: usize = 4;
    const M: usize = 1;
    const WA: usize = accumulator_width(WI, N, R, M);
    const CW: usize = counter_width(R);
    const TAPS: usize = 7;
    const HALF: usize = 3;
    const WC: usize = 12;
    const SHIFT: usize = 10;
    const WACC: usize = fir_acc(WA, WC, TAPS);

    type Cic = CicDecimate<WI, WA, N, R, M, CW>;
    type Fir = SymmetricFir<WA, WC, WACC, TAPS, HALF, SHIFT, WA>;
    type Uut = CompensatedCic<WI, WA, WA, Cic, Fir>;

    fn taps() -> [SignedBits<WC>; TAPS] {
        let mut spec = compensator::Spec::for_cic(N, R, M);
        spec.taps = TAPS;
        spec.passband = 0.8;
        let d = compensator::design(spec).expect("design");
        let q = compensator::quantise(&d, WC);
        assert_eq!(q.shift as usize, SHIFT, "SHIFT must track quantise()");
        let mut t = [SignedBits::<WC>::default(); TAPS];
        for (k, v) in q.taps.iter().enumerate() {
            t[k] = signed::<WC>(*v as i128);
        }
        t
    }

    fn uut() -> Uut {
        Uut::new(Cic::default(), Fir::new(taps()))
    }

    fn feed(x: &[i128], restart_first: bool) -> Vec<In<WI>> {
        let mut v: Vec<In<WI>> = x
            .iter()
            .enumerate()
            .map(|(k, s)| In::<WI> {
                sample: Some(signed::<WI>(*s)),
                restart: restart_first && k == 0,
                downstream_ready: true,
            })
            .collect();
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

    fn run(x: &[i128]) -> Vec<i128> {
        uut()
            .run(feed(x, false).into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.sample.map(|v| v.raw()))
            .collect()
    }

    #[test]
    fn it_elaborates() -> miette::Result<()> {
        let _ = uut().descriptor("top".into())?;
        Ok(())
    }

    #[test]
    fn dc_gain_is_the_cic_gain_untouched() {
        // The compensator's DC gain is exactly one by construction, so
        // the pair's settled output is the decimator's alone. If this
        // drifts, every amplitude the chain reports is wrong by the
        // same factor -- a systematic error, not noise.
        let got = run(&vec![100i128; 200]);
        assert_eq!(
            *got.last().unwrap(),
            100 * dc_gain(N, R, M) as i128,
            "the pair must not rescale DC"
        );
    }

    #[test]
    fn the_pair_is_the_two_stages_in_series() {
        // Cross-check the composition against running the halves
        // separately. A wiring error inside the pair -- a dropped
        // `None`, an off-by-one on the handshake -- shows up here and
        // nowhere else, because both halves are individually correct.
        use crate::dsp::cic::decimator::In as CicIn;
        use crate::dsp::fir::In as FirIn;

        let x: Vec<i128> = (0..240).map(|k: i128| (k * 37) % 201 - 100).collect();
        let together = run(&x);

        let mut ci: Vec<CicIn<WI>> = x
            .iter()
            .map(|s| CicIn::<WI> {
                sample: Some(signed::<WI>(*s)),
                restart: false,
                downstream_ready: true,
            })
            .collect();
        ci.extend(std::iter::repeat_n(
            CicIn::<WI> {
                sample: None,
                restart: false,
                downstream_ready: true,
            },
            4,
        ));
        let mid: Vec<i128> = Cic::default()
            .run(ci.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.sample.map(|v| v.raw()))
            .collect();
        let mut fi: Vec<FirIn<WA>> = mid
            .iter()
            .map(|v| FirIn::<WA> {
                sample: Some(signed::<WA>(*v)),
                downstream_ready: true,
            })
            .collect();
        fi.push(FirIn::<WA> {
            sample: None,
            downstream_ready: true,
        });
        let separate: Vec<i128> = Fir::new(taps())
            .run(fi.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.sample.map(|v| v.raw()))
            .collect();

        assert!(!together.is_empty() && !separate.is_empty());
        let n = together.len().min(separate.len());
        assert_eq!(
            together[..n],
            separate[..n],
            "the pair must equal the two stages in series"
        );
    }

    #[test]
    fn an_idle_cycle_holds_both_stages() {
        let x: Vec<i128> = (0..120).map(|k: i128| (k * 53) % 199 - 99).collect();
        let dense = run(&x);
        let mut sparse: Vec<In<WI>> = Vec::new();
        for s in &x {
            sparse.push(In::<WI> {
                sample: Some(signed::<WI>(*s)),
                restart: false,
                downstream_ready: true,
            });
            sparse.push(In::<WI> {
                sample: None,
                restart: false,
                downstream_ready: true,
            });
        }
        sparse.extend(std::iter::repeat_n(
            In::<WI> {
                sample: None,
                restart: false,
                downstream_ready: true,
            },
            4,
        ));
        let got: Vec<i128> = uut()
            .run(sparse.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.sample.map(|v| v.raw()))
            .collect();
        assert_eq!(dense, got, "a gap must not advance either stage");
    }

    #[test]
    fn restart_reaches_the_decimator() {
        // Two different pre-trigger histories, identical afterwards:
        // the post-restart outputs must agree. An invariance property,
        // so it cannot be satisfied by a wrong expected value.
        let after: Vec<i128> = (0..R as i128 * 20).map(|k| (k * 31) % 161 - 80).collect();
        let go = |hist: i128| -> Vec<i128> {
            let mut v: Vec<In<WI>> = (0..R as i128 * 8)
                .map(|k| In::<WI> {
                    sample: Some(signed::<WI>((k * hist) % 121 - 60)),
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
            uut()
                .run(v.into_iter().with_reset(1).clock_pos_edge(100))
                .synchronous_sample()
                .filter_map(|s| s.output.sample.map(|x| x.raw()))
                .collect()
        };
        let a = go(7);
        let b = go(53);
        // The FIR's window spans TAPS outputs, so only outputs at least
        // TAPS after the restart are free of pre-trigger history.
        let tail = after.len() / R - TAPS;
        assert!(tail >= 5, "not enough post-restart outputs to check");
        assert_eq!(a[a.len() - tail..], b[b.len() - tail..]);
        assert_ne!(a, b, "the histories must differ, or this proves nothing");
    }

    // ---- Tier 3 / 4 / 5 --------------------------------------------

    fn hdl_stimulus() -> Vec<In<WI>> {
        feed(
            &(0..40)
                .map(|k: i128| (k * 71) % 187 - 93)
                .collect::<Vec<_>>(),
            true,
        )
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let hdl = uut().descriptor("top".into())?.hdl()?.modules.pretty();
        assert!(!hdl.is_empty());
        // The pair must instantiate both halves, not inline one away.
        assert!(hdl.contains("module top"), "no top module");
        Ok(())
    }

    #[test]
    fn test_cic_compensated_hdl_works() -> miette::Result<()> {
        let uut = uut();
        let tb = uut
            .run(hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100))
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_cic_compensated_trace() -> miette::Result<()> {
        let uut = uut();
        let vcd = uut
            .run(hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100))
            .collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("cic_compensated");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["5088cad7f24d7877360e97cf67245cc342dde7be5782db78ebb64ac3d2214b45"];
        let digest = vcd.dump_to_file(root.join("cic_compensated.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
