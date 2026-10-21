//! Tier 10 of `roce-v2-architecture.md`: the transmitter's own frames,
//! fed into the receiver.
//!
//! # Why this is not circular
//!
//! Composing two halves of one crate and checking they agree would
//! prove nothing on its own — a shared misunderstanding of the wire
//! format would pass. What makes it meaningful is that both ends are
//! *independently* anchored to Scapy: `tests/vectors.rs` and
//! `tests/fuzz.rs` check the transmitter's bytes against
//! `scapy.contrib.roce`, and `tests/rx_vectors.rs` checks the parser
//! against those same bytes in reverse. A misunderstanding would have
//! to have survived both of those to reach here.
//!
//! What this adds on top of them is **composition**: byte framing,
//! multi-packet segmentation, and the two widgets' interfaces actually
//! fitting together. The vector tests work on one packet at a time and
//! on byte arrays; this runs a whole message through two widgets.
//!
//! # Its limit
//!
//! Loopback cannot find an error that is symmetric between the two
//! directions, and cannot validate anything only a real peer does.
//! It narrows the search space; Tiers 7 and 8 remain the ones that say
//! a real RNIC agrees.

use rhdl::prelude::*;
use rhdl_roce::cm::message::{CM_UDP_PORT, CmStatus};
use rhdl_roce::cm::node::{self, RoceUcRxNode};
use rhdl_roce::frame::FrameConfig;
use rhdl_roce::headers::{Transport, WriteOpcode};
use rhdl_roce::qp::context::{MessageRequest, Pmtu, QpContext};
use rhdl_roce::qp::mr::{MemoryRegion, MrVerdict, MrWrite};
use rhdl_roce::qp::table::QpWrite;
use rhdl_roce::rx::endpoint::{self as rxep, Placement, RoceWriteRx, RxOutcome};
use rhdl_roce::rx::parse::{RxHeaders, RxVerdict};
use rhdl_roce::{rx, tx};

const VA: u128 = 0x0000_7F00_1234_0000;
const RKEY: u128 = 0x0000_A5A5;

fn cfg() -> FrameConfig {
    FrameConfig {
        dst_mac: bits(0xec0d_9aaa_bbcc),
        src_mac: bits(0x000a_3501_0203),
        src_ip: bits(0xC0A8_0132),
        dst_ip: bits(0xC0A8_010A),
        src_port: bits(0x1234),
        dst_port: bits(4791),
        ip_id: bits(0),
        ttl: bits(64),
        dscp_ecn: bits(0),
    }
}

fn ctx(transport: Transport, pmtu: Pmtu) -> QpContext {
    QpContext {
        dqpn: bits(0x000111),
        transport,
        pmtu,
        rkey: bits(RKEY),
        pkey: bits(0xFFFF),
    }
}

/// A deterministic message body.
fn body(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| ((i * 37 + (i >> 5) * 11) & 0xFF) as u8)
        .collect()
}

// --------------------------------------------------------------- transmit

/// One frame as the transmitter emitted it.
#[derive(Debug)]
struct Frame {
    bytes: Vec<u8>,
}

/// Run [`tx::RoceWriteTx`] to completion and collect the frames it
/// emitted, split on the `start_of_frame` / `end_of_frame` markers.
///
/// Two passes, because the payload source is an input the widget reads
/// in the same cycle it emits the byte, while `payload_advance` is the
/// output telling the caller to step. Driving that closed-loop inside
/// the simulator's sample iterator puts the cursor one byte behind: the
/// closure sees the output of the cycle driven by the *previous*
/// input. So pass one learns the advance trajectory with a constant
/// payload, and pass two replays it with the real bytes.
///
/// The trajectory does not depend on the payload *values* — it comes
/// from the segmenter and the phase counter — and pass two asserts that
/// rather than assuming it.
fn transmit(ctx: QpContext, msg_len: usize, payload: &[u8], cycles: usize) -> Vec<Frame> {
    let idle = tx::In {
        start: false,
        msg: MessageRequest {
            va: bits(VA),
            length: bits(msg_len as u128),
        },
        ctx,
        cfg: cfg(),
        psn_start: bits(1),
        ackreq: false,
        payload_byte: bits(0),
        ready: true,
    };

    // --- pass one: learn where `payload_advance` lands.
    let mut stream = vec![tx::In {
        start: true,
        ..idle
    }];
    stream.extend(std::iter::repeat_n(idle, cycles));
    let uut = tx::RoceWriteTx::default();
    let input = stream.clone().into_iter().with_reset(1).clock_pos_edge(100);
    let advances: Vec<bool> = uut
        .run(input)
        .synchronous_sample()
        .map(|s| s.output.payload_advance)
        .collect();

    // Sample `m` reflects input `m-1`, so an advance reported at sample
    // `m` means input `m` must carry the next byte. The cursor for
    // input `j` is therefore the number of advances in samples `1..=j`.
    let mut cursor_for_input = vec![0usize; stream.len()];
    let mut c = 0usize;
    for (j, slot) in cursor_for_input.iter_mut().enumerate() {
        if j > 0 && advances.get(j).copied().unwrap_or(false) {
            c += 1;
        }
        *slot = c.min(payload.len().saturating_sub(1));
    }

    // --- pass two: replay with the real bytes.
    for (j, s) in stream.iter_mut().enumerate() {
        s.payload_byte = bits(*payload.get(cursor_for_input[j]).unwrap_or(&0) as u128);
    }
    let uut = tx::RoceWriteTx::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);

    let mut frames = Vec::new();
    let mut current: Option<Vec<u8>> = None;
    let mut advances2 = Vec::new();
    for s in uut.run(input).synchronous_sample() {
        let o = s.output;
        advances2.push(o.payload_advance);
        if let Some(b) = o.data {
            if o.start_of_frame {
                assert!(
                    current.is_none(),
                    "a start_of_frame arrived while a frame was still open"
                );
                current = Some(Vec::new());
            }
            match current.as_mut() {
                Some(v) => v.push(b.raw() as u8),
                None => panic!("a data byte outside any frame"),
            }
            if o.end_of_frame {
                frames.push(Frame {
                    bytes: current.take().expect("an open frame"),
                });
            }
        }
    }
    assert!(current.is_none(), "the last frame was never closed");
    assert_eq!(
        advances, advances2,
        "the payload-advance trajectory changed between the two passes, \
         so it depends on the payload values and the two-pass method is \
         not valid"
    );
    frames
}

// ---------------------------------------------------------------- receive

/// What the parser recovered from one frame.
struct Received {
    /// `None` when the frame was rejected before its header block
    /// completed, which is the correct behaviour and so cannot be
    /// unwrapped here.
    headers: Option<RxHeaders>,
    payload: Vec<u8>,
    verdict: RxVerdict,
    icrc: u32,
}

impl Received {
    /// The headers, for a frame that was expected to be accepted.
    fn headers(&self) -> RxHeaders {
        self.headers
            .unwrap_or_else(|| panic!("headers were never presented ({:?})", self.verdict))
    }
}

fn receive(frame: &[u8]) -> Received {
    let idle = rx::parse::In {
        data: None,
        start_of_frame: false,
        end_of_frame: false,
    };
    let last = frame.len() - 1;
    let mut stream = vec![idle];
    for (k, b) in frame.iter().enumerate() {
        stream.push(rx::parse::In {
            data: Some(bits(*b as u128)),
            start_of_frame: k == 0,
            end_of_frame: k == last,
        });
    }
    stream.push(idle);

    let uut = rx::parse::RxParser::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let mut headers = None;
    let mut payload = Vec::new();
    let mut verdict = RxVerdict::Accept;
    let mut icrc = 0;
    let mut dones = 0;
    for s in uut.run(input).synchronous_sample() {
        let o = s.output;
        if let Some(b) = o.payload {
            payload.push(b.raw() as u8);
        }
        if o.headers.is_some() {
            headers = o.headers;
        }
        if o.done {
            verdict = o.verdict;
            icrc = o.icrc.raw() as u32;
            dones += 1;
        }
    }
    assert_eq!(dones, 1, "exactly one frame should have been seen");
    Received {
        headers,
        payload,
        verdict,
        icrc,
    }
}

// ------------------------------------------------------------------ tests

/// **One packet, out and back.** The simplest loop: everything the
/// transmitter was told goes into one frame, and the parser recovers it.
#[test]
fn a_single_packet_message_survives_the_round_trip() {
    let payload = body(16);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, payload.len(), &payload, 200);
    assert_eq!(frames.len(), 1, "16 bytes fits in one packet");

    let r = receive(&frames[0].bytes);
    assert_eq!(r.verdict, RxVerdict::Accept);
    assert_eq!(r.headers().kind.op, WriteOpcode::Only, "one packet is ONLY");
    assert_eq!(r.headers().kind.transport, Transport::Uc);
    assert_eq!(r.headers().bth.dqpn.raw(), 0x000111);
    assert_eq!(r.headers().bth.psn.raw(), 1);
    assert!(r.headers().has_reth, "ONLY carries a RETH");
    assert_eq!(r.headers().reth.va.raw(), VA, "the address we were given");
    assert_eq!(r.headers().reth.rkey.raw(), RKEY);
    assert_eq!(r.headers().reth.dma_len.raw(), 16);
    assert_eq!(r.payload, payload, "the payload bytes");
    assert_eq!(
        r.headers().src_ip.raw(),
        cfg().src_ip.raw(),
        "the sender's address, as the sender set it"
    );
    // The ICRC the transmitter computed is where the parser looked for
    // it. The value's correctness is `tests/vectors.rs`'s business;
    // what this shows is that both ends agree on its position.
    assert_eq!(
        r.icrc,
        u32::from_be_bytes(
            frames[0].bytes[frames[0].bytes.len() - 4..]
                .try_into()
                .unwrap()
        ),
        "the reported ICRC is the frame's last four bytes"
    );
}

/// **A message too big for one packet.** The segmenter splits it, and
/// reassembling what the parser recovers must give the original body
/// back — which is the whole point of the receive path.
#[test]
fn a_multi_packet_message_reassembles_byte_for_byte() {
    let msg_len = 2500;
    let payload = body(msg_len);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, msg_len, &payload, 4200);
    assert_eq!(
        frames.len(),
        3,
        "2500 bytes at a 1024-byte PMTU is 1024 + 1024 + 452"
    );

    let mut reassembled = Vec::new();
    for (k, f) in frames.iter().enumerate() {
        let r = receive(&f.bytes);
        assert_eq!(r.verdict, RxVerdict::Accept, "packet {k}");

        let want_op = match k {
            0 => WriteOpcode::First,
            1 => WriteOpcode::Middle,
            _ => WriteOpcode::Last,
        };
        assert_eq!(r.headers().kind.op, want_op, "packet {k} opcode");
        // PSN starts at 1 and increments by one per packet, which is
        // what lets a receiver detect a gap.
        assert_eq!(r.headers().bth.psn.raw(), 1 + k as u128, "packet {k} PSN");

        // Only FIRST restates the address; MIDDLE and LAST inherit it.
        // A receiver that expected a RETH on every packet would read 16
        // payload bytes as an address, which is why this is checked
        // rather than assumed.
        assert_eq!(
            r.headers().has_reth,
            k == 0,
            "packet {k}: only FIRST carries a RETH"
        );
        if k == 0 {
            assert_eq!(r.headers().reth.va.raw(), VA);
            assert_eq!(
                r.headers().reth.dma_len.raw(),
                msg_len as u128,
                "the RETH states the whole message's length, not this \
                 packet's"
            );
        }

        let on_wire = r.payload.len();
        assert_eq!(
            on_wire,
            r.headers().payload_len.raw() as usize,
            "packet {k}: reported length and delivered bytes"
        );
        // Pad comes off by count. 2500 = 1024 + 1024 + 452 and every
        // one of those is a multiple of four, so nothing is padded in
        // *this* message -- asserted rather than left implicit, because
        // a non-zero pad here would mean the segmenter had changed and
        // the reassembly below would silently drop message bytes. The
        // padded path is
        // `a_payload_needing_pad_round_trips_with_the_pad_accounted_for`.
        let pad = r.headers().bth.padcount.raw() as usize;
        assert_eq!(
            pad, 0,
            "packet {k}: 1024 and 452 are both 4-aligned, so no pad is \
             expected"
        );
        reassembled.extend_from_slice(&r.payload[..on_wire - pad]);
    }

    assert_eq!(
        reassembled.len(),
        msg_len,
        "reassembled length after removing pad"
    );
    assert_eq!(reassembled, payload, "reassembled message body");
}

/// The same loop on the reliable transport, so the opcode's transport
/// bits are exercised in both settings rather than only UC.
#[test]
fn the_loop_closes_on_the_reliable_transport_too() {
    let payload = body(100);
    let c = ctx(Transport::Rc, Pmtu::B1024);
    let frames = transmit(c, payload.len(), &payload, 400);
    assert_eq!(frames.len(), 1);
    let r = receive(&frames[0].bytes);
    assert_eq!(r.verdict, RxVerdict::Accept);
    assert_eq!(
        r.headers().kind.transport,
        Transport::Rc,
        "the transport must come back as RC, not UC"
    );
    assert_eq!(r.payload, payload);
}

/// **A payload that is not a multiple of four**, so the pad path is
/// exercised end to end: the transmitter adds the pad, the parser
/// delivers it, and the pad count says how much to drop.
#[test]
fn a_payload_needing_pad_round_trips_with_the_pad_accounted_for() {
    for len in [1usize, 2, 3, 13, 15] {
        let payload = body(len);
        let c = ctx(Transport::Uc, Pmtu::B1024);
        let frames = transmit(c, len, &payload, 200);
        assert_eq!(frames.len(), 1, "{len} bytes is one packet");
        let r = receive(&frames[0].bytes);
        assert_eq!(r.verdict, RxVerdict::Accept, "{len} bytes");

        let pad = r.headers().bth.padcount.raw() as usize;
        assert_eq!(
            (len + pad) % 4,
            0,
            "{len} bytes plus {pad} pad should reach a 4-byte boundary"
        );
        assert_eq!(
            r.payload.len(),
            len + pad,
            "{len} bytes: the on-wire payload includes the pad"
        );
        assert_eq!(&r.payload[..len], &payload[..], "{len} bytes: the body");
        assert!(
            r.payload[len..].iter().all(|b| *b == 0),
            "{len} bytes: pad should be zero, got {:?}",
            &r.payload[len..]
        );
        assert_eq!(
            r.headers().reth.dma_len.raw() as usize,
            len,
            "{len} bytes: the RETH states the unpadded message length"
        );
    }
}

/// A zero-length WRITE is legal and must survive the loop — the
/// `Header -> Trailer` transition with no payload at all.
#[test]
fn a_zero_length_write_survives_the_round_trip() {
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, 0, &[], 200);
    assert_eq!(frames.len(), 1);
    let r = receive(&frames[0].bytes);
    assert_eq!(r.verdict, RxVerdict::Accept);
    assert_eq!(r.headers().kind.op, WriteOpcode::Only);
    assert!(r.payload.is_empty(), "no payload bytes: {:?}", r.payload);
    assert_eq!(r.headers().payload_len.raw(), 0);
    assert_eq!(r.headers().reth.dma_len.raw(), 0);
    assert_eq!(frames[0].bytes.len(), 74, "70 header bytes plus 4 of ICRC");
}

/// **The negative control.** A single flipped byte in a transmitted
/// frame must not still be accepted — otherwise every assertion above
/// could be passing for reasons unrelated to the parser working.
#[test]
fn a_corrupted_transmitted_frame_is_rejected() {
    let payload = body(16);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, payload.len(), &payload, 200);
    let good = &frames[0].bytes;
    assert_eq!(receive(good).verdict, RxVerdict::Accept, "the control");

    for (at, to, want) in [
        (13usize, 0x06u8, RxVerdict::NotIpv4),
        (23, 0x06, RxVerdict::NotUdp),
        (37, 0xB6, RxVerdict::NotRocePort),
        (42, 0x00, RxVerdict::UnknownOpcode),
    ] {
        let mut f = good.clone();
        assert_ne!(f[at], to, "offset {at} is already {to:#04x}");
        f[at] = to;
        assert_eq!(
            receive(&f).verdict,
            want,
            "a frame with offset {at} set to {to:#04x} must be rejected"
        );
    }
}

// ================================================================
// Tier 10, part (a): the whole path, transmit into receive
// ================================================================
//
// This is the Phase 4 gate's host-free half. A message leaves the
// transmitter as frames, arrives at `RoceWriteRx`, is parsed, its ICRC
// verified, its R_Key checked against a registered memory region, and
// its payload written to translated local addresses -- and the bytes
// that land are compared against the bytes that were sent.

const LOCAL_BASE: u128 = 0x0002_0000;

fn region(len: u128) -> MemoryRegion {
    MemoryRegion {
        rkey: bits(RKEY),
        va_base: bits(VA),
        length: bits(len),
        local_base: bits(LOCAL_BASE),
        remote_write: true,
    }
}

/// What a receive run produced.
struct Placed {
    /// Every placement, in order.
    writes: Vec<(u32, u8)>,
    /// One entry per frame that ended.
    outcomes: Vec<RxOutcome>,
    /// Bytes reported placed, per frame.
    counts: Vec<u32>,
}

/// Feed frames into a receiver that has the queue pair and the memory
/// region registered first.
fn receive_into(frames: &[Frame], mr: Option<MemoryRegion>, qp: Option<QpContext>) -> Placed {
    let idle = rxep::In {
        data: None,
        start_of_frame: false,
        end_of_frame: false,
        mr_write: None,
        qp_write: None,
    };

    // Configure first, then let the RAMs settle.
    let mut stream = vec![
        rxep::In {
            mr_write: Some(MrWrite {
                rkey: bits(RKEY),
                region: mr,
            }),
            ..idle
        },
        rxep::In {
            qp_write: Some(QpWrite {
                qpn: bits(0x000111),
                ctx: qp,
            }),
            ..idle
        },
        idle,
        idle,
    ];
    for f in frames {
        let last = f.bytes.len() - 1;
        for (k, b) in f.bytes.iter().enumerate() {
            stream.push(rxep::In {
                data: Some(bits(*b as u128)),
                start_of_frame: k == 0,
                end_of_frame: k == last,
                ..idle
            });
        }
        // Idle cycles between frames, as a real link has.
        stream.extend(std::iter::repeat_n(idle, 4));
    }
    stream.extend(std::iter::repeat_n(idle, 4));

    let uut: RoceWriteRx<4, 4> = RoceWriteRx::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let mut r = Placed {
        writes: Vec::new(),
        outcomes: Vec::new(),
        counts: Vec::new(),
    };
    for s in uut.run(input).synchronous_sample() {
        let o = s.output;
        if let Some(Placement { addr, data }) = o.write {
            r.writes.push((addr.raw() as u32, data.raw() as u8));
        }
        if o.done {
            r.outcomes.push(o.outcome);
            r.counts.push(o.placed.raw() as u32);
        }
    }
    r
}

/// **The gate.** A multi-packet message goes out of the transmitter and
/// lands in local memory, byte for byte, at the addresses the memory
/// region translates the sender's virtual addresses to.
#[test]
fn a_multi_packet_message_is_placed_at_the_translated_addresses() {
    let msg_len = 2500;
    let payload = body(msg_len);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, msg_len, &payload, 4200);
    assert_eq!(frames.len(), 3, "1024 + 1024 + 452");

    let r = receive_into(&frames, Some(region(msg_len as u128)), Some(c));

    assert_eq!(
        r.outcomes,
        vec![RxOutcome::Placed; 3],
        "all three packets should be placed"
    );
    assert_eq!(
        r.counts,
        vec![1024, 1024, 452],
        "and each should report its own byte count"
    );

    assert_eq!(
        r.writes.len(),
        msg_len,
        "one write per message byte, no more"
    );
    for (i, (addr, data)) in r.writes.iter().enumerate() {
        assert_eq!(
            *addr as u128,
            LOCAL_BASE + i as u128,
            "byte {i} went to the wrong address"
        );
        assert_eq!(*data, payload[i], "byte {i} has the wrong value");
    }
}

/// The pad path: pad bytes are **not** placed, because they are not
/// message data.
#[test]
fn pad_bytes_are_not_written_to_memory() {
    for len in [1usize, 2, 3, 13, 15, 16] {
        let payload = body(len);
        let c = ctx(Transport::Uc, Pmtu::B1024);
        let frames = transmit(c, len, &payload, 300);
        let r = receive_into(&frames, Some(region(len as u128)), Some(c));
        assert_eq!(r.outcomes, vec![RxOutcome::Placed], "{len} bytes");
        assert_eq!(
            r.writes.len(),
            len,
            "{len} bytes: pad must not be placed, got {} writes",
            r.writes.len()
        );
        for (i, (addr, data)) in r.writes.iter().enumerate() {
            assert_eq!(*addr as u128, LOCAL_BASE + i as u128);
            assert_eq!(*data, payload[i]);
        }
    }
}

/// **A corrupted frame is rejected and the reassembly state is not
/// advanced**, so the message is abandoned rather than continuing at a
/// cursor that no longer means anything.
#[test]
fn a_corrupted_frame_is_rejected_and_abandons_the_message() {
    let msg_len = 2500;
    let payload = body(msg_len);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let mut frames = transmit(c, msg_len, &payload, 4200);
    frames[0].bytes[100] ^= 0xFF;

    let r = receive_into(&frames, Some(region(msg_len as u128)), Some(c));
    assert_eq!(
        r.outcomes[0],
        RxOutcome::Frame(RxVerdict::IcrcMismatch),
        "the corrupted frame must fail its checksum"
    );
    assert_eq!(
        r.outcomes[1],
        RxOutcome::NoMessage,
        "the next MIDDLE has nothing to continue"
    );
    assert_eq!(r.outcomes[2], RxOutcome::NoMessage);
    // The honest part: the corrupt frame's bytes *were* written,
    // because the ICRC is only knowable at the end of the frame. They
    // are inside the authorized region, so this is corruption and not a
    // memory-safety failure.
    assert!(
        !r.writes.is_empty(),
        "the speculative writes of the corrupt frame are expected"
    );
    assert!(
        r.writes.iter().all(|(a, _)| {
            (*a as u128) >= LOCAL_BASE && (*a as u128) < LOCAL_BASE + msg_len as u128
        }),
        "but every one must be inside the region the R_Key authorized"
    );
}

/// **The security test.** A frame presenting the wrong R_Key places
/// nothing at all -- not speculatively, not partially, nothing.
#[test]
fn a_frame_with_the_wrong_rkey_places_nothing() {
    let payload = body(64);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, payload.len(), &payload, 300);

    // A region registered under a *different* R_Key that aliases to the
    // same table slot: the index matches, the key does not.
    let aliasing = MemoryRegion {
        rkey: bits(0x1000_0000 | RKEY),
        ..region(64)
    };
    let r = receive_into(&frames, Some(aliasing), Some(c));
    assert_eq!(
        r.outcomes,
        vec![RxOutcome::Access(MrVerdict::RkeyMismatch)],
        "an R_Key that only aliases must be refused"
    );
    assert!(
        r.writes.is_empty(),
        "and nothing may be written -- authorization is never \
         speculative. Got {} writes.",
        r.writes.len()
    );
}

/// No region registered at all: refused, nothing written.
#[test]
fn a_frame_with_no_registered_region_places_nothing() {
    let payload = body(64);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, payload.len(), &payload, 300);
    let r = receive_into(&frames, None, Some(c));
    assert_eq!(r.outcomes, vec![RxOutcome::Access(MrVerdict::NoRegion)]);
    assert!(r.writes.is_empty());
}

/// A region one byte too small for the message: refused on the range
/// check, nothing written. The RETH declares the *message* length, so
/// this is caught at the FIRST packet rather than part way through.
#[test]
fn a_region_too_small_for_the_message_places_nothing() {
    let msg_len = 2500;
    let payload = body(msg_len);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, msg_len, &payload, 4200);
    let r = receive_into(&frames, Some(region(msg_len as u128 - 1)), Some(c));
    assert_eq!(
        r.outcomes[0],
        RxOutcome::Access(MrVerdict::OutOfRange),
        "the FIRST packet declares 2500 bytes, which does not fit"
    );
    assert!(
        r.writes.is_empty(),
        "and not one byte is written, even though the first 2499 would \
         have fitted -- the authorization is for the whole message"
    );
}

/// An unconfigured queue pair is refused, and nothing is written.
#[test]
fn a_frame_for_an_unconfigured_queue_pair_places_nothing() {
    let payload = body(64);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, payload.len(), &payload, 300);
    let r = receive_into(&frames, Some(region(64)), None);
    assert_eq!(r.outcomes, vec![RxOutcome::NoQueuePair]);
    assert!(r.writes.is_empty());
}

/// **A sequence-number gap abandons the message.** The packet that
/// would have continued it is dropped rather than written at a cursor
/// that no longer corresponds to it.
#[test]
fn a_dropped_middle_packet_abandons_the_rest_of_the_message() {
    let msg_len = 2500;
    let payload = body(msg_len);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, msg_len, &payload, 4200);

    // Deliver FIRST and LAST, losing the MIDDLE -- which is what an
    // unreliable connection does.
    let delivered = vec![
        Frame {
            bytes: frames[0].bytes.clone(),
        },
        Frame {
            bytes: frames[2].bytes.clone(),
        },
    ];
    let r = receive_into(&delivered, Some(region(msg_len as u128)), Some(c));
    assert_eq!(r.outcomes[0], RxOutcome::Placed, "the FIRST is fine");
    assert_eq!(
        r.outcomes[1],
        RxOutcome::PsnGap,
        "the LAST carries PSN 3 where PSN 2 was expected"
    );
    assert_eq!(
        r.writes.len(),
        1024,
        "only the FIRST packet's bytes are placed; the LAST's would \
         have gone to the MIDDLE's addresses"
    );
}

/// And a fresh FIRST after a gap resynchronises, which is what makes UC
/// usable for streaming: one lost message, not a dead connection.
#[test]
fn a_fresh_first_resynchronises_after_a_gap() {
    let msg_len = 2500;
    let payload = body(msg_len);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, msg_len, &payload, 4200);

    let short = body(16);
    let good = transmit(c, 16, &short, 300);
    let delivered = vec![
        Frame {
            bytes: frames[0].bytes.clone(),
        },
        Frame {
            bytes: frames[2].bytes.clone(),
        },
        Frame {
            bytes: good[0].bytes.clone(),
        },
    ];
    let r = receive_into(&delivered, Some(region(msg_len as u128)), Some(c));
    assert_eq!(r.outcomes[2], RxOutcome::Placed, "the new ONLY recovers");
    // Its 16 bytes land at the region's base, because its RETH says so.
    let tail = &r.writes[r.writes.len() - 16..];
    for (i, (addr, data)) in tail.iter().enumerate() {
        assert_eq!(*addr as u128, LOCAL_BASE + i as u128);
        assert_eq!(*data, short[i]);
    }
}

// ================================================================
// Phase 4, end to end: negotiate over UDP, then write
// ================================================================

/// Build a CM Connect request for the queue pair and region the
/// loopback transmitter actually uses, assembled by hand so the codec
/// is not tested against itself. Mirrors `tests/cm.rs`.
fn cm_connect_frame(length: u32, local_base: u32) -> Vec<u8> {
    const OUR_MAC: u64 = 0x000a_3501_0203;
    const PEER_MAC: u64 = 0xec0d_9aaa_bbcc;
    const OUR_IP: u32 = 0xC0A8_010A;
    const PEER_IP: u32 = 0xC0A8_0132;
    let mut f: Vec<u8> = Vec::new();
    f.extend_from_slice(&OUR_MAC.to_be_bytes()[2..]);
    f.extend_from_slice(&PEER_MAC.to_be_bytes()[2..]);
    f.extend_from_slice(&0x0800u16.to_be_bytes());
    f.push(0x45);
    f.push(0x00);
    f.extend_from_slice(&68u16.to_be_bytes());
    f.extend_from_slice(&0u16.to_be_bytes());
    f.extend_from_slice(&0u16.to_be_bytes());
    f.push(64);
    f.push(17);
    f.extend_from_slice(&0u16.to_be_bytes());
    f.extend_from_slice(&PEER_IP.to_be_bytes());
    f.extend_from_slice(&OUR_IP.to_be_bytes());
    f.extend_from_slice(&0x9ABCu16.to_be_bytes());
    f.extend_from_slice(&(CM_UDP_PORT as u16).to_be_bytes());
    f.extend_from_slice(&48u16.to_be_bytes());
    f.extend_from_slice(&0u16.to_be_bytes());
    f.extend_from_slice(b"RCM1");
    f.push(1); // Connect
    f.push(0); // status
    f.push(1); // UC
    f.push(3); // MTU 1024
    f.extend_from_slice(&0x000111u32.to_be_bytes()[1..]); // the QP we receive on
    f.push(0);
    f.extend_from_slice(&0x000222u32.to_be_bytes()[1..]);
    f.push(0);
    f.extend_from_slice(&1u32.to_be_bytes()[1..]);
    f.push(0);
    f.extend_from_slice(&(RKEY as u32).to_be_bytes());
    f.extend_from_slice(&(VA as u64).to_be_bytes());
    f.extend_from_slice(&length.to_be_bytes());
    f.extend_from_slice(&local_base.to_be_bytes());
    assert_eq!(f.len(), 82);
    f
}

/// What a node run produced.
struct NodeRun {
    writes: Vec<(u32, u8)>,
    outcomes: Vec<RxOutcome>,
    cm_statuses: Vec<CmStatus>,
    replies: usize,
}

/// Drive the node: a connection-manager datagram, then RoCE frames, on
/// one byte stream. Nothing is configured by hand.
fn run_node(cm: &[Vec<u8>], roce: &[Frame]) -> NodeRun {
    let idle = node::In {
        data: None,
        start_of_frame: false,
        end_of_frame: false,
        cfg: FrameConfig {
            dst_mac: bits(0xDEAD_DEAD_DEAD),
            src_mac: bits(0x000a_3501_0203),
            src_ip: bits(0xC0A8_010A),
            dst_ip: bits(0xDEAD_BEEF),
            src_port: bits(0),
            dst_port: bits(0),
            ip_id: bits(0),
            ttl: bits(64),
            dscp_ecn: bits(0),
        },
        cm_port: bits(CM_UDP_PORT),
        reply_ready: true,
        qp_write: None,
        mr_write: None,
    };
    let mut stream = vec![idle];
    // Not `mut`: the closure takes the vector as an argument rather
    // than capturing it, so it is `Fn`, not `FnMut`.
    let push = |stream: &mut Vec<node::In>, bytes: &[u8]| {
        let last = bytes.len() - 1;
        for (k, b) in bytes.iter().enumerate() {
            stream.push(node::In {
                data: Some(bits(*b as u128)),
                start_of_frame: k == 0,
                end_of_frame: k == last,
                ..idle
            });
        }
        // Long enough for an 82-byte reply to drain before the next
        // frame arrives, as a real host would wait for it.
        stream.extend(std::iter::repeat_n(idle, 120));
    };
    for f in cm {
        push(&mut stream, f);
    }
    for f in roce {
        push(&mut stream, &f.bytes);
    }

    let uut: RoceUcRxNode<4, 4> = RoceUcRxNode::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let mut r = NodeRun {
        writes: Vec::new(),
        outcomes: Vec::new(),
        cm_statuses: Vec::new(),
        replies: 0,
    };
    for s in uut.run(input).synchronous_sample() {
        let o = s.output;
        if let Some(Placement { addr, data }) = o.write {
            r.writes.push((addr.raw() as u32, data.raw() as u8));
        }
        if o.done {
            r.outcomes.push(o.outcome);
        }
        if o.cm_done {
            r.cm_statuses.push(o.cm_status);
        }
        if o.reply_eof {
            r.replies += 1;
        }
    }
    r
}

/// The outcome sequence a node run produces.
///
/// One [`RxVerdict::NotRocePort`] per connection-manager datagram comes
/// first: the receiver sees **every** frame on the wire, including the
/// manager's, and rejects the ones addressed to another UDP port. That
/// rejection is the entire basis of the fan-out in `cm::node`, so it is
/// written into the expectations here rather than filtered out.
fn expect_outcomes(cm_frames: usize, roce: &[RxOutcome]) -> Vec<RxOutcome> {
    let mut v = vec![RxOutcome::Frame(RxVerdict::NotRocePort); cm_frames];
    v.extend_from_slice(roce);
    v
}

/// **Phase 4's gate, part (a), end to end.** A queue pair and a memory
/// region are negotiated over UDP — nothing is configured by hand —
/// and then a multi-packet RDMA WRITE lands at the translated local
/// addresses.
#[test]
fn a_negotiated_connection_then_receives_a_write() {
    let msg_len = 2500usize;
    let payload = body(msg_len);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, msg_len, &payload, 4200);

    let r = run_node(
        &[cm_connect_frame(msg_len as u32, LOCAL_BASE as u32)],
        &frames,
    );

    assert_eq!(
        r.cm_statuses,
        vec![CmStatus::Accepted],
        "the connection must be accepted"
    );
    assert_eq!(r.replies, 1, "and answered");
    assert_eq!(
        r.outcomes,
        expect_outcomes(1, &[RxOutcome::Placed; 3]),
        "then all three packets placed, with no hand configuration"
    );
    assert_eq!(r.writes.len(), msg_len);
    for (i, (addr, data)) in r.writes.iter().enumerate() {
        assert_eq!(*addr as u128, LOCAL_BASE + i as u128, "byte {i} address");
        assert_eq!(*data, payload[i], "byte {i} value");
    }
}

/// **Without the negotiation, the same frames place nothing.** This is
/// what makes the test above mean something: the configuration is doing
/// the work, not a default that happens to be right.
#[test]
fn without_the_negotiation_the_same_write_is_refused() {
    let msg_len = 2500usize;
    let payload = body(msg_len);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, msg_len, &payload, 4200);

    let r = run_node(&[], &frames);
    assert!(r.cm_statuses.is_empty(), "nothing was negotiated");
    assert_eq!(
        r.outcomes,
        expect_outcomes(0, &[RxOutcome::NoQueuePair; 3]),
        "an unconfigured queue pair must refuse every frame"
    );
    assert!(r.writes.is_empty(), "and place nothing");
}

/// A `Disconnect` undoes it: the same frames are then refused again.
#[test]
fn a_disconnect_stops_further_writes() {
    let payload = body(64);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, payload.len(), &payload, 300);

    let mut disconnect = cm_connect_frame(64, LOCAL_BASE as u32);
    disconnect[46] = 2; // the opcode byte: Connect -> Disconnect

    // Connect, write, disconnect, write again.
    let r = run_node(
        &[cm_connect_frame(64, LOCAL_BASE as u32)],
        &[Frame {
            bytes: frames[0].bytes.clone(),
        }],
    );
    assert_eq!(
        r.outcomes,
        expect_outcomes(1, &[RxOutcome::Placed]),
        "the first write lands"
    );

    let r2 = run_node(
        &[cm_connect_frame(64, LOCAL_BASE as u32), disconnect],
        &[Frame {
            bytes: frames[0].bytes.clone(),
        }],
    );
    assert_eq!(
        r2.cm_statuses,
        vec![CmStatus::Accepted, CmStatus::Accepted],
        "both requests accepted"
    );
    assert_eq!(
        r2.outcomes,
        expect_outcomes(2, &[RxOutcome::NoQueuePair]),
        "after the disconnect the queue pair is gone"
    );
    assert!(r2.writes.is_empty());
}

/// **The two children ignore each other's traffic.** A connection
/// manager datagram must not look like a RoCE frame to the receiver,
/// and a RoCE frame must not look like a request to the manager.
#[test]
fn each_child_ignores_the_other_s_frames() {
    let payload = body(64);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, payload.len(), &payload, 300);

    // Only a CM datagram: the receiver should report it as not-RoCE and
    // place nothing.
    let r = run_node(&[cm_connect_frame(64, LOCAL_BASE as u32)], &[]);
    assert_eq!(r.cm_statuses, vec![CmStatus::Accepted]);
    assert_eq!(
        r.outcomes,
        vec![RxOutcome::Frame(RxVerdict::NotRocePort)],
        "the receiver sees the manager's datagram and rejects it on the \
         port, which is the whole basis of the fan-out"
    );
    assert!(r.writes.is_empty());

    // Only a RoCE frame: the manager should say nothing at all.
    let r2 = run_node(&[], &frames);
    assert!(
        r2.cm_statuses.is_empty() && r2.replies == 0,
        "the manager must not answer a RoCE data frame"
    );
}

/// A region the negotiation made too small refuses the write, which
/// shows the negotiated length is really the one being enforced.
#[test]
fn a_negotiated_region_that_is_too_small_refuses_the_write() {
    let msg_len = 2500usize;
    let payload = body(msg_len);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, msg_len, &payload, 4200);

    let r = run_node(
        &[cm_connect_frame(msg_len as u32 - 1, LOCAL_BASE as u32)],
        &frames,
    );
    assert_eq!(r.cm_statuses, vec![CmStatus::Accepted]);
    // Only the FIRST carries a RETH, so only it is refused on the range
    // check; the MIDDLE and LAST have no address of their own and find
    // no message to continue. Which is the right answer -- a receiver
    // that re-ran the range check on them would be inventing a RETH
    // they do not carry.
    assert_eq!(
        r.outcomes,
        expect_outcomes(
            1,
            &[
                RxOutcome::Access(MrVerdict::OutOfRange),
                RxOutcome::NoMessage,
                RxOutcome::NoMessage,
            ]
        ),
        "one byte short is short"
    );
    assert!(r.writes.is_empty());
}

/// And the negotiated `local_base` is the address actually used, so the
/// translation is coming from the datagram rather than a constant.
#[test]
fn the_negotiated_local_base_is_where_the_bytes_land() {
    let payload = body(32);
    let c = ctx(Transport::Uc, Pmtu::B1024);
    let frames = transmit(c, payload.len(), &payload, 300);

    for base in [0x0000_1000u32, 0x0009_9000, 0x0100_0000] {
        let r = run_node(&[cm_connect_frame(32, base)], &frames);
        assert_eq!(
            r.outcomes,
            expect_outcomes(1, &[RxOutcome::Placed]),
            "base {base:#x}"
        );
        assert_eq!(r.writes.len(), 32);
        for (i, (addr, data)) in r.writes.iter().enumerate() {
            assert_eq!(
                *addr,
                base + i as u32,
                "base {base:#x}: byte {i} went to the wrong address"
            );
            assert_eq!(*data, payload[i]);
        }
    }
}
