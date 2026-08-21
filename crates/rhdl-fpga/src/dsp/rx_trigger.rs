#![warn(missing_docs)]
//! `RxTrigger` — marks one received sample as the start of an acquisition.
//!
//! A receive chain needs to say *which* sample the acquisition began on,
//! and it needs to say it at the point where the answer is still exactly
//! knowable — at the source, before decimation, buffering, or any
//! downstream stage has had a chance to lose count. This widget is that
//! point. It passes samples through untouched and stamps
//! [`SyncMark`] on exactly one of them.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+RxTrigger+---------------+
      |                           |
+---->+ stream                    |
      |   Option<Item<Iq<W>,()>>  |
+---->+ arm         stream:       |
      |   pulse     RCStream      +----->
+---->+ downstream_   <Iq<W>,     |
      |   ready        SyncMark>  |
      |                    armed  +----->
      |                  overrun  +----->
      +---------------------------+
")]
//!
//!# Internals
//!
//! Three sub-circuits. The edge detector makes the request
//! level-insensitive, `pending` holds it until a sample can take it, and
//! `out` carries the sample and its marker in **one** register so the
//! two cannot come adrift.
#![doc = badascii_doc::badascii!(r"
                  +-+EdgeDetector+-+
   arm  +--------->                +--+ rising
                  +----------------+  |
                                      v
                  +-+DFF<bool>+----+ OR/latch
                  |    pending     +<-+
                  +-------+--------+  |
                          |           | cleared when a
                          v           | sample takes it
                       mark_now-------+
                          |
                  +-------v--------+
 stream +-------->+ DFF<Option<    +--------> stream
                  |  Item<Iq,Sync>>|          (marked)
                  +----------------+
")]
//!
//! # Trigger, not gate
//!
//! This marks a single sample; it does not open and close a window. An
//! acquisition *gate* — one that counts a length and stops — is a
//! strictly larger widget and can be built on top of this one. Nothing
//! about naming the start instant requires knowing the duration, and
//! the two concerns have different failure modes, so they are kept
//! apart.
//!
//! # The framing type changes here, on purpose
//!
//! Input is `Option<Item<Iq<W>, ()>>` — un-framed, as it comes off the
//! converter. Output is framed with [`SyncMark`]. That type change is
//! load-bearing: an un-framed sample stream cannot be connected to
//! anything expecting an anchored one, so a chain that forgot to
//! include the trigger fails to compile rather than silently acquiring
//! from an arbitrary sample.
//!
//! # Arming
//!
//! `arm` is taken on its **rising edge**, not its level. Holding the
//! line high therefore requests one mark, not a run of them — the
//! anchor stays unambiguous whatever the caller does with the signal.
//!
//! The request latches, and the mark is applied to the next sample that
//! actually passes, so arming during an idle cycle does the expected
//! thing rather than being dropped. With an isochronous source, which
//! is the normal case here, "the next sample" is the one present on the
//! same cycle as the edge.
//!
//! Re-arming while a mark is still pending is idempotent: one edge in,
//! one marked sample out. [`Out::armed`] exposes the pending state so a
//! sequencer can see that its request has not yet been consumed.
//!
//! # Latency
//!
//! [`RX_TRIGGER_LATENCY`] cycles from a sample arriving to it appearing
//! on the output, marker attached. One, because the output is
//! registered — and, like every latency in [`super::nco::latency`], it
//! is measured against the hardware rather than asserted, by
//! `latency_is_as_declared`.
//!
//! This constant is what a sequencer adds to the receive side when it
//! computes how far ahead of an acquisition to issue the oscillator's
//! configuration change. See
//! [`crate::dsp::sync`] for the alignment contract that arithmetic has
//! to satisfy.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/rx_trigger.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/rx_trigger.md")]

use rhdl::prelude::*;

use crate::core::dff;
use crate::core::edge_detector::EdgeDetector;
use crate::dsp::iq::Iq;
use crate::dsp::sync::{SyncMark, when};
use crate::rcstream::bus::{Item, RCStream};

/// Cycles from a sample arriving on `stream` to it leaving, marked.
///
/// One: the output item is registered. Measured by
/// `latency_is_as_declared`, not asserted — a latency constant that has
/// never been checked against the hardware is a comment that a
/// sequencer trusts with the experiment's timing.
pub const RX_TRIGGER_LATENCY: usize = 1;

/// Marks one received sample as the start of an acquisition.
///
/// `W` is the width of each `Iq` component.
#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
pub struct RxTrigger<const W: usize>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The registered output item, marker included.
    ///
    /// One register for sample and marker together, so the marker
    /// cannot come adrift from the sample it names.
    out: dff::DFF<Option<Item<Iq<W>, SyncMark>>>,
    /// An arm request has been latched and no sample has consumed it yet.
    pending: dff::DFF<bool>,
    /// Rising-edge detect on `arm`.
    ///
    /// The request is taken on the *edge*, not the level. Without this
    /// a caller who holds `arm` high marks every sample for as long as
    /// it is held, which makes the anchor ambiguous — caught by
    /// `a_held_arm_still_marks_only_one`, which failed on the
    /// level-sensitive first version.
    arm_edge: EdgeDetector,
}

/// Inputs to [`RxTrigger`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const W: usize>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The un-framed received sample stream.
    ///
    /// `None` is an idle cycle. An idle cycle does not consume a
    /// pending arm.
    pub stream: Option<Item<Iq<W>, ()>>,
    /// Request that the next passing sample be marked.
    ///
    /// **Rising-edge sensitive.** Holding it high requests one mark, not
    /// one per cycle. The request latches until a sample consumes it,
    /// and is idempotent while one is already pending.
    pub arm: bool,
    /// Downstream's ready, per the `RCStream` contract.
    ///
    /// **This widget does not stall**, and cannot: it sits on an
    /// isochronous receive path where a sample withheld is a sample
    /// lost, not delayed. A low `ready` is reported by [`Out::overrun`]
    /// rather than absorbed.
    pub downstream_ready: bool,
}

/// Outputs from [`RxTrigger`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const W: usize>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The sample stream, now framed with [`SyncMark`].
    ///
    /// `stream.ready` is vacuously `true`: the output register is
    /// overwritten every cycle, so this widget is always able to accept
    /// from upstream. Claiming anything else would be a false promise
    /// of the kind `dsp::mixer` was audited for.
    pub stream: RCStream<Iq<W>, SyncMark>,
    /// A mark is latched and waiting for a sample to attach to.
    pub armed: bool,
    /// A sample was presented while `downstream_ready` was low, and is
    /// gone.
    ///
    /// Combinational on `downstream_ready`, matching `Nco::overrun`:
    /// the sample at risk is the one on `stream` this cycle.
    pub overrun: bool,
}

impl<const W: usize> SynchronousIO for RxTrigger<W>
where
    rhdl::bits::W<W>: BitWidth,
{
    type I = In<W>;
    type O = Out<W>;
    type Kernel = rx_trigger_kernel<W>;
}

#[kernel]
#[doc(hidden)]
pub fn rx_trigger_kernel<const W: usize>(cr: ClockReset, i: In<W>, q: Q<W>) -> (Out<W>, D<W>)
where
    rhdl::bits::W<W>: BitWidth,
{
    let mut d = D::<W>::dont_care();

    let zero = Iq::<W> {
        re: signed::<W>(0),
        im: signed::<W>(0),
    };

    // Bind the incoming sample in both arms with a real zero rather than
    // `dont_care()`.  Both arms of a mux evaluate in hardware, and a
    // don't-care here reads as 0 in the Rust simulator but propagates as
    // `x` through `iverilog` -- a divergence the Tier-4 round-trip
    // catches and the Rust tiers do not.
    let (have, sample) = match i.stream {
        Some(it) => (true, it.data),
        None => (false, zero),
    };

    // Edge, not level -- see the `arm_edge` field. The detector's
    // output is combinational on its input, so an edge this cycle
    // counts this cycle and a sample present alongside it is the one
    // marked.
    d.arm_edge = i.arm;
    let pending = q.pending || q.arm_edge.rising;
    let mark_now = have && pending;

    d.out = if have {
        Some(Item::<Iq<W>, SyncMark> {
            data: sample,
            frame: when(mark_now),
        })
    } else {
        None
    };
    // Consumed only by a sample that actually passed.
    d.pending = pending && !have;

    let mut o = Out::<W> {
        stream: RCStream::<Iq<W>, SyncMark> {
            data: q.out,
            ready: true,
        },
        armed: q.pending,
        overrun: !i.downstream_ready,
    };

    if cr.reset.any() {
        d.out = None;
        d.pending = false;
        o.armed = false;
        o.overrun = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;

    const W: usize = 18;
    type Uut = RxTrigger<W>;

    fn sample(k: i128) -> Item<Iq<W>, ()> {
        Item::<Iq<W>, ()> {
            data: Iq::<W> {
                re: signed::<W>(k),
                im: signed::<W>(-k),
            },
            frame: (),
        }
    }

    fn feed(k: i128, arm: bool) -> In<W> {
        In::<W> {
            stream: Some(sample(k)),
            arm,
            downstream_ready: true,
        }
    }

    fn idle(arm: bool) -> In<W> {
        In::<W> {
            stream: None,
            arm,
            downstream_ready: true,
        }
    }

    /// A `Q` bundle for direct kernel calls.
    ///
    /// `rising` is the edge detector's *output*, which the framework
    /// would compute from `d.arm_edge`; at this tier the test supplies
    /// it, which is what lets a single cycle be examined in isolation.
    fn q_state(pending: bool, rising: bool, out: Option<Item<Iq<W>, SyncMark>>) -> Q<W> {
        Q::<W> {
            out,
            pending,
            arm_edge: crate::core::edge_detector::Edges {
                rising,
                falling: false,
                any: rising,
            },
        }
    }

    fn run(seq: Vec<In<W>>) -> Vec<Out<W>> {
        let uut = Uut::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect()
    }

    /// Marked-sample indices in an output stream.
    fn marked(out: &[Out<W>]) -> Vec<usize> {
        out.iter()
            .enumerate()
            .filter(|(_, o)| match o.stream.data {
                Some(item) => item.frame.sync,
                None => false,
            })
            .map(|(k, _)| k)
            .collect()
    }

    // ---- Tier 1: the kernel directly --------------------------------

    #[test]
    fn default_construction() {
        let _ = Uut::default();
    }

    /// Arming alongside a live sample marks that sample and consumes
    /// the request in the same cycle.
    #[test]
    fn kernel_marks_and_consumes_together() {
        let cr = ClockReset::dont_care();
        let q = q_state(false, true, None);
        let (_, d) = rx_trigger_kernel::<W>(cr, feed(7, true), q);
        match d.out {
            Some(item) => assert!(item.frame.sync, "the live sample should be marked"),
            None => panic!("a live sample must be forwarded"),
        }
        assert!(
            !d.pending,
            "the request was consumed, so nothing stays pending"
        );
    }

    /// Arming on an idle cycle latches instead of being dropped.
    #[test]
    fn kernel_latches_an_arm_with_no_sample() {
        let cr = ClockReset::dont_care();
        let q = q_state(false, true, None);
        let (_, d) = rx_trigger_kernel::<W>(cr, idle(true), q);
        assert_eq!(d.out, None, "an idle cycle forwards nothing");
        assert!(d.pending, "the request must survive to the next sample");
    }

    /// A latched request is applied to the next sample that arrives.
    #[test]
    fn kernel_applies_a_latched_request() {
        let cr = ClockReset::dont_care();
        let q = q_state(true, false, None);
        let (_, d) = rx_trigger_kernel::<W>(cr, feed(3, false), q);
        match d.out {
            Some(item) => assert!(item.frame.sync),
            None => panic!("a live sample must be forwarded"),
        }
        assert!(!d.pending);
    }

    /// An un-armed sample passes through unmarked and unaltered.
    #[test]
    fn kernel_passes_an_unarmed_sample_untouched() {
        let cr = ClockReset::dont_care();
        let q = q_state(false, false, None);
        let (_, d) = rx_trigger_kernel::<W>(cr, feed(1234, false), q);
        match d.out {
            Some(item) => {
                assert!(!item.frame.sync);
                assert_eq!(item.data.re, signed::<W>(1234));
                assert_eq!(item.data.im, signed::<W>(-1234));
            }
            None => panic!("a live sample must be forwarded"),
        }
    }

    /// Reset clears the pending request and the output.
    #[test]
    fn kernel_reset_clears_everything() {
        let cr = clock_reset(clock(false), reset(true));
        let q = q_state(
            true,
            true,
            Some(Item::<Iq<W>, SyncMark> {
                data: Iq::<W> {
                    re: signed::<W>(5),
                    im: signed::<W>(5),
                },
                frame: SyncMark { sync: true },
            }),
        );
        let (o, d) = rx_trigger_kernel::<W>(cr, feed(9, true), q);
        assert_eq!(d.out, None);
        assert!(!d.pending);
        assert!(!o.armed);
        assert!(!o.overrun);
    }

    // ---- Tier 2: simulation -----------------------------------------

    /// **Exactly one sample is marked per arm pulse.**
    ///
    /// The single most important property: a trigger that marked two
    /// samples would make the anchor ambiguous, and one that marked
    /// none would make it absent.
    #[test]
    fn one_pulse_marks_exactly_one_sample() {
        let mut seq: Vec<In<W>> = (0..16).map(|k| feed(k as i128, false)).collect();
        seq[5].arm = true;
        let out = run(seq);
        assert_eq!(marked(&out), vec![5 + 1 + RX_TRIGGER_LATENCY]);
    }

    /// Holding `arm` high does not mark a run of samples.
    ///
    /// The request is consumed by the first sample that takes it; a
    /// level-sensitive implementation would mark every sample for as
    /// long as the line was high, which is the bug this guards.
    #[test]
    fn a_held_arm_still_marks_only_one() {
        let mut seq: Vec<In<W>> = (0..16).map(|k| feed(k as i128, false)).collect();
        for s in seq.iter_mut().take(10).skip(4) {
            s.arm = true;
        }
        let out = run(seq);
        assert_eq!(
            marked(&out).len(),
            1,
            "a held arm marked {:?}; the request must be consumed once",
            marked(&out)
        );
    }

    /// An arm during a gap waits, and `armed` says so meanwhile.
    #[test]
    fn an_arm_during_a_gap_waits_for_the_next_sample() {
        let mut seq: Vec<In<W>> = (0..16).map(|k| feed(k as i128, false)).collect();
        for s in seq.iter_mut().take(9).skip(5) {
            s.stream = None;
        }
        seq[6].arm = true;
        let out = run(seq);
        // Sample 9 is the first live one after the gap.
        assert_eq!(marked(&out), vec![9 + 1 + RX_TRIGGER_LATENCY]);
        assert!(
            out.iter().any(|o| o.armed),
            "the pending state must be visible"
        );
    }

    /// **[`RX_TRIGGER_LATENCY`] measured, not asserted.**
    ///
    /// Finds the marked sample's index and the index at which the
    /// corresponding payload appears, and requires the declared
    /// constant to explain the gap. A sequencer adds this to the
    /// receive side when it decides how far ahead to configure the
    /// oscillator, so a wrong value mis-times an acquisition.
    #[test]
    fn latency_is_as_declared() {
        const ARM_AT: usize = 5;
        const RESET_CYCLES: usize = 1;
        let mut seq: Vec<In<W>> = (0..16).map(|k| feed(k as i128 + 1, false)).collect();
        seq[ARM_AT].arm = true;
        let out = run(seq);

        let marker = *marked(&out).first().expect("nothing was marked");
        // The payload identifies which stimulus sample this is: `re`
        // was seeded with the stimulus index + 1.
        let payload = match out[marker].stream.data {
            Some(item) => item.data.re.raw(),
            None => panic!("the marked cycle must carry a sample"),
        };
        assert_eq!(
            payload,
            ARM_AT as i128 + 1,
            "the marked sample is not the one that was present when armed"
        );
        assert_eq!(marker - (ARM_AT + RESET_CYCLES), RX_TRIGGER_LATENCY);
    }

    /// Nothing is marked while reset is asserted.
    #[test]
    fn reset_never_marks() {
        const RESET_CYCLES: usize = 4;
        let uut = Uut::default();
        let seq: Vec<In<W>> = (0..12).map(|k| feed(k as i128, true)).collect();
        let out: Vec<Out<W>> = uut
            .run(seq.into_iter().with_reset(RESET_CYCLES).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect();
        for (k, o) in out.iter().take(RESET_CYCLES).enumerate() {
            if let Some(item) = o.stream.data {
                assert!(!item.frame.sync, "sample {k} marked during reset");
            }
        }
    }

    /// A lost sample is reported rather than hidden.
    #[test]
    fn a_lost_sample_is_reported() {
        let mut seq: Vec<In<W>> = (0..12).map(|k| feed(k as i128, false)).collect();
        for s in seq.iter_mut().take(8).skip(5) {
            s.downstream_ready = false;
        }
        let out = run(seq);
        assert!(out.iter().any(|o| o.overrun), "the loss must be surfaced");
    }

    /// The outgoing ready does not depend on downstream's.
    ///
    /// This widget consumes unconditionally, so it is always ready to
    /// accept; forwarding `downstream_ready` would be a false claim.
    /// See `notes/dsp-nco-modulator-defects.md` finding 2 for the audit
    /// that established the criterion.
    #[test]
    fn ready_does_not_depend_on_downstream() {
        let mut seq: Vec<In<W>> = (0..8).map(|k| feed(k as i128, false)).collect();
        for s in seq.iter_mut() {
            s.downstream_ready = false;
        }
        let out = run(seq);
        assert!(out.iter().all(|o| o.stream.ready));
    }

    /// The claims made in `examples/rx_trigger.rs` prose, checked.
    ///
    /// The example's comments name specific sample indices; a reader
    /// compares them against the committed trace. Pinning them here
    /// means the prose cannot drift from the widget silently.
    #[test]
    fn the_example_behaves_as_its_comments_claim() {
        let uut = Uut::default();
        let seq: Vec<In<W>> = (0..28i128)
            .map(|k| {
                let theta = 2.0 * std::f64::consts::PI * (k as f64) / 16.0;
                let amp = 100_000.0;
                let gap = (20..23).contains(&k);
                In::<W> {
                    stream: if gap {
                        None
                    } else {
                        Some(Item::<Iq<W>, ()> {
                            data: Iq::<W> {
                                re: signed::<W>((theta.cos() * amp) as i128),
                                im: signed::<W>((theta.sin() * amp) as i128),
                            },
                            frame: (),
                        })
                    },
                    arm: k == 8 || k == 20,
                    downstream_ready: true,
                }
            })
            .collect();
        let out: Vec<Out<W>> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect();

        // Two arms, two marked samples: one against a live sample at
        // stimulus 8, one deferred out of the gap to stimulus 23.
        assert_eq!(
            marked(&out),
            vec![8 + 1 + RX_TRIGGER_LATENCY, 23 + 1 + RX_TRIGGER_LATENCY],
            "the example's marked samples are not where its comments say"
        );
        // `armed` is only observable across the gap.
        let armed: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, o)| o.armed)
            .map(|(k, _)| k)
            .collect();
        assert!(
            !armed.is_empty(),
            "the gap should make the pending state visible"
        );
        assert!(
            armed.iter().all(|&k| k > 20),
            "armed should only be high across the gap, saw {armed:?}"
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
            module top_out
            module top_pending
            module top_arm_edge
            module top_arm_edge_prev"#]];
        expect.assert_eq(&shape);
        Ok(())
    }

    // ---- Tier 4: iverilog round-trip --------------------------------

    fn hdl_stimulus() -> Vec<In<W>> {
        let mut seq: Vec<In<W>> = (0..16).map(|k| feed(k as i128 * 100, false)).collect();
        seq[4].arm = true;
        seq[9].stream = None;
        seq[9].arm = true;
        seq[12].downstream_ready = false;
        seq
    }

    #[test]
    fn test_rx_trigger_hdl_works() -> miette::Result<()> {
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
    fn test_rx_trigger_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("rx_trigger");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["9425fe8e4cb2d2296a63444983a5519e1295b5fe2439992d78f00f1be7c1ae93"];
        let digest = vcd.dump_to_file(root.join("rx_trigger.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
