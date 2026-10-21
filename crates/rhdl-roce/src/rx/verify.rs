//! A parsed frame whose Invariant CRC has actually been checked.
//!
//! [`crate::rx::parse::RxParser`] reports the ICRC it found in a frame
//! and compares nothing. This composes it with [`crate::icrc::Icrc`] to
//! recompute the checksum over the arriving bytes and compare, which is
//! what turns a verdict into something worth acting on.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
            +-+RxVerify+-------------+
            |                        |
  ?u8       |                        |  ?RxHeaders
 +--------->| data         headers +--------->
            |                        |
  bool      |                        |  ?u8
 +--------->| sof          payload +--------->
            |                        |
  bool      |                        |  bool
 +--------->| eof             done +--------->
            |                        |
            |                        |  RxVerdict
            |              verdict +--------->
            |                        |
            |                        |  b32
            |            computed +--------->
            +------------------------+
")]
//!
//! # Internals
//!
//! Two widgets and no new logic worth the name. The parser already
//! knows the frame geometry, so it decides which bytes the checksum
//! covers and emits them on `icrc_feed` alongside the frame index the
//! masking needs; this wires those straight to the engine.
#![doc = badascii_doc::badascii!("
   data/sof/eof
        |
        v
  +-----+--------+   icrc_feed      +----------+
  |   RxParser   +----------------->+          |
  |              |   frame_index    |   Icrc   |
  |              +----------------->+  engine  |
  |              |   frame_start    |          |
  |              +----------------->+          |
  +--+-----------+                  +----+-----+
     |                                   |
     | headers, payload, done,           | wire
     | verdict, icrc (received)          | (computed)
     v                                   v
    +---------------------------------------+
    |  compare at `done`; Accept becomes    |
    |  IcrcMismatch if they differ          |
    +---------------------------------------+
")]
//!
//! # Why the parser emits the feed rather than this widget deriving it
//!
//! Deriving coverage here would mean re-deriving the frame geometry —
//! where the ICRC field starts follows from the IPv4 total length,
//! which the parser has already captured. Two places computing the same
//! offsets is two places to get them out of step, and the failure would
//! be a checksum over the wrong byte range: silent, and indistinguishable
//! from a corrupted frame.
//!
//! It also removes a timing question. `icrc_feed`, `frame_index` and
//! `frame_start` are produced by one source in one cycle, so wiring them
//! across needs no reasoning about their relative delay.
//!
//! # The masking works unchanged in this direction
//!
//! Worth stating because it is the part that looks like it should need
//! work and does not. The ICRC blanks the fields a router may rewrite,
//! and [`crate::icrc::icrc_mask`] selects them **by frame offset** —
//! so an engine fed a received frame's bytes with their real offsets
//! masks exactly the same seven bytes it masked when generating. The
//! engine is byte-direction-agnostic; only the driving differs.
//!
//! # What a mismatch means, and what it does not
//!
//! A mismatch means the frame's covered bytes are not what the sender
//! computed over. It does **not** localise the damage: the ICRC is a
//! checksum, so a single verdict covers the headers and the payload
//! together, and there is no way to learn which part changed. That is
//! the IBTA's design and not a limitation of this widget.
//!
//! Note also what the ICRC does not protect: the Ethernet and IP
//! addresses before offset 14, and the seven masked bytes. A frame can
//! pass this check and still have had its TTL, DSCP or ECN bits
//! rewritten — by design, since that is what routers are permitted to
//! do.
//!
//! # Example
//!
//!```
#![doc = include_str!("../../examples/rx_verify.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/rx_verify.md")]

use crate::icrc::Icrc;
use crate::rx::parse::{RxHeaders, RxParser, RxPhase, RxVerdict};
use rhdl::prelude::*;

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
/// A frame parser with ICRC verification. See the [module docs](self).
pub struct RxVerify {
    parser: RxParser,
    engine: Icrc,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [`RxVerify`] — the same byte-and-framing interface as
/// [`crate::rx::parse::RxParser`], passed straight through.
pub struct In {
    /// The next frame byte, or `None` when the link is idle.
    pub data: Option<Bits<8>>,
    /// This byte is the first of a frame.
    pub start_of_frame: bool,
    /// This byte is the last of a frame.
    pub end_of_frame: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [`RxVerify`].
pub struct Out {
    /// The recovered headers, as [`crate::rx::parse::RxParser`]
    /// presents them: from the first payload byte's cycle to the end of
    /// the frame.
    ///
    /// Presented **before** the checksum is known, necessarily — the
    /// last covered byte has not arrived yet. A consumer places
    /// speculatively and commits on `done && verdict == Accept`.
    pub headers: Option<RxHeaders>,
    /// A payload byte, in the cycle it arrives. Also speculative.
    pub payload: Option<Bits<8>>,
    /// The frame ended this cycle.
    pub done: bool,
    /// The verdict, final when `done`, and now including the checksum:
    /// a frame whose headers were acceptable but whose ICRC did not
    /// match reports [`RxVerdict::IcrcMismatch`].
    pub verdict: RxVerdict,
    /// The ICRC as it appeared in the frame.
    pub received: Bits<32>,
    /// The ICRC recomputed from the frame's bytes. Equal to `received`
    /// exactly when the frame is intact.
    pub computed: Bits<32>,
    /// A frame is in progress.
    pub busy: bool,
    /// The parser's phase, forwarded for debug and waveform reading.
    pub phase: RxPhase,
}

impl SynchronousIO for RxVerify {
    type I = In;
    type O = Out;
    type Kernel = rx_verify;
}

#[kernel]
/// Kernel for [`RxVerify`].
pub fn rx_verify(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut o = Out::dont_care();

    // The byte interface goes straight through.
    d.parser.data = i.data;
    d.parser.start_of_frame = i.start_of_frame;
    d.parser.end_of_frame = i.end_of_frame;

    // And the parser drives the checksum engine. `start` re-seeds and
    // takes precedence over `enable` in the engine, which is safe here
    // because `frame_start` is offset zero and coverage begins at
    // offset 14 -- the two never coincide.
    let p = q.parser;
    d.engine.frame_index = p.frame_index;
    d.engine.start = p.frame_start;
    match p.icrc_feed {
        Some(b) => {
            d.engine.byte = b;
            d.engine.enable = true;
        }
        None => {
            d.engine.byte = bits(0);
            d.engine.enable = false;
        }
    }

    o.headers = p.headers;
    o.payload = p.payload;
    o.done = p.done;
    o.busy = p.busy;
    o.phase = p.phase;
    o.received = p.icrc;
    o.computed = q.engine.wire;

    // A frame the parser accepted is only truly ours if the checksum
    // agrees. A frame it rejected keeps its own verdict: the ICRC of a
    // frame that is not even IPv4 is not the useful thing to report.
    o.verdict = p.verdict;
    if p.done && p.verdict == RxVerdict::Accept && p.icrc != q.engine.wire {
        o.verdict = RxVerdict::IcrcMismatch;
    }

    if cr.reset.any() {
        d.parser.data = None;
        d.parser.start_of_frame = false;
        d.parser.end_of_frame = false;
        d.engine.byte = bits(0);
        d.engine.frame_index = bits(0);
        d.engine.enable = false;
        d.engine.start = true;
        o.headers = None;
        o.payload = None;
        o.done = false;
        o.verdict = RxVerdict::Accept;
        o.received = bits(0);
        o.computed = bits(0);
        o.busy = false;
        o.phase = RxPhase::Idle;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;

    /// The `uc_write_only_16` frame and its Scapy-computed ICRC.
    fn vector(name: &str) -> Vec<u8> {
        let corpus = include_str!("../../tests/vectors/write_vectors.txt");
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

    struct Run {
        verdict: RxVerdict,
        received: u32,
        computed: u32,
        dones: usize,
        payload: Vec<u8>,
        saw_headers: bool,
    }

    fn run(frame: &[u8]) -> Run {
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
        stream.push(idle);
        stream.push(idle);

        let uut = RxVerify::default();
        let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
        let mut r = Run {
            verdict: RxVerdict::Accept,
            received: 0,
            computed: 0,
            dones: 0,
            payload: Vec::new(),
            saw_headers: false,
        };
        for s in uut.run(input).synchronous_sample() {
            let o = s.output;
            if let Some(b) = o.payload {
                r.payload.push(b.raw() as u8);
            }
            if o.headers.is_some() {
                r.saw_headers = true;
            }
            if o.done {
                r.verdict = o.verdict;
                r.received = o.received.raw() as u32;
                r.computed = o.computed.raw() as u32;
                r.dones += 1;
            }
        }
        r
    }

    #[test]
    fn an_intact_frame_verifies() {
        let r = run(&vector("uc_write_only_16"));
        assert_eq!(r.verdict, RxVerdict::Accept);
        assert_eq!(r.computed, r.received, "the two checksums agree");
        assert_eq!(r.dones, 1);
        assert_eq!(r.payload.len(), 16);
    }

    /// **The headers and payload are still delivered on a bad frame.**
    /// Not a defect — it is the speculative-placement contract, and a
    /// consumer that ignored `done` would be the bug. Pinned so the
    /// behaviour cannot drift silently.
    #[test]
    fn a_corrupt_frame_is_rejected_but_its_bytes_were_still_presented() {
        let mut f = vector("uc_write_only_16");
        f[70] ^= 0xFF;
        let r = run(&f);
        assert_eq!(r.verdict, RxVerdict::IcrcMismatch);
        assert_ne!(r.computed, r.received);
        assert!(
            r.saw_headers,
            "headers are presented before the checksum can be known"
        );
        assert_eq!(
            r.payload.len(),
            16,
            "and so are the payload bytes -- which is why a consumer \
             must commit on `done`, not on arrival"
        );
    }

    /// A frame rejected before the checksum matters keeps the parser's
    /// own verdict. Reporting `IcrcMismatch` for a frame that is not
    /// even IPv4 would name the wrong problem.
    #[test]
    fn a_frame_rejected_by_the_parser_keeps_its_own_verdict() {
        let good = vector("uc_write_only_16");
        for (at, to, want) in [
            (13usize, 0x06u8, RxVerdict::NotIpv4),
            (23, 0x06, RxVerdict::NotUdp),
            (37, 0xB6, RxVerdict::NotRocePort),
            (42, 0x00, RxVerdict::UnknownOpcode),
        ] {
            let mut f = good.clone();
            f[at] = to;
            assert_eq!(
                run(&f).verdict,
                want,
                "offset {at} set to {to:#04x} should keep the parser's verdict"
            );
        }
    }

    /// Two frames in a row: the engine must re-seed, or the second
    /// frame's checksum would continue the first's.
    #[test]
    fn the_checksum_reseeds_between_frames() {
        let f = vector("uc_write_only_16");
        let idle = In {
            data: None,
            start_of_frame: false,
            end_of_frame: false,
        };
        let mut stream = vec![idle];
        for _ in 0..3 {
            let last = f.len() - 1;
            for (k, b) in f.iter().enumerate() {
                stream.push(In {
                    data: Some(bits(*b as u128)),
                    start_of_frame: k == 0,
                    end_of_frame: k == last,
                });
            }
            stream.push(idle);
        }
        let uut = RxVerify::default();
        let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
        let mut verdicts = Vec::new();
        for s in uut.run(input).synchronous_sample() {
            if s.output.done {
                verdicts.push(s.output.verdict);
            }
        }
        assert_eq!(
            verdicts,
            vec![RxVerdict::Accept; 3],
            "all three repetitions of the same frame must verify"
        );
    }

    fn test_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In)>> {
        let f = vector("uc_write_only_0");
        let idle = In {
            data: None,
            start_of_frame: false,
            end_of_frame: false,
        };
        let last = f.len() - 1;
        let mut stream = vec![idle];
        for (k, b) in f.iter().enumerate() {
            stream.push(In {
                data: Some(bits(*b as u128)),
                start_of_frame: k == 0,
                end_of_frame: k == last,
            });
        }
        stream.push(idle);
        stream.into_iter().with_reset(1).clock_pos_edge(100)
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = RxVerify::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        assert!(hdl.contains("module top"));
        Ok(())
    }

    #[test]
    fn test_widget_hdl_works() -> miette::Result<()> {
        let uut = RxVerify::default();
        let tb = uut
            .run(test_stream())
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_widget_trace() -> miette::Result<()> {
        let uut = RxVerify::default();
        let vcd = uut.run(test_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("rx_verify");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["6bf46e78a7f5216dba6093d12493e3e9f89f2cc92b7d9dd99a1446ee57452d17"];
        let digest = vcd.dump_to_file(root.join("rx_verify.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
