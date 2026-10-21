//! The assembled receiver: a frame arrives as bytes and its payload is
//! written to the local address the sender asked for — if, and only if,
//! the sender was entitled to ask.
//!
//! This is what [`crate::rx::parse`], [`crate::rx::verify`],
//! [`crate::qp::table`] and [`crate::qp::mr`] are for. It composes
//! them and adds the one thing none of them has: the per-queue-pair
//! reassembly state that lets a multi-packet message be placed, since
//! only a message's FIRST packet carries the address.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
            +-+RoceWriteRx+-----------+
            |                          |
  ?u8       |                          |  ?Placement
 +--------->| data              write +--------->
            |                          |
  bool      |                          |  bool
 +--------->| sof                done +--------->
            |                          |
  bool      |                          |  RxOutcome
 +--------->| eof             outcome +--------->
            |                          |
  ?MrWrite  |                          |  b13
 +--------->| mr_write         placed +--------->
            |                          |
  ?QpWrite  |                          |  ?RxHeaders
 +--------->| qp_write        headers +--------->
            +--------------------------+
")]
//!
//! # Internals
#![doc = badascii_doc::badascii!("
  bytes                                      stage 0        stage 1
    |
    v
  +-+--------------+  headers (held)   +--------------+
  |   RxVerify     +------------------>+  QpTable     +---+
  |  parse + ICRC  |  dqpn             +--------------+   |
  |                +------------------>+  MrTable     +---+---> decide:
  |                |  reth.rkey        +--------------+   |      authorize,
  |                |                   +--------------+   |      translate,
  |                +------------------>+  msg context +---+      latch
  +-+--------------+  dqpn             +--------------+          |
    |                                                            |
    | payload byte                                               v
    +---------------> [ 1-cycle delay ] --------------------> write(addr,data)
")]
//!
//! # Why the payload is delayed by one cycle
//!
//! Not a pipelining nicety — the frame layout forces it. A MIDDLE or
//! LAST packet's BTH ends at offset 53 and its first payload byte is
//! offset 54, so the queue-pair number is known exactly one cycle
//! before the byte that depends on it. Three synchronous RAMs have to
//! be read with that number (the queue-pair context, the memory
//! region, and this message's reassembly state) and a RAM answers the
//! cycle after it is addressed.
//!
//! So the payload passes through one register while the lookups
//! resolve, and placement runs one cycle behind the wire. The lookups
//! are addressed from `headers`, which the parser *holds* for the whole
//! packet, so the addresses are stable and only the first byte actually
//! needs the slack — delaying every byte uniformly is what keeps the
//! logic one shape instead of two.
//!
//! # Authorization is never speculative; only integrity is
//!
//! This is the central rule of this widget and the reason to read its
//! code before changing it.
//!
//! A frame's ICRC cannot be known until its last byte, so bytes are
//! placed before the frame is known to be intact. That is unavoidable
//! and is the same bargain a real RNIC makes — the alternative costs a
//! PMTU of buffering and the latency that motivates RDMA.
//!
//! **The R_Key check is in an entirely different position.** It
//! resolves from the RETH, which arrives before the first payload byte,
//! so there is no reason to write anything before it has passed — and
//! every reason not to. A write is emitted only once
//! [`crate::qp::mr::mr_validate`] has returned
//! [`crate::qp::mr::MrVerdict::Accept`] for the whole range. An
//! unauthorized frame places nothing at all, not even provisionally.
//!
//! The consequence of the speculative half is honest and worth
//! stating: **a frame that fails its ICRC will have left its bytes in
//! memory.** They are inside an authorized region, so this is data
//! corruption and not a memory-safety problem, and on an unreliable
//! connection a lost message is the failure mode the transport already
//! has. The reassembly state is *not* advanced for such a frame, so the
//! message is abandoned and the next FIRST resynchronises — which is
//! exactly what `roce-v2-architecture.md` §5.3 says UC buys.
//!
//! # Reassembly, and what a sequence-number gap costs
//!
//! Only FIRST and ONLY carry a RETH. MIDDLE and LAST say where they go
//! only implicitly: they continue the message their predecessor began.
//! So a receiver has to remember, per queue pair, where the message is
//! up to — the local cursor, how many bytes remain, and which sequence
//! number comes next.
//!
//! A packet whose PSN is not the expected one is dropped
//! ([`RxOutcome::PsnGap`]), and so is every subsequent MIDDLE or LAST,
//! because the cursor can no longer be trusted. A FIRST or ONLY
//! restarts from its own RETH and recovers. **Placing a MIDDLE after a
//! gap would write the right bytes to the wrong address**, which is why
//! the gap is fatal to the message rather than something to paper over.
//!
//! # Example
//!
//!```
#![doc = include_str!("../../examples/rx_endpoint.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/rx_endpoint.md")]

use crate::qp::mr::{MrTable, MrVerdict, MrWrite, mr_validate};
use crate::qp::table::{QpTable, QpWrite, qpn_index};
use crate::rx::parse::{RxHeaders, RxVerdict};
use crate::rx::verify::RxVerify;
use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_fpga::core::ram::synchronous::{SyncBRAM, Write};

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// One byte, and where it goes.
pub struct Placement {
    /// The local address to write.
    pub addr: Bits<32>,
    /// The byte.
    pub data: Bits<8>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// Where a message is up to, per queue pair.
///
/// Private to the receiver rather than a public table, because nothing
/// else needs it: the send side has no equivalent, and the connection
/// manager configures queue pairs rather than tracking messages.
pub struct RxMessage {
    /// The sequence number the next packet of this message must carry.
    pub expected_psn: Bits<24>,
    /// The local address the next payload byte goes to.
    pub cursor: Bits<32>,
    /// How many bytes of the message are still to come.
    pub remaining: Bits<32>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// What became of a frame.
///
/// One value covering every stage, so a consumer has a single thing to
/// read. The two data-carrying variants forward the verdict of the
/// stage that rejected the frame rather than flattening it, because
/// "the R_Key was stale" and "the frame was not even IPv4" call for
/// different responses and a receiver's counters are the only place
/// that distinction survives.
pub enum RxOutcome {
    #[default]
    /// Nothing has happened yet this frame.
    Idle,
    /// The frame was accepted, verified, authorized, and its payload
    /// written.
    Placed,
    /// Rejected by the parser or the checksum.
    Frame(RxVerdict),
    /// Rejected by the memory-region check. **No bytes were written.**
    Access(MrVerdict),
    /// No queue pair is configured for this frame's destination QPN.
    NoQueuePair,
    /// A MIDDLE or LAST arrived with no message in progress on that
    /// queue pair — its predecessor was lost, or never sent.
    NoMessage,
    /// The sequence number was not the expected one. The message is
    /// abandoned; a FIRST or ONLY will resynchronise.
    PsnGap,
    /// The packet carried more payload than the message had left to
    /// place. The excess is not written.
    Overrun,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
#[doc(hidden)]
/// The receiver's own registers, bundled per CLAUDE.md §3.1.
pub struct RxRegs {
    /// The payload byte from the previous cycle — the one-cycle delay
    /// that lets the lookups resolve.
    pub held: Option<Bits<8>>,
    /// `held`'s index within this packet's payload.
    pub held_index: Bits<13>,
    /// Payload bytes taken from the wire so far this packet.
    pub taken: Bits<13>,
    /// Set once the decision below has been made for this packet.
    pub decided: bool,
    /// Whether this packet may write at all.
    pub authorized: bool,
    /// Local address of this packet's first payload byte.
    pub base: Bits<32>,
    /// How many bytes this packet may write, after clamping to what the
    /// message has left.
    pub limit: Bits<13>,
    /// The outcome latched for this packet.
    pub outcome: RxOutcome,
    /// Whether `headers` have been seen for this packet yet.
    pub seen_headers: bool,
    /// The headers, registered so stage 1 sees the same values the
    /// lookups were addressed with.
    pub headers: RxHeaders,
    /// Bytes actually written this packet.
    pub placed: Bits<13>,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// A complete UC RDMA WRITE receiver. See the [module docs](self).
///
/// `QPN_W` sizes the queue-pair and message tables; `RKEY_W` sizes the
/// memory-region table. Both index by the low bits of their key, and
/// both compare the full key — see [`crate::qp::mr`] on aliasing.
pub struct RoceWriteRx<const QPN_W: usize, const RKEY_W: usize>
where
    rhdl::bits::W<QPN_W>: BitWidth,
    rhdl::bits::W<RKEY_W>: BitWidth,
{
    verify: RxVerify,
    qp: QpTable<QPN_W>,
    mr: MrTable<RKEY_W>,
    msg: SyncBRAM<Option<RxMessage>, QPN_W>,
    regs: dff::DFF<RxRegs>,
}

impl<const QPN_W: usize, const RKEY_W: usize> Default for RoceWriteRx<QPN_W, RKEY_W>
where
    rhdl::bits::W<QPN_W>: BitWidth,
    rhdl::bits::W<RKEY_W>: BitWidth,
{
    fn default() -> Self {
        Self {
            verify: RxVerify::default(),
            qp: QpTable::default(),
            mr: MrTable::default(),
            // No message in progress on any queue pair.
            msg: SyncBRAM::new((0..(1 << QPN_W)).map(|i| (bits(i as u128), None))),
            regs: dff::DFF::default(),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [`RoceWriteRx`].
pub struct In {
    /// The next frame byte, or `None` when the link is idle.
    pub data: Option<Bits<8>>,
    /// This byte is the first of a frame.
    pub start_of_frame: bool,
    /// This byte is the last of a frame.
    pub end_of_frame: bool,
    /// Register or deregister a memory region.
    pub mr_write: Option<MrWrite>,
    /// Configure or tear down a queue pair.
    pub qp_write: Option<QpWrite>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [`RoceWriteRx`].
pub struct Out {
    /// A byte to write to local memory, with its address. Emitted only
    /// for an authorized packet — see the module docs on what is and is
    /// not speculative.
    pub write: Option<Placement>,
    /// The frame ended this cycle; `outcome` is final.
    pub done: bool,
    /// What became of the frame.
    pub outcome: RxOutcome,
    /// How many payload bytes this frame placed.
    pub placed: Bits<13>,
    /// The recovered headers, for observability.
    pub headers: Option<RxHeaders>,
    /// A frame is in progress.
    pub busy: bool,
}

impl<const QPN_W: usize, const RKEY_W: usize> SynchronousIO for RoceWriteRx<QPN_W, RKEY_W>
where
    rhdl::bits::W<QPN_W>: BitWidth,
    rhdl::bits::W<RKEY_W>: BitWidth,
{
    type I = In;
    type O = Out;
    type Kernel = roce_write_rx<QPN_W, RKEY_W>;
}

#[kernel]
/// How many bytes of this packet may be written: its payload, less any
/// pad, clamped to what the message has left.
///
/// The clamp is the guard against a peer sending a packet longer than
/// the message it declared. Without it the excess would be written past
/// the end of the region the R_Key authorized — the range check passed
/// on the *declared* length, so the surplus is exactly the part nothing
/// else has validated.
pub fn writable_bytes(payload_len: Bits<13>, padcount: Bits<2>, remaining: Bits<32>) -> Bits<13> {
    // Pad is not message data. Clamp first so the subtraction cannot
    // wrap on a malformed packet whose pad exceeds its payload.
    let pad = padcount.resize::<13>();
    let body = if payload_len > pad {
        payload_len - pad
    } else {
        bits::<13>(0)
    };
    // `resize` truncates, so compare in 32 bits and only narrow once
    // the value is known to fit.
    let body32 = body.resize::<32>();
    let allowed = if body32 > remaining {
        remaining
    } else {
        body32
    };
    (allowed & bits(0x1FFF)).resize::<13>()
}

#[kernel]
// Two kernel-subset facts, both learned the hard way, both the reason
// for the allows below:
//
//   - `if let` is not accepted. `#[kernel]` lowers `match` on an
//     `Option`, not arbitrary `Option` methods or sugar over them, so
//     the explicit single-arm matches are the only spelling that
//     compiles to hardware. Same reasoning and the same allow as
//     `crate::endpoint`.
//   - **match guards are not accepted either** -- `Some(b) if cond =>`
//     is rejected with "Guards are not supported in rhdl kernel
//     function match expressions". That is what clippy's
//     `collapsible_match` suggestion here would require, so it cannot
//     be taken.
#[allow(clippy::single_match, clippy::collapsible_match)]
/// Kernel for [`RoceWriteRx`].
pub fn roce_write_rx<const QPN_W: usize, const RKEY_W: usize>(
    cr: ClockReset,
    i: In,
    q: Q<QPN_W, RKEY_W>,
) -> (Out, D<QPN_W, RKEY_W>)
where
    rhdl::bits::W<QPN_W>: BitWidth,
    rhdl::bits::W<RKEY_W>: BitWidth,
{
    let mut d = D::<QPN_W, RKEY_W>::dont_care();
    let mut o = Out::dont_care();
    let mut r = q.regs;

    // ---- the byte interface goes to the parser/verifier unchanged.
    d.verify.data = i.data;
    d.verify.start_of_frame = i.start_of_frame;
    d.verify.end_of_frame = i.end_of_frame;
    let v = q.verify;

    o.headers = v.headers;
    o.busy = v.busy;
    o.done = false;
    o.outcome = RxOutcome::Idle;
    o.placed = bits(0);
    o.write = None;

    // ---- stage 0: address the three tables from the held headers.
    //
    // `headers` is held for the whole packet, so these are stable and a
    // read issued on any cycle of the packet answers on the next.
    let have = match v.headers {
        Some(_h) => true,
        None => false,
    };
    // Before the headers arrive, and after the frame ends, the
    // registered copy is what the lookups are addressed with -- so a
    // read is always issued with a stable value rather than a
    // half-assembled one.
    let hdr = match v.headers {
        Some(h) => h,
        None => r.headers,
    };
    let lookup_qpn = hdr.bth.dqpn;
    let lookup_rkey = hdr.reth.rkey;

    d.qp.read_qpn = lookup_qpn;
    d.mr.read_rkey = lookup_rkey;
    d.msg.read_addr = qpn_index::<QPN_W>(lookup_qpn);

    // Host-side configuration writes pass straight through.
    d.qp.write = i.qp_write;
    d.mr.write = i.mr_write;

    // ---- stage 0: take this cycle's payload byte into the delay.
    let next_held = v.payload;
    // Taken before the increment, so it indexes the byte being held.
    // Meaningless when `next_held` is `None`, and unread in that case.
    let next_held_index = r.taken;
    match v.payload {
        Some(_b) => {
            r.taken += bits(1);
        }
        None => {}
    }

    // The first cycle headers appear is when the lookups are issued, so
    // the decision is made on the next one.
    let decide_now = have && !r.seen_headers;
    if have {
        r.seen_headers = true;
        r.headers = hdr;
    }

    // ---- stage 1: the decision, one cycle after the lookups issued.
    if !q.regs.decided && q.regs.seen_headers && !decide_now {
        let h = q.regs.headers;
        // Initialised to the no-queue-pair rejection so that the `None`
        // arm below needs no body -- every other path overwrites it.
        let mut outcome = RxOutcome::NoQueuePair;
        let mut authorized = false;
        let mut base = bits::<32>(0);
        let mut limit = bits::<13>(0);

        match q.qp.ctx {
            None => {}
            Some(_ctx) => {
                // The context exists. A queue pair we have never been
                // told about does not get to write to us, which is what
                // the initial value above reports.
                if h.has_reth {
                    // FIRST or ONLY: the packet states its own address,
                    // and the R_Key must authorize the whole message.
                    let look = mr_validate(q.mr.region, h.reth.rkey, h.reth.va, h.reth.dma_len);
                    if look.verdict == MrVerdict::Accept {
                        authorized = true;
                        base = look.local_addr;
                        limit = writable_bytes(h.payload_len, h.bth.padcount, h.reth.dma_len);
                        outcome = RxOutcome::Placed;
                    } else {
                        outcome = RxOutcome::Access(look.verdict);
                    }
                } else {
                    // MIDDLE or LAST: it continues whatever the queue
                    // pair was doing, and must be the next packet of it.
                    match q.msg {
                        None => {
                            outcome = RxOutcome::NoMessage;
                        }
                        Some(m) => {
                            if h.bth.psn != m.expected_psn {
                                outcome = RxOutcome::PsnGap;
                            } else {
                                authorized = true;
                                base = m.cursor;
                                limit = writable_bytes(h.payload_len, h.bth.padcount, m.remaining);
                                outcome = RxOutcome::Placed;
                            }
                        }
                    }
                }
                // More payload than the message had room for. The
                // clamp above already refuses to write the surplus;
                // this reports it.
                let body = writable_bytes(h.payload_len, h.bth.padcount, bits(0xFFFF_FFFF));
                if authorized && body > limit {
                    outcome = RxOutcome::Overrun;
                }
            }
        }
        r.decided = true;
        r.authorized = authorized;
        r.base = base;
        r.limit = limit;
        r.outcome = outcome;
    }

    // ---- stage 1: place the delayed byte.
    //
    // Gated on `authorized`, which is the R_Key check having passed.
    // Never gated on the checksum, which cannot be known yet.
    // The decision is read from `r`, not from `q.regs`.
    //
    // This is load-bearing and was a real bug. The decision is computed
    // into `r` in the block above, and `r` does not become visible as
    // `q.regs` until the next cycle -- but the byte it authorizes is
    // *already* in the delay register during the very cycle the
    // decision is made. Reading `q.regs.authorized` here therefore saw
    // the pre-decision value, `false`, and silently dropped the first
    // payload byte of every packet: 1023 bytes placed out of 1024.
    //
    // On every later cycle of the packet `r` starts as a copy of
    // `q.regs` and the decision block does not re-run, so `r` and
    // `q.regs` agree and this reads the latched values. The held byte
    // and its index must still come from `q.regs`, because those *are*
    // the delay.
    match q.regs.held {
        Some(b) => {
            if r.authorized && q.regs.held_index < r.limit {
                let mut p = Placement::dont_care();
                p.addr = r.base + q.regs.held_index.resize::<32>();
                p.data = b;
                o.write = Some(p);
                r.placed += bits(1);
            }
        }
        None => {}
    }

    // ---- end of frame: report, and commit the reassembly state.
    let mut msg_write = Write::<Option<RxMessage>, QPN_W>::dont_care();
    msg_write.addr = qpn_index::<QPN_W>(lookup_qpn);
    msg_write.value = None;
    msg_write.enable = false;

    if v.done {
        o.done = true;
        o.placed = r.placed;
        if v.verdict != RxVerdict::Accept {
            // The frame was not intact or not ours. Whatever the
            // authorization said, the message cannot continue.
            o.outcome = RxOutcome::Frame(v.verdict);
            msg_write.value = None;
            msg_write.enable = true;
        } else if r.outcome != RxOutcome::Placed {
            o.outcome = r.outcome;
            msg_write.value = None;
            msg_write.enable = true;
        } else {
            o.outcome = RxOutcome::Placed;
            // Advance, or clear if the message is complete. A message
            // ends when the bytes placed account for all of it.
            let h = r.headers;
            let written = r.placed.resize::<32>();
            let mut m = RxMessage::dont_care();
            let advanced = q.regs.base + written;
            let had = if h.has_reth {
                h.reth.dma_len
            } else {
                match q.msg {
                    Some(prev) => prev.remaining,
                    None => bits(0),
                }
            };
            let left = if had > written {
                had - written
            } else {
                bits::<32>(0)
            };
            m.cursor = advanced;
            m.remaining = left;
            m.expected_psn = h.bth.psn + bits(1);
            msg_write.value = if left == bits(0) { None } else { Some(m) };
            msg_write.enable = true;
        }
        // A new packet starts from scratch.
        r.decided = false;
        r.authorized = false;
        r.seen_headers = false;
        r.taken = bits(0);
        r.placed = bits(0);
        r.outcome = RxOutcome::Idle;
    }

    d.msg.write = msg_write;
    r.held = next_held;
    r.held_index = next_held_index;

    if cr.reset.any() {
        d.verify.data = None;
        d.verify.start_of_frame = false;
        d.verify.end_of_frame = false;
        d.qp.read_qpn = bits(0);
        d.qp.write = None;
        d.mr.read_rkey = bits(0);
        d.mr.write = None;
        d.msg.read_addr = bits(0);
        d.msg.write.addr = bits(0);
        d.msg.write.value = None;
        d.msg.write.enable = false;
        r = RxRegs::default();
        o.write = None;
        o.done = false;
        o.outcome = RxOutcome::Idle;
        o.placed = bits(0);
        o.headers = None;
        o.busy = false;
    }
    d.regs = r;
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;

    /// A frame from the committed Scapy corpus, by vector name.
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

    // ---------------------------------------- Tier 1: the clamp

    /// **The guard against a packet longer than the message declared.**
    /// The range check authorized the *declared* length, so any surplus
    /// is the part nothing has validated — and must not be written.
    #[test]
    fn writable_bytes_clamps_to_what_the_message_has_left() {
        // Ordinary case: the whole payload fits.
        assert_eq!(writable_bytes(bits(16), bits(0), bits(1000)).raw(), 16);
        // Pad is not message data.
        assert_eq!(writable_bytes(bits(16), bits(3), bits(1000)).raw(), 13);
        // Clamped by what is left.
        assert_eq!(writable_bytes(bits(1024), bits(0), bits(100)).raw(), 100);
        assert_eq!(writable_bytes(bits(1024), bits(0), bits(0)).raw(), 0);
        // Exactly the remainder.
        assert_eq!(writable_bytes(bits(452), bits(0), bits(452)).raw(), 452);
        // A pad count larger than the payload must clamp to zero, not
        // wrap to 8191.
        assert_eq!(
            writable_bytes(bits(1), bits(3), bits(1000)).raw(),
            0,
            "a malformed packet whose pad exceeds its payload"
        );
        assert_eq!(writable_bytes(bits(0), bits(3), bits(1000)).raw(), 0);
        // A remainder above 13 bits must not truncate into a small
        // number -- `resize` truncates, so the comparison has to happen
        // in 32 bits.
        assert_eq!(
            writable_bytes(bits(4096), bits(0), bits(0x0001_0000)).raw(),
            4096,
            "a 65536-byte remainder does not truncate to zero"
        );
    }

    #[test]
    fn writable_bytes_kernel_vm_and_verilog() -> miette::Result<()> {
        let mut inputs = Vec::new();
        for len in [0u128, 1, 3, 16, 452, 1024, 4096, 8191] {
            for pad in 0u128..4 {
                for rem in [0u128, 1, 100, 452, 4096, 0x0001_0000, 0xFFFF_FFFF] {
                    inputs.push((bits::<13>(len), bits::<2>(pad), bits::<32>(rem)));
                }
            }
        }
        test_kernel_vm_and_verilog_synchronous::<writable_bytes, _, _, _>(
            writable_bytes,
            inputs.into_iter(),
        )?;
        Ok(())
    }

    // ------------------------------------ Tier 2: direct kernel probes

    /// Reset clears the registers and emits nothing.
    #[test]
    fn reset_places_nothing() {
        let cr = clock_reset(clock(false), reset(true));
        let q = Q::<4, 4>::dont_care();
        let i = In {
            data: Some(bits(0x42)),
            start_of_frame: false,
            end_of_frame: false,
            mr_write: None,
            qp_write: None,
        };
        let (o, d) = roce_write_rx::<4, 4>(cr, i, q);
        assert_eq!(o.write, None, "no placement under reset");
        assert!(!o.done);
        assert_eq!(o.outcome, RxOutcome::Idle);
        assert_eq!(d.regs, RxRegs::default(), "registers cleared");
        assert!(!d.msg.write.enable, "and no reassembly state written");
    }

    // ---------------------------------------- Tiers 3, 4 and 5

    fn test_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In)>> {
        let idle = In {
            data: None,
            start_of_frame: false,
            end_of_frame: false,
            mr_write: None,
            qp_write: None,
        };
        let mut stream = vec![
            In {
                mr_write: Some(MrWrite {
                    rkey: bits(0x0000_A5A5),
                    region: Some(crate::qp::mr::MemoryRegion {
                        rkey: bits(0x0000_A5A5),
                        va_base: bits(0x0000_7F00_1234_0000),
                        length: bits(4096),
                        local_base: bits(0x0002_0000),
                        remote_write: true,
                    }),
                }),
                ..idle
            },
            In {
                qp_write: Some(QpWrite {
                    qpn: bits(0x000111),
                    ctx: Some(crate::qp::context::QpContext {
                        dqpn: bits(0x000111),
                        transport: crate::headers::Transport::Uc,
                        pmtu: crate::qp::context::Pmtu::B1024,
                        rkey: bits(0x0000_A5A5),
                        pkey: bits(0xFFFF),
                    }),
                }),
                ..idle
            },
            idle,
            idle,
        ];
        let f = vector("uc_write_only_16");
        let last = f.len() - 1;
        for (k, b) in f.iter().enumerate() {
            stream.push(In {
                data: Some(bits(*b as u128)),
                start_of_frame: k == 0,
                end_of_frame: k == last,
                ..idle
            });
        }
        stream.push(idle);
        stream.into_iter().with_reset(1).clock_pos_edge(100)
    }

    /// The whole path places the vector's sixteen payload bytes at the
    /// translated addresses. The corpus-wide and transmitter-driven
    /// versions of this live in `tests/loopback.rs`; this is the
    /// in-module smoke test that the widget works at all.
    #[test]
    fn the_vector_frame_is_placed() {
        let uut: RoceWriteRx<4, 4> = RoceWriteRx::default();
        let mut writes = Vec::new();
        let mut outcomes = Vec::new();
        for s in uut.run(test_stream()).synchronous_sample() {
            if let Some(p) = s.output.write {
                writes.push((p.addr.raw(), p.data.raw() as u8));
            }
            if s.output.done {
                outcomes.push(s.output.outcome);
            }
        }
        assert_eq!(outcomes, vec![RxOutcome::Placed]);
        assert_eq!(writes.len(), 16);
        for (i, (addr, data)) in writes.iter().enumerate() {
            assert_eq!(*addr, 0x0002_0000 + i as u128, "byte {i}");
            assert_eq!(*data, i as u8, "the vector's payload is 00..0f");
        }
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut: RoceWriteRx<4, 4> = RoceWriteRx::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        assert!(hdl.contains("module top"));
        Ok(())
    }

    #[test]
    fn test_widget_hdl_works() -> miette::Result<()> {
        let uut: RoceWriteRx<4, 4> = RoceWriteRx::default();
        let tb = uut
            .run(test_stream())
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_widget_trace() -> miette::Result<()> {
        let uut: RoceWriteRx<4, 4> = RoceWriteRx::default();
        let vcd = uut.run(test_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("rx_endpoint");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["5dfe782ba7f555aa2a33cd7853717d0f826b8814b64e123ba1fe3dc30e410a04"];
        let digest = vcd.dump_to_file(root.join("rx_endpoint.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
