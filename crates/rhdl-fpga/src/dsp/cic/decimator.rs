#![warn(missing_docs)]
//! `CicDecimate` — an `N`-stage CIC decimator on a real sample stream.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+CicDecimate+------------+
      |                          |
+---->+ sample                   |
      |   Option<SignedBits<WI>> |
      |                   sample |
      |    Option<SignedBits<WA>>+----->
+---->+ downstream_ready         |
      |                  overrun +----->
      +--------------------------+
")]
//!
//!# Internals
#![doc = badascii_doc::badascii!(r"
  in ->[I0]->[I1]->..[In]-+          +->[C0]->[C1]->..[Cn]-> out
       integrators, every |  /R      |  combs, once per R
       input sample       +--gate----+  (y = x - x[-M])
")]
//!
//! # What the widths mean
//!
//! `W_IN` is the input sample width. `W_ACC` is the width every
//! integrator and comb runs at, and it must satisfy
//! [`super::accumulator_width_is_sufficient`] — `Default` asserts it.
//! Too narrow is not a precision trade, it is a wrong answer: the
//! integrators wrap continuously and only cancel in the combs when the
//! datapath is wide enough to carry `(R·M)^N` times the input.
//!
//! `STAGES`, `R` and `M` are the cascade depth, decimation factor and
//! differential delay. `CW` is the decimation counter's width and must
//! hold `R`; it is a separate parameter only because Rust cannot derive
//! an array or integer width from another const generic without
//! `generic_const_exprs`, and `Default` asserts it too.
//!
//! # The output is not scaled
//!
//! Deliberately. The DC gain is exactly [`super::dc_gain`], and undoing
//! it costs either a multiply or a shift that throws away bits the
//! filter was built to keep. Which is right depends on what comes next,
//! so the widget reports the gain rather than guessing.
//!
//! # Idle cycles hold the filter
//!
//! `sample: None` advances nothing — not the integrators, not the
//! decimation phase. A CIC's state is a running sum over *samples*, not
//! over *cycles*, so a gap in the stream must not be read as a zero.
//! That makes the widget correct on a gated stream as well as an
//! isochronous one.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/cic_decimate.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/cic_decimate.md")]

use rhdl::prelude::*;

use super::{accumulator_width_is_sufficient, counter_width};
use crate::core::dff;
use crate::dsp::sign_extend;

/// An `N`-stage CIC decimator.
///
/// See the module docs for what each width means and why `W_ACC` is
/// checked rather than trusted.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct CicDecimate<
    const W_IN: usize,
    const W_ACC: usize,
    const STAGES: usize,
    const R: usize,
    const M: usize,
    const CW: usize,
> where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_ACC>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
{
    /// Running sums, one per stage, at the input rate.
    integrators: dff::DFF<[SignedBits<W_ACC>; STAGES]>,
    /// Comb delay lines, `M` deep per stage, at the output rate.
    combs: dff::DFF<[[SignedBits<W_ACC>; M]; STAGES]>,
    /// Counts input samples toward the next output.
    phase: dff::DFF<Bits<CW>>,
    /// The decimated result, registered.
    out: dff::DFF<Option<SignedBits<W_ACC>>>,
}

/// Inputs to [`CicDecimate`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const W_IN: usize>
where
    rhdl::bits::W<W_IN>: BitWidth,
{
    /// The input sample, or `None` for an idle cycle.
    ///
    /// An idle cycle holds the whole filter — see the module docs.
    pub sample: Option<SignedBits<W_IN>>,
    /// Downstream's ready, per the `RCStream` contract.
    ///
    /// **This widget does not stall.** Its state is a running sum tied
    /// to the input stream; pausing would desynchronise the decimation
    /// phase rather than delay it. A low `ready` on a cycle that
    /// produces output loses that output, which [`Out::overrun`]
    /// reports.
    pub downstream_ready: bool,
}

/// Outputs from [`CicDecimate`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const W_ACC: usize>
where
    rhdl::bits::W<W_ACC>: BitWidth,
{
    /// The decimated sample, present on one cycle in `R`.
    ///
    /// Carries the full `(R·M)^N` DC gain — see the module docs on why
    /// it is not scaled here.
    pub sample: Option<SignedBits<W_ACC>>,
    /// An output sample was produced while `downstream_ready` was low,
    /// and is gone.
    pub overrun: bool,
}

impl<
    const W_IN: usize,
    const W_ACC: usize,
    const STAGES: usize,
    const R: usize,
    const M: usize,
    const CW: usize,
> Default for CicDecimate<W_IN, W_ACC, STAGES, R, M, CW>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_ACC>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
{
    fn default() -> Self {
        // Checked, not trusted. A too-narrow accumulator does not
        // degrade the output, it corrupts it -- the integrator wraps
        // stop cancelling in the combs and the result is a plausible
        // looking signal that is simply wrong.
        assert!(
            accumulator_width_is_sufficient(W_IN, W_ACC, STAGES, R, M),
            "W_ACC must be at least W_IN + STAGES*ceil(log2(R*M)) for the \
             integrator wraps to cancel in the combs"
        );
        assert!(
            CW >= counter_width(R),
            "CW must be wide enough to count to R"
        );
        assert!(R >= 2, "a decimation factor below two is not a decimator");
        assert!(M >= 1, "the differential delay must be at least one");
        Self {
            integrators: dff::DFF::new([SignedBits::<W_ACC>::default(); STAGES]),
            combs: dff::DFF::new([[SignedBits::<W_ACC>::default(); M]; STAGES]),
            phase: dff::DFF::new(bits::<CW>(0)),
            out: dff::DFF::new(None),
        }
    }
}

impl<
    const W_IN: usize,
    const W_ACC: usize,
    const STAGES: usize,
    const R: usize,
    const M: usize,
    const CW: usize,
> SynchronousIO for CicDecimate<W_IN, W_ACC, STAGES, R, M, CW>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_ACC>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
{
    type I = In<W_IN>;
    type O = Out<W_ACC>;
    type Kernel = cic_decimate_kernel<W_IN, W_ACC, STAGES, R, M, CW>;
}

#[kernel]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn cic_decimate_kernel<
    const W_IN: usize,
    const W_ACC: usize,
    const STAGES: usize,
    const R: usize,
    const M: usize,
    const CW: usize,
>(
    cr: ClockReset,
    i: In<W_IN>,
    q: Q<W_IN, W_ACC, STAGES, R, M, CW>,
) -> (Out<W_ACC>, D<W_IN, W_ACC, STAGES, R, M, CW>)
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_ACC>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
{
    let mut d = D::<W_IN, W_ACC, STAGES, R, M, CW>::dont_care();

    // Hold by default: every register keeps its value unless a sample
    // moves it.  An idle cycle must not advance the running sums.
    d.integrators = q.integrators;
    d.combs = q.combs;
    d.phase = q.phase;
    d.out = None;

    let mut have = false;
    let mut x = signed::<W_ACC>(0);
    if let Some(s) = i.sample {
        have = true;
        // `sign_extend`, not `resize`.
        //
        // `s` is unwrapped from an `Option`, and `resize` on such a
        // value zero-extends in the emitted Verilog while the Rust
        // simulator sign-extends. Tiers 1 and 2 pass either way; only
        // the `iverilog` round-trip catches it, which is exactly how
        // this was found. See `crate::dsp::sign_extend`.
        x = sign_extend::<W_IN, W_ACC>(s);
    }

    if have {
        // ---- integrator cascade, at the input rate ----
        //
        // Each stage accumulates the previous stage's *new* value, so
        // the cascade is a single pass rather than a delay line.
        let mut ints = q.integrators;
        let mut carry = x;
        for k in 0..STAGES {
            // Wraps, and must: see the module docs.
            let acc = q.integrators[k] + carry;
            ints[k] = acc;
            carry = acc;
        }
        d.integrators = ints;

        // ---- decimation gate ----
        let last = q.phase == bits::<CW>((R - 1) as u128);
        d.phase = if last {
            bits::<CW>(0)
        } else {
            q.phase + bits::<CW>(1)
        };

        if last {
            // ---- comb cascade, once per R input samples ----
            let mut cs = q.combs;
            let mut v = carry;
            for k in 0..STAGES {
                // y = x - x[-M]; then shift this stage's delay line.
                let delayed = q.combs[k][M - 1];
                let diff = v - delayed;
                let mut line = q.combs[k];
                for j in 0..M {
                    // Shift toward the tail, newest at index 0.
                    let idx = M - 1 - j;
                    line[idx] = if idx == 0 { v } else { q.combs[k][idx - 1] };
                }
                cs[k] = line;
                v = diff;
            }
            d.combs = cs;
            d.out = Some(v);
        }
    }

    let mut o = Out::<W_ACC> {
        sample: q.out,
        // Combinational on `downstream_ready`: the sample at risk is
        // the one on the output this cycle.
        overrun: !i.downstream_ready,
    };

    if cr.reset.any() {
        d.integrators = [signed::<W_ACC>(0); STAGES];
        d.combs = [[signed::<W_ACC>(0); M]; STAGES];
        d.phase = bits::<CW>(0);
        d.out = None;
        o.overrun = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::super::{accumulator_width, dc_gain};
    use super::*;
    use expect_test::expect;
    use std::f64::consts::TAU;

    /// A validated small configuration: 8-bit input, two stages,
    /// decimate by four.
    const WI: usize = 8;
    const WA: usize = 12;
    const S: usize = 2;
    const R: usize = 4;
    const M: usize = 1;
    const CW: usize = 2;
    type Uut = CicDecimate<WI, WA, S, R, M, CW>;

    /// An independent software CIC, written straight from the
    /// definition rather than from the widget.
    ///
    /// The point of writing it twice: a transcription error in the
    /// widget's cascade would have to be reproduced exactly here to go
    /// unnoticed, and the two are structured differently enough that
    /// this is unlikely.
    fn model(x: &[i128], stages: usize, r: usize, m: usize) -> Vec<i128> {
        let mut ints = vec![0i128; stages];
        let mut combs = vec![vec![0i128; m]; stages];
        let mut out = Vec::new();
        for (n, s) in x.iter().enumerate() {
            let mut carry = *s;
            for i in ints.iter_mut() {
                *i += carry;
                carry = *i;
            }
            if (n + 1) % r == 0 {
                let mut v = carry;
                for line in combs.iter_mut() {
                    let diff = v - line[m - 1];
                    for j in (1..m).rev() {
                        line[j] = line[j - 1];
                    }
                    line[0] = v;
                    v = diff;
                }
                out.push(v);
            }
        }
        out
    }

    fn stimulus(x: &[i128]) -> Vec<In<WI>> {
        let mut seq: Vec<In<WI>> = x
            .iter()
            .map(|v| In::<WI> {
                sample: Some(signed::<WI>(*v)),
                downstream_ready: true,
            })
            .collect();
        // Drain: the output is registered, so the final decimated
        // sample needs a cycle to emerge. Idle cycles hold the filter,
        // so this cannot change the result -- only let it out.
        seq.extend(std::iter::repeat_n(
            In::<WI> {
                sample: None,
                downstream_ready: true,
            },
            2,
        ));
        seq
    }

    fn run(x: &[i128]) -> Vec<i128> {
        let uut = Uut::default();
        uut.run(stimulus(x).into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.sample.map(|v| v.raw()))
            .collect()
    }

    // ---- Tier 1 / 2: it is the filter it claims to be ---------------

    #[test]
    fn default_construction() {
        let _ = Uut::default();
    }

    /// **A constant input settles at exactly the published DC gain.**
    ///
    /// `(R·M)^N`, not approximately. If this is off, the integrator and
    /// comb cascades disagree about depth.
    #[test]
    fn a_constant_input_settles_at_the_dc_gain() {
        let got = run(&vec![1i128; 40]);
        assert_eq!(
            *got.last().unwrap(),
            dc_gain(S, R, M) as i128,
            "steady-state gain must be (R*M)^N exactly"
        );
    }

    /// Matches an independently written model, sample for sample.
    #[test]
    fn matches_the_model_on_a_varying_input() {
        let x: Vec<i128> = (0..64).map(|k| (k % 17) - 8).collect();
        assert_eq!(run(&x), model(&x, S, R, M));
    }

    /// **The defining property: nulls at multiples of the output rate.**
    ///
    /// A CIC's whole purpose is that its `sinc^N` response puts zeros
    /// exactly where decimation would fold energy back into the band.
    /// Feed a tone at `fs/R` — the first null — and almost nothing
    /// should come out. Compare against a tone at DC, which passes at
    /// full gain.
    ///
    /// This is what separates "the arithmetic matches a model" from "it
    /// is a filter": a cascade wired in the wrong order can still match
    /// a model written the same wrong way, but it will not null.
    #[test]
    fn a_tone_at_the_first_null_is_rejected() {
        let n = 256;
        let amp = 60.0;
        // Tone at fs/R -- exactly the first null of the sinc^N response.
        let at_null: Vec<i128> = (0..n)
            .map(|k| (amp * (TAU * (k as f64) / (R as f64)).cos()).round() as i128)
            .collect();
        let dc: Vec<i128> = (0..n).map(|_| amp.round() as i128).collect();

        let null_out = run(&at_null);
        let dc_out = run(&dc);

        // Ignore the first few outputs while the cascade fills.
        let settle = 4;
        let null_peak = null_out[settle..].iter().map(|v| v.abs()).max().unwrap();
        let dc_level = dc_out[settle..].iter().map(|v| v.abs()).max().unwrap();

        assert!(
            null_peak * 20 < dc_level,
            "a tone at the first null should be deeply attenuated: \
             null peak {null_peak} vs DC level {dc_level}"
        );
    }

    /// An idle cycle holds the filter rather than injecting a zero.
    ///
    /// Load-bearing for a gated stream: reading a gap as a zero sample
    /// would both corrupt the running sums and slip the decimation
    /// phase.
    #[test]
    fn an_idle_cycle_holds_the_filter() {
        let x: Vec<i128> = (0..32).map(|k| (k % 11) - 5).collect();
        let dense = run(&x);

        // The same samples, with idle cycles interleaved.
        let uut = Uut::default();
        let mut gapped: Vec<In<WI>> = Vec::new();
        for v in &x {
            gapped.push(In::<WI> {
                sample: Some(signed::<WI>(*v)),
                downstream_ready: true,
            });
            gapped.push(In::<WI> {
                sample: None,
                downstream_ready: true,
            });
        }
        gapped.extend(std::iter::repeat_n(
            In::<WI> {
                sample: None,
                downstream_ready: true,
            },
            2,
        ));
        let sparse: Vec<i128> = uut
            .run(gapped.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| s.output.sample.map(|v| v.raw()))
            .collect();

        assert_eq!(
            dense, sparse,
            "gaps in the stream must not change the filtered result"
        );
    }

    /// Reset clears the cascade, so a second burst is not contaminated.
    #[test]
    fn reset_clears_the_cascade() {
        let x = vec![7i128; 24];
        let first = run(&x);
        let second = run(&x);
        assert_eq!(first, second, "a fresh widget must start from zero");
    }

    /// A lost output is reported rather than hidden.
    #[test]
    fn a_lost_sample_is_reported() {
        let uut = Uut::default();
        let seq: Vec<In<WI>> = (0..24)
            .map(|_| In::<WI> {
                sample: Some(signed::<WI>(3)),
                downstream_ready: false,
            })
            .collect();
        let any = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .any(|s| s.output.overrun);
        assert!(
            any,
            "an output produced while downstream was not ready is lost"
        );
    }

    /// **A too-narrow accumulator is refused, not tolerated.**
    ///
    /// The failure it prevents is silent: the integrator wraps stop
    /// cancelling in the combs and the output becomes a plausible but
    /// wrong signal.
    #[test]
    #[should_panic(expected = "W_ACC")]
    fn a_narrow_accumulator_is_rejected() {
        assert_eq!(accumulator_width(WI, S, R, M), WA);
        let _ = CicDecimate::<WI, 11, S, R, M, CW>::default();
    }

    /// As is a counter too narrow to reach `R`.
    #[test]
    #[should_panic(expected = "CW")]
    fn a_narrow_counter_is_rejected() {
        let _ = CicDecimate::<WI, WA, S, R, M, 1>::default();
    }

    /// The claims in `examples/cic_decimate.rs` prose, checked.
    ///
    /// The example names a specific settled value and asserts the tone
    /// at the first null collapses. Pinning both here means the
    /// description cannot drift away from the widget.
    #[test]
    fn the_example_behaves_as_its_comments_claim() {
        let x: Vec<i128> = (0..48)
            .map(|k| {
                if k < 24 {
                    100
                } else {
                    match k % 4 {
                        0 => 100,
                        2 => -100,
                        _ => 0,
                    }
                }
            })
            .collect();
        let out = run(&x);

        // The DC stretch settles at 100 * (R*M)^N = 100 * 16.
        let dc_settled = out[4];
        assert_eq!(
            dc_settled,
            100 * dc_gain(S, R, M) as i128,
            "the DC stretch should settle at the full gain"
        );

        // The tone at fs/4 is the first null; the tail must collapse.
        let tail = *out.last().unwrap();
        assert!(
            tail.abs() * 20 < dc_settled,
            "the tone at the first null should collapse: tail {tail} vs \
             settled {dc_settled}"
        );
    }

    // ---- Tier 3: HDL emission ---------------------------------------

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
            module top_integrators
            module top_combs
            module top_phase
            module top_out"#]];
        expect.assert_eq(&shape);
        Ok(())
    }

    // ---- Tier 4: iverilog round-trip --------------------------------

    fn hdl_stimulus() -> Vec<In<WI>> {
        let x: Vec<i128> = (0..24).map(|k| ((k * 5) % 19) - 9).collect();
        let mut seq = stimulus(&x);
        seq[7].sample = None;
        seq[15].downstream_ready = false;
        seq
    }

    #[test]
    fn test_cic_decimate_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let tb = uut
            .run(hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100))
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    // ---- Tier 5: VCD digest -----------------------------------------

    #[test]
    fn test_cic_decimate_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("cic_decimate");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["a9d90a9a8edaf9d744c515d01088cb79429e170a616d2f68c12442c169caa460"];
        let digest = vcd.dump_to_file(root.join("cic_decimate.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
