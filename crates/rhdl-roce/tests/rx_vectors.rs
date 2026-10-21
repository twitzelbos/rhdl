//! The receive path, validated against the same Scapy corpus as the
//! transmit path — but driven backwards.
//!
//! `tests/vectors.rs` and `tests/fuzz.rs` ask "do we produce the bytes
//! Scapy produces?". This file asks the converse: "given the bytes
//! Scapy produced, do we recover the fields Scapy was given?". The two
//! together pin the wire format from both sides, which is what makes
//! the Tier-10 loopback test of `roce-v2-architecture.md` meaningful
//! rather than circular — a shared misunderstanding would have to
//! survive the comparison in *both* directions.
//!
//! # Why this is the strongest test available for the parser
//!
//! The input bytes are not this crate's. They came out of
//! `scapy.contrib.roce`, an independent implementation, via
//! `tools/gen_vectors.py`. The expected output is the field values that
//! generator was *given*. So nothing in the loop is this crate's own
//! idea of the format, at either end.
//!
//! The fuzz corpus does not store payload bytes — they are a formula of
//! a recorded seed, documented in the corpus header — and does not
//! store the ICRC inside the frame. Both are reconstructed here, which
//! is how a complete frame is rebuilt from a header-only record.

use rhdl::prelude::*;
use rhdl_roce::frame::{FrameConfig, bth_from_plan};
use rhdl_roce::headers::{Reth, Transport, WriteOpcode};
use rhdl_roce::qp::context::{Pmtu, QpContext};
use rhdl_roce::qp::segmenter::PacketPlan;
use rhdl_roce::rx::parse::{In, RxHeaders, RxParser, RxVerdict};
use rhdl_roce::rx::verify::{self, RxVerify};

const CORPUS: &str = include_str!("vectors/write_vectors.txt");

// ---------------------------------------------------------------- harness

/// Everything a run of the parser produced, aggregated.
#[derive(Debug, Default)]
struct Parsed {
    /// The last headers presented. `None` if they never were.
    headers: Option<RxHeaders>,
    /// The cycle count on which `headers` first appeared, relative to
    /// the first byte.
    headers_at: Option<usize>,
    /// Every byte that came out of `payload`, in order.
    payload: Vec<u8>,
    /// The verdict reported with `done`.
    verdict: RxVerdict,
    /// The ICRC reported with `done`.
    icrc: u32,
    /// How many times `done` was asserted. Must be exactly one.
    dones: usize,
}

/// Drive a complete frame through the parser, one byte per cycle.
fn parse(frame: &[u8]) -> Parsed {
    assert!(!frame.is_empty(), "a frame needs at least one byte");
    let idle = In {
        data: None,
        start_of_frame: false,
        end_of_frame: false,
    };
    let last = frame.len() - 1;
    let mut stream = vec![idle];
    for (k, b) in frame.iter().enumerate() {
        stream.push(In {
            data: Some(bits(*b as u128)),
            start_of_frame: k == 0,
            end_of_frame: k == last,
        });
    }
    // Two idle cycles after, so a spurious `done` or a late payload byte
    // still lands inside the window being observed.
    stream.push(idle);
    stream.push(idle);

    let uut = RxParser::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let mut r = Parsed::default();
    for (cycle, s) in uut.run(input).synchronous_sample().enumerate() {
        let o = s.output;
        if let Some(b) = o.payload {
            r.payload.push(b.raw() as u8);
        }
        if o.headers.is_some() {
            if r.headers_at.is_none() {
                r.headers_at = Some(cycle);
            }
            r.headers = o.headers;
        }
        if o.done {
            r.verdict = o.verdict;
            r.icrc = o.icrc.raw() as u32;
            r.dones += 1;
        }
    }
    r
}

fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "odd-length hex {s:?}");
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex"))
        .collect()
}

fn num(s: &str) -> u128 {
    match s.strip_prefix("0x") {
        Some(h) => u128::from_str_radix(h, 16).expect("hex number"),
        None => s.parse().expect("decimal number"),
    }
}

// ------------------------------------------------- the curated vectors

/// One curated `key = value` record.
struct Vector {
    name: String,
    fields: std::collections::BTreeMap<String, String>,
}

impl Vector {
    fn get(&self, k: &str) -> &str {
        self.fields
            .get(k)
            .unwrap_or_else(|| panic!("vector {} has no field {k}", self.name))
    }
    fn n(&self, k: &str) -> u128 {
        num(self.get(k))
    }
}

fn vectors() -> Vec<Vector> {
    let mut out = Vec::new();
    for record in CORPUS.split("\n\n") {
        let mut fields = std::collections::BTreeMap::new();
        for line in record.lines() {
            if line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                fields.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        if let Some(name) = fields.get("name").cloned() {
            out.push(Vector { name, fields });
        }
    }
    assert_eq!(out.len(), 8, "the corpus should hold 8 curated vectors");
    out
}

/// **Every curated vector, parsed back.** The frame field is the whole
/// Ethernet frame including the ICRC, so this needs no reconstruction:
/// the bytes go straight in.
#[test]
fn every_curated_vector_parses_back_to_the_fields_it_was_built_from() {
    for v in vectors() {
        let frame = unhex(v.get("frame"));
        let r = parse(&frame);

        assert_eq!(
            r.verdict,
            RxVerdict::Accept,
            "{}: a frame Scapy generated must be accepted",
            v.name
        );
        assert_eq!(r.dones, 1, "{}: exactly one done", v.name);

        let h = r
            .headers
            .unwrap_or_else(|| panic!("{}: headers were never presented", v.name));

        // --- the network layers
        assert_eq!(h.src_ip.raw(), v.n("src_ip"), "{}: source address", v.name);
        assert_eq!(h.dst_ip.raw(), v.n("dst_ip"), "{}: dest address", v.name);
        assert_eq!(h.src_port.raw(), v.n("sport"), "{}: source port", v.name);

        // --- the transport header
        assert_eq!(h.bth.opcode.raw(), v.n("opcode"), "{}: opcode", v.name);
        assert_eq!(h.bth.dqpn.raw(), v.n("dqpn"), "{}: dest QP", v.name);
        assert_eq!(h.bth.psn.raw(), v.n("psn"), "{}: PSN", v.name);
        assert_eq!(
            h.bth.padcount.raw(),
            v.n("padcount"),
            "{}: pad count",
            v.name
        );
        assert_eq!(h.bth.ackreq, v.n("ackreq") == 1, "{}: ackreq", v.name);
        assert_eq!(h.bth.pkey.raw(), 0xFFFF, "{}: partition key", v.name);

        // --- the RDMA header, when there is one
        assert_eq!(h.has_reth, v.n("has_reth") == 1, "{}: RETH present", v.name);
        if h.has_reth {
            assert_eq!(h.reth.va.raw(), v.n("va"), "{}: virtual address", v.name);
            assert_eq!(h.reth.rkey.raw(), v.n("rkey"), "{}: rkey", v.name);
            assert_eq!(
                h.reth.dma_len.raw(),
                v.n("dma_len"),
                "{}: DMA length",
                v.name
            );
        }

        // --- the payload, byte for byte.
        //
        // The corpus's `payload` field is the payload *on the wire*, so
        // it already carries the pad bytes; `payload_len` counts them
        // too. That is the same quantity the parser reports, which is
        // the right one for a receiver — the pad bytes occupy wire
        // cycles and have to be consumed. Stripping them is the
        // business of whatever acts on `dma_len`, not of the parser.
        let expected = unhex(v.get("payload"));
        let pad = v.n("padcount") as usize;
        assert_eq!(
            expected.len(),
            v.n("payload_len") as usize,
            "{}: the corpus's own payload field and payload_len disagree",
            v.name
        );
        assert_eq!(r.payload, expected, "{}: payload bytes", v.name);
        assert_eq!(
            h.payload_len.raw() as usize,
            expected.len(),
            "{}: reported payload length",
            v.name
        );
        // And the pad bytes really are the zero tail, which is what
        // makes it safe for a consumer to drop them by count.
        assert!(
            expected[expected.len() - pad..].iter().all(|b| *b == 0),
            "{}: the {pad} pad bytes must be zero, got {:?}",
            v.name,
            &expected[expected.len() - pad..]
        );

        // --- and the ICRC, as received
        let icrc = unhex(v.get("icrc"));
        let want = u32::from_be_bytes([icrc[0], icrc[1], icrc[2], icrc[3]]);
        assert_eq!(
            r.icrc, want,
            "{}: received ICRC — expected {want:#010x}, got {:#010x}",
            v.name, r.icrc
        );
    }
}

// ---------------------------------------------------- the fuzz corpus

/// One fuzz case, rebuilt into a complete frame.
struct Fuzz {
    name: String,
    frame: Vec<u8>,
    cfg: FrameConfig,
    ctx: QpContext,
    plan: PacketPlan,
    ackreq: bool,
    payload: Vec<u8>,
    icrc: u32,
}

const FIELDS: usize = 22;

fn write_opcode(name: &str) -> WriteOpcode {
    match name {
        "First" => WriteOpcode::First,
        "Middle" => WriteOpcode::Middle,
        "Last" => WriteOpcode::Last,
        "LastWithImmediate" => WriteOpcode::LastWithImmediate,
        "Only" => WriteOpcode::Only,
        "OnlyWithImmediate" => WriteOpcode::OnlyWithImmediate,
        other => panic!("unknown opcode name {other:?}"),
    }
}

/// The payload formula the generator used, documented in the corpus
/// header. Payload bytes are not stored, so they are recomputed.
fn payload_of(seed: u32, data_len: usize, pad: usize) -> Vec<u8> {
    let mut v: Vec<u8> = (0..data_len)
        .map(|i| {
            let i = i as u32;
            (seed
                .wrapping_add(i.wrapping_mul(31))
                .wrapping_add((i >> 3).wrapping_mul(7))
                & 0xFF) as u8
        })
        .collect();
    v.extend(std::iter::repeat_n(0u8, pad));
    v
}

fn fuzz_corpus() -> Vec<Fuzz> {
    let mut out = Vec::new();
    for line in CORPUS.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != FIELDS || !f[0].starts_with("fuzz") {
            continue;
        }
        let opcode = num(f[2]) as u8;
        let has_reth = num(f[13]) == 1;
        let data_len = num(f[17]) as usize;
        let pad = num(f[18]) as usize;
        let payload = payload_of(num(f[19]) as u32, data_len, pad);
        let icrc = unhex(f[21]);

        // The corpus records the header block and the ICRC separately,
        // because that is what the generator and the ICRC engine each
        // produce. A complete frame is the two with the payload between.
        let mut frame = unhex(f[20]);
        frame.extend_from_slice(&payload);
        frame.extend_from_slice(&icrc);

        out.push(Fuzz {
            name: f[0].to_string(),
            frame,
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
            payload,
            icrc: u32::from_be_bytes([icrc[0], icrc[1], icrc[2], icrc[3]]),
        });
    }
    assert_eq!(
        out.len(),
        2000,
        "the committed corpus should hold 2000 cases"
    );
    out
}

/// **The headline test: 2000 Scapy-generated frames, parsed back.**
///
/// For each one the recovered BTH is compared against
/// [`bth_from_plan`] applied to the fields the generator was given —
/// so the comparison is against the whole header record at once, not a
/// field list that could quietly omit one.
#[test]
fn every_fuzz_frame_parses_back_to_the_fields_it_was_built_from() {
    let corpus = fuzz_corpus();
    let mut with_reth = 0;
    let mut without_reth = 0;
    let mut padded = 0;
    let mut empty_payload = 0;

    for c in &corpus {
        let r = parse(&c.frame);
        assert_eq!(
            r.verdict,
            RxVerdict::Accept,
            "{}: rejected a frame Scapy generated",
            c.name
        );
        assert_eq!(r.dones, 1, "{}: exactly one done", c.name);
        let h = r
            .headers
            .unwrap_or_else(|| panic!("{}: headers never presented", c.name));

        // The whole BTH at once.
        assert_eq!(
            h.bth,
            bth_from_plan(c.ctx, c.plan, c.ackreq),
            "{}: recovered BTH differs from the one that was sent",
            c.name
        );
        assert_eq!(h.kind.transport, c.ctx.transport, "{}: transport", c.name);
        assert_eq!(h.kind.op, c.plan.opcode, "{}: operation", c.name);

        assert_eq!(h.has_reth, c.plan.carries_reth, "{}: RETH present", c.name);
        if h.has_reth {
            assert_eq!(h.reth, c.plan.reth, "{}: recovered RETH", c.name);
            with_reth += 1;
        } else {
            without_reth += 1;
        }

        assert_eq!(h.src_ip, c.cfg.src_ip, "{}: source address", c.name);
        assert_eq!(h.dst_ip, c.cfg.dst_ip, "{}: dest address", c.name);
        assert_eq!(h.src_port, c.cfg.src_port, "{}: source port", c.name);

        assert_eq!(r.payload, c.payload, "{}: payload bytes", c.name);
        assert_eq!(
            h.payload_len.raw() as usize,
            c.payload.len(),
            "{}: reported payload length",
            c.name
        );
        assert_eq!(r.icrc, c.icrc, "{}: received ICRC", c.name);

        if c.plan.padcount.raw() > 0 {
            padded += 1;
        }
        if c.payload.is_empty() {
            empty_payload += 1;
        }
    }

    // The corpus has to actually span the cases, or the loop above is
    // 2000 repetitions of one shape.
    assert!(with_reth > 200, "too few RETH-carrying frames: {with_reth}");
    assert!(
        without_reth > 200,
        "too few frames without a RETH: {without_reth}"
    );
    assert!(padded > 200, "too few padded frames: {padded}");
    assert!(
        empty_payload > 0,
        "no zero-payload frame in the corpus, so the Header -> Trailer \
         transition is untested here"
    );
}

/// **Headers arrive with the first payload byte.** Asserted separately
/// because it is a timing contract the placement path depends on, and
/// the aggregate test above would pass if they arrived late.
#[test]
fn headers_are_presented_exactly_when_the_first_payload_byte_arrives() {
    for c in fuzz_corpus().iter().filter(|c| !c.payload.is_empty()) {
        let idle = In {
            data: None,
            start_of_frame: false,
            end_of_frame: false,
        };
        let last = c.frame.len() - 1;
        let mut stream = vec![idle];
        for (k, b) in c.frame.iter().enumerate() {
            stream.push(In {
                data: Some(bits(*b as u128)),
                start_of_frame: k == 0,
                end_of_frame: k == last,
            });
        }
        stream.push(idle);

        let uut = RxParser::default();
        let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
        let mut first_headers = None;
        let mut first_payload = None;
        for (cycle, s) in uut.run(input).synchronous_sample().enumerate() {
            if s.output.headers.is_some() && first_headers.is_none() {
                first_headers = Some(cycle);
            }
            if s.output.payload.is_some() && first_payload.is_none() {
                first_payload = Some(cycle);
            }
        }
        assert_eq!(
            first_headers, first_payload,
            "{}: headers and the first payload byte must appear in the \
             same cycle — the RETH address has to be known before the \
             first byte it addresses",
            c.name
        );
    }
}

// ------------------------------------------------ ICRC verification

/// What a run of [`RxVerify`] produced.
struct Verified {
    verdict: RxVerdict,
    received: u32,
    computed: u32,
    dones: usize,
}

fn verify_frame(frame: &[u8]) -> Verified {
    let idle = verify::In {
        data: None,
        start_of_frame: false,
        end_of_frame: false,
    };
    let last = frame.len() - 1;
    let mut stream = vec![idle];
    for (k, b) in frame.iter().enumerate() {
        stream.push(verify::In {
            data: Some(bits(*b as u128)),
            start_of_frame: k == 0,
            end_of_frame: k == last,
        });
    }
    stream.push(idle);
    stream.push(idle);

    let uut = RxVerify::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let mut r = Verified {
        verdict: RxVerdict::Accept,
        received: 0,
        computed: 0,
        dones: 0,
    };
    for s in uut.run(input).synchronous_sample() {
        if s.output.done {
            r.verdict = s.output.verdict;
            r.received = s.output.received.raw() as u32;
            r.computed = s.output.computed.raw() as u32;
            r.dones += 1;
        }
    }
    r
}

/// **Every curated frame's ICRC recomputes to the value Scapy put in
/// it.** This is the test that says the receive-side checksum is real:
/// the expected value was produced by an independent implementation,
/// not by this crate.
#[test]
fn every_curated_frame_passes_its_own_icrc() {
    for v in vectors() {
        let frame = unhex(v.get("frame"));
        let r = verify_frame(&frame);
        assert_eq!(r.dones, 1, "{}: exactly one done", v.name);
        assert_eq!(
            r.computed, r.received,
            "{}: recomputed ICRC {:#010x} does not match the {:#010x} in \
             the frame",
            v.name, r.computed, r.received
        );
        assert_eq!(
            r.received,
            u32::from_be_bytes(unhex(v.get("icrc"))[..4].try_into().unwrap()),
            "{}: and that value is the corpus's",
            v.name
        );
        assert_eq!(r.verdict, RxVerdict::Accept, "{}: accepted", v.name);
    }
}

/// **All 2000 fuzz frames.** Rebuilt complete, including the ICRC the
/// generator recorded, and every one must verify.
#[test]
fn every_fuzz_frame_passes_its_own_icrc() {
    for c in fuzz_corpus() {
        let r = verify_frame(&c.frame);
        assert_eq!(
            r.verdict,
            RxVerdict::Accept,
            "{}: verdict (received {:#010x}, computed {:#010x})",
            c.name,
            r.received,
            r.computed
        );
        assert_eq!(r.computed, c.icrc, "{}: recomputed ICRC", c.name);
        assert_eq!(r.received, c.icrc, "{}: received ICRC", c.name);
    }
}

/// **The negative control, and the reason this widget exists.** Flip
/// any single byte the ICRC covers and the frame must be rejected.
///
/// The sweep is over every covered offset, not a sample, because a
/// checksum that misses one byte of its range is exactly the defect
/// that a handful of spot checks would pass.
#[test]
fn flipping_any_covered_byte_is_caught() {
    let v = vectors()
        .into_iter()
        .find(|v| v.name == "uc_write_only_16")
        .expect("the vector");
    let good = unhex(v.get("frame"));
    assert_eq!(
        verify_frame(&good).verdict,
        RxVerdict::Accept,
        "the control must pass"
    );

    // Covered: offset 14 up to but excluding the 4-byte ICRC field.
    let icrc_at = good.len() - 4;
    let masked = [15usize, 22, 24, 25, 40, 41, 46];
    let mut checked = 0;
    for at in 14..icrc_at {
        let mut f = good.clone();
        f[at] ^= 0xFF;
        let r = verify_frame(&f);
        if masked.contains(&at) {
            // These are blanked to all-ones before the checksum, so
            // changing them cannot change it -- that is the point of
            // the masking, and it is what lets a router rewrite them.
            assert_eq!(
                r.computed, r.received,
                "offset {at} is masked, so flipping it must NOT change \
                 the checksum"
            );
            continue;
        }
        // A changed header field may be rejected before the checksum is
        // even reached -- flipping the protocol byte makes it not-UDP.
        // Either outcome is a rejection; what must not happen is
        // Accept.
        assert_ne!(
            r.verdict,
            RxVerdict::Accept,
            "flipping covered byte {at} must be caught; got received \
             {:#010x} computed {:#010x}",
            r.received,
            r.computed
        );
        checked += 1;
    }
    assert!(
        checked > 60,
        "the sweep should cover the whole payload and header: {checked}"
    );
}

/// And a corrupted ICRC *field* is caught too -- the other side of the
/// comparison.
#[test]
fn a_corrupted_icrc_field_is_rejected() {
    let v = vectors()
        .into_iter()
        .find(|v| v.name == "uc_write_only_16")
        .expect("the vector");
    let good = unhex(v.get("frame"));
    let n = good.len();
    for at in n - 4..n {
        let mut f = good.clone();
        f[at] ^= 0x01;
        let r = verify_frame(&f);
        assert_eq!(
            r.verdict,
            RxVerdict::IcrcMismatch,
            "a one-bit change to ICRC byte {at} must be reported as a \
             mismatch"
        );
        assert_ne!(r.received, r.computed);
    }
}

/// The masked bytes, stated as a positive claim rather than only as an
/// exception in the sweep above: a router rewriting TTL, DSCP/ECN, the
/// IP or UDP checksum, or the BTH's congestion bits must not invalidate
/// the frame.
#[test]
fn a_router_rewriting_the_masked_fields_does_not_break_the_frame() {
    let v = vectors()
        .into_iter()
        .find(|v| v.name == "uc_write_only_16")
        .expect("the vector");
    let good = unhex(v.get("frame"));
    for (at, what) in [
        (15usize, "DSCP/ECN"),
        (22, "TTL"),
        (24, "IP checksum high"),
        (25, "IP checksum low"),
        (40, "UDP checksum high"),
        (41, "UDP checksum low"),
        (46, "BTH FECN/BECN/reserved"),
    ] {
        let mut f = good.clone();
        f[at] = 0x5A;
        let r = verify_frame(&f);
        assert_eq!(
            r.computed, r.received,
            "rewriting {what} at offset {at} must not change the ICRC"
        );
    }
}
