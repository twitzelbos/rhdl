//! A queue of messages waiting to be sent.
//!
//! Without this, `start` asserted while the transmitter is busy
//! *abandons* the message in flight — documented behaviour, but a
//! documented sharp edge rather than a usable interface. An application
//! posting three writes in quick succession would land two of them in
//! the bin.
//!
//! This is FERoCE's `RoCE_simple_work_queue` in RHDL terms: the
//! requester posts work, the queue holds it, and the send path pulls
//! one message at a time.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +-----+WorkQueue+--------+
     |                        |
?Msg |                        | ?Msg
+---->| post            current+--->
     |                        |
<----+ accepted         depth +---> B<D+1>
     |                        |
bool |                        |
+--->| complete          full +---> bool
bool |                        |
+--->| flush            empty +---> bool
     +------------------------+
")]
//!
//!# How it is driven
//!
//! `post` offers a message and `accepted` reports whether it was taken
//! — the ordinary ready/valid handshake, so a requester that ignores
//! `accepted` loses work and one that honours it cannot. `current` is
//! the message at the head, held stable until `complete` strobes, which
//! is what the send path asserts when it has finished the last packet
//! of that message.
//!
//! **`complete`, not `ready`.** A send path that pulled on a ready
//! signal would have the queue advance when the *first* packet of a
//! multi-packet message was accepted, which is exactly wrong: the head
//! has to stay put until the whole message is on the wire. Naming the
//! input after the event rather than the handshake makes that hard to
//! misuse.
//!
//!# `flush` exists because of SQD
//!
//! [`crate::qp::state::QpState::Sqd`] means finish what is outstanding
//! and accept nothing new. That is this widget's job rather than the
//! state machine's, so the composition drives `flush` from
//! `draining` — the head keeps draining, and `accepted` stays low.
//!
//!# Internals
//!
//! A `fifo::synchronous` and nothing else. The depth is a const
//! generic; two or four entries is plenty for a sensor pushing
//! acquisition buffers, and the resource cost is one block RAM either
//! way.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/work_queue.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/work_queue.md")]

use super::context::MessageRequest;
use rhdl::prelude::*;
use rhdl_fpga::fifo::synchronous::SyncFIFO;

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [`WorkQueue`].
pub struct In {
    /// A message to enqueue, or `None`. Taken only when `accepted` is
    /// reported on the same cycle.
    pub post: Option<MessageRequest>,
    /// The send path has finished the **whole** message at the head.
    /// Strobe for one cycle; the head advances.
    pub complete: bool,
    /// Accept nothing new, but keep draining. Driven from the queue
    /// pair's `draining` output.
    pub flush: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs of the [`WorkQueue`].
pub struct Out {
    /// The message at the head, held until `complete`.
    pub current: Option<MessageRequest>,
    /// The posted message was taken this cycle.
    pub accepted: bool,
    /// No room for another message.
    pub full: bool,
    /// Nothing outstanding.
    pub empty: bool,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// A queue of `2^N - 1` pending messages.
///
/// `N` is the FIFO's address width, so the usable depth is `2^N - 1` —
/// one slot is spent distinguishing full from empty, as in every
/// ring-buffer FIFO.
pub struct WorkQueue<const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    fifo: SyncFIFO<MessageRequest, N>,
}

impl<const N: usize> Default for WorkQueue<N>
where
    rhdl::bits::W<N>: BitWidth,
{
    fn default() -> Self {
        Self {
            fifo: SyncFIFO::default(),
        }
    }
}

impl<const N: usize> SynchronousIO for WorkQueue<N>
where
    rhdl::bits::W<N>: BitWidth,
{
    type I = In;
    type O = Out;
    type Kernel = work_queue<N>;
}

#[kernel]
/// Kernel for [`WorkQueue`].
pub fn work_queue<const N: usize>(cr: ClockReset, i: In, q: Q<N>) -> (Out, D<N>)
where
    rhdl::bits::W<N>: BitWidth,
{
    let mut d = D::<N>::dont_care();
    let mut o = Out::dont_care();

    // Refuse new work while flushing, and when there is no room.
    let room = !q.fifo.full;
    let taking = match i.post {
        Some(_m) => room && !i.flush,
        None => false,
    };
    d.fifo.data = if taking { i.post } else { None };

    // `SyncFIFO` has no `empty` output: emptiness is `data == None`,
    // because the head is an `Option`. That is a better interface than a
    // separate flag -- the two cannot disagree -- but it is worth saying
    // once rather than rediscovering.
    let head_present = match q.fifo.data {
        Some(_m) => true,
        None => false,
    };

    // The head advances only on `complete`, never on a packet boundary.
    d.fifo.next = i.complete && head_present;

    o.current = q.fifo.data;
    o.accepted = taking;
    o.full = q.fifo.full;
    o.empty = !head_present;

    if cr.reset.any() {
        d.fifo.data = None;
        d.fifo.next = false;
        o.current = None;
        o.accepted = false;
        o.full = false;
        o.empty = true;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    type Uut = WorkQueue<3>; // 7 usable slots

    fn msg(n: u128) -> MessageRequest {
        MessageRequest {
            va: bits(0x1000 + n * 0x100),
            length: bits(n * 16),
        }
    }

    fn idle() -> In {
        In {
            post: None,
            complete: false,
            flush: false,
        }
    }

    /// Run a stimulus and return (accepted messages, head sequence).
    fn run(stream: Vec<In>) -> (Vec<MessageRequest>, Vec<Option<MessageRequest>>) {
        let uut = Uut::default();
        let inputs = stream.clone();
        let samples: Vec<_> = uut
            .run(stream.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| (s.output.accepted, s.output.current))
            .collect();
        // `with_reset(1)` shifts samples one later than inputs.
        let accepted = samples
            .iter()
            .enumerate()
            .filter(|(_, (a, _))| *a)
            .filter_map(|(m, _)| inputs.get(m.wrapping_sub(1)).and_then(|i| i.post))
            .collect();
        let heads = samples.iter().map(|(_, c)| *c).collect();
        (accepted, heads)
    }

    // ---------------- Tier 1 ----------------

    #[test]
    fn an_empty_queue_has_no_head() {
        let cr = ClockReset::dont_care();
        let q = Q::<3> {
            fifo: rhdl_fpga::fifo::synchronous::Out {
                data: None,
                full: false,
                almost_empty: true,
                almost_full: false,
                overflow: false,
                underflow: false,
            },
        };
        let (o, _d) = work_queue::<3>(cr, idle(), q);
        assert_eq!(o.current, None);
    }

    /// The head advances on `complete` and on nothing else — in
    /// particular not merely because the queue is non-empty.
    #[test]
    fn the_head_advances_only_on_complete() {
        let mut stream = vec![
            In {
                post: Some(msg(1)),
                ..idle()
            },
            In {
                post: Some(msg(2)),
                ..idle()
            },
        ];
        // Many idle cycles: the head must not move.
        for _ in 0..6 {
            stream.push(idle());
        }
        stream.push(In {
            complete: true,
            ..idle()
        });
        for _ in 0..6 {
            stream.push(idle());
        }
        let (_acc, heads) = run(stream);
        let seen: Vec<MessageRequest> = heads.iter().flatten().copied().collect();
        // Only two distinct heads should ever appear, in order.
        let mut distinct: Vec<MessageRequest> = Vec::new();
        for h in seen {
            if distinct.last() != Some(&h) {
                distinct.push(h);
            }
        }
        assert_eq!(
            distinct,
            vec![msg(1), msg(2)],
            "the head should be msg(1) until `complete`, then msg(2)"
        );
    }

    /// Posting is refused while flushing, which is how SQD drains.
    #[test]
    fn flush_refuses_new_work_but_keeps_the_head() {
        let cr = ClockReset::dont_care();
        let q = Q::<3> {
            fifo: rhdl_fpga::fifo::synchronous::Out {
                data: Some(msg(1)),
                full: false,
                almost_empty: false,
                almost_full: false,
                overflow: false,
                underflow: false,
            },
        };
        let (o, d) = work_queue::<3>(
            cr,
            In {
                post: Some(msg(2)),
                complete: false,
                flush: true,
            },
            q,
        );
        assert!(!o.accepted, "flush must refuse new work");
        assert_eq!(d.fifo.data, None, "and must not enqueue it");
        assert_eq!(o.current, Some(msg(1)), "but the head keeps draining");
    }

    /// A full queue refuses work rather than dropping it silently.
    #[test]
    fn a_full_queue_refuses_work() {
        let cr = ClockReset::dont_care();
        let q = Q::<3> {
            fifo: rhdl_fpga::fifo::synchronous::Out {
                data: Some(msg(1)),
                full: true,
                almost_empty: false,
                almost_full: true,
                overflow: false,
                underflow: false,
            },
        };
        let (o, d) = work_queue::<3>(
            cr,
            In {
                post: Some(msg(9)),
                ..idle()
            },
            q,
        );
        assert!(o.full);
        assert!(!o.accepted);
        assert_eq!(d.fifo.data, None);
    }

    /// `complete` on an empty queue is a no-op, not an underflow.
    #[test]
    fn complete_on_an_empty_queue_does_nothing() {
        let cr = ClockReset::dont_care();
        let q = Q::<3> {
            fifo: rhdl_fpga::fifo::synchronous::Out {
                data: None,
                full: false,
                almost_empty: true,
                almost_full: false,
                overflow: false,
                underflow: false,
            },
        };
        let (_o, d) = work_queue::<3>(
            cr,
            In {
                complete: true,
                ..idle()
            },
            q,
        );
        assert!(!d.fifo.next, "must not pop an empty queue");
    }

    #[test]
    fn reset_empties_the_queue() {
        let cr = clock_reset(clock(false), reset(true));
        let q = Q::<3> {
            fifo: rhdl_fpga::fifo::synchronous::Out {
                data: Some(msg(1)),
                full: true,
                almost_empty: false,
                almost_full: true,
                overflow: false,
                underflow: false,
            },
        };
        let (o, d) = work_queue::<3>(
            cr,
            In {
                post: Some(msg(2)),
                complete: true,
                flush: false,
            },
            q,
        );
        assert_eq!(o.current, None);
        assert!(o.empty);
        assert!(!o.accepted);
        assert_eq!(d.fifo.data, None);
        assert!(!d.fifo.next);
    }

    // ---------------- Tier 2 ----------------

    /// Several messages posted back to back come out in order, one per
    /// `complete`, and none are lost.
    #[test]
    fn messages_come_out_in_order_and_none_are_lost() {
        let mut stream = Vec::new();
        for n in 1..=4u128 {
            stream.push(In {
                post: Some(msg(n)),
                ..idle()
            });
        }
        let mut order: Vec<MessageRequest> = Vec::new();
        for _ in 0..4 {
            stream.push(In {
                complete: true,
                ..idle()
            });
            stream.push(idle());
        }
        let (accepted, heads) = run(stream);
        assert_eq!(
            accepted,
            (1..=4).map(msg).collect::<Vec<_>>(),
            "all four posts should be accepted"
        );
        for h in heads.iter().flatten() {
            if order.last() != Some(h) {
                order.push(*h);
            }
        }
        assert_eq!(
            order,
            (1..=4).map(msg).collect::<Vec<_>>(),
            "heads should appear in posting order"
        );
    }

    /// Posting into a queue that is being emptied at the same time
    /// works — the FIFO handles the simultaneous case and this test
    /// pins that the wrapper does not break it.
    #[test]
    fn posting_and_completing_on_the_same_cycle_is_fine() {
        let mut stream = vec![In {
            post: Some(msg(1)),
            ..idle()
        }];
        for n in 2..=4u128 {
            stream.push(In {
                post: Some(msg(n)),
                complete: true,
                flush: false,
            });
        }
        for _ in 0..6 {
            stream.push(In {
                complete: true,
                ..idle()
            });
            stream.push(idle());
        }
        let (accepted, _heads) = run(stream);
        assert_eq!(
            accepted.len(),
            4,
            "all four posts should be accepted even while completing: {accepted:?}"
        );
    }

    // ---------------- Tier 3/4/5 ----------------

    fn stimulus() -> Vec<In> {
        let mut v = Vec::new();
        for n in 1..=3u128 {
            v.push(In {
                post: Some(msg(n)),
                ..idle()
            });
        }
        v.push(idle());
        v.push(In {
            complete: true,
            ..idle()
        });
        v.push(idle());
        // Refuse a post while flushing.
        v.push(In {
            post: Some(msg(9)),
            complete: false,
            flush: true,
        });
        v.push(In {
            complete: true,
            ..idle()
        });
        v.push(idle());
        v.push(In {
            complete: true,
            ..idle()
        });
        v.push(idle());
        v
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        // Memory-backed, so the full text is mostly RAM declarations;
        // pinned by structure like `fifo::synchronous` itself.
        assert!(hdl.contains("module top"));
        assert!(
            hdl.contains("top_fifo"),
            "the FIFO child should be instantiated"
        );
        let expect = expect!["true"];
        expect.assert_eq(&(hdl.matches("endmodule").count() >= 2).to_string());
        Ok(())
    }

    #[test]
    fn test_work_queue_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let input = stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        // `.skip(2)`: the FIFO is block-RAM backed, so its output
        // register is `x` in Verilog until the first read completes.
        let opts = TestBenchOptions::default().skip(2);
        tb.rtl(&uut, &opts)?.run_iverilog()?;
        tb.ntl(&uut, &opts)?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_work_queue_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let input = stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("work_queue");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["713265f4cc611bc828743175bfc5f88d4e60fb5d9e73d40cb963ed3b5028ba27"];
        let digest = vcd.dump_to_file(root.join("work_queue.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
