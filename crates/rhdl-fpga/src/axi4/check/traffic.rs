//! Bus traffic that breaks the protocol on purpose.
//!
//! Host-side generators, not widgets. Each produces a cycle-by-cycle
//! write-channel transcript that is either conformant or conformant
//! except for one named violation — which is what lets a monitor test
//! assert the monitor reported *that* fault, rather than merely that
//! something somewhere was wrong.
//!
//! # Why one violation at a time
//!
//! A transcript that broke several rules at once would let a monitor
//! pass by reporting any one of them, and would hide the case where two
//! checks are wired to the same flag. One injected fault per transcript
//! makes the expected verdict exact.
//!
//! Getting this wrong is easy and was gotten wrong here first: the
//! original `WValidDropped` generator withdrew a beat and then *did not
//! resend it*, so the burst also ended a beat short with `WLAST` in the
//! wrong place. It would have passed a monitor that had no handshake
//! check at all, on the strength of the `WLAST` check alone. The
//! injections below are each arranged to leave every *other* rule
//! intact, and [`Violation::expected`] is what a test compares against.
//!
//! # Two vocabularies
//!
//! [`Violation`] is what the generator *does*; [`super::rules::Fault`]
//! is what a monitor *reports*. They are deliberately separate types,
//! because several traffic shapes can break one rule —
//! [`Violation::BrespWithoutAw`] and [`Violation::BrespSurplus`] are
//! different bus behaviour and the same violated clause. Collapsing
//! them into one enum would have forced either a fake distinction in
//! the monitor or a fake merge in the generator.

use super::super::types::{
    AxiAddress, AxiReadBeat, AxiResponse, AxiWriteBeat, AxiWriteResponse, BurstKind,
};
use super::rules::Fault;
use rhdl::prelude::*;

/// A rule a transcript can break, named by how the traffic misbehaves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Violation {
    /// Conformant traffic. The control, and not a formality: without
    /// it a monitor that reported a fault on *every* transcript would
    /// pass every other test in the suite.
    None,
    /// `AWVALID` deasserted before `AWREADY` was ever seen.
    ///
    /// The spec's strongest handshake rule — once a beat is offered it
    /// stays offered until it is taken.
    AwValidDropped,
    /// `AWVALID` held, but the address changed while waiting.
    ///
    /// The subtler half of the same rule and the more dangerous: the
    /// handshake looks perfectly well formed, and the subordinate
    /// latches a different address from the one the manager believes it
    /// sent.
    AwPayloadChanged,
    /// `WVALID` deasserted before `WREADY`.
    WValidDropped,
    /// `WVALID` held, but the beat changed while waiting.
    WPayloadChanged,
    /// `WLAST` asserted before the final beat.
    WLastEarly,
    /// The final beat sent without `WLAST`.
    WLastMissing,
    /// `BVALID` deasserted before `BREADY`.
    ///
    /// The response channel's handshake binds the *subordinate*, and
    /// it is the one a manager-side test bench is least likely to
    /// exercise -- which is exactly how the monitor came to carry an
    /// untested check for it. See the `check` module docs.
    BValidDropped,
    /// `BVALID` held, but the response changed while waiting.
    BPayloadChanged,
    /// A write response for an ID with no transaction outstanding.
    BrespWithoutAw,
    /// Two responses for one write transaction.
    BrespSurplus,
    /// An `Incr` burst crossing a 4 KiB boundary.
    IncrCrosses4K,
    /// A `Wrap` burst whose length is not 2, 4, 8 or 16 beats.
    WrapBadLength,
    /// A `Wrap` burst whose start address is not size-aligned.
    WrapUnaligned,
    /// `AxSIZE` larger than the data bus.
    SizeExceedsBus,
    /// `WSTRB` asserting a byte lane the beat's address and size do not
    /// select.
    StrobeOutsideLanes,
}

impl Violation {
    /// The fault a correct monitor reports for this traffic.
    ///
    /// This is the test contract, and the reason the two enums are
    /// separate: the mapping is many-to-one.
    pub fn expected(self) -> Fault {
        match self {
            Violation::None => Fault::None,
            Violation::AwValidDropped => Fault::AwValidDropped,
            Violation::AwPayloadChanged => Fault::AwPayloadChanged,
            Violation::WValidDropped => Fault::WValidDropped,
            Violation::WPayloadChanged => Fault::WPayloadChanged,
            Violation::WLastEarly => Fault::WLastEarly,
            Violation::WLastMissing => Fault::WLastMissing,
            Violation::BValidDropped => Fault::BValidDropped,
            Violation::BPayloadChanged => Fault::BPayloadChanged,
            // Both shapes break the same clause: a response arrived
            // with nothing outstanding to respond to.
            Violation::BrespWithoutAw => Fault::BrespUnexpected,
            Violation::BrespSurplus => Fault::BrespUnexpected,
            Violation::IncrCrosses4K => Fault::IncrCrosses4K,
            Violation::WrapBadLength => Fault::WrapBadLength,
            Violation::WrapUnaligned => Fault::WrapUnaligned,
            Violation::SizeExceedsBus => Fault::SizeExceedsBus,
            Violation::StrobeOutsideLanes => Fault::StrobeOutsideLanes,
        }
    }

    /// Every violation, so a test can require all of them be caught.
    ///
    /// Written out rather than derived; the `match` in
    /// [`Violation::expected`] is what stops compiling if a variant is
    /// added, and a test asserts this list's length against it.
    pub fn all() -> Vec<Violation> {
        vec![
            Violation::None,
            Violation::AwValidDropped,
            Violation::AwPayloadChanged,
            Violation::WValidDropped,
            Violation::WPayloadChanged,
            Violation::WLastEarly,
            Violation::WLastMissing,
            Violation::BValidDropped,
            Violation::BPayloadChanged,
            Violation::BrespWithoutAw,
            Violation::BrespSurplus,
            Violation::IncrCrosses4K,
            Violation::WrapBadLength,
            Violation::WrapUnaligned,
            Violation::SizeExceedsBus,
            Violation::StrobeOutsideLanes,
        ]
    }
}

/// One cycle of the write channels, as a monitor sees them.
///
/// A monitor is a passive observer, so this carries both halves of
/// every handshake and drives nothing.
#[derive(Clone, Copy, PartialEq, Debug, Digital, Default)]
pub struct WriteCycle<const A: usize, const D: usize, const STRB: usize, const ID: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<D>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    /// `AWVALID`.
    pub aw_valid: bool,
    /// `AWREADY`.
    pub aw_ready: bool,
    /// The AW payload. Meaningful only while `aw_valid`.
    pub aw: AxiAddress<A, ID>,
    /// `WVALID`.
    pub w_valid: bool,
    /// `WREADY`.
    pub w_ready: bool,
    /// The W payload.
    pub w: AxiWriteBeat<D, STRB>,
    /// `BVALID`.
    pub b_valid: bool,
    /// `BREADY`.
    pub b_ready: bool,
    /// The B payload.
    pub b: AxiWriteResponse<ID>,
}

/// One conformant write burst with the named fault injected into it.
///
/// `bus_bytes` is the data bus width in bytes — `D / 8`, passed rather
/// than derived because `Bits<{ D / 8 }>` does not compile on stable
/// Rust (see `axi4/mod.rs`).
///
/// The shape is deliberately plain — a single four-beat aligned `Incr`
/// burst, both sides ready except where a fault needs a stall — because
/// the subject is the fault, not the throughput.
pub fn write_transcript<const A: usize, const D: usize, const STRB: usize, const ID: usize>(
    bus_bytes: u128,
    fault: Violation,
) -> Vec<WriteCycle<A, D, STRB, ID>>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<D>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    let size = bus_bytes.trailing_zeros() as u128;
    let beats: u128 = 4;
    let all_lanes = (1u128 << bus_bytes) - 1;

    let mut aw = AxiAddress::<A, ID> {
        id: bits(1),
        addr: bits(0x1000),
        len: bits(beats - 1),
        size: bits(size),
        burst: BurstKind::Incr,
        ..Default::default()
    };

    // Per-fault edits to the address phase. Each leaves the burst
    // otherwise legal.
    match fault {
        Violation::IncrCrosses4K => {
            // Start one beat short of the page end so four beats run
            // over it.
            aw.addr = bits(0x2000 - bus_bytes);
        }
        Violation::WrapBadLength => {
            aw.burst = BurstKind::Wrap;
            aw.len = bits(2); // three beats: not 2, 4, 8 or 16
        }
        Violation::WrapUnaligned => {
            aw.burst = BurstKind::Wrap;
            aw.addr = bits(0x1000 + 1); // not size-aligned
        }
        Violation::SizeExceedsBus => {
            aw.size = bits(size + 1);
        }
        Violation::StrobeOutsideLanes => {
            // A half-width transfer selects only the low lanes of the
            // bus; the data phase below then asserts all of them.
            aw.size = bits(size - 1);
        }
        _ => {}
    }

    let idle = WriteCycle::<A, D, STRB, ID>::default();
    let mut out: Vec<WriteCycle<A, D, STRB, ID>> = Vec::new();

    // ---- address phase
    match fault {
        Violation::AwValidDropped => {
            // Offered while the subordinate is not ready, then
            // withdrawn, then properly re-offered so that the rest of
            // the transcript stays well formed.
            let mut c = idle;
            c.aw_valid = true;
            c.aw = aw;
            out.push(c);
            out.push(idle); // withdrawn, never accepted: the fault
            let mut ok = c;
            ok.aw_ready = true;
            out.push(ok);
        }
        Violation::AwPayloadChanged => {
            let mut c = idle;
            c.aw_valid = true;
            c.aw = aw;
            out.push(c);
            // Still offered, still not accepted, different address.
            let mut c2 = c;
            c2.aw.addr = bits(0x3000);
            out.push(c2);
            let mut c3 = c2;
            c3.aw_ready = true;
            out.push(c3);
        }
        _ => {
            let mut c = idle;
            c.aw_valid = true;
            c.aw_ready = true;
            c.aw = aw;
            out.push(c);
        }
    }

    // ---- data phase: exactly `beats` accepted beats in every case, so
    // that a handshake fault does not also become a WLAST fault.
    for beat in 0..beats {
        let mut c = idle;
        c.w_valid = true;
        c.w_ready = true;
        c.w.data = bits(0xA0 + beat);
        // Every lane asserted. For the full-width bursts that is
        // correct; for the half-width burst that `StrobeOutsideLanes`
        // sets up it is exactly the fault.
        c.w.strb = bits(all_lanes);
        c.w.last = beat + 1 == beats;

        match fault {
            Violation::WLastEarly if beat == 1 => c.w.last = true,
            Violation::WLastMissing => c.w.last = false,
            Violation::WValidDropped if beat == 1 => {
                // Offer, stall, withdraw -- then fall through and send
                // the beat properly, so the burst is still four beats.
                let mut stalled = c;
                stalled.w_ready = false;
                out.push(stalled);
                out.push(idle);
            }
            Violation::WPayloadChanged if beat == 1 => {
                let mut stalled = c;
                stalled.w_ready = false;
                out.push(stalled);
                // Still offered, not accepted, different data.
                let mut changed = stalled;
                changed.w.data = bits(0xFF);
                out.push(changed);
                // Accept the changed beat; the burst stays four beats.
                let mut taken = changed;
                taken.w_ready = true;
                out.push(taken);
                continue;
            }
            _ => {}
        }
        out.push(c);
    }

    // ---- response phase
    match fault {
        Violation::BrespWithoutAw => {
            // The legitimate response, and then one for an ID that was
            // never addressed. The first keeps the transaction closed
            // so the surplus is the only fault.
            for id in [1u128, 7u128] {
                let mut c = idle;
                c.b_valid = true;
                c.b_ready = true;
                c.b.id = bits(id);
                c.b.resp = AxiResponse::Okay;
                out.push(c);
            }
        }
        Violation::BValidDropped => {
            // Offered while the manager is not ready, then withdrawn,
            // then delivered properly so the transaction still closes.
            let mut c = idle;
            c.b_valid = true;
            c.b.id = bits(1);
            c.b.resp = AxiResponse::Okay;
            out.push(c);
            out.push(idle); // withdrawn, never accepted: the fault
            let mut ok = c;
            ok.b_ready = true;
            out.push(ok);
        }
        Violation::BPayloadChanged => {
            let mut c = idle;
            c.b_valid = true;
            c.b.id = bits(1);
            c.b.resp = AxiResponse::Okay;
            out.push(c);
            // Still offered, still not accepted, different response.
            let mut c2 = c;
            c2.b.resp = AxiResponse::SlvErr;
            out.push(c2);
            let mut c3 = c2;
            c3.b_ready = true;
            out.push(c3);
        }
        Violation::BrespSurplus => {
            for _ in 0..2 {
                let mut c = idle;
                c.b_valid = true;
                c.b_ready = true;
                c.b.id = bits(1);
                c.b.resp = AxiResponse::Okay;
                out.push(c);
            }
        }
        _ => {
            let mut c = idle;
            c.b_valid = true;
            c.b_ready = true;
            c.b.id = bits(1);
            c.b.resp = AxiResponse::Okay;
            out.push(c);
        }
    }

    out.push(idle);
    out.push(idle);
    out
}

/// A read-channel rule a transcript can break.
///
/// Separate from [`Violation`] because the channels are separate
/// widgets with separate transcripts; both map into the one shared
/// [`Fault`] vocabulary via `expected()`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReadViolation {
    /// Conformant traffic. The control.
    None,
    /// `ARVALID` deasserted before `ARREADY` was ever seen.
    ArValidDropped,
    /// `ARVALID` held, but the address changed while waiting.
    ArPayloadChanged,
    /// `RVALID` deasserted before `RREADY`.
    ///
    /// Like the B channel's equivalent this binds the *subordinate*,
    /// and is the kind of rule a manager-shaped generator forgets to
    /// exercise. See the `check` module docs on what that cost.
    RValidDropped,
    /// `RVALID` held, but the beat changed while waiting.
    RPayloadChanged,
    /// `RLAST` asserted before the final beat.
    RLastEarly,
    /// The final beat returned without `RLAST`.
    RLastMissing,
    /// A read beat with no transaction outstanding.
    RBeatWithoutAr,
    /// A read beat whose `RID` is not the outstanding transaction's.
    RIdMismatch,
    /// An `Incr` burst crossing a 4 KiB boundary.
    IncrCrosses4K,
    /// A `Wrap` burst whose length is not 2, 4, 8 or 16 beats.
    WrapBadLength,
    /// A `Wrap` burst whose start address is not size-aligned.
    WrapUnaligned,
    /// `ARSIZE` larger than the data bus.
    SizeExceedsBus,
}

impl ReadViolation {
    /// The fault a correct read monitor reports for this traffic.
    pub fn expected(self) -> Fault {
        match self {
            ReadViolation::None => Fault::None,
            ReadViolation::ArValidDropped => Fault::ArValidDropped,
            ReadViolation::ArPayloadChanged => Fault::ArPayloadChanged,
            ReadViolation::RValidDropped => Fault::RValidDropped,
            ReadViolation::RPayloadChanged => Fault::RPayloadChanged,
            ReadViolation::RLastEarly => Fault::RLastEarly,
            ReadViolation::RLastMissing => Fault::RLastMissing,
            ReadViolation::RBeatWithoutAr => Fault::RBeatUnexpected,
            ReadViolation::RIdMismatch => Fault::RIdMismatch,
            ReadViolation::IncrCrosses4K => Fault::IncrCrosses4K,
            ReadViolation::WrapBadLength => Fault::WrapBadLength,
            ReadViolation::WrapUnaligned => Fault::WrapUnaligned,
            ReadViolation::SizeExceedsBus => Fault::SizeExceedsBus,
        }
    }

    /// Every read violation, so a test can require all be caught.
    pub fn all() -> Vec<ReadViolation> {
        vec![
            ReadViolation::None,
            ReadViolation::ArValidDropped,
            ReadViolation::ArPayloadChanged,
            ReadViolation::RValidDropped,
            ReadViolation::RPayloadChanged,
            ReadViolation::RLastEarly,
            ReadViolation::RLastMissing,
            ReadViolation::RBeatWithoutAr,
            ReadViolation::RIdMismatch,
            ReadViolation::IncrCrosses4K,
            ReadViolation::WrapBadLength,
            ReadViolation::WrapUnaligned,
            ReadViolation::SizeExceedsBus,
        ]
    }
}

/// One cycle of the read channels, as a monitor sees them.
#[derive(Clone, Copy, PartialEq, Debug, Digital, Default)]
pub struct ReadCycle<const A: usize, const DW: usize, const ID: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    /// `ARVALID`.
    pub ar_valid: bool,
    /// `ARREADY`.
    pub ar_ready: bool,
    /// The AR payload. Meaningful only while `ar_valid`.
    pub ar: AxiAddress<A, ID>,
    /// `RVALID`.
    pub r_valid: bool,
    /// `RREADY`.
    pub r_ready: bool,
    /// The R payload.
    pub r: AxiReadBeat<DW, ID>,
}

/// One conformant read burst with the named fault injected into it.
///
/// Same shape and the same discipline as [`write_transcript`]: a single
/// four-beat aligned `Incr` burst, one injected fault, and every other
/// rule left intact so the expected verdict is exact.
pub fn read_transcript<const A: usize, const DW: usize, const ID: usize>(
    bus_bytes: u128,
    fault: ReadViolation,
) -> Vec<ReadCycle<A, DW, ID>>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    let size = bus_bytes.trailing_zeros() as u128;
    let beats: u128 = 4;

    let mut ar = AxiAddress::<A, ID> {
        id: bits(1),
        addr: bits(0x1000),
        len: bits(beats - 1),
        size: bits(size),
        burst: BurstKind::Incr,
        ..Default::default()
    };

    match fault {
        ReadViolation::IncrCrosses4K => ar.addr = bits(0x2000 - bus_bytes),
        ReadViolation::WrapBadLength => {
            ar.burst = BurstKind::Wrap;
            ar.len = bits(2); // three beats: not 2, 4, 8 or 16
        }
        ReadViolation::WrapUnaligned => {
            ar.burst = BurstKind::Wrap;
            ar.addr = bits(0x1000 + 1);
        }
        ReadViolation::SizeExceedsBus => ar.size = bits(size + 1),
        _ => {}
    }

    let idle = ReadCycle::<A, DW, ID>::default();
    let mut out: Vec<ReadCycle<A, DW, ID>> = Vec::new();

    // ---- address phase
    match fault {
        ReadViolation::ArValidDropped => {
            let mut c = idle;
            c.ar_valid = true;
            c.ar = ar;
            out.push(c);
            out.push(idle); // withdrawn, never accepted: the fault
            let mut ok = c;
            ok.ar_ready = true;
            out.push(ok);
        }
        ReadViolation::ArPayloadChanged => {
            let mut c = idle;
            c.ar_valid = true;
            c.ar = ar;
            out.push(c);
            let mut c2 = c;
            c2.ar.addr = bits(0x3000);
            out.push(c2);
            let mut c3 = c2;
            c3.ar_ready = true;
            out.push(c3);
        }
        _ => {
            let mut c = idle;
            c.ar_valid = true;
            c.ar_ready = true;
            c.ar = ar;
            out.push(c);
        }
    }

    // ---- data phase: four accepted beats in every case, so that a
    // handshake fault does not also become an RLAST fault.
    for beat in 0..beats {
        let mut c = idle;
        c.r_valid = true;
        c.r_ready = true;
        c.r.id = bits(1);
        c.r.data = bits(0xB0 + beat);
        c.r.resp = AxiResponse::Okay;
        c.r.last = beat + 1 == beats;

        match fault {
            ReadViolation::RLastEarly if beat == 1 => c.r.last = true,
            ReadViolation::RLastMissing => c.r.last = false,
            ReadViolation::RIdMismatch if beat == 1 => c.r.id = bits(2),
            ReadViolation::RValidDropped if beat == 1 => {
                // Offer, stall, withdraw -- then fall through and send
                // the beat properly, so the burst is still four beats.
                let mut stalled = c;
                stalled.r_ready = false;
                out.push(stalled);
                out.push(idle);
            }
            ReadViolation::RPayloadChanged if beat == 1 => {
                let mut stalled = c;
                stalled.r_ready = false;
                out.push(stalled);
                let mut changed = stalled;
                changed.r.data = bits(0xFF);
                out.push(changed);
                let mut taken = changed;
                taken.r_ready = true;
                out.push(taken);
                continue;
            }
            _ => {}
        }
        out.push(c);
    }

    // ---- a surplus beat after the transaction has retired
    if fault == ReadViolation::RBeatWithoutAr {
        let mut c = idle;
        c.r_valid = true;
        c.r_ready = true;
        c.r.id = bits(1);
        c.r.resp = AxiResponse::Okay;
        c.r.last = true;
        out.push(c);
    }

    out.push(idle);
    out.push(idle);
    out
}
