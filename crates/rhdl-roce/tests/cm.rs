//! The connection manager, driven with datagrams assembled by hand.
//!
//! # Why the requests are built byte by byte here
//!
//! The obvious way to build a test request is to call
//! [`rhdl_roce::cm::message::cm_byte`] — the crate's own encoder. That
//! would test the decoder against the encoder and pass even if both
//! agreed on the wrong layout.
//!
//! So every request in this file is assembled from explicit
//! big-endian pushes against the table in `cm::message`'s docs, and the
//! replies are taken apart the same way. The layout in the
//! documentation is the specification, and this file is an independent
//! reading of it.

use rhdl::prelude::*;
use rhdl_roce::cm::manager::{self, ConnectionManager};
use rhdl_roce::cm::message::{CM_UDP_PORT, CmStatus};
use rhdl_roce::frame::FrameConfig;
use rhdl_roce::headers::Transport;
use rhdl_roce::qp::context::Pmtu;
use rhdl_roce::qp::mr::MrWrite;
use rhdl_roce::qp::table::QpWrite;

const OUR_MAC: u64 = 0x000a_3501_0203;
const PEER_MAC: u64 = 0xec0d_9aaa_bbcc;
const OUR_IP: u32 = 0xC0A8_0132;
const PEER_IP: u32 = 0xC0A8_010A;
const PEER_PORT: u16 = 0x9ABC;

/// Everything a connection-manager request carries, as host-side
/// values.
#[derive(Clone, Copy)]
struct Request {
    dport: u16,
    ethertype: u16,
    ver_ihl: u8,
    protocol: u8,
    magic: [u8; 4],
    opcode: u8,
    transport: u8,
    pmtu: u8,
    qpn: u32,
    remote_qpn: u32,
    psn: u32,
    rkey: u32,
    va: u64,
    length: u32,
    local_base: u32,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            dport: CM_UDP_PORT as u16,
            ethertype: 0x0800,
            ver_ihl: 0x45,
            protocol: 17,
            magic: *b"RCM1",
            opcode: 1,    // Connect
            transport: 1, // Uc
            pmtu: 3,      // 1024
            qpn: 0x000111,
            remote_qpn: 0x000222,
            psn: 1,
            rkey: 0x0000_A5A5,
            va: 0x0000_7F00_1234_0000,
            length: 4096,
            local_base: 0x0002_0000,
        }
    }
}

/// Assemble the frame. Big-endian throughout, per the layout table in
/// `cm::message`'s module docs.
fn frame_of(r: Request) -> Vec<u8> {
    let mut f: Vec<u8> = Vec::new();
    // --- Ethernet
    f.extend_from_slice(&OUR_MAC.to_be_bytes()[2..]); // destination: us
    f.extend_from_slice(&PEER_MAC.to_be_bytes()[2..]); // source: the peer
    f.extend_from_slice(&r.ethertype.to_be_bytes());
    // --- IPv4, 20 bytes
    f.push(r.ver_ihl);
    f.push(0x00); // DSCP/ECN
    f.extend_from_slice(&68u16.to_be_bytes()); // 20 + 8 + 40
    f.extend_from_slice(&0u16.to_be_bytes()); // identification
    f.extend_from_slice(&0u16.to_be_bytes()); // flags / fragment
    f.push(64); // TTL
    f.push(r.protocol);
    // The manager does not verify the IPv4 header checksum -- see the
    // test `the_manager_does_not_verify_the_ip_checksum`, which pins
    // that as a deliberate, documented limitation rather than an
    // accident.
    f.extend_from_slice(&0u16.to_be_bytes());
    f.extend_from_slice(&PEER_IP.to_be_bytes()); // source
    f.extend_from_slice(&OUR_IP.to_be_bytes()); // destination
    // --- UDP, 8 bytes
    f.extend_from_slice(&PEER_PORT.to_be_bytes());
    f.extend_from_slice(&r.dport.to_be_bytes());
    f.extend_from_slice(&48u16.to_be_bytes()); // 8 + 40
    f.extend_from_slice(&0u16.to_be_bytes()); // checksum
    // --- the 40-byte payload
    f.extend_from_slice(&r.magic);
    f.push(r.opcode);
    f.push(0); // status, zero in a request
    f.push(r.transport);
    f.push(r.pmtu);
    f.extend_from_slice(&r.qpn.to_be_bytes()[1..]); // 24 bits
    f.push(0); // reserved
    f.extend_from_slice(&r.remote_qpn.to_be_bytes()[1..]);
    f.push(0);
    f.extend_from_slice(&r.psn.to_be_bytes()[1..]);
    f.push(0);
    f.extend_from_slice(&r.rkey.to_be_bytes());
    f.extend_from_slice(&r.va.to_be_bytes());
    f.extend_from_slice(&r.length.to_be_bytes());
    f.extend_from_slice(&r.local_base.to_be_bytes());
    assert_eq!(f.len(), 82, "14 Ethernet + 20 IPv4 + 8 UDP + 40 payload");
    f
}

fn our_cfg() -> FrameConfig {
    FrameConfig {
        // The destination fields are overwritten from the request, so
        // deliberately wrong here: if the reply used them, the test
        // below would see it.
        dst_mac: bits(0xDEAD_DEAD_DEAD),
        src_mac: bits(OUR_MAC as u128),
        src_ip: bits(OUR_IP as u128),
        dst_ip: bits(0xDEAD_BEEF),
        src_port: bits(0),
        dst_port: bits(0),
        ip_id: bits(0),
        ttl: bits(64),
        dscp_ecn: bits(0),
    }
}

/// What a run of the manager produced.
struct Outcome {
    qp_writes: Vec<QpWrite>,
    mr_writes: Vec<MrWrite>,
    statuses: Vec<CmStatus>,
    /// Reply frames, split on their own start/end markers.
    replies: Vec<Vec<u8>>,
}

fn run(frames: &[Vec<u8>]) -> Outcome {
    let idle = manager::In {
        data: None,
        start_of_frame: false,
        end_of_frame: false,
        cfg: our_cfg(),
        port: bits(CM_UDP_PORT),
        ready: true,
    };
    let mut stream = vec![idle];
    for f in frames {
        let last = f.len() - 1;
        for (k, b) in f.iter().enumerate() {
            stream.push(manager::In {
                data: Some(bits(*b as u128)),
                start_of_frame: k == 0,
                end_of_frame: k == last,
                ..idle
            });
        }
        // Enough idle cycles for the whole 82-byte reply to drain.
        stream.extend(std::iter::repeat_n(idle, 100));
    }

    let uut = ConnectionManager::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let mut out = Outcome {
        qp_writes: Vec::new(),
        mr_writes: Vec::new(),
        statuses: Vec::new(),
        replies: Vec::new(),
    };
    let mut current: Option<Vec<u8>> = None;
    for s in uut.run(input).synchronous_sample() {
        let o = s.output;
        if let Some(w) = o.qp_write {
            out.qp_writes.push(w);
        }
        if let Some(w) = o.mr_write {
            out.mr_writes.push(w);
        }
        if o.done {
            out.statuses.push(o.status);
        }
        if let Some(b) = o.reply {
            if o.reply_sof {
                current = Some(Vec::new());
            }
            if let Some(v) = current.as_mut() {
                v.push(b.raw() as u8);
            }
            if o.reply_eof
                && let Some(v) = current.take()
            {
                out.replies.push(v);
            }
        }
    }
    out
}

/// Take a reply apart by hand, against the same layout table.
struct Reply {
    dst_mac: u64,
    src_mac: u64,
    ethertype: u16,
    protocol: u8,
    src_ip: u32,
    dst_ip: u32,
    src_port: u16,
    dst_port: u16,
    magic: [u8; 4],
    opcode: u8,
    status: u8,
    qpn: u32,
    rkey: u32,
    va: u64,
}

fn parse_reply(f: &[u8]) -> Reply {
    assert_eq!(f.len(), 82, "a reply frame is 82 bytes");
    let be16 = |o: usize| u16::from_be_bytes([f[o], f[o + 1]]);
    let be32 = |o: usize| u32::from_be_bytes([f[o], f[o + 1], f[o + 2], f[o + 3]]);
    let mac = |o: usize| {
        let mut v = 0u64;
        for k in 0..6 {
            v = (v << 8) | f[o + k] as u64;
        }
        v
    };
    Reply {
        dst_mac: mac(0),
        src_mac: mac(6),
        ethertype: be16(12),
        protocol: f[23],
        src_ip: be32(26),
        dst_ip: be32(30),
        src_port: be16(34),
        dst_port: be16(36),
        magic: [f[42], f[43], f[44], f[45]],
        opcode: f[46],
        status: f[47],
        // Payload offset 8..10 is frame offset 50..52: a 24-bit
        // field, most significant byte first.
        qpn: ((f[50] as u32) << 16) | ((f[51] as u32) << 8) | f[52] as u32,
        rkey: be32(62),
        va: u64::from_be_bytes([f[66], f[67], f[68], f[69], f[70], f[71], f[72], f[73]]),
    }
}

// ----------------------------------------------------------- tests

/// **The headline: a host configures a queue pair over UDP.**
#[test]
fn a_connect_request_configures_the_queue_pair_and_the_region() {
    let r = Request::default();
    let out = run(&[frame_of(r)]);

    assert_eq!(out.statuses, vec![CmStatus::Accepted]);
    assert_eq!(out.qp_writes.len(), 1, "one queue-pair write");
    assert_eq!(out.mr_writes.len(), 1, "one region write");

    let qw = out.qp_writes[0];
    assert_eq!(qw.qpn.raw(), r.qpn as u128, "the QP named in the request");
    let ctx = qw.ctx.expect("a Connect configures, it does not tear down");
    assert_eq!(
        ctx.dqpn.raw(),
        r.remote_qpn as u128,
        "the context's destination is the *peer's* QP number, not ours"
    );
    assert_eq!(ctx.transport, Transport::Uc, "transport code 1 is UC");
    assert_eq!(ctx.pmtu, Pmtu::B1024, "MTU code 3 is 1024, per the IBTA");
    assert_eq!(ctx.rkey.raw(), r.rkey as u128);
    assert_eq!(ctx.pkey.raw(), 0xFFFF);

    let mw = out.mr_writes[0];
    assert_eq!(mw.rkey.raw(), r.rkey as u128);
    let reg = mw.region.expect("a Connect registers");
    assert_eq!(reg.va_base.raw(), r.va as u128);
    assert_eq!(reg.length.raw(), r.length as u128);
    assert_eq!(reg.local_base.raw(), r.local_base as u128);
    assert!(reg.remote_write, "a registered region accepts writes");
}

/// **The reply goes back to whoever asked**, addressed entirely from
/// the request. Nothing about the peer is configured — `our_cfg()`
/// deliberately holds nonsense in its destination fields, so if the
/// reply used them this test fails.
#[test]
fn the_reply_is_addressed_from_the_request() {
    let out = run(&[frame_of(Request::default())]);
    assert_eq!(out.replies.len(), 1, "exactly one reply");
    let p = parse_reply(&out.replies[0]);

    assert_eq!(p.dst_mac, PEER_MAC, "addressed to the sender's MAC");
    assert_eq!(p.src_mac, OUR_MAC, "from ours");
    assert_eq!(p.dst_ip, PEER_IP, "to the sender's address");
    assert_eq!(p.src_ip, OUR_IP, "from ours");
    assert_eq!(
        p.dst_port, PEER_PORT,
        "to the port the request came from, not to the manager's port"
    );
    assert_eq!(p.src_port, CM_UDP_PORT as u16, "from the manager's port");
    assert_eq!(p.ethertype, 0x0800);
    assert_eq!(p.protocol, 17, "UDP");
}

/// The reply says what happened, and echoes enough of the request for
/// the sender to match it to what it asked.
#[test]
fn the_reply_reports_the_outcome_and_echoes_the_request() {
    let r = Request::default();
    let out = run(&[frame_of(r)]);
    let p = parse_reply(&out.replies[0]);
    assert_eq!(&p.magic, b"RCM1");
    assert_eq!(p.opcode, 3, "a reply carries the Response opcode");
    assert_eq!(p.status, 0, "Accepted");
    assert_eq!(p.qpn, r.qpn, "the queue pair it configured");
    assert_eq!(p.rkey, r.rkey);
    assert_eq!(p.va, r.va);
}

/// A `Disconnect` tears both down, and says so.
#[test]
fn a_disconnect_tears_the_queue_pair_and_region_down() {
    let out = run(&[frame_of(Request {
        opcode: 2,
        ..Request::default()
    })]);
    assert_eq!(out.statuses, vec![CmStatus::Accepted]);
    assert_eq!(out.qp_writes[0].ctx, None, "the queue pair is removed");
    assert_eq!(out.mr_writes[0].region, None, "and so is the region");
}

/// **A stray datagram on the port cannot reconfigure anything.** The
/// magic number is what makes the port safe to listen on.
#[test]
fn a_wrong_magic_number_changes_nothing() {
    let out = run(&[frame_of(Request {
        magic: *b"XXXX",
        ..Request::default()
    })]);
    assert_eq!(out.statuses, vec![CmStatus::BadMagic]);
    assert!(out.qp_writes.is_empty(), "nothing configured");
    assert!(out.mr_writes.is_empty(), "nothing registered");
    // But it is still answered, because the sender is entitled to know.
    assert_eq!(out.replies.len(), 1);
    assert_eq!(parse_reply(&out.replies[0]).status, 1, "BadMagic");
}

/// A datagram that is not ours gets **no reply at all**. Answering a
/// datagram we could not identify would make this an amplifier.
#[test]
fn a_datagram_for_another_port_or_protocol_is_silently_ignored() {
    for (what, r) in [
        (
            "a different UDP port",
            Request {
                dport: 1234,
                ..Request::default()
            },
        ),
        (
            "the RoCE data port, not ours",
            Request {
                dport: 4791,
                ..Request::default()
            },
        ),
        (
            "not IPv4",
            Request {
                ethertype: 0x86DD,
                ..Request::default()
            },
        ),
        (
            "not UDP",
            Request {
                protocol: 6,
                ..Request::default()
            },
        ),
        (
            "IPv4 with options",
            Request {
                ver_ihl: 0x46,
                ..Request::default()
            },
        ),
    ] {
        let out = run(&[frame_of(r)]);
        assert!(
            out.replies.is_empty(),
            "{what}: must not be answered, or this widget is an amplifier"
        );
        assert!(out.qp_writes.is_empty(), "{what}: nothing configured");
        assert!(out.statuses.is_empty(), "{what}: no decision reported");
    }
}

/// **A reply is not a request.** Two managers pointed at each other
/// must not generate an endless exchange, so a datagram carrying the
/// Response opcode is refused.
#[test]
fn a_response_opcode_is_refused() {
    let out = run(&[frame_of(Request {
        opcode: 3,
        ..Request::default()
    })]);
    assert_eq!(out.statuses, vec![CmStatus::BadOpcode]);
    assert!(out.qp_writes.is_empty());
}

/// Codes we do not implement are refused rather than being coerced to a
/// default — a request for a 2048-byte MTU must not quietly become 1024.
#[test]
fn unimplemented_parameter_codes_are_refused() {
    for (what, r) in [
        (
            "transport code 2",
            Request {
                transport: 2,
                ..Request::default()
            },
        ),
        (
            "transport code 255",
            Request {
                transport: 255,
                ..Request::default()
            },
        ),
        (
            "MTU code 0",
            Request {
                pmtu: 0,
                ..Request::default()
            },
        ),
        (
            "MTU code 6",
            Request {
                pmtu: 6,
                ..Request::default()
            },
        ),
    ] {
        let out = run(&[frame_of(r)]);
        assert_eq!(
            out.statuses,
            vec![CmStatus::BadParameter],
            "{what} should be refused"
        );
        assert!(out.qp_writes.is_empty(), "{what}: nothing configured");
    }
    // And every code that *is* implemented is accepted, so the test
    // above is not passing because everything is refused.
    for pmtu in 1..=5u8 {
        for transport in 0..=1u8 {
            let out = run(&[frame_of(Request {
                pmtu,
                transport,
                ..Request::default()
            })]);
            assert_eq!(
                out.statuses,
                vec![CmStatus::Accepted],
                "MTU code {pmtu} with transport {transport} is implemented"
            );
        }
    }
}

/// A zero-length region cannot be written to, so registering one is
/// refused rather than creating a region that rejects every access.
#[test]
fn a_zero_length_region_is_refused() {
    let out = run(&[frame_of(Request {
        length: 0,
        ..Request::default()
    })]);
    assert_eq!(out.statuses, vec![CmStatus::BadRegion]);
    assert!(out.mr_writes.is_empty());
}

/// A truncated datagram is refused rather than acted on with whatever
/// happened to be in the registers.
#[test]
fn a_truncated_datagram_is_refused() {
    let full = frame_of(Request::default());
    for cut in [41usize, 45, 60, 80, 81] {
        let out = run(&[full[..cut].to_vec()]);
        assert!(
            out.qp_writes.is_empty(),
            "a datagram cut at {cut} bytes must not configure anything"
        );
        assert!(
            out.statuses.iter().all(|s| *s != CmStatus::Accepted),
            "nor be accepted: {:?}",
            out.statuses
        );
    }
}

/// **A frame too short to have been identified as ours draws no
/// reply.** Otherwise this widget is an amplifier: the UDP destination
/// port sits at offset 37, so a frame that ends before it has passed
/// every check that ran without ever having been shown to be addressed
/// here — and answering it would turn one byte into 82.
///
/// Found by reading the generated state diagram, which showed an
/// `Idle -> Reply` edge that should not have existed.
#[test]
fn a_frame_too_short_to_identify_draws_no_reply() {
    let full = frame_of(Request::default());
    for cut in 1..38usize {
        let out = run(&[full[..cut].to_vec()]);
        assert!(
            out.replies.is_empty(),
            "a {cut}-byte frame has not reached the port check at offset \
             37, so it must not be answered -- {} reply/replies sent",
            out.replies.len()
        );
        assert!(out.statuses.is_empty(), "{cut} bytes: no decision either");
        assert!(out.qp_writes.is_empty(), "{cut} bytes: nothing configured");
    }
    // From offset 38 the port has been checked, so a truncated-but-ours
    // datagram *is* answered -- the sender is entitled to know why it
    // was refused.
    for cut in [38usize, 45, 81] {
        let out = run(&[full[..cut].to_vec()]);
        assert_eq!(
            out.replies.len(),
            1,
            "a {cut}-byte frame has been identified as ours and should \
             get an explanation"
        );
        assert_eq!(out.statuses, vec![CmStatus::BadMagic], "{cut} bytes");
    }
}

/// Two requests in a row are both handled, which means the registers
/// and the reply emitter are reset between them.
#[test]
fn two_requests_in_a_row_are_both_handled() {
    let a = Request::default();
    let b = Request {
        qpn: 0x000333,
        remote_qpn: 0x000444,
        rkey: 0x1234_5678,
        va: 0x0000_1111_2222_3333,
        length: 8192,
        local_base: 0x0003_0000,
        pmtu: 1,
        transport: 0,
        ..Request::default()
    };
    let out = run(&[frame_of(a), frame_of(b)]);
    assert_eq!(out.statuses, vec![CmStatus::Accepted, CmStatus::Accepted]);
    assert_eq!(out.qp_writes.len(), 2);
    assert_eq!(out.replies.len(), 2);

    assert_eq!(out.qp_writes[1].qpn.raw(), b.qpn as u128);
    let ctx = out.qp_writes[1].ctx.expect("configured");
    assert_eq!(ctx.transport, Transport::Rc, "code 0 is RC");
    assert_eq!(ctx.pmtu, Pmtu::B256, "code 1 is 256");
    assert_eq!(out.mr_writes[1].rkey.raw(), b.rkey as u128);
    let reg = out.mr_writes[1].region.expect("registered");
    assert_eq!(reg.length.raw(), 8192);
    assert_eq!(parse_reply(&out.replies[1]).qpn, b.qpn);
}

/// **A documented limitation, pinned so it cannot change silently.**
///
/// The manager does not verify the IPv4 header checksum, and there is
/// no Invariant CRC on a connection-manager datagram — the RoCE data
/// path's ICRC covers RoCE frames only. Integrity therefore rests on
/// the Ethernet FCS, which the MAC checks before a frame reaches this
/// widget, exactly as it does for any other UDP service.
///
/// That is adequate on a local link and weaker than the data path on a
/// routed one. It is recorded as a follow-up rather than left to be
/// discovered, and this test exists so that a future change which
/// *does* verify the checksum has to come here and say so.
#[test]
fn the_manager_does_not_verify_the_ip_checksum() {
    // `frame_of` writes a zero checksum, which is wrong for this
    // header, and the request is accepted regardless.
    let f = frame_of(Request::default());
    assert_eq!(&f[24..26], &[0, 0], "the fixture's checksum really is zero");
    let out = run(&[f]);
    assert_eq!(
        out.statuses,
        vec![CmStatus::Accepted],
        "accepted despite a wrong IPv4 checksum -- see this test's docs"
    );
}
