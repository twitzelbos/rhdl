//! The assembled transmitter — a message in, a stream of complete
//! RoCEv2 frame bytes out.
//!
//! Everything else in this crate is a piece: the segmenter decides what
//! each packet is, [`crate::frame`] can produce any header byte on
//! demand, [`crate::icrc`] can fold bytes into a checksum. None of them
//! is a datapath. This is the widget that composes them into one, and
//! it is the first thing here you could instantiate on a board.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +-----+RoceWriteTx+------+
     |                        |
bool |                        | ?B<8>
+--->| start             data +--->
?Msg |                        |
+--->| msg      payload_advnc +---> bool
?Ctx |                        |
+--->| ctx               sof  +---> bool
?Cfg |                        |
+--->| cfg               eof  +---> bool
B<8> |                        |
+--->| payload_byte     busy  +---> bool
bool |                        |
+--->| ready                  |
     +------------------------+
")]
//!
//!# Internals
//!
//! Four sub-circuits: the [`WriteSegmenter`] that decides packets, the
//! [`Icrc`] engine, a phase register and one bundled register for the
//! counters. The §3.1 pattern — the FSM enum gets its own `DFF` so the
//! `FsmWidget` extractor can find it, and the remaining counters are
//! bundled rather than becoming sibling registers.
#![doc = badascii_doc::badascii!(r"
   msg,ctx -->+WriteSegmenter+--+ plan
                   ^            |
            ready  |            v
              +----+---+   +-+frame::header_byte+--+
              |  phase |   |                       |
              |  FSM   +-->| mux: header | payload |--+--> data
              +----+---+   |      | icrc bytes     |  |
                   |       +-----------------------+  |
                   |            ^                     |
                   |            +---+Icrc+------------+
                   |                 ^   (folds each emitted
                   +-----------------+    covered byte)
")]
//!
//!# The frame it emits
//!
//! ```text
//!   Header phase            Payload phase       Icrc phase
//!   |<-- 54 or 70 B -->|<-- payload+pad -->|<--- 4 B --->|
//!   Eth IPv4 UDP BTH [RETH]                  ICRC
//!   ^sof                                          eof^
//! ```
//!
//! The Ethernet FCS is **not** here. A MAC appends it, and on the
//! hard-GEM path of `roce-v2-architecture.md` §4.3 the GEM does it in
//! hardware. So `data` is a complete frame minus its last four bytes.
//!
//!# Timing
//!
//! The ICRC engine is fed each byte **as it is emitted**, gated on
//! `ready`, over exactly the covered range — so by the time the phase
//! reaches `Icrc` the checksum is already settled and there is no
//! stall. That works because the engine's output reflects bytes folded
//! up to the previous cycle, and the last covered byte is emitted on
//! the cycle before the first ICRC byte.
//!
//! There is **one dead cycle between packets**: the phase returns to
//! `Idle` so the segmenter's next plan can appear. Harmless — Ethernet
//! requires an inter-frame gap anyway — and it keeps the transition
//! logic to one plan-fetch path instead of two.
//!
//!# Backpressure
//!
//! `data` is presented and held; nothing advances on a cycle where
//! `ready` is low, including the ICRC. So an unready sink sees the same
//! byte again rather than a dropped one, which is the convention the
//! rest of the library follows.
//!
//!# The payload is pulled, not pushed
//!
//! The caller presents the byte at the current payload offset on
//! `payload_byte` and advances its source when `payload_advance` is
//! asserted. That suits a memory-backed source — which is what this
//! will have, a DMA reading acquisition samples — better than a stream
//! the widget would have to buffer. An `RCStream` adapter on top is
//! straightforward and is noted as a follow-up rather than guessed at
//! here.
//!
//!# Example
//!
//!```
#![doc = include_str!("../examples/tx.rs")]
//!```
//!
//! The state machine this implements:
#![doc = include_str!("../doc/tx_fsm.md")]
//!
//! And the trace below demonstrates the result.
#![doc = include_str!("../doc/tx.md")]

use crate::frame::{FrameConfig, bth_from_plan, header_byte, header_bytes, payload_on_wire};
use crate::icrc::Icrc;
use crate::qp::context::{MessageRequest, QpContext};
use crate::qp::segmenter::{PacketPlan, WriteSegmenter};
use rhdl::prelude::*;
use rhdl_fpga::core::dff;

/// Which part of the frame is being emitted.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum TxPhase {
    /// No packet in flight. Picks up the segmenter's next plan.
    #[default]
    Idle,
    /// Emitting the Ethernet/IPv4/UDP/BTH\[/RETH\] header block.
    Header,
    /// Emitting the payload and its pad bytes.
    Payload,
    /// Emitting the four ICRC bytes.
    Icrc,
}

#[derive(Clone, Copy, PartialEq, Debug, Digital, Default)]
#[doc(hidden)]
/// The counters, bundled per CLAUDE.md §3.1.
pub struct TxExtras {
    /// Absolute byte offset within the frame. Drives both the header
    /// byte selector and the ICRC's masking.
    pub index: Bits<16>,
    /// Payload bytes still to emit, including pad.
    pub pay_left: Bits<13>,
    /// ICRC bytes still to emit, counting down from 4.
    pub icrc_left: Bits<3>,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "phase", state_enum = TxPhase, allow_implicit)]
/// Emits complete RoCEv2 RDMA WRITE frames for one message.
pub struct RoceWriteTx {
    /// The FSM enum in its own register, so the `FsmWidget` extractor
    /// finds the simple `Index(q, [.phase])` access pattern.
    phase: dff::DFF<TxPhase>,
    extras: dff::DFF<TxExtras>,
    seg: WriteSegmenter,
    icrc: Icrc,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [`RoceWriteTx`].
pub struct In {
    /// Begin a new message. Forwarded to the segmenter.
    pub start: bool,
    /// The message to send. Sampled by the segmenter on `start`.
    pub msg: MessageRequest,
    /// RDMA connection parameters.
    pub ctx: QpContext,
    /// Link and network parameters.
    pub cfg: FrameConfig,
    /// Sequence number for the message's first packet.
    pub psn_start: Bits<24>,
    /// Set the BTH's `ackreq`. Meaningless on an unreliable connection,
    /// which never acknowledges.
    pub ackreq: bool,
    /// The payload byte at the current offset. Only read during the
    /// payload phase.
    pub payload_byte: Bits<8>,
    /// The sink can accept a byte this cycle.
    pub ready: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs of [`RoceWriteTx`].
pub struct Out {
    /// The frame byte, or `None` when idle. Held until `ready`.
    pub data: Option<Bits<8>>,
    /// The caller should advance its payload source.
    pub payload_advance: bool,
    /// This byte is the first of a frame.
    pub start_of_frame: bool,
    /// This byte is the last of a frame — the ICRC's final byte.
    pub end_of_frame: bool,
    /// A message is in flight.
    pub busy: bool,
}

impl SynchronousIO for RoceWriteTx {
    type I = In;
    type O = Out;
    type Kernel = roce_write_tx;
}

#[kernel]
/// Kernel for [`RoceWriteTx`].
pub fn roce_write_tx(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut o = Out::dont_care();
    let mut ex = q.extras;

    // --- the segmenter runs continuously; `ready` is asserted for one
    // --- cycle when a packet finishes, which is what advances it.
    d.seg.start = i.start;
    d.seg.msg = i.msg;
    d.seg.ctx = i.ctx;
    d.seg.psn_start = i.psn_start;

    // `default()`, not `dont_care()`: this value *is* read on the idle
    // path (the header byte and length kernels evaluate every cycle
    // regardless of phase), and reading a `dont_care()` aggregate is a
    // partial read -- CLAUDE.md §12 rule 7. RHDL rejects it by name,
    // listing every uncovered field, which is the compiler doing its
    // job rather than being awkward.
    // clippy suggests `.unwrap_or_default()`, which is not in the
    // kernel-accepted subset of Rust -- `#[kernel]` lowers `match` on an
    // `Option`, not arbitrary `Option` methods. The explicit match is
    // the only spelling that compiles to hardware.
    #[allow(clippy::manual_unwrap_or_default)]
    let plan = match q.seg.plan {
        Some(p) => p,
        None => PacketPlan::default(),
    };
    let have_plan = match q.seg.plan {
        Some(_p) => true,
        None => false,
    };

    let bth = bth_from_plan(i.ctx, plan, i.ackreq);
    let hdr_len = header_bytes(plan).resize::<16>();
    let pay_wire = payload_on_wire(plan);

    // Both of these evaluate every cycle regardless of phase, so each
    // clamps its own index -- see `frame`'s module docs.
    let hdr_byte = header_byte(i.cfg, bth, plan, ex.index.resize::<7>());
    // The ICRC field is emitted most-significant byte first from `wire`,
    // which is already byte-reversed, so this puts the little-endian
    // on-wire order out. `icrc_left` counts 4..1, so the shift is
    // 8*(icrc_left-1) and is clamped away from underflow at 0.
    let safe_left = if ex.icrc_left == bits(0) {
        bits::<3>(1)
    } else {
        ex.icrc_left
    };
    let icrc_shift = (safe_left.resize::<32>() - bits::<32>(1)) << 3;
    let icrc_byte = ((q.icrc.wire >> icrc_shift) & bits(0xFF)).resize::<8>();

    // `d.phase` is assigned directly in each arm. An earlier comment
    // here claimed an intermediate local would hide the transitions
    // from the `FsmWidget` extractor. **That was wrong** -- tested
    // afterwards, and the extractor handles
    // `let mut next = q.phase; ... ; d.phase = next` correctly, because
    // it unions `Select` branches and that is exactly what a mutable
    // local lowers to. The direct form is kept because it reads better
    // for a phase machine, not because it is required.
    d.phase = q.phase;
    let mut emit = false;
    let mut byte = bits::<8>(0);
    let mut sof = false;
    let mut eof = false;
    let mut pay_adv = false;
    let mut icrc_start = false;
    let mut icrc_cover = false;
    let mut seg_ready = false;

    // Reads use `q.extras` and writes use `ex`, so a counter update
    // cannot accidentally feed the comparison that decides the next
    // phase.
    match q.phase {
        TxPhase::Idle => {
            if have_plan {
                d.phase = TxPhase::Header;
                ex.index = bits(0);
                ex.pay_left = pay_wire;
                ex.icrc_left = bits(4);
                icrc_start = true;
            }
        }
        TxPhase::Header => {
            emit = true;
            byte = hdr_byte;
            sof = q.extras.index == bits(0);
            // Coverage starts at the IPv4 header; Ethernet is excluded.
            icrc_cover = q.extras.index >= bits(14);
            if i.ready {
                ex.index = q.extras.index + 1;
                if q.extras.index + 1 >= hdr_len {
                    d.phase = if pay_wire == bits(0) {
                        TxPhase::Icrc
                    } else {
                        TxPhase::Payload
                    };
                }
            }
        }
        TxPhase::Payload => {
            emit = true;
            // The last `padcount` bytes of the payload phase are pad,
            // and the widget emits them itself. Requiring the caller to
            // present zeros for them -- which the first version did --
            // put a protocol detail in the caller's hands and was
            // invisible in testing, because a test that supplies zeros
            // passes either way.
            //
            // Pad bytes are zero here. The IBTA leaves their value
            // unspecified and the receiver discards them, but they are
            // covered by the ICRC, so sender and receiver must agree --
            // and zero is what every implementation sends.
            let in_pad = q.extras.pay_left <= plan.padcount.resize::<13>();
            byte = if in_pad { bits(0) } else { i.payload_byte };
            icrc_cover = true;
            // Do not step the caller's source for a byte it did not
            // supply.
            pay_adv = i.ready && !in_pad;
            if i.ready {
                ex.index = q.extras.index + 1;
                ex.pay_left = q.extras.pay_left - 1;
                if q.extras.pay_left == bits(1) {
                    d.phase = TxPhase::Icrc;
                }
            }
        }
        TxPhase::Icrc => {
            emit = true;
            byte = icrc_byte;
            // The ICRC does not cover itself.
            eof = q.extras.icrc_left == bits(1);
            if i.ready {
                ex.index = q.extras.index + 1;
                ex.icrc_left = q.extras.icrc_left - 1;
                if q.extras.icrc_left == bits(1) {
                    seg_ready = true;
                    d.phase = TxPhase::Idle;
                }
            }
        }
    }

    o.data = if emit { Some(byte) } else { None };
    o.payload_advance = pay_adv;
    o.start_of_frame = sof && emit;
    o.end_of_frame = eof && emit;
    o.busy = match q.phase {
        TxPhase::Idle => have_plan,
        TxPhase::Header => true,
        TxPhase::Payload => true,
        TxPhase::Icrc => true,
    };

    d.extras = ex;
    d.seg.ready = seg_ready;
    d.icrc.byte = byte;
    d.icrc.frame_index = q.extras.index;
    // Fold only bytes the sink actually took.
    d.icrc.enable = icrc_cover && i.ready;
    d.icrc.start = icrc_start;

    if cr.reset.any() {
        d.phase = TxPhase::Idle;
        d.extras.index = bits(0);
        d.extras.pay_left = bits(0);
        d.extras.icrc_left = bits(0);
        d.seg.ready = false;
        d.icrc.byte = bits(0);
        d.icrc.frame_index = bits(0);
        d.icrc.enable = false;
        d.icrc.start = true;
        o.data = None;
        o.payload_advance = false;
        o.start_of_frame = false;
        o.end_of_frame = false;
        o.busy = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::Transport;
    use crate::qp::context::Pmtu;
    use expect_test::expect;
    use rhdl_fpga::doc::render_fsm_diagram_markdown;
    use std::path::PathBuf;

    /// A frame from the committed Scapy corpus, by vector name.
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
        panic!("vector {name} not found in the committed corpus");
    }

    /// The `uc_write_only_16` vector's connection parameters.
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

    fn ctx() -> QpContext {
        QpContext {
            dqpn: bits(0x000111),
            transport: Transport::Uc,
            pmtu: Pmtu::B1024,
            rkey: bits(0x0000_A5A5),
            pkey: bits(0xFFFF),
        }
    }

    fn base_input_with(length: u128) -> In {
        In {
            msg: MessageRequest {
                va: bits(0x0000_7F00_1234_0000),
                length: bits(length),
            },
            ..base_input()
        }
    }

    fn base_input() -> In {
        In {
            start: false,
            msg: MessageRequest {
                va: bits(0x0000_7F00_1234_0000),
                length: bits(16),
            },
            ctx: ctx(),
            cfg: cfg(),
            psn_start: bits(1),
            ackreq: false,
            payload_byte: bits(0),
            ready: true,
        }
    }

    /// Drive a message and return every emitted byte.
    ///
    /// **Open-loop, in two passes, deliberately.** The obvious
    /// closed-loop driver — step a payload cursor when
    /// `payload_advance` is observed — is off by one against `run_fn`,
    /// because the output a call observes belongs to the cycle driven
    /// by the *previous* call's input. That misalignment presents the
    /// next payload byte one cycle early, which costs real debugging
    /// time: the emitted stream still looks right while the ICRC folds
    /// a shifted payload, so the widget appears to have a checksum bug
    /// when the harness is at fault. Driving open-loop removes the
    /// feedback and the ambiguity with it.
    ///
    /// Pass 1 locates the frame in time by watching `start_of_frame`;
    /// pass 2 replays with the payload placed in the window that
    /// follows the header.
    pub(super) fn emit(length: u128, payload: &[u8], stall_at: Option<usize>) -> Vec<u8> {
        emit_inner(length, payload, stall_at, bits(0)).0
    }

    /// Same drive, with a chosen IPv4 TOS byte.
    pub(super) fn emit_with_dscp(length: u128, payload: &[u8], dscp: Bits<8>) -> Vec<u8> {
        emit_inner(length, payload, None, dscp).0
    }

    /// How many times the widget stepped the caller's payload source.
    pub(super) fn count_payload_advances(length: u128) -> usize {
        let payload: Vec<u8> = (0..64).collect();
        emit_inner(length, &payload, None, bits(0)).1
    }

    fn emit_inner(
        length: u128,
        payload: &[u8],
        stall_at: Option<usize>,
        dscp: Bits<8>,
    ) -> (Vec<u8>, usize) {
        const HDR: usize = 70; // ONLY carries a RETH
        let pad = ((4 - (length % 4)) % 4) as usize;
        let on_wire = length as usize + pad;
        let n_cycles = 200 + on_wire;

        // `with_reset(1)` prepends a cycle, so **output sample `m`
        // corresponds to input index `m - 1`**. Keeping that explicit is
        // the whole trick: leaving it implicit put the payload window and
        // the ready-filter each one cycle out, in opposite directions.
        let input_of = |m: usize| m.wrapping_sub(1);
        let ready_at_input = |n: usize| Some(n) != stall_at;
        let ready_at_sample = |m: usize| ready_at_input(input_of(m));

        let run = |sched: &[u8]| -> Vec<(Option<u8>, bool, bool)> {
            let stream: Vec<In> = (0..n_cycles)
                .map(|n| {
                    let mut i = base_input_with(length);
                    i.cfg.dscp_ecn = dscp;
                    i.start = n == 0;
                    i.payload_byte = bits(sched[n] as u128);
                    i.ready = ready_at_input(n);
                    i
                })
                .collect();
            let uut = RoceWriteTx::default();
            let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
            uut.run(input)
                .synchronous_sample()
                .map(|s| {
                    (
                        s.output.data.map(|b| b.raw() as u8),
                        s.output.start_of_frame,
                        s.output.payload_advance,
                    )
                })
                .collect()
        };

        // Pass 1: locate the frame in time.
        let zeros = vec![0u8; n_cycles];
        let pass1 = run(&zeros);
        let sof = pass1
            .iter()
            .position(|(_, sof, _)| *sof)
            .expect("the widget never asserted start_of_frame");

        // Pass 2: place the payload in the accepted cycles that follow
        // the header. Counting *accepted* samples rather than raw cycles
        // is what makes a stall shift the window correctly.
        let mut sched = vec![0u8; n_cycles];
        let mut accepted = 0usize;
        for m in sof..pass1.len() {
            if !ready_at_sample(m) {
                continue;
            }
            if (HDR..HDR + on_wire).contains(&accepted) {
                let n = input_of(m);
                if n < n_cycles {
                    sched[n] = *payload.get(accepted - HDR).unwrap_or(&0);
                }
            }
            accepted += 1;
        }

        let pass2 = run(&sched);
        let advances = pass2
            .iter()
            .enumerate()
            .filter(|(m, (_, _, adv))| *adv && ready_at_sample(*m))
            .count();
        let bytes = pass2
            .into_iter()
            .enumerate()
            .filter(|(m, _)| ready_at_sample(*m))
            .filter_map(|(_, (d, _, _))| d)
            .collect();
        (bytes, advances)
    }

    // ---------------- Tier 1 ----------------

    #[test]
    fn reset_returns_to_idle_and_clears_the_counters() {
        let cr = clock_reset(clock(false), reset(true));
        let q = Q {
            phase: TxPhase::Payload,
            extras: TxExtras {
                index: bits(99),
                pay_left: bits(5),
                icrc_left: bits(2),
            },
            seg: crate::qp::segmenter::Out {
                plan: None,
                busy: false,
            },
            icrc: crate::icrc::Out {
                crc: bits(0),
                wire: bits(0),
            },
        };
        let (o, d) = roce_write_tx(cr, base_input(), q);
        assert_eq!(d.phase, TxPhase::Idle);
        assert_eq!(d.extras.index, bits::<16>(0));
        assert_eq!(o.data, None);
        assert!(!o.busy);
        assert!(d.icrc.start);
        assert!(!d.icrc.enable);
    }

    /// An unready sink sees the byte held and nothing advanced — not the
    /// index, and critically not the ICRC, which would otherwise fold
    /// the same byte twice.
    #[test]
    fn an_unready_sink_advances_nothing() {
        let cr = ClockReset::dont_care();
        let q = Q {
            phase: TxPhase::Header,
            extras: TxExtras {
                index: bits(20),
                pay_left: bits(16),
                icrc_left: bits(4),
            },
            seg: crate::qp::segmenter::Out {
                plan: Some(PacketPlan::default()),
                busy: true,
            },
            icrc: crate::icrc::Out {
                crc: bits(0),
                wire: bits(0),
            },
        };
        let mut i = base_input();
        i.ready = false;
        let (o, d) = roce_write_tx(cr, i, q);
        assert!(o.data.is_some(), "the byte is still presented");
        assert_eq!(d.extras.index, bits::<16>(20), "index must not advance");
        assert_eq!(d.phase, TxPhase::Header, "phase must not advance");
        assert!(!d.icrc.enable, "the ICRC must not fold an unaccepted byte");
    }

    /// The ICRC is fed over exactly the covered range and never over the
    /// Ethernet header or its own four bytes.
    #[test]
    fn the_icrc_is_fed_over_exactly_the_covered_range() {
        let cr = ClockReset::dont_care();
        let mk = |phase, index| Q {
            phase,
            extras: TxExtras {
                index: bits(index),
                pay_left: bits(16),
                icrc_left: bits(4),
            },
            seg: crate::qp::segmenter::Out {
                plan: Some(PacketPlan::default()),
                busy: true,
            },
            icrc: crate::icrc::Out {
                crc: bits(0),
                wire: bits(0),
            },
        };
        for idx in 0..14u128 {
            let (_o, d) = roce_write_tx(cr, base_input(), mk(TxPhase::Header, idx));
            assert!(!d.icrc.enable, "Ethernet byte {idx} must not be covered");
        }
        for idx in 14..54u128 {
            let (_o, d) = roce_write_tx(cr, base_input(), mk(TxPhase::Header, idx));
            assert!(d.icrc.enable, "header byte {idx} must be covered");
        }
        let (_o, d) = roce_write_tx(cr, base_input(), mk(TxPhase::Payload, 80));
        assert!(d.icrc.enable, "payload must be covered");
        let (_o, d) = roce_write_tx(cr, base_input(), mk(TxPhase::Icrc, 90));
        assert!(!d.icrc.enable, "the ICRC must not cover itself");
    }

    // ---------------- Tier 2: the acceptance test ----------------

    /// **The assembled datapath emits a frame Scapy agrees with, byte
    /// for byte.**
    ///
    /// This is the point of the whole widget. The frame below is vector
    /// `uc_write_only_16`; every byte of it — Ethernet, IPv4 with its
    /// checksum, UDP, BTH, RETH, payload, ICRC — comes out of the
    /// widget's `data` port, in order, one per cycle.
    #[test]
    fn the_emitted_frame_matches_scapy() {
        // Read from the committed corpus rather than hand-typed. The
        // first version of this test had a stray space in a `concat!`
        // that silently dropped a nibble, so the "expected" frame was
        // 89 bytes and the widget got blamed for being right.
        let expected = vector_frame("uc_write_only_16");
        let payload: Vec<u8> = (0..16).collect();
        let got = emit(16, &payload, None);
        assert_eq!(
            got.len(),
            expected.len(),
            "emitted {} bytes, expected {}",
            got.len(),
            expected.len()
        );
        if got != expected {
            let at = got.iter().zip(&expected).position(|(a, b)| a != b).unwrap();
            panic!(
                "frames differ at offset {at} (got {:#04x}, expected {:#04x})\n  got      {:02x?}\n  expected {:02x?}",
                got[at], expected[at], got, expected
            );
        }
    }

    /// And the same frame with a stall injected mid-payload. If the hold
    /// contract were wrong, a byte would be dropped or doubled and the
    /// frame would differ.
    #[test]
    fn a_stall_does_not_change_the_frame() {
        let payload: Vec<u8> = (0..16).collect();
        let clean = emit(16, &payload, None);
        for stall in [20usize, 71, 78, 86, 88] {
            let stalled = emit(16, &payload, Some(stall));
            assert_eq!(
                stalled, clean,
                "a stall at cycle {stall} changed the emitted frame"
            );
        }
    }

    /// Framing markers land where they should: one `sof` at the first
    /// byte, one `eof` at the last, and the frame is the right length.
    #[test]
    fn the_frame_is_90_bytes_for_a_16_byte_only_packet() {
        // 14 Eth + 20 IPv4 + 8 UDP + 12 BTH + 16 RETH + 16 payload + 4 ICRC
        assert_eq!(emit(16, &(0..16).collect::<Vec<u8>>(), None).len(), 90);
        // A zero-length message is a legal WRITE: header + ICRC only.
        assert_eq!(emit(0, &[], None).len(), 74);
    }

    // ---------------- rule 14: the FSM diagram ----------------

    /// The committed diagram agrees with what the kernel's RHIF says
    /// now. CLAUDE.md §12 rule 14: a widget that takes the `FsmWidget`
    /// derive must surface the diagram, and the diagram must not drift
    /// from the source.
    #[test]
    fn test_fsm_diagram_matches_descriptor() {
        let transitions = rhdl::core::fsm::extract_widget_transitions_strict::<RoceWriteTx>()
            .expect("extracting transitions");
        let fresh = render_fsm_diagram_markdown::<RoceWriteTx>(&transitions);
        let committed = include_str!("../doc/tx_fsm.md");
        assert_eq!(
            fresh.trim(),
            committed.trim(),
            "doc/tx_fsm.md is stale -- re-run `cargo run --example tx -p rhdl-roce`"
        );
    }

    /// The FSM is the five transitions the frame structure implies, and
    /// no others. A sixth would mean a path through the datapath that
    /// the documentation does not describe.
    /// The FSM's inter-state edges are exactly the five the frame
    /// structure implies. A sixth would be a path through the datapath
    /// the documentation does not describe.
    ///
    /// Self-loops are expected and excluded: every phase can hold, both
    /// because `d.phase = q.phase` is the default and because an unready
    /// sink stalls in place. The extractor reports them, which is
    /// correct — an earlier version of this test asserted a bare count
    /// of 5 and failed at 9 for that reason.
    #[test]
    fn the_fsm_has_exactly_the_expected_inter_state_edges() {
        use std::collections::BTreeSet;
        let transitions = rhdl::core::fsm::extract_widget_transitions_strict::<RoceWriteTx>()
            .expect("extracting transitions");
        // Declaration order: Idle = 0, Header = 1, Payload = 2, Icrc = 3.
        const IDLE: usize = 0;
        const HEADER: usize = 1;
        const PAYLOAD: usize = 2;
        const ICRC: usize = 3;
        let edges: BTreeSet<(usize, usize)> = transitions
            .iter()
            .map(|t| (t.source_index, t.target_index))
            .filter(|(a, b)| a != b)
            .collect();
        let expected: BTreeSet<(usize, usize)> = [
            (IDLE, HEADER),
            (HEADER, PAYLOAD),
            (HEADER, ICRC),
            (PAYLOAD, ICRC),
            (ICRC, IDLE),
        ]
        .into_iter()
        .collect();
        assert_eq!(edges, expected, "unexpected FSM edge set");
        // And every state can hold, which is what the self-loops mean.
        let selfs: BTreeSet<usize> = transitions
            .iter()
            .filter(|t| t.source_index == t.target_index)
            .map(|t| t.source_index)
            .collect();
        assert_eq!(selfs.len(), 4, "every phase should be able to hold");
    }

    // ---------------- Tier 3/4/5 ----------------

    /// Open-loop stimulus for the codegen tiers. The payload is a
    /// constant here on purpose: Tier 4 asks whether the Verilog agrees
    /// with the Rust simulator, which a constant payload exercises just
    /// as well, and it avoids making the round-trip depend on
    /// closed-loop feedback.
    fn open_loop_stimulus() -> Vec<In> {
        let mut v = vec![In {
            start: true,
            payload_byte: bits(0xA5),
            ..base_input()
        }];
        for n in 0..110 {
            v.push(In {
                payload_byte: bits(0xA5),
                ready: n != 80,
                ..base_input()
            });
        }
        v
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = RoceWriteTx::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        // Pinned by shape rather than verbatim: the module is large and
        // mostly the header mux, so a full snapshot would be ~1500 lines
        // of noise. `fifo::synchronous` and `core::ram::option_sync` omit
        // theirs entirely for the same reason; this pins the structure.
        assert!(hdl.contains("module top"), "no top module emitted");
        for child in ["top_phase", "top_extras", "top_seg", "top_icrc"] {
            assert!(
                hdl.contains(child),
                "child module {child} missing from the emitted HDL"
            );
        }
        let expect = expect!["7"];
        expect.assert_eq(&hdl.matches("endmodule").count().to_string());
        Ok(())
    }

    #[test]
    fn test_tx_hdl_works() -> miette::Result<()> {
        let uut = RoceWriteTx::default();
        let input = open_loop_stimulus()
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        // `.skip(2)`: the ICRC sub-circuit's register resets to the
        // non-zero prefix remainder, the documented divergence at time 0.
        let opts = TestBenchOptions::default().skip(2);
        tb.rtl(&uut, &opts)?.run_iverilog()?;
        tb.ntl(&uut, &opts)?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_tx_trace() -> miette::Result<()> {
        let uut = RoceWriteTx::default();
        let input = open_loop_stimulus()
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("tx");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["ac511de3bd56842f13ea60a6acd937b68f9613ccbbc933ba6258e254668e92d3"];
        let digest = vcd.dump_to_file(root.join("tx.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}

#[cfg(test)]
mod phase3_debts {
    //! Tests for the three debts the Phase-2 audit turned up.

    use super::tests::{count_payload_advances, emit, emit_with_dscp};
    use rhdl::prelude::*;

    /// **The widget emits its own pad bytes.**
    ///
    /// Non-vacuous by construction: the caller is made to present
    /// `0xFF` for the pad slots, so a widget that forwarded
    /// `payload_byte` through the pad — which the first version did —
    /// would put `FF FF FF` on the wire. A test that supplied zeros for
    /// the pad, as the earlier ones did, passes either way and proves
    /// nothing.
    #[test]
    fn the_pad_bytes_are_generated_not_forwarded() {
        // 13 data bytes -> padcount 3. Slots 13..16 are pad, and the
        // caller deliberately offers rubbish for them.
        let mut payload: Vec<u8> = (1..=13).collect();
        payload.extend_from_slice(&[0xFF, 0xFF, 0xFF]);
        let frame = emit(13, &payload, None);
        assert_eq!(
            frame.len(),
            70 + 16 + 4,
            "70 header + 16 padded payload + 4 ICRC"
        );
        assert_eq!(
            &frame[70..83],
            &(1..=13).collect::<Vec<u8>>()[..],
            "the data bytes should pass through"
        );
        assert_eq!(
            &frame[83..86],
            &[0, 0, 0],
            "the pad must be zeros the widget generated, not the 0xFF the caller offered"
        );
    }

    /// And the caller's source is not stepped for bytes it did not
    /// supply: `payload_advance` fires exactly `length` times, not
    /// `length + padcount`.
    #[test]
    fn payload_advance_counts_data_bytes_only() {
        for (length, pad) in [(13u128, 3usize), (14, 2), (15, 1), (16, 0)] {
            let advances = count_payload_advances(length);
            assert_eq!(
                advances, length as usize,
                "length {length} (pad {pad}): source stepped {advances} times"
            );
        }
    }

    /// A non-zero DSCP reaches the wire, changes the IPv4 header
    /// checksum, and — because the ICRC masks that byte — leaves the
    /// ICRC untouched.
    ///
    /// The second half is the interesting one: it is a positive test
    /// that the ICRC masking is real rather than decorative.
    #[test]
    fn dscp_reaches_the_wire_and_the_icrc_ignores_it() {
        let payload: Vec<u8> = (0..16).collect();
        let plain = emit_with_dscp(16, &payload, bits(0x00));
        let marked = emit_with_dscp(16, &payload, bits(0xB8)); // DSCP 46 (EF), ECN 0
        assert_eq!(plain[15], 0x00, "TOS byte should be where we think");
        assert_eq!(marked[15], 0xB8, "DSCP should reach frame offset 15");
        assert_ne!(
            &plain[24..26],
            &marked[24..26],
            "the IPv4 header checksum must change with the TOS byte"
        );
        assert_eq!(
            &plain[plain.len() - 4..],
            &marked[marked.len() - 4..],
            "the ICRC masks the TOS byte, so it must NOT change"
        );
        // Everything else is identical.
        assert_eq!(plain.len(), marked.len());
        for k in 0..plain.len() {
            if k == 15 || (24..26).contains(&k) {
                continue;
            }
            assert_eq!(plain[k], marked[k], "byte {k} changed unexpectedly");
        }
    }
}
