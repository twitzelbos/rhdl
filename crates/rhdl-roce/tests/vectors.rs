//! Tier 6 — interoperability against an independent implementation.
//!
//! `roce-v2-architecture.md` §10 argues that `iverilog` agreeing with
//! the Rust simulator proves only that RHDL is self-consistent, and says
//! nothing about whether a real NIC will accept the packet. This file is
//! the tier that closes that gap: every header's bytes and every frame's
//! ICRC are checked against reference packets built by
//! [Scapy](https://scapy.net)'s `scapy.contrib.roce` — an independent
//! implementation, used in practice to dissect real RoCE traffic.
//!
//! The vectors are committed and these tests only read them, so a full
//! `cargo test` leaves the tree clean (CLAUDE.md §8). Regenerate them
//! deliberately with `tools/gen_vectors.py`.
//!
//! The vector file records `icrc_input` — the exact byte sequence the
//! CRC is computed over — in addition to the finished frame. That is
//! deliberate: it lets a mismatch be localised to either the *masking*
//! or the *CRC itself*, which are independent failure modes that would
//! otherwise be indistinguishable.

use rhdl::prelude::*;
use rhdl_fpga::core::crc32::Crc32;
use rhdl_roce::headers::{
    Bth, Reth, Transport, WriteOpcode, bth_byte, encode_write_opcode, reth_byte,
};
use rhdl_roce::icrc::{COVERAGE_START, Icrc, In as IcrcIn};
use rhdl_roce::qp::context::{Pmtu, QpContext};
use rhdl_roce::qp::segmenter::PacketPlan;

// ---------------------------------------------------------------------
// The vector file
// ---------------------------------------------------------------------

const VECTORS: &str = include_str!("vectors/write_vectors.txt");

/// One reference packet.
struct Vector {
    name: String,
    /// The generator's one-line description, quoted back on failure.
    note: String,
    src_mac: u64,
    dst_mac: u64,
    src_ip: u32,
    dst_ip: u32,
    sport: u16,
    ip_id: u16,
    ttl: u8,
    opcode: u8,
    dqpn: u32,
    psn: u32,
    padcount: u8,
    ackreq: bool,
    has_reth: bool,
    va: u64,
    rkey: u32,
    dma_len: u32,
    payload: Vec<u8>,
    icrc_input: Vec<u8>,
    icrc: Vec<u8>,
    frame: Vec<u8>,
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("bad hex in vector file"))
        .collect()
}

fn num(s: &str) -> u128 {
    let t = s.trim();
    match t.strip_prefix("0x") {
        Some(hex) => u128::from_str_radix(hex, 16).expect("bad hex number"),
        None => t.parse().expect("bad decimal number"),
    }
}

fn vectors() -> Vec<Vector> {
    let mut out = Vec::new();
    for record in VECTORS.split("\n\n") {
        let mut f = std::collections::HashMap::new();
        for line in record.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            // Split on '=' rather than " = ": a zero-length payload
            // records as `payload =` with nothing after it, which
            // `split_once(" = ")` misses entirely.
            if let Some((k, v)) = line.split_once('=') {
                f.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        if !f.contains_key("frame") {
            continue;
        }
        let has_reth = num(&f["has_reth"]) == 1;
        out.push(Vector {
            name: f["name"].clone(),
            note: f["note"].clone(),
            src_mac: num(&f["src_mac"]) as u64,
            dst_mac: num(&f["dst_mac"]) as u64,
            src_ip: num(&f["src_ip"]) as u32,
            dst_ip: num(&f["dst_ip"]) as u32,
            sport: num(&f["sport"]) as u16,
            ip_id: num(&f["ip_id"]) as u16,
            ttl: num(&f["ttl"]) as u8,
            opcode: num(&f["opcode"]) as u8,
            dqpn: num(&f["dqpn"]) as u32,
            psn: num(&f["psn"]) as u32,
            padcount: num(&f["padcount"]) as u8,
            ackreq: num(&f["ackreq"]) == 1,
            has_reth,
            va: if has_reth { num(&f["va"]) as u64 } else { 0 },
            rkey: if has_reth { num(&f["rkey"]) as u32 } else { 0 },
            dma_len: if has_reth {
                num(&f["dma_len"]) as u32
            } else {
                0
            },
            payload: unhex(&f["payload"]),
            icrc_input: unhex(&f["icrc_input"]),
            icrc: unhex(&f["icrc"]),
            frame: unhex(&f["frame"]),
        });
    }
    assert!(
        out.len() >= 8,
        "expected the committed vector set, found {} records",
        out.len()
    );
    out
}

/// Layer offsets in an IPv4, untagged RoCEv2 frame.
const IP_OFF: usize = 14;
const UDP_OFF: usize = 34;
const BTH_OFF: usize = 42;
const RETH_OFF: usize = 54;

impl Vector {
    fn bth(&self) -> Bth {
        Bth {
            opcode: bits(self.opcode as u128),
            solicited: false,
            migreq: false,
            padcount: bits(self.padcount as u128),
            version: bits(0),
            pkey: bits(0xFFFF),
            fecn: false,
            becn: false,
            resv6: bits(0),
            dqpn: bits(self.dqpn as u128),
            ackreq: self.ackreq,
            resv7: bits(0),
            psn: bits(self.psn as u128),
        }
    }

    fn reth(&self) -> Reth {
        Reth {
            va: bits(self.va as u128),
            rkey: bits(self.rkey as u128),
            dma_len: bits(self.dma_len as u128),
        }
    }
}

/// The masked offsets in [`rhdl_roce::icrc`] are absolute, which bakes
/// in "IPv4, no VLAN tag". This test pins that assumption to the
/// vectors: if a future vector set ever carries a VLAN tag or IPv6, the
/// layout check fails here rather than silently producing a wrong ICRC.
#[test]
fn the_frame_layout_the_masking_assumes_actually_holds() {
    for v in vectors() {
        assert_eq!(
            &v.frame[12..14],
            &[0x08, 0x00],
            "{}: EtherType should be IPv4 (0x0800) -- a VLAN tag would shift every masked offset",
            v.name
        );
        assert_eq!(
            v.frame[IP_OFF] >> 4,
            4,
            "{}: IP version nibble should be 4",
            v.name
        );
        assert_eq!(
            v.frame[IP_OFF] & 0x0F,
            5,
            "{}: IP header should be 20 bytes (IHL 5); options would shift the UDP offset",
            v.name
        );
        assert_eq!(
            v.frame[IP_OFF + 9],
            17,
            "{}: IP protocol should be UDP (17)",
            v.name
        );
        assert_eq!(
            u16::from_be_bytes([v.frame[UDP_OFF + 2], v.frame[UDP_OFF + 3]]),
            4791,
            "{}: UDP destination port should be the IANA-assigned RoCEv2 port",
            v.name
        );
        assert_eq!(
            u16::from_be_bytes([v.frame[UDP_OFF + 6], v.frame[UDP_OFF + 7]]),
            0,
            "{}: RoCEv2 requires the UDP checksum to be zero",
            v.name
        );
    }
}

// ---------------------------------------------------------------------
// Headers
// ---------------------------------------------------------------------

/// Every BTH we build serialises to the bytes Scapy put on the wire.
#[test]
fn bth_bytes_match_scapy() {
    for v in vectors() {
        let expected = &v.frame[BTH_OFF..BTH_OFF + 12];
        let got: Vec<u8> = (0..12u128)
            .map(|i| bth_byte(v.bth(), bits(i)).raw() as u8)
            .collect();
        assert_eq!(
            got, expected,
            "BTH mismatch for {}\n  got      {:02x?}\n  expected {:02x?}",
            v.name, got, expected
        );
    }
}

/// And every RETH, where the opcode carries one.
#[test]
fn reth_bytes_match_scapy() {
    let mut checked = 0;
    for v in vectors() {
        if !v.has_reth {
            continue;
        }
        let expected = &v.frame[RETH_OFF..RETH_OFF + 16];
        let got: Vec<u8> = (0..16u128)
            .map(|i| reth_byte(v.reth(), bits(i)).raw() as u8)
            .collect();
        assert_eq!(
            got, expected,
            "RETH mismatch for {}\n  got      {:02x?}\n  expected {:02x?}",
            v.name, got, expected
        );
        checked += 1;
    }
    assert!(checked >= 4, "only {checked} vectors carried a RETH");
}

/// The typed opcode encoding reproduces the raw byte, for every
/// transport and operation the vectors exercise.
///
/// This is what makes [`encode_write_opcode`] load-bearing rather than
/// decorative: if the enum-to-byte mapping drifts, the packet is for a
/// different operation and the peer rejects it.
#[test]
fn typed_opcodes_reproduce_the_wire_byte() {
    let cases = [
        (Transport::Rc, WriteOpcode::First, 0x06u8),
        (Transport::Rc, WriteOpcode::Middle, 0x07),
        (Transport::Rc, WriteOpcode::Last, 0x08),
        (Transport::Rc, WriteOpcode::LastWithImmediate, 0x09),
        (Transport::Rc, WriteOpcode::Only, 0x0A),
        (Transport::Rc, WriteOpcode::OnlyWithImmediate, 0x0B),
        (Transport::Uc, WriteOpcode::First, 0x26),
        (Transport::Uc, WriteOpcode::Middle, 0x27),
        (Transport::Uc, WriteOpcode::Last, 0x28),
        (Transport::Uc, WriteOpcode::LastWithImmediate, 0x29),
        (Transport::Uc, WriteOpcode::Only, 0x2A),
        (Transport::Uc, WriteOpcode::OnlyWithImmediate, 0x2B),
    ];
    for (t, op, want) in cases {
        assert_eq!(
            encode_write_opcode(t, op).raw() as u8,
            want,
            "{t:?}/{op:?} should encode to {want:#04x}"
        );
    }
    // And every opcode byte appearing in the vectors is one of them.
    for v in vectors() {
        assert!(
            cases.iter().any(|(_, _, b)| *b == v.opcode),
            "vector {} uses opcode {:#04x}, which the typed encoding cannot produce",
            v.name,
            v.opcode
        );
    }
}

/// The payload sits where the opcode says it should.
///
/// This is what ties `carries_reth` to the wire: a FIRST or ONLY packet
/// pushes the payload 16 bytes further along than a MIDDLE or LAST,
/// and getting that wrong puts the payload inside the RETH.
#[test]
fn the_payload_sits_at_the_offset_the_opcode_implies() {
    use rhdl_roce::headers::carries_reth;
    for v in vectors() {
        let payload_off = if v.has_reth {
            RETH_OFF + 16
        } else {
            BTH_OFF + 12
        };
        assert_eq!(
            &v.frame[payload_off..payload_off + v.payload.len()],
            &v.payload[..],
            "{}: payload is not at offset {payload_off}",
            v.name
        );
        assert_eq!(
            v.frame.len(),
            payload_off + v.payload.len() + 4,
            "{}: frame length should be headers + payload + 4-byte ICRC",
            v.name
        );
        // And the typed predicate agrees with what the vector records.
        let op = match v.opcode & 0x1F {
            0x06 => WriteOpcode::First,
            0x07 => WriteOpcode::Middle,
            0x08 => WriteOpcode::Last,
            0x09 => WriteOpcode::LastWithImmediate,
            0x0A => WriteOpcode::Only,
            0x0B => WriteOpcode::OnlyWithImmediate,
            other => panic!("{}: unexpected operation {other:#04x}", v.name),
        };
        assert_eq!(
            carries_reth(op),
            v.has_reth,
            "{}: carries_reth({op:?}) disagrees with the vector",
            v.name
        );
    }
}

/// Out-of-range byte indices clamp rather than wrapping or tripping the
/// kernel VM's shift bound. See `headers`' module docs.
#[test]
fn byte_selectors_clamp_out_of_range_indices() {
    let v = &vectors()[0];
    let last_bth = bth_byte(v.bth(), bits(11));
    for i in 12u128..16 {
        assert_eq!(
            bth_byte(v.bth(), bits(i)),
            last_bth,
            "bth_byte({i}) should clamp to the last byte"
        );
    }
    let last_reth = reth_byte(v.reth(), bits(15));
    for i in 16u128..32 {
        assert_eq!(
            reth_byte(v.reth(), bits(i)),
            last_reth,
            "reth_byte({i}) should clamp to the last byte"
        );
    }
}

// ---------------------------------------------------------------------
// ICRC
// ---------------------------------------------------------------------

/// Drive a frame through the [`Icrc`] widget and return the wire-order
/// ICRC it produces.
fn icrc_of(frame: &[u8]) -> [u8; 4] {
    let covered_end = frame.len() - 4;
    let mut stream: Vec<IcrcIn> = vec![IcrcIn {
        byte: bits(0),
        frame_index: bits(0),
        enable: false,
        start: true,
    }];
    for (idx, &byte) in frame.iter().enumerate() {
        stream.push(IcrcIn {
            byte: bits(byte as u128),
            frame_index: bits(idx as u128),
            enable: idx >= COVERAGE_START as usize && idx < covered_end,
            start: false,
        });
    }
    stream.push(IcrcIn {
        byte: bits(0),
        frame_index: bits(0),
        enable: false,
        start: false,
    });
    let uut = Icrc::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let wire = uut
        .run(input)
        .synchronous_sample()
        .map(|s| s.output.wire.raw() as u32)
        .last()
        .expect("the widget produced no samples");
    wire.to_be_bytes()
}

/// **The acceptance test for this phase.** Every committed frame's ICRC
/// is reproduced exactly by the hardware.
#[test]
fn icrc_matches_scapy_for_every_vector() {
    for v in vectors() {
        let got = icrc_of(&v.frame);
        assert_eq!(
            &got[..],
            &v.icrc[..],
            "ICRC mismatch for {} ({})\n  got      {:02x?}\n  expected {:02x?}",
            v.name,
            v.note,
            got,
            v.icrc
        );
    }
}

/// The CRC-32 over the committed `icrc_input` agrees too.
///
/// This separates the two failure modes: if this passes and
/// [`icrc_matches_scapy_for_every_vector`] fails, the masking is wrong
/// and the CRC is fine. If both fail, suspect the CRC.
#[test]
fn crc32_over_the_recorded_icrc_input_agrees() {
    use rhdl_fpga::core::crc32::In as CrcIn;
    for v in vectors() {
        let mut stream: Vec<CrcIn> = vec![CrcIn {
            byte: bits(0),
            enable: false,
            clear: true,
        }];
        for &b in &v.icrc_input {
            stream.push(CrcIn {
                byte: bits(b as u128),
                enable: true,
                clear: false,
            });
        }
        stream.push(CrcIn {
            byte: bits(0),
            enable: false,
            clear: false,
        });
        let uut = Crc32::default();
        let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
        let crc = uut
            .run(input)
            .synchronous_sample()
            .map(|s| s.output.raw() as u32)
            .last()
            .unwrap();
        // The on-wire field is the value little-endian.
        assert_eq!(
            crc.to_le_bytes().to_vec(),
            v.icrc,
            "CRC-32 over icrc_input disagrees for {}",
            v.name
        );
    }
}

/// A corrupted frame must produce a different ICRC.
///
/// Without this, an engine stuck at a constant would pass every test
/// above for a single vector set. Each covered byte is flipped in turn
/// and the ICRC is required to change.
#[test]
fn icrc_detects_corruption_in_every_covered_byte() {
    let v = &vectors()[0];
    let good = icrc_of(&v.frame);
    let covered_end = v.frame.len() - 4;
    for idx in (COVERAGE_START as usize)..covered_end {
        let mut bad = v.frame.clone();
        bad[idx] ^= 0x01;
        let got = icrc_of(&bad);
        if rhdl_roce::icrc::is_masked_offset(idx as u128) {
            assert_eq!(
                got, good,
                "offset {idx} is masked, so flipping it must NOT change the ICRC"
            );
        } else {
            assert_ne!(
                got, good,
                "flipping covered byte {idx} did not change the ICRC"
            );
        }
    }
}

/// Bytes before the coverage window — the Ethernet header — must not
/// affect the ICRC at all.
#[test]
fn the_ethernet_header_is_not_covered() {
    let v = &vectors()[0];
    let good = icrc_of(&v.frame);
    for idx in 0..(COVERAGE_START as usize) {
        let mut altered = v.frame.clone();
        altered[idx] ^= 0xFF;
        assert_eq!(
            icrc_of(&altered),
            good,
            "Ethernet-header byte {idx} changed the ICRC; it is outside coverage"
        );
    }
}

// ---------------------------------------------------------------------
// Segmenter
// ---------------------------------------------------------------------

/// **The segmenter against real packets.** The vector set contains a
/// 600-byte message at a 256-byte path MTU as three Scapy-generated
/// packets — `uc_write_first_256`, `uc_write_middle_256` and
/// `uc_write_last_88`. The segmenter is asked for the same message and
/// every field of its plan is checked against what is actually on those
/// wires: the opcode byte, the sequence number, the payload length, the
/// pad count, and the RETH that only the first packet carries.
///
/// This is what makes the segmenter's correctness a claim about the
/// protocol rather than about my own arithmetic. The three vectors were
/// generated independently of this code and of each other.
#[test]
fn the_segmenter_reproduces_a_real_multi_packet_message() {
    use rhdl_roce::headers::{Transport, carries_reth, encode_write_opcode};
    use rhdl_roce::qp::context::{MessageRequest, Pmtu, QpContext};
    use rhdl_roce::qp::segmenter::{In as SegIn, WriteSegmenter};

    let all = vectors();
    let expected: Vec<&Vector> = [
        "uc_write_first_256",
        "uc_write_middle_256",
        "uc_write_last_88",
    ]
    .iter()
    .map(|n| {
        all.iter()
            .find(|v| v.name == *n)
            .unwrap_or_else(|| panic!("vector {n} is missing"))
    })
    .collect();

    // Every vector in the message must agree on the total length, or
    // the premise of the test is wrong.
    let total: usize = expected.iter().map(|v| v.payload.len()).sum();
    assert_eq!(
        total, 600,
        "the three vectors should carry 600 bytes in all"
    );

    let idle = SegIn {
        start: false,
        msg: MessageRequest {
            va: bits(expected[0].va as u128),
            length: bits(total as u128),
        },
        ctx: QpContext {
            dqpn: bits(expected[0].dqpn as u128),
            transport: Transport::Uc,
            pmtu: Pmtu::B256,
            rkey: bits(expected[0].rkey as u128),
            pkey: bits(0xFFFF),
        },
        psn_start: bits(expected[0].psn as u128),
        ready: true,
    };
    let mut stream = vec![SegIn {
        start: true,
        ..idle
    }];
    for _ in 0..12 {
        stream.push(idle);
    }

    let uut = WriteSegmenter::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let plans: Vec<_> = uut
        .run(input)
        .synchronous_sample()
        .filter_map(|s| s.output.plan)
        .collect();

    assert_eq!(
        plans.len(),
        expected.len(),
        "expected {} packets, got {}",
        expected.len(),
        plans.len()
    );

    for (plan, want) in plans.iter().zip(&expected) {
        // The opcode, compared as the byte that actually goes on the
        // wire rather than as an enum, so the encoding is in scope too.
        assert_eq!(
            encode_write_opcode(Transport::Uc, plan.opcode).raw() as u8,
            want.opcode,
            "{}: opcode byte",
            want.name
        );
        assert_eq!(plan.psn.raw() as u32, want.psn, "{}: PSN", want.name);
        assert_eq!(
            plan.payload_bytes.raw() as usize,
            want.payload.len(),
            "{}: payload length",
            want.name
        );
        assert_eq!(
            plan.padcount.raw() as u8,
            want.padcount,
            "{}: pad count",
            want.name
        );
        assert_eq!(
            plan.carries_reth, want.has_reth,
            "{}: RETH presence",
            want.name
        );
        assert_eq!(
            carries_reth(plan.opcode),
            want.has_reth,
            "{}: carries_reth disagrees with the opcode",
            want.name
        );
        if want.has_reth {
            // The RETH holds the message's base address and *total*
            // length -- 600, not this packet's 256. The single field
            // most easily got wrong.
            assert_eq!(plan.reth.va.raw() as u64, want.va, "{}: RETH va", want.name);
            assert_eq!(
                plan.reth.rkey.raw() as u32,
                want.rkey,
                "{}: RETH rkey",
                want.name
            );
            assert_eq!(
                plan.reth.dma_len.raw() as u32,
                want.dma_len,
                "{}: RETH dma_len should be the whole message",
                want.name
            );
        }
    }
}

/// The single-packet vectors are reproduced too, including the ragged
/// 13-byte one whose pad count is 3 and the zero-length one.
#[test]
fn the_segmenter_reproduces_every_single_packet_vector() {
    use rhdl_roce::headers::{Transport, encode_write_opcode};
    use rhdl_roce::qp::context::{MessageRequest, Pmtu, QpContext};
    use rhdl_roce::qp::segmenter::{In as SegIn, WriteSegmenter};

    for v in vectors() {
        // Only the ONLY packets are whole messages on their own.
        if v.opcode & 0x1F != 0x0A {
            continue;
        }
        let transport = if v.opcode & 0xE0 == 0x20 {
            Transport::Uc
        } else {
            Transport::Rc
        };
        // `dma_len` is the message length; the payload on the wire
        // includes the pad bytes, so it can be longer.
        let idle = SegIn {
            start: false,
            msg: MessageRequest {
                va: bits(v.va as u128),
                length: bits(v.dma_len as u128),
            },
            ctx: QpContext {
                dqpn: bits(v.dqpn as u128),
                transport,
                pmtu: Pmtu::B1024,
                rkey: bits(v.rkey as u128),
                pkey: bits(0xFFFF),
            },
            psn_start: bits(v.psn as u128),
            ready: true,
        };
        let mut stream = vec![SegIn {
            start: true,
            ..idle
        }];
        for _ in 0..4 {
            stream.push(idle);
        }
        let uut = WriteSegmenter::default();
        let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
        let plans: Vec<_> = uut
            .run(input)
            .synchronous_sample()
            .filter_map(|s| s.output.plan)
            .collect();

        assert_eq!(plans.len(), 1, "{}: should be a single packet", v.name);
        let p = &plans[0];
        assert_eq!(
            encode_write_opcode(transport, p.opcode).raw() as u8,
            v.opcode,
            "{}: opcode byte",
            v.name
        );
        assert_eq!(p.psn.raw() as u32, v.psn, "{}: PSN", v.name);
        assert_eq!(
            p.payload_bytes.raw() as u32,
            v.dma_len,
            "{}: payload length",
            v.name
        );
        assert_eq!(p.padcount.raw() as u8, v.padcount, "{}: pad count", v.name);
        assert_eq!(
            p.reth.dma_len.raw() as u32,
            v.dma_len,
            "{}: RETH dma_len",
            v.name
        );
    }
}

// ---------------------------------------------------------------------
// Whole-frame header assembly
// ---------------------------------------------------------------------

impl Vector {
    fn frame_config(&self) -> rhdl_roce::frame::FrameConfig {
        rhdl_roce::frame::FrameConfig {
            dst_mac: bits(self.dst_mac as u128),
            src_mac: bits(self.src_mac as u128),
            src_ip: bits(self.src_ip as u128),
            dst_ip: bits(self.dst_ip as u128),
            src_port: bits(self.sport as u128),
            ip_id: bits(self.ip_id as u128),
            ttl: bits(self.ttl as u128),
        }
    }

    fn qp_context(&self) -> QpContext {
        QpContext {
            dqpn: bits(self.dqpn as u128),
            transport: self.transport(),
            pmtu: Pmtu::B1024,
            rkey: bits(self.rkey as u128),
            pkey: bits(0xFFFF),
        }
    }

    fn transport(&self) -> Transport {
        if self.opcode & 0xE0 == 0x20 {
            Transport::Uc
        } else {
            Transport::Rc
        }
    }

    /// The plan this frame corresponds to, built from the recorded
    /// fields rather than from the segmenter, so that a frame-assembly
    /// failure cannot be blamed on the segmenter.
    fn plan(&self) -> PacketPlan {
        PacketPlan {
            opcode: self.write_opcode(),
            psn: bits(self.psn as u128),
            // `payload` on the wire includes the pad bytes; the plan's
            // `payload_bytes` is the data length.
            payload_bytes: bits((self.payload.len() - self.padcount as usize) as u128),
            padcount: bits(self.padcount as u128),
            carries_reth: self.has_reth,
            reth: self.reth(),
        }
    }

    fn write_opcode(&self) -> WriteOpcode {
        match self.opcode & 0x1F {
            0x06 => WriteOpcode::First,
            0x07 => WriteOpcode::Middle,
            0x08 => WriteOpcode::Last,
            0x09 => WriteOpcode::LastWithImmediate,
            0x0A => WriteOpcode::Only,
            0x0B => WriteOpcode::OnlyWithImmediate,
            other => panic!("{}: unexpected operation {other:#04x}", self.name),
        }
    }
}

/// **Complete frame headers, byte for byte, against Scapy.**
///
/// Everything from the first Ethernet byte up to the payload: the
/// Ethernet II header, the IPv4 header *including its checksum*, the UDP
/// header with its mandated zero checksum and computed length, the BTH,
/// and the RETH where the opcode carries one. This is the strongest
/// statement available about the send path short of putting a frame on
/// a wire — the only bytes not generated here are the payload, which
/// comes from a stream, and the ICRC, which has its own test.
#[test]
fn whole_frame_headers_match_scapy() {
    use rhdl_roce::frame::{bth_from_plan, header_byte, header_bytes};
    for v in vectors() {
        let cfg = v.frame_config();
        let ctx = v.qp_context();
        let plan = v.plan();
        let n = header_bytes(plan).raw() as usize;
        assert_eq!(
            n,
            if v.has_reth { 70 } else { 54 },
            "{}: header length",
            v.name
        );
        let got: Vec<u8> = (0..n)
            .map(|i| {
                header_byte(
                    cfg,
                    bth_from_plan(ctx, plan, v.ackreq),
                    plan,
                    bits(i as u128),
                )
                .raw() as u8
            })
            .collect();
        let expected = &v.frame[..n];
        assert_eq!(
            got, expected,
            "{} ({}): header block mismatch\n  got      {:02x?}\n  expected {:02x?}",
            v.name, v.note, got, expected
        );
    }
}

/// The IPv4 header checksum is computed, not copied.
///
/// Called out separately because it is the one field in the header block
/// whose value is arithmetic rather than a parameter, and because a
/// router drops a frame with a bad header checksum long before the ICRC
/// is ever looked at.
#[test]
fn the_ipv4_header_checksum_is_correct() {
    use rhdl_roce::frame::ipv4_checksum;
    for v in vectors() {
        let want = u16::from_be_bytes([v.frame[IP_OFF + 10], v.frame[IP_OFF + 11]]);
        let got = ipv4_checksum(v.frame_config(), v.plan()).raw() as u16;
        assert_eq!(got, want, "{}: IPv4 header checksum", v.name);
        // And the standard property: the one's-complement sum over the
        // whole header, checksum included, is zero.
        let mut acc: u32 = 0;
        for i in (0..20).step_by(2) {
            acc += u16::from_be_bytes([v.frame[IP_OFF + i], v.frame[IP_OFF + i + 1]]) as u32;
        }
        while acc >> 16 != 0 {
            acc = (acc & 0xFFFF) + (acc >> 16);
        }
        assert_eq!(acc, 0xFFFF, "{}: header does not sum to zero", v.name);
    }
}

/// The length fields agree with the frame that was actually built.
///
/// An off-by-one in either makes the peer's stack discard the packet,
/// and neither is covered by the ICRC — the ICRC masks the IPv4
/// checksum but not the lengths.
#[test]
fn the_length_fields_match_the_frame() {
    use rhdl_roce::frame::{ip_total_length, payload_on_wire, udp_length};
    for v in vectors() {
        let plan = v.plan();
        let want_ip = u16::from_be_bytes([v.frame[IP_OFF + 2], v.frame[IP_OFF + 3]]);
        let want_udp = u16::from_be_bytes([v.frame[UDP_OFF + 4], v.frame[UDP_OFF + 5]]);
        assert_eq!(
            ip_total_length(plan).raw() as u16,
            want_ip,
            "{}: IP total length",
            v.name
        );
        assert_eq!(
            udp_length(plan).raw() as u16,
            want_udp,
            "{}: UDP length",
            v.name
        );
        // Both must also agree with the frame's real size.
        assert_eq!(
            want_ip as usize,
            v.frame.len() - IP_OFF,
            "{}: IP total length disagrees with the frame",
            v.name
        );
        assert_eq!(
            payload_on_wire(plan).raw() as usize,
            v.payload.len(),
            "{}: padded payload length",
            v.name
        );
    }
}

/// **The capstone: every byte of a complete RoCEv2 frame.**
///
/// Header block from [`rhdl_roce::frame::header_byte`], payload from the
/// vector (it would come from a stream in hardware), ICRC from the
/// [`Icrc`] widget driven over the bytes just assembled — not over
/// Scapy's frame. So the ICRC is computed from RHDL's own output, which
/// means a header byte that is wrong in a way the ICRC covers would
/// produce *two* differences rather than cancelling out.
///
/// This is as close to "RHDL emits a valid RoCEv2 frame" as it is
/// possible to get without a wire: the only thing between this and a
/// NIC accepting the packet is the Ethernet FCS, which the MAC appends.
#[test]
fn a_complete_frame_matches_scapy_byte_for_byte() {
    use rhdl_roce::frame::{bth_from_plan, header_byte, header_bytes};
    for v in vectors() {
        let cfg = v.frame_config();
        let ctx = v.qp_context();
        let plan = v.plan();
        let n = header_bytes(plan).raw() as usize;

        // 1. the header block, from the frame generator
        let mut frame: Vec<u8> = (0..n)
            .map(|i| {
                header_byte(
                    cfg,
                    bth_from_plan(ctx, plan, v.ackreq),
                    plan,
                    bits(i as u128),
                )
                .raw() as u8
            })
            .collect();
        // 2. the payload, including its pad bytes
        frame.extend_from_slice(&v.payload);
        // 3. the ICRC, computed over what we just built
        let covered: Vec<u8> = frame[COVERAGE_START as usize..].to_vec();
        let mut stream: Vec<IcrcIn> = vec![IcrcIn {
            byte: bits(0),
            frame_index: bits(0),
            enable: false,
            start: true,
        }];
        for (k, &byte) in covered.iter().enumerate() {
            stream.push(IcrcIn {
                byte: bits(byte as u128),
                frame_index: bits((COVERAGE_START as usize + k) as u128),
                enable: true,
                start: false,
            });
        }
        stream.push(IcrcIn {
            byte: bits(0),
            frame_index: bits(0),
            enable: false,
            start: false,
        });
        let uut = Icrc::default();
        let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
        let wire = uut
            .run(input)
            .synchronous_sample()
            .map(|s| s.output.wire.raw() as u32)
            .last()
            .unwrap();
        frame.extend_from_slice(&wire.to_be_bytes());

        assert_eq!(
            frame.len(),
            v.frame.len(),
            "{}: frame length {} vs expected {}",
            v.name,
            frame.len(),
            v.frame.len()
        );
        if frame != v.frame {
            // Report the first difference rather than two hex dumps.
            let at = frame
                .iter()
                .zip(&v.frame)
                .position(|(a, b)| a != b)
                .unwrap();
            panic!(
                "{} ({}): frames differ at offset {} (got {:#04x}, expected {:#04x})\n  got      {:02x?}\n  expected {:02x?}",
                v.name, v.note, at, frame[at], v.frame[at], frame, v.frame
            );
        }
    }
}
