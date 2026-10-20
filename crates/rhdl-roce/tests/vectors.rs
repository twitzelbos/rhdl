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

// ---------------------------------------------------------------------
// The vector file
// ---------------------------------------------------------------------

const VECTORS: &str = include_str!("vectors/write_vectors.txt");

/// One reference packet.
struct Vector {
    name: String,
    /// The generator's one-line description, quoted back on failure.
    note: String,
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
