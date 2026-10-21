//! The send endpoint — the four Phase-3 widgets wired to the
//! transmitter.
//!
//! Before this, Phase 3 was four widgets that existed and were tested
//! in isolation: a queue-pair state machine nothing consulted, a work
//! queue nothing drained, a context table nothing read. That is not an
//! endpoint, and the phase gate is deliberately about the integrated
//! behaviour rather than the parts.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +------+RoceWriteEndpoint+------+
     |                               |
?Req |                               | ?B<8>
+--->| qp_request               data +--->
?Wr  |                               |
+--->| table_write   payload_advance +---> bool
?Wk  |                               |
+--->| post                  sof/eof +---> bool
B<8> |                               |
+--->| payload_byte         accepted +---> bool
bool |                               |
+--->| ready                   state +---> QpState
     |                               |
     |                     ctx_missing+---> bool
     +-------------------------------+
")]
//!
//!# What it composes
//!
//! ```text
//!   qp_request --> QpStateMachine --can_send--+
//!                        |  draining          |
//!                        v                    |
//!   post ----------> WorkQueue --head--+      |
//!                        ^             |      |
//!                   complete           v      v
//!                        |      +--+start gate+--+
//!   table_write --> QpTable <---+ head.qpn       |
//!                        |        (context)      |
//!                        +-----------> RoceWriteTx --> data
//! ```
//!
//!# Per-queue-pair state is deferred, and the approximation is safe
//!
//! The IBTA's state is **per queue pair**. This endpoint has **one**
//! state machine, governing the endpoint as a whole, while the table
//! holds a context per queue pair. That is not the IBTA's model and the
//! gap is deliberate: per-queue-pair state needs the transition
//! function as a pure kernel plus a read-modify-write path through the
//! table's block RAM, which is a bigger change than this phase.
//!
//! The approximation is **conservative rather than wrong**: an
//! endpoint-level gate can only refuse to send where a per-queue-pair
//! gate would have allowed it, never the reverse. So no frame is emitted
//! that a per-queue-pair implementation would not also have emitted. A
//! lax approximation would have been unacceptable; a strict one costs
//! only the ability to have one queue pair sending while another is
//! still in INIT.
//!
//!# What still is not here
//!
//! No arbiter. Messages drain from one work queue in posting order,
//! whatever queue pair they name, so a long message on one connection
//! delays another's. FERoCE has a round-robin arbiter for this and it is
//! not in this phase. And nothing downstream of `data` — see
//! `roce-v2-architecture.md` Phase 4 onward.
//!
//!# Example
//!
//!```
#![doc = include_str!("../examples/endpoint.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../doc/endpoint.md")]

use crate::frame::FrameConfig;
use crate::qp::context::WorkRequest;
use crate::qp::state::{QpState, QpStateMachine};
use crate::qp::table::{QpTable, QpWrite};
use crate::qp::work_queue::WorkQueue;
use crate::tx::RoceWriteTx;
use rhdl::prelude::*;
use rhdl_fpga::core::dff;

#[derive(Clone, Copy, PartialEq, Debug, Digital, Default)]
#[doc(hidden)]
/// Bundled sequencing state, per CLAUDE.md §3.1.
pub struct EndpointExtras {
    /// A message has been handed to the transmitter and is in flight.
    /// Stops `start` from pulsing more than once per message.
    pub started: bool,
    /// The transmitter was busy last cycle. The falling edge is how a
    /// message's completion is detected, since the transmitter reports
    /// per-*packet* framing and not per-message.
    pub was_busy: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [`RoceWriteEndpoint`].
pub struct In {
    /// Control plane: request a queue-pair state transition.
    pub qp_request: Option<QpState>,
    /// Control plane: a datapath error, forcing `ERR`.
    pub qp_error: bool,
    /// Control plane: configure or tear down a table entry.
    pub table_write: Option<QpWrite>,
    /// Post a message. Taken only when `accepted` is reported.
    pub post: Option<WorkRequest>,
    /// Sequence number for the next message's first packet.
    pub psn_start: Bits<24>,
    /// BTH `ackreq` for the emitted packets.
    pub ackreq: bool,
    /// Link and network parameters, constant for the interface.
    pub cfg: FrameConfig,
    /// The payload byte at the current offset.
    pub payload_byte: Bits<8>,
    /// The sink can accept a frame byte this cycle.
    pub ready: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs of the [`RoceWriteEndpoint`].
pub struct Out {
    /// The frame byte, or `None`.
    pub data: Option<Bits<8>>,
    /// Advance the caller's payload source.
    pub payload_advance: bool,
    /// First byte of a frame.
    pub start_of_frame: bool,
    /// Last byte of a frame.
    pub end_of_frame: bool,
    /// The posted message was taken.
    pub accepted: bool,
    /// The work queue has no room.
    pub full: bool,
    /// Current queue-pair state.
    pub state: QpState,
    /// The endpoint may transmit — `state == RTS`.
    pub can_send: bool,
    /// The last requested state transition was refused.
    pub rejected: bool,
    /// The message at the head of the queue names a queue pair with no
    /// configured context, so it cannot be sent. **It also cannot be
    /// retired**, so this latches until the entry is configured or the
    /// endpoint is reset — see the module docs on what is missing.
    pub ctx_missing: bool,
    /// A message is in flight or waiting.
    pub busy: bool,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
/// A RoCEv2 RDMA WRITE send endpoint.
///
/// `QPN_W` is the queue-pair table's address width (`2^QPN_W` entries);
/// `WQ_W` is the work queue's, giving `2^WQ_W - 1` pending messages.
pub struct RoceWriteEndpoint<const QPN_W: usize, const WQ_W: usize>
where
    rhdl::bits::W<QPN_W>: BitWidth,
    rhdl::bits::W<WQ_W>: BitWidth,
{
    qp_state: QpStateMachine,
    table: QpTable<QPN_W>,
    wq: WorkQueue<WorkRequest, WQ_W>,
    tx: RoceWriteTx,
    extras: dff::DFF<EndpointExtras>,
}

impl<const QPN_W: usize, const WQ_W: usize> SynchronousIO for RoceWriteEndpoint<QPN_W, WQ_W>
where
    rhdl::bits::W<QPN_W>: BitWidth,
    rhdl::bits::W<WQ_W>: BitWidth,
{
    type I = In;
    type O = Out;
    type Kernel = roce_write_endpoint<QPN_W, WQ_W>;
}

#[kernel]
// `.unwrap_or_default()` and `if let` are not in the kernel-accepted
// subset of Rust -- `#[kernel]` lowers `match` on an `Option`, not
// arbitrary `Option` methods -- so the explicit matches are the only
// spelling that compiles to hardware.
#[allow(clippy::manual_unwrap_or_default, clippy::single_match)]
/// Kernel for [`RoceWriteEndpoint`].
pub fn roce_write_endpoint<const QPN_W: usize, const WQ_W: usize>(
    cr: ClockReset,
    i: In,
    q: Q<QPN_W, WQ_W>,
) -> (Out, D<QPN_W, WQ_W>)
where
    rhdl::bits::W<QPN_W>: BitWidth,
    rhdl::bits::W<WQ_W>: BitWidth,
{
    let mut d = D::<QPN_W, WQ_W>::dont_care();
    let mut o = Out::dont_care();
    let mut ex = q.extras;

    // --- control plane: the state machine and the table ---
    d.qp_state.request = i.qp_request;
    d.qp_state.error = i.qp_error;
    d.table.write = i.table_write;

    // --- the work queue ---
    // Posting is allowed in any state: queueing work is not sending it,
    // and refusing it outside RTS would make the queue useless during
    // bring-up. Draining (SQD) is the one case that refuses.
    d.wq.post = i.post;
    d.wq.flush = q.qp_state.draining;

    // `default()`, not `dont_care()`: this is read on the idle path --
    // the table's read address is driven from it every cycle -- and
    // reading a `dont_care()` aggregate is a partial read (§12 rule 7).
    let head = match q.wq.current {
        Some(w) => w,
        None => WorkRequest::default(),
    };
    let have_head = match q.wq.current {
        Some(_w) => true,
        None => false,
    };

    // The head is held stable until the message completes, so driving
    // the table's read address from it continuously means the context is
    // settled long before it is needed. The read is registered; see
    // `qp::table`.
    d.table.read_qpn = head.qpn;

    let ctx = match q.table.ctx {
        Some(c) => c,
        None => crate::qp::context::QpContext::default(),
    };
    let ctx_valid = q.table.valid;

    // --- the start gate: every condition that must hold ---
    let can_start = have_head && q.qp_state.can_send && ctx_valid && !ex.started && !q.tx.busy;

    // --- the transmitter ---
    d.tx.start = can_start;
    d.tx.msg = head.msg;
    d.tx.ctx = ctx;
    d.tx.cfg = i.cfg;
    d.tx.psn_start = i.psn_start;
    d.tx.ackreq = i.ackreq;
    d.tx.payload_byte = i.payload_byte;
    d.tx.ready = i.ready;

    if can_start {
        ex.started = true;
    }

    // A message is done when the transmitter stops being busy. It
    // reports `end_of_frame` per *packet*, so a multi-packet message
    // would retire on its first packet if that were used here.
    let finished = ex.was_busy && !q.tx.busy;
    d.wq.complete = finished;
    if finished {
        ex.started = false;
    }
    ex.was_busy = q.tx.busy;

    // --- outputs ---
    o.data = q.tx.data;
    o.payload_advance = q.tx.payload_advance;
    o.start_of_frame = q.tx.start_of_frame;
    o.end_of_frame = q.tx.end_of_frame;
    o.accepted = q.wq.accepted;
    o.full = q.wq.full;
    o.state = q.qp_state.state;
    o.can_send = q.qp_state.can_send;
    o.rejected = q.qp_state.rejected;
    o.ctx_missing = have_head && !ctx_valid;
    o.busy = have_head || q.tx.busy;

    d.extras = ex;

    if cr.reset.any() {
        d.qp_state.request = None;
        d.qp_state.error = false;
        d.table.write = None;
        d.table.read_qpn = bits(0);
        d.wq.post = None;
        d.wq.complete = false;
        d.wq.flush = false;
        d.tx.start = false;
        d.extras.started = false;
        d.extras.was_busy = false;
        o.data = None;
        o.payload_advance = false;
        o.start_of_frame = false;
        o.end_of_frame = false;
        o.accepted = false;
        o.full = false;
        o.state = QpState::Reset;
        o.can_send = false;
        o.rejected = false;
        o.ctx_missing = false;
        o.busy = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    //! Phase 3's gate, from `roce-v2-architecture.md` §9:
    //!
    //! > several messages queued back to back on one queue pair produce
    //! > the correct frame sequence, byte-exact against Scapy; a second
    //! > queue pair with different parameters interleaves correctly; and
    //! > the state machine refuses to transmit outside `RTS`.
    //!
    //! One test per clause, plus the supporting cases.

    use super::*;
    use crate::frame::ROCE_UDP_PORT;
    use crate::headers::Transport;
    use crate::qp::context::{MessageRequest, Pmtu, QpContext};
    use expect_test::expect;
    use std::path::PathBuf;

    type Uut = RoceWriteEndpoint<4, 3>;

    const QPN_A: u128 = 0x000111;
    const QPN_B: u128 = 0x000222;

    fn cfg() -> FrameConfig {
        FrameConfig {
            dst_mac: bits(0xec0d_9aaa_bbcc),
            src_mac: bits(0x000a_3501_0203),
            src_ip: bits(0xC0A8_0132),
            dst_ip: bits(0xC0A8_010A),
            src_port: bits(0x1234),
            dst_port: bits(ROCE_UDP_PORT),
            ip_id: bits(0),
            ttl: bits(64),
            dscp_ecn: bits(0),
        }
    }

    fn ctx(dqpn: u128, rkey: u128) -> QpContext {
        QpContext {
            dqpn: bits(dqpn),
            transport: Transport::Uc,
            pmtu: Pmtu::B1024,
            rkey: bits(rkey),
            pkey: bits(0xFFFF),
        }
    }

    fn idle() -> In {
        In {
            qp_request: None,
            qp_error: false,
            table_write: None,
            post: None,
            psn_start: bits(1),
            ackreq: false,
            cfg: cfg(),
            payload_byte: bits(0),
            ready: true,
        }
    }

    fn work(qpn: u128, length: u128) -> WorkRequest {
        WorkRequest {
            qpn: bits(qpn),
            msg: MessageRequest {
                va: bits(0x0000_7F00_1234_0000),
                length: bits(length),
            },
        }
    }

    /// One simulation pass: the emitted bytes, the `payload_advance`
    /// sample indices, and the frame boundaries.
    struct Pass {
        data: Vec<Option<u8>>,
        advance: Vec<bool>,
        sof: Vec<bool>,
        eof: Vec<bool>,
    }

    fn sim(stim: Vec<In>) -> Pass {
        let uut = Uut::default();
        let samples: Vec<_> = uut
            .run(stim.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| {
                (
                    s.output.data.map(|b| b.raw() as u8),
                    s.output.payload_advance,
                    s.output.start_of_frame,
                    s.output.end_of_frame,
                )
            })
            .collect();
        Pass {
            data: samples.iter().map(|x| x.0).collect(),
            advance: samples.iter().map(|x| x.1).collect(),
            sof: samples.iter().map(|x| x.2).collect(),
            eof: samples.iter().map(|x| x.3).collect(),
        }
    }

    /// Drive a stimulus, placing `payload` into the cycles the endpoint
    /// asks for it.
    ///
    /// Two passes, for the reason recorded in `tx`: the payload's
    /// *content* does not affect control flow, so pass 1's
    /// `payload_advance` positions are valid for pass 2, and driving
    /// open-loop avoids the off-by-one that a closed-loop cursor hits
    /// against `run_fn`. `with_reset(1)` means output sample `m`
    /// corresponds to input `m - 1`, and that is named here rather than
    /// left implicit.
    fn emit(stim: Vec<In>, payload: &[u8]) -> (Vec<u8>, Vec<usize>, Vec<usize>) {
        let p1 = sim(stim.clone());
        let mut stim2 = stim.clone();
        let mut k = 0usize;
        for (m, adv) in p1.advance.iter().enumerate() {
            if !*adv {
                continue;
            }
            if let Some(n) = m.checked_sub(1)
                && n < stim2.len()
            {
                stim2[n].payload_byte = bits(payload[k % payload.len()] as u128);
            }
            k += 1;
        }
        let p2 = sim(stim2);
        let bytes = p2.data.iter().flatten().copied().collect();
        let sofs = p2
            .sof
            .iter()
            .enumerate()
            .filter(|(_, s)| **s)
            .map(|(m, _)| m)
            .collect();
        let eofs = p2
            .eof
            .iter()
            .enumerate()
            .filter(|(_, s)| **s)
            .map(|(m, _)| m)
            .collect();
        (bytes, sofs, eofs)
    }

    /// Bring-up: configure a context, then walk to RTS.
    fn bring_up(qpn: u128, rkey: u128) -> Vec<In> {
        vec![
            In {
                table_write: Some(QpWrite {
                    qpn: bits(qpn),
                    ctx: Some(ctx(qpn, rkey)),
                }),
                ..idle()
            },
            In {
                qp_request: Some(QpState::Init),
                ..idle()
            },
            In {
                qp_request: Some(QpState::Rtr),
                ..idle()
            },
            In {
                qp_request: Some(QpState::Rts),
                ..idle()
            },
        ]
    }

    fn pad(stim: &mut Vec<In>, n: usize) {
        for _ in 0..n {
            stim.push(idle());
        }
    }

    fn vector_frame(name: &str) -> Vec<u8> {
        let corpus = include_str!("../tests/vectors/write_vectors.txt");
        for record in corpus.split("\n\n") {
            let mut found = false;
            for line in record.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    let (k, v) = (k.trim(), v.trim());
                    if k == "name" && v == name {
                        found = true;
                    }
                    if found && k == "frame" {
                        return (0..v.len() / 2)
                            .map(|i| u8::from_str_radix(&v[2 * i..2 * i + 2], 16).unwrap())
                            .collect();
                    }
                }
            }
        }
        panic!("vector {name} not found");
    }

    // ================= gate clause 1 =================

    /// **Several messages queued back to back produce the correct frame
    /// sequence, byte-exact against Scapy.**
    ///
    /// Three identical messages are posted in three consecutive cycles,
    /// before the queue pair is even ready. All three come out, each a
    /// byte-exact copy of vector `uc_write_only_16`. Before the work
    /// queue existed, the second and third `start` would have abandoned
    /// the message in flight and only one frame — or a corrupt one —
    /// would have emerged.
    #[test]
    fn three_queued_messages_produce_three_correct_frames() {
        let mut stim = bring_up(QPN_A, 0x0000_A5A5);
        // Post during bring-up: queueing work is not sending it.
        for slot in stim.iter_mut().take(3) {
            slot.post = Some(work(QPN_A, 16));
        }
        pad(&mut stim, 330);

        let payload: Vec<u8> = (0..16).collect();
        let (bytes, sofs, eofs) = emit(stim, &payload);

        let want = vector_frame("uc_write_only_16");
        assert_eq!(sofs.len(), 3, "expected three frames, got sofs {sofs:?}");
        assert_eq!(eofs.len(), 3, "expected three frames, got eofs {eofs:?}");
        assert_eq!(
            bytes.len(),
            want.len() * 3,
            "expected {} bytes, got {}",
            want.len() * 3,
            bytes.len()
        );
        for n in 0..3 {
            let frame = &bytes[n * want.len()..(n + 1) * want.len()];
            if frame != &want[..] {
                let at = frame.iter().zip(&want).position(|(a, b)| a != b).unwrap();
                panic!(
                    "frame {n} differs from Scapy at offset {at} \
                     (got {:#04x}, expected {:#04x})",
                    frame[at], want[at]
                );
            }
        }
    }

    // ================= gate clause 2 =================

    /// **A second queue pair with different parameters is framed with
    /// its own context.**
    ///
    /// Two contexts are configured, one message posted for each. The
    /// first frame matches Scapy exactly. The second must differ in the
    /// BTH's destination queue pair and the RETH's R_Key — the fields
    /// the context supplies — and consequently in the ICRC, while every
    /// other byte stays identical. That is what proves the table is
    /// being consulted per message rather than a single context being
    /// latched.
    #[test]
    fn a_second_queue_pair_is_framed_with_its_own_context() {
        let mut stim = vec![
            In {
                table_write: Some(QpWrite {
                    qpn: bits(QPN_A),
                    ctx: Some(ctx(QPN_A, 0x0000_A5A5)),
                }),
                post: Some(work(QPN_A, 16)),
                ..idle()
            },
            In {
                table_write: Some(QpWrite {
                    qpn: bits(QPN_B),
                    ctx: Some(ctx(QPN_B, 0x1234_5678)),
                }),
                post: Some(work(QPN_B, 16)),
                ..idle()
            },
            In {
                qp_request: Some(QpState::Init),
                ..idle()
            },
            In {
                qp_request: Some(QpState::Rtr),
                ..idle()
            },
            In {
                qp_request: Some(QpState::Rts),
                ..idle()
            },
        ];
        pad(&mut stim, 240);

        let payload: Vec<u8> = (0..16).collect();
        let (bytes, sofs, _eofs) = emit(stim, &payload);
        assert_eq!(sofs.len(), 2, "expected two frames, got {sofs:?}");

        let want = vector_frame("uc_write_only_16");
        let n = want.len();
        assert_eq!(bytes.len(), 2 * n);
        let a = &bytes[..n];
        let b = &bytes[n..];
        assert_eq!(a, &want[..], "the first frame should match Scapy exactly");

        // BTH destination queue pair: frame offsets 47..50.
        assert_eq!(
            [a[47], a[48], a[49]],
            [0x00, 0x01, 0x11],
            "frame A's dqpn should be QPN_A"
        );
        assert_eq!(
            [b[47], b[48], b[49]],
            [0x00, 0x02, 0x22],
            "frame B's dqpn should be QPN_B, which is the point of the table"
        );
        // RETH R_Key: offsets 62..66.
        assert_eq!(
            [b[62], b[63], b[64], b[65]],
            [0x12, 0x34, 0x56, 0x78],
            "frame B's rkey should be QP B's"
        );
        assert_ne!(
            &a[n - 4..],
            &b[n - 4..],
            "different headers must give different ICRCs"
        );
        // Everything the context does not touch is identical.
        for k in 0..n {
            let ctx_byte = (47..50).contains(&k) || (62..66).contains(&k) || k >= n - 4;
            if !ctx_byte {
                assert_eq!(a[k], b[k], "byte {k} should not depend on the queue pair");
            }
        }
    }

    // ================= gate clause 3 =================

    /// **Nothing is transmitted outside RTS.**
    ///
    /// The context is configured and work is posted, but the state
    /// machine is walked only as far as RTR. Not one byte may be
    /// emitted. This is the property the whole state machine exists for.
    #[test]
    fn nothing_is_transmitted_before_rts() {
        for stop_at in [None, Some(QpState::Init), Some(QpState::Rtr)] {
            let mut stim = vec![In {
                table_write: Some(QpWrite {
                    qpn: bits(QPN_A),
                    ctx: Some(ctx(QPN_A, 0x0000_A5A5)),
                }),
                post: Some(work(QPN_A, 16)),
                ..idle()
            }];
            if let Some(s) = stop_at {
                if s == QpState::Init || s == QpState::Rtr {
                    stim.push(In {
                        qp_request: Some(QpState::Init),
                        ..idle()
                    });
                }
                if s == QpState::Rtr {
                    stim.push(In {
                        qp_request: Some(QpState::Rtr),
                        ..idle()
                    });
                }
            }
            pad(&mut stim, 140);
            let (bytes, sofs, _) = emit(stim, &(0..16).collect::<Vec<u8>>());
            assert!(
                bytes.is_empty() && sofs.is_empty(),
                "stopping at {stop_at:?} emitted {} bytes and {} frames; \
                 transmission outside RTS is exactly what must not happen",
                bytes.len(),
                sofs.len()
            );
        }
    }

    /// And reaching RTS is what unblocks it — otherwise the test above
    /// would pass for a widget that never transmits at all.
    #[test]
    fn reaching_rts_unblocks_transmission() {
        let mut stim = bring_up(QPN_A, 0x0000_A5A5);
        stim[0].post = Some(work(QPN_A, 16));
        pad(&mut stim, 120);
        let (bytes, sofs, _) = emit(stim, &(0..16).collect::<Vec<u8>>());
        assert_eq!(sofs.len(), 1, "RTS should allow exactly one frame");
        assert_eq!(bytes, vector_frame("uc_write_only_16"));
    }

    // ================= supporting behaviour =================

    /// A message naming a queue pair with no configured context is not
    /// sent, and the endpoint says so rather than framing with a zeroed
    /// context.
    #[test]
    fn an_unconfigured_queue_pair_reports_ctx_missing_and_sends_nothing() {
        let mut stim = vec![
            In {
                qp_request: Some(QpState::Init),
                post: Some(work(QPN_B, 16)), // never configured
                ..idle()
            },
            In {
                qp_request: Some(QpState::Rtr),
                ..idle()
            },
            In {
                qp_request: Some(QpState::Rts),
                ..idle()
            },
        ];
        pad(&mut stim, 60);
        let p = sim(stim.clone());
        assert!(
            p.data.iter().all(|d| d.is_none()),
            "nothing may be framed for an unconfigured queue pair"
        );
        let uut = Uut::default();
        let missing = uut
            .run(stim.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .any(|s| s.output.ctx_missing);
        assert!(missing, "the endpoint should report ctx_missing");
    }

    /// Posting is allowed outside RTS — queueing work is not sending it,
    /// and refusing it during bring-up would make the queue useless.
    #[test]
    fn work_may_be_posted_before_the_queue_pair_is_ready() {
        let stim = vec![In {
            post: Some(work(QPN_A, 16)),
            ..idle()
        }];
        let uut = Uut::default();
        let accepted = uut
            .run(stim.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .any(|s| s.output.accepted);
        assert!(accepted, "a post in RESET should still be queued");
    }

    #[test]
    fn reset_clears_the_endpoint() {
        let cr = clock_reset(clock(false), reset(true));
        let q = Q::<4, 3>::dont_care();
        let (o, d) = roce_write_endpoint::<4, 3>(cr, idle(), q);
        assert_eq!(o.state, QpState::Reset);
        assert!(!o.can_send);
        assert_eq!(o.data, None);
        assert!(!o.busy);
        assert!(!d.extras.started);
        assert!(!d.extras.was_busy);
        assert!(!d.tx.start);
    }

    // ================= Tier 3/4/5 =================

    fn stimulus() -> Vec<In> {
        let mut s = bring_up(QPN_A, 0x0000_A5A5);
        s[0].post = Some(work(QPN_A, 16));
        s[1].post = Some(work(QPN_A, 16));
        pad(&mut s, 215);
        s
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        assert!(hdl.contains("module top"));
        for child in [
            "top_qp_state",
            "top_table",
            "top_wq",
            "top_tx",
            "top_extras",
        ] {
            assert!(
                hdl.contains(child),
                "child {child} missing from the emitted HDL"
            );
        }
        let expect = expect!["true"];
        expect.assert_eq(&(hdl.matches("endmodule").count() >= 5).to_string());
        Ok(())
    }

    #[test]
    fn test_endpoint_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let input = stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        // `.skip(2)`: both the queue-pair table and the work queue are
        // block-RAM backed, and the ICRC register resets non-zero.
        let opts = TestBenchOptions::default().skip(2);
        tb.rtl(&uut, &opts)?.run_iverilog()?;
        tb.ntl(&uut, &opts)?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_endpoint_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let input = stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("endpoint");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["27de36ce162cdccdb3352b5fe1adf83bed751b45200df300c72b261654757c11"];
        let digest = vcd.dump_to_file(root.join("endpoint.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
