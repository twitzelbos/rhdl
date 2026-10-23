//! A passive AXI4 write-channel protocol monitor — validation Tier A.
//!
//! Observes AW, W and B without driving anything and latches the first
//! rule the traffic breaks. It is the fast oracle of
//! `axi4-architecture.md` §7: cheap enough to leave switched on inside
//! every other AXI4 test in the crate.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
      +-+Axi4WriteMonitor+---------+
      |                            |
 Write|                      fault |  Fault
+---->| cycle                      +------->
      |                     faults |  b16
      |                            +------->
      |            unchecked_beats |  b16
      |                            +------->
      +----------------------------+
")]
//!
//! # What it is not
//!
//! It is **not** an independent oracle. It is RHDL checking RHDL, so a
//! misreading of the spec shared between this monitor and a widget
//! under test passes silently. That is the whole reason
//! `axi4-architecture.md` §7 specifies a second, hand-written
//! plain-Verilog monitor (Tier B) rather than stopping here, and the
//! reason the suite is driven by
//! [`super::traffic`]'s deliberately broken transcripts rather than by
//! traffic this crate also generates correctly.
//!
//! # The blind spot, counted rather than hidden
//!
//! AXI4 permits write data to arrive **before** its address. A beat
//! seen with no address latched therefore cannot be position-checked —
//! the monitor does not know which burst it belongs to or how long that
//! burst is. Rather than skip such beats silently, it counts them in
//! `unchecked_beats`, so a test can assert the number of beats its
//! checks actually applied to. A monitor whose coverage gap is a number
//! on an output port is auditable; one whose gap is an unwritten `else`
//! is not. Buffering early write data to close the gap properly needs
//! the outstanding-transaction table, which is Phase 4's work.
//!
//! The strobe check models the reduced first beat of an **unaligned**
//! burst: beat zero covers only from `AxADDR` to the end of its
//! size-aligned block, so the lanes below the start address must stay
//! dark. Before Phase 3 the low lane was aligned down unconditionally,
//! which made the monitor permissive exactly there — it would have
//! accepted a first beat writing bytes the manager was never asked to
//! write. An unaligned `Wrap` is still a [`Fault::WrapUnaligned`],
//! because the spec forbids it outright.
//!
//! # Per-ID tracking
//!
//! Responses are matched **by ID**: one outstanding count per `AWID`,
//! indexed by the full ID so nothing aliases. So a response bearing an
//! ID that was never addressed is caught even while other transactions
//! are in flight, and responses completing out of order across IDs are
//! *accepted* rather than merely tolerated — each ID has its own count,
//! so the order they return in does not matter.
//!
//! Phase 1 kept a single global count, which could only catch a surplus
//! response while nothing at all was outstanding. `NID` must be
//! `1 << ID`; [`Axi4WriteMonitor::check_widths`] enforces it.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/axi4_write_monitor.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/axi4_write_monitor.md")]

use super::super::types::{
    AxiAddress, AxiWriteBeat, AxiWriteResponse, beat_address, burst_beats, bytes_per_beat,
    size_fits_bus,
};
use super::rules::{Fault, address_fault, first_fault};
use super::traffic::WriteCycle;
use crate::core::dff;
use rhdl::prelude::*;

/// The monitor's internal registers, bundled.
///
/// One `Digital` struct in one `DFF` rather than a dozen sibling
/// `DFF`s, per CLAUDE.md §3.1: the auto-generated `Q`/`D` tuples stop
/// at 12 elements, and these are one observer's state, not a dozen
/// sub-circuits.
#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct State<
    const A: usize,
    const DW: usize,
    const STRB: usize,
    const ID: usize,
    const NID: usize,
> where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    /// Has any cycle been observed? Gates the stability checks, which
    /// are comparisons against a previous cycle that does not exist on
    /// the first one.
    seen: bool,
    /// Last cycle's AW handshake and payload.
    p_aw_valid: bool,
    p_aw_ready: bool,
    p_aw: AxiAddress<A, ID>,
    /// Last cycle's W handshake and payload.
    p_w_valid: bool,
    p_w_ready: bool,
    p_w: AxiWriteBeat<DW, STRB>,
    /// Last cycle's B handshake and payload.
    p_b_valid: bool,
    p_b_ready: bool,
    p_b: AxiWriteResponse<ID>,
    /// Is a write burst's address latched and its data phase in flight?
    aw_active: bool,
    /// The latched address phase of the burst in flight.
    aw: AxiAddress<A, ID>,
    /// Beats of the burst in flight already accepted.
    beat: Bits<8>,
    /// Transactions addressed but not yet responded to, **per ID**.
    ///
    /// Indexed by the full `AWID`, so nothing aliases: `NID` is
    /// `1 << ID`. Phase 1 kept a single global count, which could only
    /// catch a surplus response while *nothing* was outstanding — a
    /// response bearing an ID never addressed passed unnoticed whenever
    /// any other transaction was in flight. Per-ID counts are what make
    /// out-of-order completion across IDs checkable rather than merely
    /// tolerated.
    outstanding: [Bits<8>; NID],
    /// The first fault seen.
    fault: Fault,
    /// How many faults were seen in total.
    faults: Bits<16>,
    /// Write beats the position checks could not be applied to.
    unchecked: Bits<16>,
}

impl<const A: usize, const DW: usize, const STRB: usize, const ID: usize, const NID: usize> Default
    for State<A, DW, STRB, ID, NID>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    fn default() -> Self {
        // Written out because `std` has no blanket `[T; N]: Default`
        // for a generic `N`, only fixed sizes.
        Self {
            seen: false,
            p_aw_valid: false,
            p_aw_ready: false,
            p_aw: AxiAddress::default(),
            p_w_valid: false,
            p_w_ready: false,
            p_w: AxiWriteBeat::default(),
            p_b_valid: false,
            p_b_ready: false,
            p_b: AxiWriteResponse::default(),
            aw_active: false,
            aw: AxiAddress::default(),
            beat: bits(0),
            outstanding: [bits(0); NID],
            fault: Fault::default(),
            faults: bits(0),
            unchecked: bits(0),
        }
    }
}

/// What the monitor reports.
#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct Out {
    /// The first rule broken, or [`Fault::None`].
    pub fault: Fault,
    /// How many faults were observed, counting repeats.
    pub faults: Bits<16>,
    /// Write beats seen with no address latched, and so not
    /// position-checked. See the module docs.
    pub unchecked_beats: Bits<16>,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// A passive write-channel protocol monitor.
///
/// `A` is the address width, `DW` the data width, `STRB = DW / 8` and
/// `ID` the transaction-ID width.
///
/// The data width is `DW` and not `D` as everywhere else in this
/// module tree because `#[rhdl(dq_no_prefix)]` generates types *named*
/// `Q` and `D`, and a const generic called `D` shadows the generated
/// one. The error is a confusing `type provided when a constant was
/// expected` pointing at the widget's own field list.
pub struct Axi4WriteMonitor<
    const A: usize,
    const DW: usize,
    const STRB: usize,
    const ID: usize,
    const NID: usize,
> where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    state: dff::DFF<State<A, DW, STRB, ID, NID>>,
}

impl<const A: usize, const DW: usize, const STRB: usize, const ID: usize, const NID: usize> Default
    for Axi4WriteMonitor<A, DW, STRB, ID, NID>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    fn default() -> Self {
        Self {
            state: dff::DFF::new(State::dont_care()),
        }
    }
}

impl<const A: usize, const DW: usize, const STRB: usize, const ID: usize, const NID: usize>
    Axi4WriteMonitor<A, DW, STRB, ID, NID>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    /// The invariant the type system cannot state: `NID == 1 << ID`.
    ///
    /// One outstanding-count slot per possible ID, so nothing aliases.
    /// Force it with
    /// `const _: () = Axi4WriteMonitor::<32, 32, 4, 4, 16>::check_widths();`.
    pub const fn check_widths() {
        assert!(1 << ID == NID, "NID must be 1 << ID");
    }
}

impl<const A: usize, const DW: usize, const STRB: usize, const ID: usize, const NID: usize>
    SynchronousIO for Axi4WriteMonitor<A, DW, STRB, ID, NID>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    type I = WriteCycle<A, DW, STRB, ID>;
    type O = Out;
    type Kernel = write_monitor<A, DW, STRB, ID, NID>;
}

#[kernel]
/// Kernel for [Axi4WriteMonitor].
#[allow(clippy::too_many_lines)]
pub fn write_monitor<
    const A: usize,
    const DW: usize,
    const STRB: usize,
    const ID: usize,
    const NID: usize,
>(
    cr: ClockReset,
    i: WriteCycle<A, DW, STRB, ID>,
    q: Q<A, DW, STRB, ID, NID>,
) -> (Out, D<A, DW, STRB, ID, NID>)
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    let s = q.state;
    let mut n = s;

    let bus_bytes = bits::<9>(STRB as u128);

    let mut fault = s.fault;
    let mut hits = bits::<16>(0);

    // ---- handshake stability: a beat offered and not accepted must
    // still be offered, unchanged, next cycle. Checked against the
    // registered previous cycle, so it is skipped on the first one.
    let aw_held = s.seen && s.p_aw_valid && !s.p_aw_ready;
    let aw_dropped = aw_held && !i.aw_valid;
    let aw_changed = aw_held && i.aw_valid && i.aw != s.p_aw;
    let w_held = s.seen && s.p_w_valid && !s.p_w_ready;
    let w_dropped = w_held && !i.w_valid;
    let w_changed = w_held && i.w_valid && i.w != s.p_w;
    let b_held = s.seen && s.p_b_valid && !s.p_b_ready;
    let b_dropped = b_held && !i.b_valid;
    let b_changed = b_held && i.b_valid && i.b != s.p_b;

    // ---- address phase legality, checked as the AW is accepted, by
    // the rules AW and AR share. Written once in `rules` rather than
    // twice: two copies would be two places to fix a misreading, and a
    // drift between them would leave both monitors' tests passing.
    let aw_fire = i.aw_valid && i.aw_ready;
    let addr_fault = address_fault::<A, ID>(i.aw, bus_bytes);
    let addr_bad = aw_fire
        && match addr_fault {
            Fault::None => false,
            _ => true,
        };

    // ---- data phase
    let w_fire = i.w_valid && i.w_ready;
    let checked = w_fire && s.aw_active;
    let unchecked = w_fire && !s.aw_active;
    let last_beat = s.beat == burst_beats(s.aw.len).resize::<8>() - bits::<8>(1);
    let last_early = checked && i.w.last && !last_beat;
    let last_missing = checked && !i.w.last && last_beat;

    // ---- strobe lanes. The legal mask is the transfer's bytes, placed
    // at the beat address's offset within the bus.
    //
    // Computed in 33 bits so that neither shift can run off the end:
    // `safe_bytes <= 16` and `lane <= 15`, and the product of the two
    // shifts reaches at most bit 32. Per CLAUDE.md §4 the *operand* is
    // clamped rather than the result, because both arms of an `if`
    // evaluate in hardware and in the kernel VM -- an unclamped
    // `1 << safe_bytes` for an oversized `AxSIZE` would trip the VM's
    // shift bound on traffic the monitor exists to report.
    let fits = size_fits_bus(s.aw.size, bus_bytes);
    let tb = bytes_per_beat(s.aw.size);
    let safe_bytes = if fits { tb } else { bits::<9>(1) }.resize::<33>();
    let addr = beat_address::<A>(s.aw.addr, s.aw.size, s.aw.len, s.aw.burst, s.beat);
    let bus_mask = bus_bytes.resize::<33>() - bits::<33>(1);

    // The size-aligned block this beat sits in, as a lane range.
    // `beat_address` already returns the *aligned* address, so this is
    // the block's low lane and `hi` its high lane.
    let base = addr.resize::<33>() & bus_mask & !(safe_bytes - bits::<33>(1));
    let hi = base + safe_bytes - bits::<33>(1);

    // An unaligned burst's **first** beat is narrower than its size: it
    // covers only from `AxADDR` up to the end of that block, and the
    // lanes below the start address must stay dark. Aligning the low
    // lane down unconditionally -- which is what this did before Phase
    // 3 -- made the monitor *permissive* exactly there: it would accept
    // a first beat that wrote bytes the manager was never asked to
    // write. Later beats are size-aligned by construction, so `base`
    // is already right for them.
    let start_lane = s.aw.addr.resize::<33>() & bus_mask;
    let first_beat = s.beat == bits::<8>(0);
    let lo = if first_beat && start_lane > base {
        start_lane
    } else {
        base
    };
    let width = hi - lo + bits::<33>(1);
    let ones = (bits::<33>(1) << width) - bits::<33>(1);
    let legal = (ones << lo).resize::<STRB>();
    // Suppressed when `AxSIZE` does not fit the bus, because then the
    // lane check has no defined answer: the clamp above is an arbitrary
    // choice made to keep the shift in bounds, not a reading of the
    // spec. The two monitors clamped differently -- this one to one
    // byte, the Verilog one to the full bus -- so an illegal `AxSIZE`
    // drew five faults from Tier A and one from Tier B. Neither count
    // was right. The `AxSIZE` fault itself is already reported, and a
    // lane verdict derived from an illegal size would be noise on top
    // of it.
    let strobe_bad = checked && fits && (i.w.strb & !legal) != bits::<STRB>(0);

    // ---- response phase
    let b_fire = i.b_valid && i.b_ready;
    // Keyed on the response's own ID, so a response for an ID that was
    // never addressed is caught even while other transactions are in
    // flight -- and responses completing out of order across IDs are
    // accepted, because each ID carries its own count.
    let b_slot = i.b.id.resize::<ID>();
    let bresp_bad = b_fire && s.outstanding[b_slot] == bits::<8>(0);

    // Rules in order. `first_fault` keeps the earliest; the order here
    // only decides which of several faults *in one cycle* is named.
    fault = first_fault(fault, aw_dropped, Fault::AwValidDropped);
    fault = first_fault(fault, aw_changed, Fault::AwPayloadChanged);
    fault = first_fault(fault, w_dropped, Fault::WValidDropped);
    fault = first_fault(fault, w_changed, Fault::WPayloadChanged);
    fault = first_fault(fault, b_dropped, Fault::BValidDropped);
    fault = first_fault(fault, b_changed, Fault::BPayloadChanged);
    fault = first_fault(fault, addr_bad, addr_fault);
    fault = first_fault(fault, last_early, Fault::WLastEarly);
    fault = first_fault(fault, last_missing, Fault::WLastMissing);
    fault = first_fault(fault, strobe_bad, Fault::StrobeOutsideLanes);
    fault = first_fault(fault, bresp_bad, Fault::BrespUnexpected);

    if aw_dropped {
        hits += bits::<16>(1);
    }
    if aw_changed {
        hits += bits::<16>(1);
    }
    if w_dropped {
        hits += bits::<16>(1);
    }
    if w_changed {
        hits += bits::<16>(1);
    }
    if b_dropped {
        hits += bits::<16>(1);
    }
    if b_changed {
        hits += bits::<16>(1);
    }
    if addr_bad {
        hits += bits::<16>(1);
    }
    if last_early {
        hits += bits::<16>(1);
    }
    if last_missing {
        hits += bits::<16>(1);
    }
    if strobe_bad {
        hits += bits::<16>(1);
    }
    if bresp_bad {
        hits += bits::<16>(1);
    }

    // ---- next state, every field assigned exactly once and
    // unconditionally. Assigning under an `if` would make every field
    // of `d` depend on that condition -- harmless for a lone DFF, and
    // the habit that caused a spurious combinational cycle in
    // `net::udp_rx`.
    n.seen = true;
    n.p_aw_valid = i.aw_valid;
    n.p_aw_ready = i.aw_ready;
    n.p_aw = i.aw;
    n.p_w_valid = i.w_valid;
    n.p_w_ready = i.w_ready;
    n.p_w = i.w;
    n.p_b_valid = i.b_valid;
    n.p_b_ready = i.b_ready;
    n.p_b = i.b;

    // The burst in flight: latched on AW, retired on the beat carrying
    // `WLAST`.
    let w_closes = checked && i.w.last;
    n.aw = if aw_fire { i.aw } else { s.aw };
    n.aw_active = if aw_fire {
        true
    } else {
        s.aw_active && !w_closes
    };
    // A new address and a closing burst both restart the count.
    n.beat = if aw_fire || w_closes {
        bits::<8>(0)
    } else if checked {
        s.beat + bits::<8>(1)
    } else {
        s.beat
    };

    // Outstanding transactions, saturating at both ends: a response
    // with nothing outstanding is reported above, and must not
    // underflow the counter into 255 and swallow the next 255 checks.
    // Each ID's count moves independently. Applied as two separate
    // updates rather than one net figure because the AW and the B of a
    // single cycle may name *different* IDs.
    let aw_slot = i.aw.id.resize::<ID>();
    if aw_fire {
        n.outstanding[aw_slot] = s.outstanding[aw_slot] + bits::<8>(1);
    }
    if b_fire && n.outstanding[b_slot] != bits::<8>(0) {
        // Saturating at zero: a surplus response is reported above and
        // must not underflow into 255, which would swallow the next 255
        // checks for that ID.
        n.outstanding[b_slot] -= bits::<8>(1);
    }

    n.fault = fault;
    n.faults = s.faults + hits;
    n.unchecked = if unchecked {
        s.unchecked + bits::<16>(1)
    } else {
        s.unchecked
    };

    let mut o = Out::dont_care();
    o.fault = fault;
    o.faults = n.faults;
    o.unchecked_beats = n.unchecked;

    let mut d = D::<A, DW, STRB, ID, NID>::dont_care();
    d.state = n;

    if cr.reset.any() {
        d.state = State::<A, DW, STRB, ID, NID>::default();
        o.fault = Fault::None;
        o.faults = bits::<16>(0);
        o.unchecked_beats = bits::<16>(0);
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axi4::check::traffic::{
        Violation, write_transcript, write_transcript_two_ids_out_of_order,
    };
    use crate::axi4::types::BurstKind;
    use expect_test::expect;
    use rhdl::prelude::vlog::Pretty;

    // A 32-bit bus: four byte lanes.
    type Mon = Axi4WriteMonitor<32, 32, 4, 4, 16>;
    const BUS_BYTES: u128 = 4;

    /// Run a transcript through the monitor and report its verdict.
    fn verdict(v: Violation) -> (Fault, u128, u128) {
        let uut = Mon::default();
        let cycles = write_transcript::<32, 32, 4, 4>(BUS_BYTES, v);
        let input = cycles.into_iter().with_reset(1).clock_pos_edge(100);
        let out = uut
            .run(input)
            .synchronous_sample()
            .map(|x| x.output)
            .collect::<Vec<_>>();
        let last = *out.last().unwrap();
        (last.fault, last.faults.raw(), last.unchecked_beats.raw())
    }

    /// **The Phase 1 gate.** Every deliberately broken transcript must
    /// draw exactly the fault it was built to break, and the
    /// conformant control must draw none.
    ///
    /// This is the test that makes the monitor worth anything. A
    /// checker validated only against traffic it also considers legal
    /// passes by doing nothing at all.
    #[test]
    fn every_injected_violation_is_caught() {
        let mut wrong = Vec::new();
        for v in Violation::all() {
            let (got, _, _) = verdict(v);
            if got != v.expected() {
                wrong.push(format!("{v:?}: expected {:?}, got {got:?}", v.expected()));
            }
        }
        assert!(
            wrong.is_empty(),
            "monitor verdicts wrong:\n{}",
            wrong.join("\n")
        );
    }

    /// The conformant control draws no fault *and* leaves no beat
    /// unchecked.
    ///
    /// The second half matters as much as the first: a monitor that
    /// classified every beat as unposition-checkable would report no
    /// fault on every transcript and pass the gate above for the wrong
    /// reason. Pinning `unchecked_beats` to zero is what rules that
    /// out.
    #[test]
    fn conformant_traffic_is_fully_checked_and_clean() {
        let (fault, faults, unchecked) = verdict(Violation::None);
        assert_eq!(fault, Fault::None);
        assert_eq!(faults, 0);
        assert_eq!(unchecked, 0, "a conformant burst left beats unchecked");
    }

    /// **Phase 4's gate, half one.** Two IDs completing out of order
    /// draw no fault.
    ///
    /// §8 asks for "two IDs completing out of order". AXI4 permits
    /// write responses to complete out of order across IDs, so the
    /// monitor must *accept* this — and the `unchecked_beats == 0`
    /// half of the assertion is what stops it being accepted for the
    /// wrong reason, by a monitor that had simply stopped looking.
    ///
    /// Note what makes this a real test only after Phase 4: a monitor
    /// keeping one global count accepts out-of-order completion by
    /// accident, because it never examines which ID a response names.
    /// The companion negative is
    /// `Violation::BrespForeignIdWhileBusy`, which that monitor
    /// accepted too — wrongly.
    #[test]
    fn two_ids_completing_out_of_order_is_accepted() {
        let uut = Mon::default();
        let cycles = write_transcript_two_ids_out_of_order::<32, 32, 4, 4>(BUS_BYTES);
        let input = cycles.into_iter().with_reset(1).clock_pos_edge(100);
        let out = uut
            .run(input)
            .synchronous_sample()
            .map(|x| x.output)
            .collect::<Vec<_>>();
        let last = *out.last().unwrap();
        assert_eq!(last.fault, Fault::None, "out-of-order across IDs is legal");
        assert_eq!(last.faults.raw(), 0);
        assert_eq!(
            last.unchecked_beats.raw(),
            0,
            "and every beat was actually checked"
        );
    }

    /// `Violation::all` really does list every variant.
    ///
    /// `expected()`'s `match` stops compiling when a variant is added,
    /// but nothing forces the new variant into `all()` -- and a
    /// violation missing from `all()` is a rule the gate silently stops
    /// testing. Pinning the count is the cheap guard.
    #[test]
    fn violation_list_is_complete() {
        expect!["18"].assert_eq(&Violation::all().len().to_string());
    }

    /// Tier 1: reset clears every latched verdict.
    ///
    /// A monitor that held a fault across reset would report the
    /// previous test's traffic, which is the kind of bug that makes a
    /// whole suite look broken at once.
    #[test]
    fn reset_clears_the_verdict() {
        let s = State::<32, 32, 4, 4, 16> {
            fault: Fault::WLastEarly,
            faults: bits(3),
            unchecked: bits(2),
            ..Default::default()
        };
        let q = Q::<32, 32, 4, 4, 16> { state: s };
        let i = WriteCycle::<32, 32, 4, 4>::default();
        let cr = clock_reset(clock(true), reset(true));
        let (o, d) = write_monitor::<32, 32, 4, 4, 16>(cr, i, q);
        assert_eq!(o.fault, Fault::None);
        assert_eq!(o.faults.raw(), 0);
        assert_eq!(o.unchecked_beats.raw(), 0);
        assert_eq!(d.state, State::<32, 32, 4, 4, 16>::default());
    }

    /// Tier 1: accepting an address latches the burst and zeroes the
    /// beat counter.
    #[test]
    fn accepting_an_address_latches_the_burst() {
        let i = WriteCycle::<32, 32, 4, 4> {
            aw_valid: true,
            aw_ready: true,
            aw: AxiAddress {
                // An explicit, non-zero ID: the count is now per-ID, so
                // which slot it lands in is part of what is tested.
                id: bits(1),
                addr: bits(0x40),
                len: bits(3),
                size: bits(2),
                burst: BurstKind::Incr,
                ..Default::default()
            },
            ..Default::default()
        };
        let q = Q::<32, 32, 4, 4, 16> {
            state: State::default(),
        };
        let cr = clock_reset(clock(true), reset(false));
        let (o, d) = write_monitor::<32, 32, 4, 4, 16>(cr, i, q);
        assert_eq!(o.fault, Fault::None);
        assert!(d.state.aw_active);
        assert_eq!(d.state.beat.raw(), 0);
        assert_eq!(d.state.aw.addr.raw(), 0x40);
        assert_eq!(d.state.outstanding[1].raw(), 1, "counted against AWID 1");
    }

    /// Tier 1: `AxSIZE` wider than the bus is caught as the address is
    /// accepted, and the strobe check does not trip the kernel VM's
    /// shift bound on the way past.
    ///
    /// The second half is the point. Per CLAUDE.md §4 both arms of an
    /// `if` evaluate, so an unclamped `1 << bytes_per_beat(size)` would
    /// have been evaluated for the oversized size this test exists to
    /// report -- passing a direct Rust call and failing only in the VM.
    #[test]
    fn oversized_size_is_caught_without_tripping_the_shift_bound() {
        let oversized = AxiAddress::<32, 4> {
            size: bits(7), // 128 bytes on a 4-byte bus
            burst: BurstKind::Incr,
            ..Default::default()
        };
        let i = WriteCycle::<32, 32, 4, 4> {
            aw_valid: true,
            aw_ready: true,
            aw: oversized,
            ..Default::default()
        };
        let s = State::<32, 32, 4, 4, 16> {
            aw_active: true,
            aw: oversized,
            ..Default::default()
        };
        let q = Q::<32, 32, 4, 4, 16> { state: s };
        let cr = clock_reset(clock(true), reset(false));
        let (o, _d) = write_monitor::<32, 32, 4, 4, 16>(cr, i, q);
        assert_eq!(o.fault, Fault::SizeExceedsBus);
    }

    /// Tier 1: a narrow transfer selects only its own lanes, so a beat
    /// asserting every lane is a strobe fault.
    #[test]
    fn narrow_transfer_rejects_lanes_outside_its_own() {
        let s = State::<32, 32, 4, 4, 16> {
            aw_active: true,
            aw: AxiAddress {
                addr: bits(0x40),
                len: bits(3),
                size: bits(1), // two bytes of a four-byte bus
                burst: BurstKind::Incr,
                ..Default::default()
            },
            beat: bits(0),
            ..Default::default()
        };
        let q = Q::<32, 32, 4, 4, 16> { state: s };
        let cr = clock_reset(clock(true), reset(false));

        // Lanes 0 and 1 are the selected ones at address 0x40.
        let ok = WriteCycle::<32, 32, 4, 4> {
            w_valid: true,
            w_ready: true,
            w: AxiWriteBeat {
                strb: bits(0b0011),
                ..Default::default()
            },
            ..Default::default()
        };
        let (o, _) = write_monitor::<32, 32, 4, 4, 16>(cr, ok, q);
        assert_eq!(o.fault, Fault::None);

        let mut bad = ok;
        bad.w.strb = bits(0b1111);
        let (o, _) = write_monitor::<32, 32, 4, 4, 16>(cr, bad, q);
        assert_eq!(o.fault, Fault::StrobeOutsideLanes);
    }

    /// Tier 1: a write beat with no address latched is counted, not
    /// silently dropped.
    ///
    /// AXI4 permits write data before its address, so the monitor
    /// cannot position-check such a beat. The contract is that it says
    /// so on an output port rather than skipping it.
    #[test]
    fn a_beat_with_no_address_is_counted_as_unchecked() {
        let i = WriteCycle::<32, 32, 4, 4> {
            w_valid: true,
            w_ready: true,
            w: AxiWriteBeat {
                strb: bits(0b1111),
                ..Default::default()
            },
            ..Default::default()
        };
        let q = Q::<32, 32, 4, 4, 16> {
            state: State::default(),
        };
        let cr = clock_reset(clock(true), reset(false));
        let (o, _d) = write_monitor::<32, 32, 4, 4, 16>(cr, i, q);
        assert_eq!(
            o.fault,
            Fault::None,
            "an unpositionable beat is not a fault"
        );
        assert_eq!(o.unchecked_beats.raw(), 1);
    }

    /// Tier 1: a response with nothing outstanding does not underflow
    /// the counter.
    ///
    /// If it wrapped to 255 the monitor would then accept 255 further
    /// spurious responses in silence -- a single fault disabling the
    /// check that found it.
    #[test]
    fn a_surplus_response_does_not_underflow_the_counter() {
        let i = WriteCycle::<32, 32, 4, 4> {
            b_valid: true,
            b_ready: true,
            ..Default::default()
        };
        let q = Q::<32, 32, 4, 4, 16> {
            state: State::default(),
        };
        let cr = clock_reset(clock(true), reset(false));
        let (o, d) = write_monitor::<32, 32, 4, 4, 16>(cr, i, q);
        assert_eq!(o.fault, Fault::BrespUnexpected);
        assert_eq!(d.state.outstanding[0].raw(), 0, "counter must not wrap");
    }

    /// Tier 3: the emitted Verilog's **interface** is the contract.
    ///
    /// The body is 893 lines, nearly five times the largest snapshot
    /// anywhere else in this crate (`core::crc32`, at 187), so pinning
    /// it inline would make this file mostly generated Verilog and make
    /// every read of the diff a skim. What is pinned instead is the part
    /// users actually depend on: the module headers, which carry the
    /// bit-packing of `In` and `Out`. RHDL emits a struct port as one
    /// packed vector, so those widths are the ABI.
    ///
    /// Three things move the numbers, and the third is the interesting
    /// one: a field added to `WriteCycle` or `Out`; a change to the
    /// compiler's packing; and **a variant added to the shared
    /// [`Fault`] enum**. Giving the read monitor its own faults took
    /// `Fault` past sixteen variants, so it now costs five bits rather
    /// than four, and this widget's output grew a bit for faults it can
    /// never itself report. That is the price of one shared vocabulary
    /// across both monitors, and for a verification widget it is the
    /// right trade -- the shared enum is what lets one `code_of`
    /// mapping and one coverage test cover both channels.
    ///
    /// Behavioural regressions are covered where they belong: Tier 4
    /// below diffs every signal of every cycle against the Rust
    /// simulation, and Tier 5 digests the waveform.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Mon::default();
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        let headers = hdl
            .lines()
            .filter(|l| l.starts_with("module "))
            .collect::<Vec<_>>()
            .join("\n");
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [113:0] i, output wire [36:0] o);
            module top_state(input wire [1:0] clock_reset, input wire [353:0] i, output reg [353:0] o);"#]];
        expect.assert_eq(&headers);
        // A lost or merged module would keep the headers above intact.
        expect!["1368"].assert_eq(&hdl.lines().count().to_string());
        Ok(())
    }

    /// Tier 4: the ground truth. The emitted Verilog must agree with
    /// the Rust simulation on every signal of every cycle, through both
    /// the RTL path and the NTL path.
    ///
    /// Both are run because `.rtl()` skips the Stage-3 netlist passes
    /// and `.ntl()` exercises them; a widget that passes one and fails
    /// the other is a bug in the optimiser, not in the widget.
    #[test]
    fn test_widget_hdl_works() -> miette::Result<()> {
        let uut = Mon::default();
        let cycles = write_transcript::<32, 32, 4, 4>(BUS_BYTES, Violation::WLastEarly);
        let input = cycles.into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        let tm = tb.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = tb.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    /// Tier 5: a digest of the waveform, to catch an ordering or
    /// timing change that the functional tests would pass.
    #[test]
    fn test_widget_trace() -> miette::Result<()> {
        let uut = Mon::default();
        let cycles = write_transcript::<32, 32, 4, 4>(BUS_BYTES, Violation::WLastEarly);
        let input = cycles.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("axi4_write_monitor");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["af720469488b0d391672fe99521eeb5cbbb4fd2372874c1a603c186ca1ad2c2f"];
        let digest = vcd.dump_to_file(root.join("monitor.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
