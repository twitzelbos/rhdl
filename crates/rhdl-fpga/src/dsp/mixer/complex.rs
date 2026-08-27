#![warn(missing_docs)]
//! `ComplexMixer` — a full complex multiply, [`Iq`] times [`Iq`],
//! generic over the framing its operands carry.
//!
//! ```text
//! (a + bi)(c + di) = (ac − bd) + (ad + bc)i
//! ```
//!
//! **Four multiplies and two adds** — the general case. Where one
//! operand is real, [`super::ComplexRealMixer`] does the same job in
//! two, which is why the two widgets exist separately rather than as
//! one generic with a mode bit.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+ComplexMixer+--------------+
      |                             |
+---->+ a                    stream +----->
      |   Option<Item<Iq,F>>        |
+---->+ b                   starved +----->
      |                             |
+---->+ downstream_ready    overrun +----->
      |                             |
      |              frame_mismatch +----->
      +-----------------------------+
")]
//!
//! # Framing, and the alignment contract
//!
//! Both operands carry framing of the same type `F`, and the product
//! carries it onward. The mixer is where two independently-framed
//! streams meet, so it is also the natural place to check that they
//! agree: if both present data and their markers differ,
//! [`Out::frame_mismatch`] is raised.
//!
//! Three cases, and the type system settles all of them:
//!
//! - **Both framed and aligned** — the marker passes through with the
//!   product.
//! - **Both framed, disagreeing** — reported. For `F = SyncMark` that
//!   is a latency-compensation bug upstream: two paths that should
//!   anchor the same instant do not. Making it visible here is the
//!   entire reason each source tags its own stream.
//! - **Unframed (`F = ()`)** — the unit type has one inhabitant, so
//!   markers cannot differ and the check is always `false`. It costs a
//!   one-bit constant rather than nothing: the comparison is padded to
//!   a non-zero width, because a zero-width comparison currently emits
//!   Verilog that evaluates to `x`. See the kernel comment and
//!   `notes/zero-width-digital-types.md`.
//!
//! There is deliberately no "one side framed, the other not" case.
//! `F` is one type parameter shared by both ports, so connecting a
//! framed stream to an unframed one is a compile error rather than a
//! runtime rule — which is what [`crate::rcstream::bus`] means by
//! framing semantics being part of the type.
//!
//! # Karatsuba is not used
//!
//! Three multiplies instead of four, at the cost of five adds. On a
//! Zynq the DSP48 slices are plentiful and the extra adds land in
//! fabric, so four DSPs is the cheaper trade. Revisit only if DSP
//! pressure appears — the arithmetic here is a drop-in swap.
//!
//! # Widths
//!
//! Each partial product is `A_W + B_W` bits and the result sums two of
//! them, so the natural width is **`A_W + B_W + 1`** — one more than
//! [`super::ComplexRealMixer`], and the same figure the AMD Complex
//! Multiplier (PG104) calls "the sum of the input widths plus one".
//! Carrying that width is what makes the maximum-negative-squared case
//! unable to overflow, and why there is no saturation logic.
//!
//! # Starvation is reported, not handled
//!
//! Both inputs are isochronous — one sample per clock, phase-locked to
//! the timebase — so a cycle where only one side presents data cannot
//! happen in a correct design. Buffering the other side would mean an
//! elastic buffer with data-dependent occupancy, which makes the path's
//! latency data-dependent and breaks the scheduler's arithmetic.
//!
//! So a mismatch sets `starved` and the sample emitted for that cycle is
//! zero — a defined idle value, not `None` and not `dont_care`, since it
//! is read back through `q.out` on the following cycle.
//!
//! # This mixer cannot stall, so `stream.ready` is vacuously true
//!
//! `out` is a register that is overwritten on **every** cycle. There is
//! no stall path and there cannot be one, for the same isochrony reason.
//! The widget is therefore always ready to accept from upstream, and
//! `stream.ready` says so unconditionally.
//!
//! Forwarding `downstream_ready` into that field instead — which this
//! widget did until the audit in
//! `notes/dsp-nco-modulator-defects.md` — answers "am I ready?" with
//! someone else's readiness, and answers it wrongly: the mixer consumes
//! its inputs whether or not downstream is ready. Contrast
//! [`IqSplit`](crate::rcstream::util::IqSplit) and
//! [`IqCombine`](crate::rcstream::util::IqCombine), which *do* forward
//! their consumer's ready and are right to: they are combinational
//! rewiring holding no register. The distinction is the DFF, not the
//! direction the signal came from.
//!
//! # A lost sample is reported, not hidden
//!
//! Because the register is overwritten unconditionally, a cycle with
//! `downstream_ready` low loses that sample outright, and
//! [`Out::overrun`] reports it — as
//! [`Nco`](crate::dsp::nco::composite::Nco) does for the same condition.
//! A silently dropped sample is the failure this codebase has shipped
//! before.
//!
//!# Example
//!
//! Read alongside [`ComplexRealMixer`](super::ComplexRealMixer)'s
//! example: the two differ in exactly the way the arithmetic does. Here
//! both operands rotate and the product's frequency is their sum, which
//! is the thing a real operand cannot do.
//!
//!```
#![doc = include_str!("../../../examples/complex_mixer.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/complex_mixer.md")]

use rhdl::prelude::*;

use crate::core::constant::Constant;
use crate::core::dff;
use crate::dsp::iq::Iq;
use crate::rcstream::bus::{Item, RCStream};

use super::rounding::convergent;

/// `Iq × Iq → Iq`, four multiplies.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct ComplexMixer<
    F: Digital + Default,
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
    /// Registered result **and the marker travelling with it**.
    ///
    /// One register rather than two, so the marker cannot come adrift
    /// from the sample it belongs to. It also keeps this sub-circuit
    /// non-zero-width for every `F`, which matters — see `idle`.
    out: dff::DFF<Item<Iq<OUT_W>, F>>,
    /// A cycle where the two inputs did not both present data.
    starved: dff::DFF<bool>,
    /// The two inputs disagreed about framing — see [`Out::frame_mismatch`].
    mismatch: dff::DFF<bool>,
    /// The idle item: a zero sample carrying `F::default()`.
    ///
    /// A kernel cannot call `<F as Default>::default()` — that is a Rust
    /// call with no hardware meaning, and the compiler correctly reports
    /// the slot as read-before-written. A [`Constant`] is how this tree
    /// gets a compile-time value into a kernel, and it folds away in
    /// synthesis, so this costs nothing.
    ///
    /// It is needed because reset and starvation must both drive `out`
    /// with an *un-marked* item. A don't-care would let `F = SyncMark`
    /// assert a spurious anchor on exactly the cycles where the data is
    /// known to be invalid — the failure this widget exists to catch.
    ///
    /// **Why the whole `Item` and not just the marker.** A `Constant<F>`
    /// is zero-width at `F = ()`, and zero-width values currently
    /// miscompile two ways: a zero-bit reset literal renders as the
    /// illegal `0'b`, so `DFF<()>` will not even parse; and a zero-width
    /// value gets no defining instruction at all, which the
    /// partial-init checker reports as "slot is read before being
    /// written". Both are filed in
    /// `notes/zero-width-digital-types.md`. Bundling the marker with the
    /// zero sample keeps this constant `2·OUT_W + |F|` bits wide for
    /// every `F`, so neither is reachable from here.
    idle: Constant<Item<Iq<OUT_W>, F>>,
}

impl<
    F: Digital + Default,
    const A_W: usize,
    const B_W: usize,
    const OUT_W: usize,
    const PROD_W: usize,
    const DROP: usize,
> Default for ComplexMixer<F, A_W, B_W, OUT_W, PROD_W, DROP>
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
    rhdl::bits::W<OUT_W>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
{
    fn default() -> Self {
        // Checked, not trusted.
        //
        // `convergent`'s second precondition -- that `v + half` must not
        // overflow `PROD_W` -- was recorded in prose and enforced by
        // nothing. Both output components here are a *difference of two*
        // products, each of which can reach `2^(A+B-2)` with the same
        // sign, so the difference reaches `2^(A+B-1)` and needs `A+B+1`
        // bits before the half-LSB is added.
        //
        // Getting it wrong is silent and it bites the largest sample in
        // the design: at `A_W = B_W = 8, PROD_W = 16` the case
        // `(-128 - 128j)(-128 + 127j)` gives `+32640`, rounding pushes
        // it to `32768`, and `SignedBits<16>` wraps that to `-32768`.
        // Found while writing `super::real_part::RealPartMixer`, which
        // forms the same difference and now carries the same check.
        assert!(
            PROD_W >= A_W + B_W + 1,
            "PROD_W must be at least A_W + B_W + 1: each output component is a \
             difference of two products, and `convergent` adds half an LSB on top"
        );
        assert!(DROP >= 1, "DROP must be at least one to round at all");
        Self {
            out: dff::DFF::default(),
            starved: dff::DFF::default(),
            mismatch: dff::DFF::default(),
            idle: Constant::new(Item::<Iq<OUT_W>, F>::default()),
        }
    }
}

/// Inputs to [`ComplexMixer`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<F: Digital + Default, const A_W: usize, const B_W: usize>
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
{
    /// First operand.
    pub a: Option<Item<Iq<A_W>, F>>,
    /// Second operand.
    pub b: Option<Item<Iq<B_W>, F>>,
    /// Downstream's ready, per the `RCStream` contract.
    pub downstream_ready: bool,
}

/// Outputs from [`ComplexMixer`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<F: Digital + Default, const OUT_W: usize>
where
    rhdl::bits::W<OUT_W>: BitWidth,
{
    /// The product stream.
    ///
    /// `stream.ready` is vacuously `true` — see the module docs. This
    /// mixer consumes on every cycle and has no stall path, so it is
    /// always ready to accept from upstream.
    pub stream: RCStream<Iq<OUT_W>, F>,
    /// The inputs did not both present data on some cycle.
    pub starved: bool,
    /// **The two inputs disagreed about framing.**
    ///
    /// Raised when both operands presented data and their framing
    /// markers differed. Registered, so it is asserted on the same cycle
    /// as the product those operands produced.
    ///
    /// For `F = ()` this can never fire: the unit type has one
    /// inhabitant, so the markers cannot differ. Getting a *defined*
    /// `false` out of that comparison needs the padding described in
    /// the kernel — unpadded, a zero-width comparison emits `x`. For
    /// `F = SyncMark`
    /// it is the alignment contract being enforced — one side claiming
    /// an anchor the other does not is a latency-compensation error,
    /// and the whole point of tagging at the source is that it becomes
    /// visible here instead of silently mis-timing an acquisition.
    ///
    /// The product still carries the `a` side's marker on a mismatched
    /// cycle. That value is not trustworthy; this flag is the one to
    /// act on. Substituting a default would be quieter and worse — it
    /// would let a chain with a scheduling bug look well-framed.
    pub frame_mismatch: bool,
    /// A sample was presented while `downstream_ready` was low, and is
    /// gone.
    ///
    /// Combinational on `downstream_ready`, which is the correct
    /// alignment: the sample at risk is the one on `stream` this cycle.
    pub overrun: bool,
}

impl<
    F: Digital + Default,
    const A_W: usize,
    const B_W: usize,
    const OUT_W: usize,
    const PROD_W: usize,
    const DROP: usize,
> SynchronousIO for ComplexMixer<F, A_W, B_W, OUT_W, PROD_W, DROP>
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
    rhdl::bits::W<OUT_W>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
{
    type I = In<F, A_W, B_W>;
    type O = Out<F, OUT_W>;
    type Kernel = complex_mixer_kernel<F, A_W, B_W, OUT_W, PROD_W, DROP>;
}

#[kernel]
#[doc(hidden)]
pub fn complex_mixer_kernel<
    F: Digital + Default,
    const A_W: usize,
    const B_W: usize,
    const OUT_W: usize,
    const PROD_W: usize,
    const DROP: usize,
>(
    cr: ClockReset,
    i: In<F, A_W, B_W>,
    q: Q<F, A_W, B_W, OUT_W, PROD_W, DROP>,
) -> (Out<F, OUT_W>, D<F, A_W, B_W, OUT_W, PROD_W, DROP>)
where
    rhdl::bits::W<A_W>: BitWidth,
    rhdl::bits::W<B_W>: BitWidth,
    rhdl::bits::W<OUT_W>: BitWidth,
    rhdl::bits::W<PROD_W>: BitWidth,
{
    let mut d = D::<F, A_W, B_W, OUT_W, PROD_W, DROP>::dont_care();
    d.out = q.out;
    d.starved = false;

    let zero_a = Iq::<A_W> {
        re: signed::<A_W>(0),
        im: signed::<A_W>(0),
    };
    let zero_b = Iq::<B_W> {
        re: signed::<B_W>(0),
        im: signed::<B_W>(0),
    };

    // Bind each operand as a whole item via `match`, rather than
    // seeding a mutable local and reassigning it under `if let`.
    //
    // The idiom `rcstream::zip` uses, and kept for the reason below
    // rather than because it is forced.
    //
    // It *was* forced: a mutable local of zero-width type, conditionally
    // reassigned, used to trip `check_rhif_flow` with "slot is read
    // before being written", so `if let` was unavailable at `F = ()`.
    // That is fixed -- the check now knows a slot with no bits cannot be
    // uninitialised -- so the choice is free again. The `match` form
    // stays because of the `None`-arm argument below, which is the part
    // that still matters.
    //
    // The `None` arms carry real zeros and the real un-marked
    // marker -- deliberately *not* `Item::dont_care()`.
    //
    // A don't-care here is a genuine bug, not a tidiness question. Both
    // arms of a mux always evaluate in hardware, so a don't-care operand
    // reaches the multiplier, and while the Rust simulator reads it as
    // zero, `iverilog` reads it as `x` and propagates. That divergence
    // showed up as `test_complex_mixer_hdl_works` failing with
    // `Expected 000111..., got 0x0111...` -- one bit of the output
    // bundle unknown in Verilog and defined in Rust.
    let (have_a, item_a) = match i.a {
        Some(it) => (true, it),
        None => (
            false,
            Item::<Iq<A_W>, F> {
                data: zero_a,
                frame: q.idle.frame,
            },
        ),
    };
    let (have_b, item_b) = match i.b {
        Some(it) => (true, it),
        None => (
            false,
            Item::<Iq<B_W>, F> {
                data: zero_b,
                frame: q.idle.frame,
            },
        ),
    };
    let av = item_a.data;
    let bv = item_b.data;
    let af = item_a.frame;
    let bf = item_b.frame;

    // The alignment check.  Gated on both operands being present: a
    // starved cycle is already reported by `starved`, and comparing a
    // real marker against a placeholder would double-report it as a
    // framing fault.
    //
    // The alignment check.  Gated on both operands being present: a
    // starved cycle is already reported by `starved`, and comparing a
    // real marker against a placeholder would double-report it as a
    // framing fault.
    //
    // At `F = ()` the markers cannot differ, and the lowering folds the
    // comparison to a constant `false` rather than materialising two
    // undriven zero-width registers.
    d.mismatch = have_a && have_b && (af != bf);

    if have_a && have_b {
        let ar = av.re.resize::<PROD_W>();
        let ai = av.im.resize::<PROD_W>();
        let br = bv.re.resize::<PROD_W>();
        let bi = bv.im.resize::<PROD_W>();

        // (ac - bd) + (ad + bc)i
        let re = ar * br - ai * bi;
        let im = ar * bi + ai * br;

        // The marker rides with the product it belongs to.  On a
        // mismatched cycle this is the `a` side's -- see
        // `Out::frame_mismatch` for why that value is reported rather
        // than quietly replaced.
        d.out = Item::<Iq<OUT_W>, F> {
            data: Iq::<OUT_W> {
                re: convergent::<PROD_W, OUT_W, DROP>(re),
                im: convergent::<PROD_W, OUT_W, DROP>(im),
            },
            frame: af,
        };
    } else {
        d.starved = true;
        // Zero sample, un-marked: a cycle with no valid product must not
        // anchor anything.
        d.out = q.idle;
    }

    let mut o = Out::<F, OUT_W> {
        stream: RCStream::<Iq<OUT_W>, F> {
            data: Some(q.out),
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
        frame_mismatch: q.mismatch,
    };

    if cr.reset.any() {
        d.out = q.idle;
        d.starved = false;
        d.mismatch = false;
        o.overrun = false;
        o.frame_mismatch = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;

    const A: usize = 18;
    const B: usize = 16;
    const O: usize = 18;
    // Complex: each partial product is A+B, and two are summed.
    const P: usize = 35;
    const DR: usize = 17;
    type Uut = ComplexMixer<(), A, B, O, P, DR>;

    const _: () = assert!(P == A + B + 1 && DR == P - O);

    fn feed(ar: i128, ai: i128, br: i128, bi: i128) -> In<(), A, B> {
        In::<(), A, B> {
            a: Some(Item::<Iq<A>, ()> {
                data: Iq::<A> {
                    re: signed::<A>(ar),
                    im: signed::<A>(ai),
                },
                frame: (),
            }),
            b: Some(Item::<Iq<B>, ()> {
                data: Iq::<B> {
                    re: signed::<B>(br),
                    im: signed::<B>(bi),
                },
                frame: (),
            }),
            downstream_ready: true,
        }
    }

    // ---- framing --------------------------------------------------
    //
    // A second instantiation at `F = SyncMark`, because the framing
    // contract is invisible at `F = ()` -- the unit type has one
    // inhabitant, so markers can never disagree there.

    use crate::dsp::sync::SyncMark;

    type Framed = ComplexMixer<SyncMark, A, B, O, P, DR>;

    /// One cycle of framed stimulus with the two markers set
    /// independently, so a test can make them disagree.
    fn feed_framed(a_sync: bool, b_sync: bool) -> In<SyncMark, A, B> {
        In::<SyncMark, A, B> {
            a: Some(Item::<Iq<A>, SyncMark> {
                data: Iq::<A> {
                    re: signed::<A>(1000),
                    im: signed::<A>(0),
                },
                frame: SyncMark { sync: a_sync },
            }),
            b: Some(Item::<Iq<B>, SyncMark> {
                data: Iq::<B> {
                    re: signed::<B>(1000),
                    im: signed::<B>(0),
                },
                frame: SyncMark { sync: b_sync },
            }),
            downstream_ready: true,
        }
    }

    fn run_framed(seq: Vec<In<SyncMark, A, B>>) -> Vec<Out<SyncMark, O>> {
        let uut = Framed::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect()
    }

    fn out_sync(o: &Out<SyncMark, O>) -> bool {
        match o.stream.data {
            Some(item) => item.frame.sync,
            None => panic!("the mixer emits every cycle"),
        }
    }

    /// Aligned markers pass through and raise nothing.
    #[test]
    fn aligned_markers_propagate() {
        let seq = vec![
            feed_framed(false, false),
            feed_framed(true, true),
            feed_framed(false, false),
            feed_framed(false, false),
        ];
        let out = run_framed(seq);
        // One cycle of reset, one cycle of mixer latency.
        const LAT: usize = 2;
        assert!(
            out.iter().all(|o| !o.frame_mismatch),
            "no mismatch expected"
        );
        let marked: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, o)| out_sync(o))
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            marked,
            vec![1 + LAT],
            "the marker should ride with its product"
        );
    }

    /// **A one-sided marker is an error, and is reported.**
    ///
    /// This is the alignment contract. Either side asserting alone means
    /// the two paths disagree about which sample is the anchor, which is
    /// a latency-compensation bug upstream; the mixer is the place it
    /// becomes visible.
    #[test]
    fn a_one_sided_marker_is_reported() {
        let seq = vec![
            feed_framed(false, false),
            feed_framed(true, false),
            feed_framed(false, true),
            feed_framed(false, false),
            feed_framed(false, false),
        ];
        let out = run_framed(seq);
        const LAT: usize = 2;
        let flagged: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, o)| o.frame_mismatch)
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            flagged,
            vec![1 + LAT, 2 + LAT],
            "both one-sided cycles should be flagged, and only those"
        );
    }

    /// A starved cycle is not a framing fault, and carries no marker.
    ///
    /// Two claims in one test because they are the same decision: with
    /// only one operand there is nothing to compare, so `starved` is the
    /// correct report and `frame_mismatch` would be a double-report --
    /// and the emitted zero sample must not anchor anything either.
    #[test]
    fn starvation_is_not_a_framing_fault() {
        let mut lone = feed_framed(true, true);
        lone.b = None;
        let seq = vec![feed_framed(false, false), lone, feed_framed(false, false)];
        let out = run_framed(seq);
        assert!(
            out.iter().all(|o| !o.frame_mismatch),
            "a starved cycle must not be reported as a framing fault"
        );
        assert!(
            out.iter().all(|o| !out_sync(o)),
            "a starved cycle must not carry a marker"
        );
        assert!(
            out.iter().any(|o| o.starved),
            "starvation should still be reported"
        );
    }

    /// Reset emits no marker and no fault.
    #[test]
    fn reset_is_unmarked() {
        const RESET_CYCLES: usize = 3;
        let uut = Framed::default();
        let seq: Vec<In<SyncMark, A, B>> = (0..6).map(|_| feed_framed(true, true)).collect();
        let out: Vec<Out<SyncMark, O>> = uut
            .run(seq.into_iter().with_reset(RESET_CYCLES).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect();
        for (k, o) in out.iter().take(RESET_CYCLES).enumerate() {
            assert!(!out_sync(o), "sample {k} was marked during reset");
            assert!(!o.frame_mismatch, "sample {k} flagged during reset");
        }
    }

    /// The framed mixer survives `iverilog`, both RTL and NTL.
    ///
    /// Separate from `test_complex_mixer_hdl_works`, which covers
    /// `F = ()`. The two instantiations emit different Verilog and the
    /// framing comparison only exists in this one.
    #[test]
    fn test_framed_mixer_hdl_works() -> miette::Result<()> {
        let uut = Framed::default();
        let seq = vec![
            feed_framed(false, false),
            feed_framed(true, true),
            feed_framed(true, false),
            feed_framed(false, false),
        ];
        let tb = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    fn run(seq: Vec<In<(), A, B>>) -> Vec<Out<(), O>> {
        let uut = Uut::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect()
    }

    fn got(o: &Out<(), O>) -> (i128, i128) {
        match o.stream.data {
            Some(item) => (item.data.re.raw(), item.data.im.raw()),
            None => panic!("the mixer must emit every cycle"),
        }
    }

    fn conv(v: i128) -> i128 {
        let half = 1i128 << (DR - 1);
        let lsbs = v & ((1i128 << DR) - 1);
        let q = (v + half) >> DR;
        if lsbs == half && q % 2 != 0 { q - 1 } else { q }
    }

    fn want(ar: i128, ai: i128, br: i128, bi: i128) -> (i128, i128) {
        (conv(ar * br - ai * bi), conv(ar * bi + ai * br))
    }

    #[test]
    fn default_construction() {
        let _uut = Uut::default();
    }

    /// The complex product is right across sign combinations, including
    /// the rotations that make cross terms cancel.
    #[test]
    fn the_complex_product_is_correct() {
        let cases = [
            (100_000i128, 0i128, 20_000i128, 0i128), // real x real
            (0, 100_000, 0, 20_000),                 // imag x imag -> negative real
            (100_000, 0, 0, 20_000),                 // real x imag -> imag
            (60_000, 80_000, 12_000, -9_000),
            (-60_000, 80_000, -12_000, -9_000),
            (131_070, -131_070, 32_767, -32_768),
        ];
        let seq: Vec<In<(), A, B>> = cases.iter().map(|c| feed(c.0, c.1, c.2, c.3)).collect();
        let out = run(seq);
        let mut checked = 0;
        for (k, c) in cases.iter().enumerate() {
            if k + 2 < out.len() {
                assert_eq!(
                    got(&out[k + 2]),
                    want(c.0, c.1, c.2, c.3),
                    "case {k}: {c:?}"
                );
                checked += 1;
            }
        }
        assert!(checked >= 4, "only {checked} cases compared");
    }

    /// `i · i = −1`: two purely imaginary operands give a **negative
    /// real** result.
    ///
    /// The sign flip is the thing most easily got wrong in a hand-
    /// written complex multiply, and it is invisible in a magnitude
    /// check.
    #[test]
    fn imaginary_times_imaginary_is_negative_real() {
        let out = run(vec![feed(0, 50_000, 0, 20_000); 4]);
        let (re, im) = got(&out[3]);
        assert!(re < 0, "i*i must be negative real, got re={re}");
        assert_eq!(im, 0, "a purely imaginary product has no imaginary part");
        assert_eq!((re, im), want(0, 50_000, 0, 20_000));
    }

    /// **The maximum-negative-squared case** — §3.3 of the design note.
    ///
    /// Both operands at full-scale negative. The natural width carries
    /// it, so no saturation is needed and none exists; this asserts the
    /// result is *correct* rather than wrapped.
    #[test]
    fn maximum_negative_squared_does_not_wrap() {
        let a_min = -(1i128 << (A - 1));
        let b_min = -(1i128 << (B - 1));
        let out = run(vec![feed(a_min, a_min, b_min, b_min); 4]);
        let (re, im) = got(&out[3]);
        let (wre, wim) = want(a_min, a_min, b_min, b_min);
        assert_eq!((re, im), (wre, wim), "full-scale-negative operands wrapped");
        // re = a_min*b_min - a_min*b_min = 0; im = 2*a_min*b_min > 0.
        assert_eq!(re, 0);
        assert!(
            im > 0,
            "the cross terms must give a large positive imaginary part"
        );
    }

    /// **Convergent rounding steers ties to even** — the property worth
    /// 5 dB of spur performance.
    ///
    /// Constructs products whose discarded bits are exactly one half and
    /// checks the result is even. Round-half-up fails this.
    #[test]
    fn ties_go_to_even() {
        // Choose operands whose real product is an exact tie:
        // ar*br - ai*bi == odd_multiple * 2^DR + 2^(DR-1)
        // A tie is a product of m*2^DR + 2^(DR-1) = 2^(DR-1) * (2m+1).
        // Factor it so both operands stay in range: ar = 2^(DR-1),
        // br = 2m+1.  Putting the tie in the *operand* would overflow
        // the 18-bit input, which is how the first version of this test
        // silently exercised nothing.
        let half = 1i128 << (DR - 1);
        assert!(
            half < (1i128 << (A - 1)),
            "the tie factor must fit the A operand"
        );
        let mut tested = 0;
        for m in [0i128, 1, 2, 3, 4] {
            let br = 2 * m + 1;
            let target = half * br;
            let out = run(vec![feed(half, 0, br, 0); 4]);
            let (re, _im) = got(&out[3]);
            assert_eq!(
                re,
                conv(target),
                "tie handling disagrees with the reference for {half} * {br}"
            );
            assert_eq!(
                re % 2,
                0,
                "a tie must round to even, got {re} for product {target}"
            );
            tested += 1;
        }
        assert_eq!(tested, 5, "not every tie case ran");
    }

    /// **No DC offset** — the property truncation would destroy.
    ///
    /// A zero-mean input sequence must give a zero-mean output. §3.6 of
    /// the design note measures truncation leaving a −79 dBc DC term,
    /// which in NMR is a fixed artefact at the carrier.
    #[test]
    fn the_output_has_no_dc_offset() {
        // A symmetric sweep: every value paired with its negation.
        const PAIRS: usize = 64;
        let mut seq: Vec<In<(), A, B>> = Vec::new();
        for k in 1..=PAIRS as i128 {
            seq.push(feed(k * 1500, k * 700, 9_000, 0));
            seq.push(feed(-k * 1500, -k * 700, 9_000, 0));
        }
        // Pad so every stimulus sample is observable: the result is one
        // cycle late and the stream is prefixed by a reset cycle.  Summing
        // an odd number of samples leaves one half of a +/- pair unmatched,
        // which reads as a huge false DC offset -- that is a test bug, and
        // it is what the first version of this test did.
        seq.extend(vec![feed(0, 0, 0, 0); 4]);
        let out = run(seq);
        let samples = 2 * PAIRS;
        let (sum_re, sum_im) = out
            .iter()
            .skip(2)
            .take(samples)
            .map(got)
            .fold((0i128, 0i128), |(a, b), (r, i)| (a + r, b + i));
        let n = samples as i128;
        // Tolerance must be well under the bias truncation produces.
        // Truncation rounds toward negative infinity, so it biases by
        // about half an LSB per sample -- a sum near -n/2.  A tolerance
        // of n/2 therefore passes under truncation and proves nothing,
        // which is what the first version of this test did.
        let tolerance = n / 8;
        assert!(
            sum_re.abs() <= tolerance && sum_im.abs() <= tolerance,
            "output mean is not zero: re sum {sum_re}, im sum {sum_im} over {n} \
             samples, tolerance {tolerance} -- truncation instead of convergent \
             rounding biases by about -n/2 = {}",
            -n / 2
        );
    }

    /// The resource claim: **four** multiplies for the general case,
    /// against two for `ComplexRealMixer`.
    /// **An under-width product is refused at construction.**
    ///
    /// `convergent`'s precondition, now enforced. Each output component
    /// is a difference of two products, so `PROD_W` needs `A+B+1` bits;
    /// at `A+B` the largest sample in a design wraps sign, silently. See
    /// the note in `Default`.
    #[test]
    #[should_panic(expected = "PROD_W must be at least")]
    fn an_under_width_product_is_rejected() {
        let _ = ComplexMixer::<(), A, B, O, { A + B }, DR>::default();
    }

    /// As is a `DROP` of zero, which cannot round.
    #[test]
    #[should_panic(expected = "DROP must be at least one")]
    fn a_zero_drop_is_rejected() {
        let _ = ComplexMixer::<(), A, B, O, P, 0>::default();
    }

    #[test]
    fn multiplier_count_is_as_claimed() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let mults = hdl.matches(" * ").count();
        assert_eq!(mults, 4, "expected 4 multiplies for Iq x Iq; found {mults}");
        Ok(())
    }

    /// Starvation is reported, not buffered.
    /// A sample presented while downstream is not ready is reported.
    ///
    /// The mixer cannot stall — `out` is overwritten every cycle — so the
    /// sample is genuinely gone rather than delayed. `Nco` reports the
    /// same condition; before the audit in
    /// `notes/dsp-nco-modulator-defects.md` this widget swallowed it.
    ///
    /// Verified able to fail: reverting `overrun` to a constant `false`
    /// makes the first assertion report it.
    #[test]
    fn a_lost_sample_is_reported() {
        let stalled = In::<(), A, B> {
            downstream_ready: false,
            ..feed(1000, 2000, 300, 400)
        };
        let mut seq = vec![feed(1000, 2000, 300, 400); 3];
        seq.push(stalled);
        seq.extend(vec![feed(1000, 2000, 300, 400); 3]);
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
    /// honest answer here does not depend on downstream.
    #[test]
    fn ready_does_not_depend_on_downstream() {
        let stalled = In::<(), A, B> {
            downstream_ready: false,
            ..feed(1000, 2000, 300, 400)
        };
        let out = run(vec![stalled; 6]);
        assert!(
            out.iter().all(|o| o.stream.ready),
            "stream.ready went low although the mixer has no stall path"
        );
    }

    #[test]
    fn starvation_is_reported() {
        let mut seq = vec![feed(1000, 2000, 300, 400); 3];
        seq.push(In::<(), A, B> {
            a: None,
            b: Some(Item::<Iq<B>, ()> {
                data: Iq::<B> {
                    re: signed::<B>(300),
                    im: signed::<B>(400),
                },
                frame: (),
            }),
            downstream_ready: true,
        });
        seq.extend(vec![feed(1000, 2000, 300, 400); 3]);
        let out = run(seq);
        assert!(out.iter().any(|o| o.starved), "starvation was not reported");
        assert!(out.iter().any(|o| !o.starved), "starved is stuck high");
    }

    /// Shared Tier-4/Tier-5 stimulus.
    ///
    /// Deliberately exercises **every** output, not just the datapath: a
    /// starved cycle and a not-ready cycle are included, so `starved` and
    /// `overrun` are driven high somewhere in the trace. A stimulus that
    /// left them constant would make the `iverilog` round-trip and the
    /// VCD digest cover them as tied-off wires, which is how a codegen
    /// bug in a flag output survives a green Tier 4.
    fn hdl_stimulus() -> Vec<In<(), A, B>> {
        let mut seq: Vec<In<(), A, B>> = (0..24i128)
            .map(|k| feed((k - 12) * 7000, (12 - k) * 5000, (k - 12) * 1500, k * 900))
            .collect();
        // Downstream drops ready: the registered sample is lost.
        seq.push(In::<(), A, B> {
            downstream_ready: false,
            ..feed(50_000, -30_000, 20_000, -10_000)
        });
        // Only one operand presents data: starvation.
        seq.push(In::<(), A, B> {
            b: None,
            ..feed(50_000, -30_000, 20_000, -10_000)
        });
        seq.extend((0..4i128).map(|k| feed(k * 2500, -k * 1800, k * 1200, k * 700)));
        seq
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
            module top_starved
            module top_mismatch
            module top_idle"#]];
        expect.assert_eq(&shape);
        Ok(())
    }

    /// Tier 4 — emitted Verilog agrees with the Rust simulation.
    #[test]
    fn test_complex_mixer_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn test_complex_mixer_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("complex_mixer");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["70394ee358d81171d62e6788b28ba54b4bad400214271aca0bff3f0fea86ed33"];
        let digest = vcd.dump_to_file(root.join("complex_mixer.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
