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
use rhdl_roce::frame::FrameConfig;
use rhdl_roce::headers::{Transport, WriteOpcode};
use rhdl_roce::qp::context::{MessageRequest, Pmtu, QpContext};
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
