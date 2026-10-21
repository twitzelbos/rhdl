//! Differential fuzzing against Scapy.
//!
//! The eight curated vectors in `vectors.rs` pin the shapes that
//! matter; they do not explore the input space. This does: 2000
//! randomised frames, generated from a fixed seed by
//! `tools/gen_vectors.py`, with [Scapy](https://scapy.net) — an
//! independent implementation — saying what each one's bytes should be.
//!
//! The corpus is committed rather than regenerated, so these tests are
//! deterministic, need no Python, and leave the tree clean (CLAUDE.md
//! §8). Regenerate deliberately when the generator changes.
//!
//! # What uniform fuzzing does *not* find
//!
//! Worth recording, because it is the more useful half of the lesson.
//! A mutation deleting the second carry fold from
//! [`rhdl_roce::frame::ipv4_checksum`] survived every curated vector
//! **and** 2000 uniformly-random fuzz cases. The reason is arithmetic:
//! the first fold only carries out of bit 15 when the accumulator lands
//! in one of about twenty values out of a range of roughly 450,000, so
//! the expected number of hits in 2000 uniform draws is about 0.09.
//! Measured: zero.
//!
//! So the corpus is **structure-aware**. Every tenth case is *solved*
//! into that window by choosing the IPv4 identification field to suit,
//! which takes the coverage from 0 cases to 199.
//! [`the_corpus_still_covers_the_carry_window`] asserts that, because
//! the whole value of those cases is lost silently if the generator
//! ever stops producing them.
//!
//! The general lesson: a fuzzer that does not know where the cliffs are
//! will not find them, and what told us where this cliff was was
//! mutation testing rather than fuzzing.
//!
//! # Payloads are derived, not stored
//!
//! Byte `i` of a case's payload is
//! `(payload_seed + i*31 + (i>>3)*7) & 0xFF` for `i < data_len`,
//! followed by `pad` zero bytes. Both the generator and this file
//! compute it, which keeps a 1 KB payload to eight characters in the
//! corpus instead of 2048.

use rhdl::prelude::*;
use rhdl_fpga::core::crc32::{INIT, XOR_OUT, crc32_step};
use rhdl_roce::frame::{FrameConfig, bth_from_plan, header_byte, header_bytes};
use rhdl_roce::headers::{Reth, Transport, WriteOpcode};
use rhdl_roce::icrc::{COVERAGE_START, PREFIX_REMAINDER, icrc_mask};
use rhdl_roce::qp::context::{Pmtu, QpContext};
use rhdl_roce::qp::segmenter::PacketPlan;

const CORPUS: &str = include_str!("vectors/write_vectors.txt");

/// One fuzz case, as the corpus records it.
struct Case {
    name: String,
    opcode: u8,
    cfg: FrameConfig,
    ctx: QpContext,
    plan: PacketPlan,
    ackreq: bool,
    has_reth: bool,
    data_len: usize,
    pad: usize,
    payload_seed: u32,
    header: Vec<u8>,
    icrc: Vec<u8>,
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("bad hex"))
        .collect()
}

fn num(s: &str) -> u128 {
    match s.strip_prefix("0x") {
        Some(h) => u128::from_str_radix(h, 16).expect("bad hex number"),
        None => s.parse().expect("bad decimal number"),
    }
}

fn write_opcode(op: &str) -> WriteOpcode {
    match op {
        "First" => WriteOpcode::First,
        "Middle" => WriteOpcode::Middle,
        "Last" => WriteOpcode::Last,
        "LastWithImmediate" => WriteOpcode::LastWithImmediate,
        "Only" => WriteOpcode::Only,
        "OnlyWithImmediate" => WriteOpcode::OnlyWithImmediate,
        other => panic!("unknown opcode name {other:?} in the corpus"),
    }
}

/// Column order is documented in the corpus header. A line with any
/// other field count is not a case — the file also holds the curated
/// `key = value` vectors and comments.
const FIELDS: usize = 22;

fn corpus() -> Vec<Case> {
    let mut out = Vec::new();
    for line in CORPUS.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != FIELDS || !f[0].starts_with("fuzz") {
            continue;
        }
        let has_reth = num(f[13]) == 1;
        let data_len = num(f[17]) as usize;
        let pad = num(f[18]) as usize;
        let opcode = num(f[2]) as u8;
        out.push(Case {
            name: f[0].to_string(),
            opcode,
            cfg: FrameConfig {
                src_mac: bits(num(f[3])),
                dst_mac: bits(num(f[4])),
                src_ip: bits(num(f[5])),
                dst_ip: bits(num(f[6])),
                src_port: bits(num(f[7])),
                dst_port: bits(4791),
                ip_id: bits(num(f[8])),
                ttl: bits(num(f[9])),
                dscp_ecn: bits(0),
            },
            ctx: QpContext {
                dqpn: bits(num(f[10])),
                transport: if opcode & 0xE0 == 0x20 {
                    Transport::Uc
                } else {
                    Transport::Rc
                },
                pmtu: Pmtu::B1024,
                rkey: bits(num(f[15])),
                pkey: bits(0xFFFF),
            },
            plan: PacketPlan {
                opcode: write_opcode(f[1]),
                psn: bits(num(f[11])),
                payload_bytes: bits(data_len as u128),
                padcount: bits(pad as u128),
                carries_reth: has_reth,
                reth: Reth {
                    va: bits(num(f[14])),
                    rkey: bits(num(f[15])),
                    dma_len: bits(num(f[16])),
                },
            },
            ackreq: num(f[12]) == 1,
            has_reth,
            data_len,
            pad,
            payload_seed: num(f[19]) as u32,
            header: unhex(f[20]),
            icrc: unhex(f[21]),
        });
    }
    assert_eq!(
        out.len(),
        2000,
        "the committed corpus should hold 2000 cases; parsed {}",
        out.len()
    );
    out
}

/// The formula the generator used, so payloads need not be stored.
fn payload_of(c: &Case) -> Vec<u8> {
    let mut v: Vec<u8> = (0..c.data_len)
        .map(|i| {
            let i = i as u32;
            (c.payload_seed
                .wrapping_add(i.wrapping_mul(31))
                .wrapping_add((i >> 3).wrapping_mul(7))
                & 0xFF) as u8
        })
        .collect();
    v.extend(std::iter::repeat_n(0u8, c.pad));
    v
}

/// Fold a byte sequence through the ICRC's masking and CRC kernels.
///
/// Uses the kernels directly rather than simulating the [`Icrc`] widget
/// around them: `vectors.rs` already drives the widget over real frames,
/// and 2000 cases of up to a kilobyte each is a lot of simulated cycles
/// to spend re-testing the register around a function.
///
/// [`Icrc`]: rhdl_roce::icrc::Icrc
fn icrc_of(frame: &[u8]) -> [u8; 4] {
    let mut rem = bits::<32>(PREFIX_REMAINDER);
    for (i, &b) in frame.iter().enumerate().skip(COVERAGE_START as usize) {
        let masked = icrc_mask(bits(b as u128), bits(i as u128));
        rem = crc32_step(rem, masked);
    }
    let finished = (rem ^ bits::<32>(XOR_OUT)).raw() as u32;
    // The on-wire field is the value little-endian.
    finished.to_le_bytes()
}

// ---------------------------------------------------------------------

/// **Every header byte of 2000 randomised frames, against Scapy.**
#[test]
fn fuzz_header_blocks_match_scapy() {
    let cases = corpus();
    for c in &cases {
        let n = header_bytes(c.plan).raw() as usize;
        assert_eq!(
            n,
            c.header.len(),
            "{}: header length {} vs corpus {}",
            c.name,
            n,
            c.header.len()
        );
        let got: Vec<u8> = (0..n)
            .map(|i| {
                header_byte(
                    c.cfg,
                    bth_from_plan(c.ctx, c.plan, c.ackreq),
                    c.plan,
                    bits(i as u128),
                )
                .raw() as u8
            })
            .collect();
        if got != c.header {
            let at = got
                .iter()
                .zip(&c.header)
                .position(|(a, b)| a != b)
                .unwrap_or(0);
            panic!(
                "{} (opcode {:#04x}): header differs at offset {} (got {:#04x}, expected {:#04x})\n  got      {:02x?}\n  expected {:02x?}",
                c.name, c.opcode, at, got[at], c.header[at], got, c.header
            );
        }
    }
}

/// **Every ICRC of 2000 randomised frames, against Scapy** — computed
/// over the frame this crate assembles, not over Scapy's bytes.
#[test]
fn fuzz_icrcs_match_scapy() {
    for c in &corpus() {
        let n = header_bytes(c.plan).raw() as usize;
        let mut frame: Vec<u8> = (0..n)
            .map(|i| {
                header_byte(
                    c.cfg,
                    bth_from_plan(c.ctx, c.plan, c.ackreq),
                    c.plan,
                    bits(i as u128),
                )
                .raw() as u8
            })
            .collect();
        frame.extend_from_slice(&payload_of(c));
        let got = icrc_of(&frame);
        assert_eq!(
            &got[..],
            &c.icrc[..],
            "{} (opcode {:#04x}, data_len {}, pad {}): ICRC mismatch",
            c.name,
            c.opcode,
            c.data_len,
            c.pad
        );
    }
}

/// The corpus has to keep covering the IPv4 checksum's second carry
/// fold, which uniform random generation reaches essentially never.
///
/// If the generator ever stops solving for that window the coverage
/// vanishes silently, and the only other thing covering it is one
/// hand-solved test in `frame.rs`. See this file's module docs.
#[test]
fn the_corpus_still_covers_the_carry_window() {
    use rhdl_roce::frame::ip_total_length;
    let mut forced = 0;
    for c in &corpus() {
        let src = c.cfg.src_ip.raw() as u32;
        let dst = c.cfg.dst_ip.raw() as u32;
        let acc: u32 = 0x4500
            + ip_total_length(c.plan).raw() as u32
            + c.cfg.ip_id.raw() as u32
            + (((c.cfg.ttl.raw() as u32) << 8) | 17)
            + (src >> 16)
            + (src & 0xFFFF)
            + (dst >> 16)
            + (dst & 0xFFFF);
        if (acc & 0xFFFF) + (acc >> 16) > 0xFFFF {
            forced += 1;
        }
    }
    assert!(
        forced >= 100,
        "only {forced} of 2000 corpus cases reach the second carry fold; \
         uniform random reaches it about 0.09 times in 2000, so this \
         means the generator stopped solving for it"
    );
}

/// The corpus is varied enough to be worth its size.
///
/// A corpus that had quietly collapsed to one opcode, or to aligned
/// payloads only, would still pass every test above while proving much
/// less than it appears to.
#[test]
fn the_corpus_is_actually_varied() {
    use std::collections::HashSet;
    let cases = corpus();
    let ops: HashSet<u8> = cases.iter().map(|c| c.opcode & 0x1F).collect();
    assert_eq!(
        ops.len(),
        6,
        "all six WRITE operations should appear: {ops:?}"
    );
    let transports: HashSet<u8> = cases.iter().map(|c| c.opcode & 0xE0).collect();
    assert_eq!(transports.len(), 2, "both RC and UC should appear");
    let pads: HashSet<usize> = cases.iter().map(|c| c.pad).collect();
    assert_eq!(pads.len(), 4, "all four pad counts should appear: {pads:?}");
    let lens: HashSet<usize> = cases.iter().map(|c| c.data_len).collect();
    assert!(
        lens.len() > 500,
        "only {} distinct payload lengths in the corpus",
        lens.len()
    );
    assert!(
        cases.iter().any(|c| c.data_len == 0),
        "a zero-length payload should appear"
    );
    assert!(
        cases.iter().any(|c| c.has_reth) && cases.iter().any(|c| !c.has_reth),
        "both RETH-carrying and RETH-free opcodes should appear"
    );
    assert!(
        cases.iter().any(|c| c.ackreq) && cases.iter().any(|c| !c.ackreq),
        "both ackreq settings should appear"
    );
}

/// The CRC-32 constants this file reaches into are the ones it thinks
/// they are. Cheap insurance against a refactor in `rhdl-fpga` silently
/// changing what `icrc_of` computes.
#[test]
fn the_crc_constants_are_what_this_file_assumes() {
    assert_eq!(INIT, 0xFFFF_FFFF);
    assert_eq!(XOR_OUT, 0xFFFF_FFFF);
    assert_eq!(PREFIX_REMAINDER, 0xDEBB_20E3);
    assert_eq!(COVERAGE_START, 14);
}
