#![warn(missing_docs)]
//! `ComplexRealMixer` — an [`Iq`] carrier times a [`Real`] envelope.
//!
//! The transmit modulator, and by commutativity also the receive
//! down-converter's first stage (real ADC samples times a complex
//! carrier). **Two multiplies**, against four for the fully complex
//! case, because a real operand has no imaginary part to cross-multiply.
//!
//! ```text
//! (a + bi) · r = ar + bri
//! ```
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+ComplexRealMixer+---+
      |                      |
+---->+ carrier       stream +----->
      |                      |
+---->+ envelope     starved +----->
      |                      |
+---->+ downstream_ready     |
      |              overrun +----->
      +----------------------+
")]
//!
//! # Widths
//!
//! `A_W` and `B_W` are independent — there is no reason to force a
//! 14-bit envelope and an 18-bit carrier to a common width. The product
//! is exactly `A_W + B_W` bits and is carried at that width, so the
//! maximum-negative-squared case cannot overflow; see the module docs
//! on the absence of saturation.
//!
//! `OUT_W` is the narrowed result. `DROP` is how many bits the
//! narrowing discards and must equal `A_W + B_W − OUT_W`; a
//! compile-time assertion enforces it, because the kernel needs the
//! shift as a value rather than deriving it from the other three.
//!
//! # Starvation is reported, not handled
//!
//! Both inputs are isochronous — one sample per clock, phase-locked to
//! the timebase — so a cycle where only one side presents data cannot
//! happen in a correct design. Buffering the other side would mean an
//! elastic buffer with data-dependent occupancy, which makes the
//! transmit path's latency data-dependent and breaks the scheduler's
//! arithmetic.
//!
//! So a mismatch sets `starved` and the output is idle for that cycle.
//! Alignment itself is the scheduler's job: it issues each source's
//! control changes at the right lead time using the per-path latency
//! constants. What this widget provides is **detectability**.
//!
//! # This mixer cannot stall, so `stream.ready` is vacuously true
//!
//! `out` is a register that is overwritten on **every** cycle. There is
//! no stall path and there cannot be one: both inputs are isochronous,
//! so holding a sample back would desynchronise the datapath from the
//! timebase rather than delay it.
//!
//! The widget is therefore always ready to accept from upstream, and
//! `stream.ready` says so unconditionally. Forwarding
//! `downstream_ready` into that field instead — which this widget did
//! until the audit in `notes/dsp-nco-modulator-defects.md` — answers
//! "am I ready?" with someone else's readiness, and answers it wrongly:
//! the mixer consumes its inputs whether or not downstream is ready.
//!
//! Note that this differs from [`IqSplit`](crate::rcstream::util::IqSplit)
//! and [`IqCombine`](crate::rcstream::util::IqCombine), which *do*
//! forward their consumer's ready. They are combinational rewiring
//! holding no register, so for them the forwarded value is the truth.
//! The distinction is the DFF, not the direction the signal came from.
//!
//! # A lost sample is reported, not hidden
//!
//! Because the register is overwritten unconditionally, a cycle with
//! `downstream_ready` low loses that sample outright. [`Out::overrun`]
//! reports it.
//!
//! This is a design error being surfaced, not a condition to handle —
//! exactly as in [`Nco`](crate::dsp::nco::composite::Nco), which sits
//! one stage upstream and reports the same thing. A silently dropped
//! sample is the failure this codebase has shipped before, and it would
//! be perverse for the oscillator to report it and the modulator
//! immediately downstream to swallow it.
//!
//! Carloni relay stations are compatible: a relay downstream of an
//! always-ready sink never deasserts. An elastic buffer with
//! data-dependent occupancy is not, and belongs downstream of the
//! acquisition gate where latency stops mattering.

//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/complex_real_mixer.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/complex_real_mixer.md")]

use rhdl::prelude::*;

use crate::core::dff;
use crate::dsp::iq::{Iq, Real};
use crate::rcstream::bus::{Item, RCStream};

use super::rounding::convergent;

/// `Iq × Real → Iq`, two multiplies.
#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
pub struct ComplexRealMixer<
    const A_W: usize,
    const B_W: usize,
    const OUT_W: usize,
    const PROD_W: usize,
    const DROP: usize,
> where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
    rhdl::bits::W<OUT_W>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
{
    /// Registered result, so the widget has one cycle of latency and
    /// the multiplier's carry chain does not extend downstream.
    out: dff::DFF<Iq<OUT_W>>,
    /// A cycle where the two inputs did not both present data.
    starved: dff::DFF<bool>,
}

/// Inputs to [`ComplexRealMixer`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const A_W: usize, const B_W: usize>
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
{
    /// The complex operand — a carrier on transmit, ADC samples on
    /// receive.
    pub carrier: Option<Item<Iq<A_W>, ()>>,
    /// The real operand — an envelope on transmit, a carrier component
    /// on receive.
    pub envelope: Option<Item<Real<B_W>, ()>>,
    /// Downstream's ready, per the `RCStream` contract.
    pub downstream_ready: bool,
}

/// Outputs from [`ComplexRealMixer`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const OUT_W: usize>
where
    rhdl::bits::W<OUT_W>: BitWidth,
{
    /// The modulated sample stream.
    ///
    /// `stream.ready` is vacuously `true` — see the module docs. This
    /// mixer consumes on every cycle and has no stall path, so it is
    /// always ready to accept from upstream.
    pub stream: RCStream<Iq<OUT_W>, ()>,
    /// The inputs did not both present data on some cycle.
    pub starved: bool,
    /// A sample was presented while `downstream_ready` was low, and is
    /// gone.
    ///
    /// Combinational on `downstream_ready`, which is the correct
    /// alignment: the sample at risk is the one on `stream` this cycle.
    pub overrun: bool,
}

impl<const A_W: usize, const B_W: usize, const OUT_W: usize, const PROD_W: usize, const DROP: usize>
    SynchronousIO for ComplexRealMixer<A_W, B_W, OUT_W, PROD_W, DROP>
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
    rhdl::bits::W<OUT_W>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
{
    type I = In<A_W, B_W>;
    type O = Out<OUT_W>;
    type Kernel = complex_real_mixer_kernel<A_W, B_W, OUT_W, PROD_W, DROP>;
}

#[kernel]
#[doc(hidden)]
pub fn complex_real_mixer_kernel<
    const A_W: usize,
    const B_W: usize,
    const OUT_W: usize,
    const PROD_W: usize,
    const DROP: usize,
>(
    cr: ClockReset,
    i: In<A_W, B_W>,
    q: Q<A_W, B_W, OUT_W, PROD_W, DROP>,
) -> (Out<OUT_W>, D<A_W, B_W, OUT_W, PROD_W, DROP>)
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
    rhdl::bits::W<OUT_W>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
{
    let mut d = D::<A_W, B_W, OUT_W, PROD_W, DROP>::dont_care();
    d.out = q.out;
    d.starved = false;

    // Tuple patterns are not accepted in kernel match arms, so the
    // two streams are unpacked separately.
    // Zero rather than dont_care: these are read on the merge path, and
    // reading a dont_care is a partial-initialisation error.
    let mut carrier = Iq::<A_W> {
        re: signed::<A_W>(0),
        im: signed::<A_W>(0),
    };
    let mut have_carrier = false;
    if let Some(c) = i.carrier {
        carrier = c.data;
        have_carrier = true;
    }

    let mut envelope = signed::<B_W>(0);
    let mut have_envelope = false;
    if let Some(e) = i.envelope {
        envelope = e.data.v;
        have_envelope = true;
    }

    if have_carrier && have_envelope {
        // Widen both operands to the product width before multiplying,
        // so the multiply is exact and the maximum-negative-squared
        // case has room.
        let e = envelope.resize::<PROD_W>();
        let re = carrier.re.resize::<PROD_W>() * e;
        let im = carrier.im.resize::<PROD_W>() * e;
        d.out = Iq::<OUT_W> {
            re: convergent::<PROD_W, OUT_W, DROP>(re),
            im: convergent::<PROD_W, OUT_W, DROP>(im),
        };
    } else {
        // Isochronous inputs, so this cannot happen in a correct
        // design.  Report it; do not buffer the other side.  The sample
        // emitted is zero -- the idle value for a transmit chain, and a
        // defined one rather than dont_care, which would be read back
        // through `q.out` next cycle.
        d.starved = true;
        d.out = Iq::<OUT_W> {
            re: signed::<OUT_W>(0),
            im: signed::<OUT_W>(0),
        };
    }

    let mut o = Out::<OUT_W> {
        stream: RCStream::<Iq<OUT_W>, ()> {
            data: Some(Item::<Iq<OUT_W>, ()> {
                data: q.out,
                frame: (),
            }),
            // Vacuously true: `out` is overwritten every cycle, so the
            // mixer is always ready to accept from upstream.  See the
            // module docs on why forwarding `downstream_ready` here
            // would be a false claim rather than a conservative one.
            ready: true,
        },
        starved: q.starved,
        // The sample on `stream` this cycle is lost if downstream is not
        // ready to take it, and the mixer will not hold it.
        overrun: !i.downstream_ready,
    };

    if cr.reset.any() {
        d.out = Iq::<OUT_W> {
            re: signed::<OUT_W>(0),
            im: signed::<OUT_W>(0),
        };
        d.starved = false;
        o.overrun = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;

    // Carrier 18 bits, envelope 16, output 18. Product is 34, so the
    // narrowing drops 16.
    const A: usize = 18;
    const B: usize = 16;
    const O: usize = 18;
    const P: usize = 34;
    const DR: usize = 16;
    type Uut = ComplexRealMixer<A, B, O, P, DR>;

    const _: () = assert!(P == A + B && DR == P - O);

    fn feed(re: i128, im: i128, env: i128) -> In<A, B> {
        In::<A, B> {
            carrier: Some(Item::<Iq<A>, ()> {
                data: Iq::<A> {
                    re: signed::<A>(re),
                    im: signed::<A>(im),
                },
                frame: (),
            }),
            envelope: Some(Item::<Real<B>, ()> {
                data: Real::<B> {
                    v: signed::<B>(env),
                },
                frame: (),
            }),
            downstream_ready: true,
        }
    }

    fn gap() -> In<A, B> {
        In::<A, B> {
            carrier: None,
            envelope: Some(Item::<Real<B>, ()> {
                data: Real::<B> {
                    v: signed::<B>(100),
                },
                frame: (),
            }),
            downstream_ready: true,
        }
    }

    fn run(seq: Vec<In<A, B>>) -> Vec<Out<O>> {
        let uut = Uut::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect()
    }

    fn got(o: &Out<O>) -> (i128, i128) {
        match o.stream.data {
            Some(item) => (item.data.re.raw(), item.data.im.raw()),
            None => panic!("the mixer must emit every cycle"),
        }
    }

    /// The reference: exact product, then convergent narrowing.
    fn want(re: i128, im: i128, env: i128) -> (i128, i128) {
        let conv = |v: i128| -> i128 {
            let half = 1i128 << (DR - 1);
            let lsbs = v & ((1i128 << DR) - 1);
            let q = (v + half) >> DR;
            if lsbs == half && q % 2 != 0 { q - 1 } else { q }
        };
        (conv(re * env), conv(im * env))
    }

    #[test]
    fn default_construction() {
        let _uut = Uut::default();
    }

    /// Tier 1/2 — the product is right across sign combinations.
    #[test]
    fn the_product_is_correct() {
        let cases = [
            (100_000i128, 50_000i128, 20_000i128),
            (-100_000, 50_000, 20_000),
            (100_000, -50_000, -20_000),
            (-100_000, -50_000, -20_000),
            (131_070, 131_070, 32_767),
            (-131_070, -131_070, -32_768),
        ];
        let seq: Vec<In<A, B>> = cases.iter().map(|(r, i, e)| feed(*r, *i, *e)).collect();
        let out = run(seq);
        let mut checked = 0;
        for (k, (r, i, e)) in cases.iter().enumerate() {
            // one cycle of latency plus one reset cycle
            if k + 2 < out.len() {
                assert_eq!(got(&out[k + 2]), want(*r, *i, *e), "case {k}: {r} {i} {e}");
                checked += 1;
            }
        }
        assert!(checked >= 4, "only {checked} cases actually compared");
    }

    /// Shared Tier-4/Tier-5 stimulus.
    ///
    /// Deliberately exercises **every** output, not just the datapath: a
    /// starved cycle and a not-ready cycle are included, so `starved` and
    /// `overrun` are driven high somewhere in the trace. A stimulus that
    /// left them constant would make the `iverilog` round-trip and the
    /// VCD digest cover them as tied-off wires, which is how a codegen
    /// bug in a flag output survives a green Tier 4.
    fn hdl_stimulus() -> Vec<In<A, B>> {
        let mut seq: Vec<In<A, B>> = (0..24i128)
            .map(|k| feed((k - 12) * 8000, (12 - k) * 6000, (k - 12) * 2000))
            .collect();
        // Downstream drops ready: the registered sample is lost.
        seq.push(In::<A, B> {
            downstream_ready: false,
            ..feed(60_000, -40_000, 12_000)
        });
        // Only the envelope presents data: starvation.
        seq.push(gap());
        seq.extend((0..4i128).map(|k| feed(k * 3000, -k * 2000, k * 1000)));
        seq
    }

    /// **Tier 4 — the emitted Verilog agrees.**
    ///
    /// The load-bearing test for this widget. Both operands are
    /// extracted from `Option` payloads, which is exactly the shape that
    /// silently loses signedness elsewhere in the tree
    /// (`dsp::nco::modulation`), and an unsigned multiply would still
    /// produce plausible-looking numbers.
    #[test]
    fn test_complex_real_mixer_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// The resource claim, made checkable.
    ///
    /// Two multiplies, not four — the entire justification for having a
    /// separate widget for a real operand rather than tying an `Iq`'s
    /// `im` to zero.
    #[test]
    fn multiplier_count_is_as_claimed() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let mults = hdl.matches(" * ").count();
        assert_eq!(
            mults, 2,
            "expected exactly 2 multiplies for Iq x Real; found {mults}.\n{hdl}"
        );
        Ok(())
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
            module top_out
            module top_starved"#]];
        expect.assert_eq(&shape);
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn test_complex_real_mixer_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("complex_real_mixer");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["d1cd0cb3a862a8a35d532b9101b2d97d1938cbac3724691fda8156f5dfbfd89a"];
        let digest = vcd
            .dump_to_file(root.join("complex_real_mixer.vcd"))
            .unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    /// The output has no DC offset, the property truncation destroys.
    #[test]
    fn the_output_has_no_dc_offset() {
        const PAIRS: usize = 64;
        let mut seq: Vec<In<A, B>> = Vec::new();
        for k in 1..=PAIRS as i128 {
            seq.push(feed(k * 1500, k * 700, 9_000));
            seq.push(feed(-k * 1500, -k * 700, 9_000));
        }
        seq.extend(vec![feed(0, 0, 0); 4]);
        let out = run(seq);
        let samples = 2 * PAIRS;
        let (sr, si) = out
            .iter()
            .skip(2)
            .take(samples)
            .map(got)
            .fold((0i128, 0i128), |(a, b), (r, i)| (a + r, b + i));
        let n = samples as i128;
        let tolerance = n / 8;
        assert!(
            sr.abs() <= tolerance && si.abs() <= tolerance,
            "output mean is not zero: re {sr}, im {si} over {n}, tolerance {tolerance}"
        );
    }

    /// A sample presented while downstream is not ready is reported.
    ///
    /// The mixer cannot stall — `out` is overwritten every cycle — so the
    /// sample is genuinely gone rather than delayed. `Nco` reports the
    /// same condition one stage upstream; before the audit in
    /// `notes/dsp-nco-modulator-defects.md` this widget swallowed it.
    ///
    /// Verified able to fail: reverting `overrun` to a constant `false`
    /// makes the first assertion report it.
    #[test]
    fn a_lost_sample_is_reported() {
        let stalled = |re, im, env| In::<A, B> {
            downstream_ready: false,
            ..feed(re, im, env)
        };
        let mut seq = vec![feed(1000, 2000, 300); 3];
        seq.push(stalled(1000, 2000, 300));
        seq.extend(vec![feed(1000, 2000, 300); 3]);
        let out = run(seq);
        assert!(
            out.iter().any(|o| o.overrun),
            "a sample was dropped while downstream was not ready, and \
             nothing said so"
        );
        assert!(
            out.iter().any(|o| !o.overrun),
            "overrun is stuck high; it must be a per-cycle report"
        );
        assert_eq!(
            out.iter().filter(|o| o.overrun).count(),
            1,
            "exactly one cycle had downstream_ready low"
        );
    }

    /// `stream.ready` is unconditionally true, because the mixer always
    /// consumes.
    ///
    /// Pins the contract in `rcstream::bus` for widget-`O` role: the
    /// field answers "am I ready to accept from upstream?", and the
    /// honest answer here does not depend on downstream. Forwarding
    /// `downstream_ready` would make this fail on the stalled cycle.
    #[test]
    fn ready_does_not_depend_on_downstream() {
        let stalled = In::<A, B> {
            downstream_ready: false,
            ..feed(1000, 2000, 300)
        };
        let out = run(vec![stalled; 6]);
        assert!(
            out.iter().all(|o| o.stream.ready),
            "stream.ready went low although the mixer has no stall path"
        );
    }

    /// Reset suppresses the overrun report.
    ///
    /// Same treatment `Nco` gives it: during reset there is no sample to
    /// lose, so reporting one would be a spurious fault at every
    /// power-on.
    #[test]
    fn kernel_reset_suppresses_overrun() {
        let q = Q::<A, B, O, P, DR> {
            out: Iq::<O> {
                re: signed::<O>(7),
                im: signed::<O>(9),
            },
            starved: true,
        };
        let cr = clock_reset(clock(false), reset(true));
        let stalled = In::<A, B> {
            downstream_ready: false,
            ..feed(1000, 2000, 300)
        };
        let (o, _d) = complex_real_mixer_kernel::<A, B, O, P, DR>(cr, stalled, q);
        assert!(!o.overrun, "reset must not report a lost sample");
    }

    /// A cycle where only one input presents data is reported.
    #[test]
    fn starvation_is_reported() {
        let mut seq = vec![feed(1000, 2000, 300); 4];
        seq.push(gap());
        seq.extend(vec![feed(1000, 2000, 300); 4]);
        let out = run(seq);
        assert!(
            out.iter().any(|o| o.starved),
            "a starved cycle was not reported"
        );
        assert!(out.iter().any(|o| !o.starved), "starved is stuck high");
    }
}
