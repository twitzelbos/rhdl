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
//! Likewise the strobe check assumes a size-aligned start address: the
//! reduced first-beat lanes of an *unaligned* burst are not modelled,
//! and an unaligned burst is reported as [`Fault::WrapUnaligned`] only
//! for `Wrap`, where the spec forbids it outright.
//!
//! # Per-ID tracking
//!
//! Responses are matched against a **count** of outstanding
//! transactions, not against their IDs, so a response bearing an ID
//! that was never addressed is caught only while no other transaction
//! is outstanding. ID-keyed matching is the ID table from
//! `axi4-architecture.md` §5 and arrives with Phase 4; the count is
//! what is sound without it.
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
    AxiAddress, AxiWriteBeat, AxiWriteResponse, BurstKind, beat_address, burst_beats,
    bytes_per_beat, crosses_4k, size_fits_bus, wrap_len_is_legal,
};
use super::traffic::WriteCycle;
use crate::core::dff;
use rhdl::prelude::*;

/// A protocol rule the monitor found broken.
///
/// Reported sticky-first: the monitor latches the earliest fault and
/// holds it, because the first divergence is the one with a diagnosable
/// cause. Later faults are usually its consequences, and the running
/// `faults` count is there to say how many there were.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub enum Fault {
    /// No rule broken so far.
    #[default]
    None,
    /// `AWVALID` fell before `AWREADY` rose.
    AwValidDropped,
    /// The AW payload changed while the beat was offered and unaccepted.
    AwPayloadChanged,
    /// `WVALID` fell before `WREADY` rose.
    WValidDropped,
    /// The W payload changed while the beat was offered and unaccepted.
    WPayloadChanged,
    /// `BVALID` fell before `BREADY` rose.
    BValidDropped,
    /// The B payload changed while the beat was offered and unaccepted.
    BPayloadChanged,
    /// `WLAST` on a beat before the burst's last.
    WLastEarly,
    /// The burst's last beat arrived without `WLAST`.
    WLastMissing,
    /// A write response with no transaction outstanding.
    BrespUnexpected,
    /// An `Incr` burst crossing a 4 KiB boundary.
    IncrCrosses4K,
    /// A `Wrap` burst of other than 2, 4, 8 or 16 beats.
    WrapBadLength,
    /// A `Wrap` burst starting off a size boundary.
    WrapUnaligned,
    /// `AxSIZE` wider than the data bus.
    SizeExceedsBus,
    /// `WSTRB` asserting a lane outside the transfer's selected bytes.
    StrobeOutsideLanes,
}

impl Fault {
    /// Every fault the monitor can report.
    ///
    /// Exists so that a test can require every one of them be reachable
    /// from some entry in [`super::traffic::Violation::all`]. A check
    /// the monitor performs that no traffic ever triggers is a check
    /// that has never been run, and two of them shipped here before a
    /// mutation run found them -- see the `check` module docs.
    pub fn all() -> Vec<Fault> {
        vec![
            Fault::None,
            Fault::AwValidDropped,
            Fault::AwPayloadChanged,
            Fault::WValidDropped,
            Fault::WPayloadChanged,
            Fault::BValidDropped,
            Fault::BPayloadChanged,
            Fault::WLastEarly,
            Fault::WLastMissing,
            Fault::BrespUnexpected,
            Fault::IncrCrosses4K,
            Fault::WrapBadLength,
            Fault::WrapUnaligned,
            Fault::SizeExceedsBus,
            Fault::StrobeOutsideLanes,
        ]
    }
}

/// The monitor's internal registers, bundled.
///
/// One `Digital` struct in one `DFF` rather than a dozen sibling
/// `DFF`s, per CLAUDE.md §3.1: the auto-generated `Q`/`D` tuples stop
/// at 12 elements, and these are one observer's state, not a dozen
/// sub-circuits.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct State<const A: usize, const DW: usize, const STRB: usize, const ID: usize>
where
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
    /// Transactions addressed but not yet responded to.
    outstanding: Bits<8>,
    /// The first fault seen.
    fault: Fault,
    /// How many faults were seen in total.
    faults: Bits<16>,
    /// Write beats the position checks could not be applied to.
    unchecked: Bits<16>,
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
pub struct Axi4WriteMonitor<const A: usize, const DW: usize, const STRB: usize, const ID: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    state: dff::DFF<State<A, DW, STRB, ID>>,
}

impl<const A: usize, const DW: usize, const STRB: usize, const ID: usize> Default
    for Axi4WriteMonitor<A, DW, STRB, ID>
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

impl<const A: usize, const DW: usize, const STRB: usize, const ID: usize> SynchronousIO
    for Axi4WriteMonitor<A, DW, STRB, ID>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    type I = WriteCycle<A, DW, STRB, ID>;
    type O = Out;
    type Kernel = write_monitor<A, DW, STRB, ID>;
}

#[kernel]
/// Latch `new` if `cond` and nothing has been latched yet.
///
/// Sticky-first in one place, so that the long chain of checks in
/// [`write_monitor`] reads as a list of rules rather than a list of
/// nested conditionals.
pub fn first_fault(current: Fault, cond: bool, new: Fault) -> Fault {
    match current {
        Fault::None => {
            if cond {
                new
            } else {
                current
            }
        }
        _ => current,
    }
}

#[kernel]
/// Is this burst a `Wrap`?
pub fn is_wrap(b: BurstKind) -> bool {
    match b {
        BurstKind::Wrap => true,
        _ => false,
    }
}

#[kernel]
/// Kernel for [Axi4WriteMonitor].
#[allow(clippy::too_many_lines)]
pub fn write_monitor<const A: usize, const DW: usize, const STRB: usize, const ID: usize>(
    cr: ClockReset,
    i: WriteCycle<A, DW, STRB, ID>,
    q: Q<A, DW, STRB, ID>,
) -> (Out, D<A, DW, STRB, ID>)
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

    // ---- address phase legality, checked as the AW is accepted
    let aw_fire = i.aw_valid && i.aw_ready;
    let wrap = is_wrap(i.aw.burst);
    let size_bad = aw_fire && !size_fits_bus(i.aw.size, bus_bytes);
    let crosses = aw_fire && crosses_4k::<A>(i.aw.addr, i.aw.size, i.aw.len, i.aw.burst);
    let wrap_len_bad = aw_fire && wrap && !wrap_len_is_legal(i.aw.len);
    // Size-alignment of a `Wrap` start. `bytes` is a power of two no
    // greater than 128, so the mask is exact.
    let aw_bytes = bytes_per_beat(i.aw.size).resize::<A>();
    let wrap_unaligned = aw_fire && wrap && (i.aw.addr & (aw_bytes - bits::<A>(1))) != bits::<A>(0);

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
    let ones = (bits::<33>(1) << safe_bytes) - bits::<33>(1);
    let bus_mask = bus_bytes.resize::<33>() - bits::<33>(1);
    // Align the offset down to the transfer size: the selected lanes of
    // an aligned transfer are contiguous and size-aligned.
    let lane = addr.resize::<33>() & bus_mask & !(safe_bytes - bits::<33>(1));
    let legal = (ones << lane).resize::<STRB>();
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
    let bresp_bad = b_fire && s.outstanding == bits::<8>(0);

    // Rules in order. `first_fault` keeps the earliest; the order here
    // only decides which of several faults *in one cycle* is named.
    fault = first_fault(fault, aw_dropped, Fault::AwValidDropped);
    fault = first_fault(fault, aw_changed, Fault::AwPayloadChanged);
    fault = first_fault(fault, w_dropped, Fault::WValidDropped);
    fault = first_fault(fault, w_changed, Fault::WPayloadChanged);
    fault = first_fault(fault, b_dropped, Fault::BValidDropped);
    fault = first_fault(fault, b_changed, Fault::BPayloadChanged);
    fault = first_fault(fault, size_bad, Fault::SizeExceedsBus);
    fault = first_fault(fault, crosses, Fault::IncrCrosses4K);
    fault = first_fault(fault, wrap_len_bad, Fault::WrapBadLength);
    fault = first_fault(fault, wrap_unaligned, Fault::WrapUnaligned);
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
    if size_bad {
        hits += bits::<16>(1);
    }
    if crosses {
        hits += bits::<16>(1);
    }
    if wrap_len_bad {
        hits += bits::<16>(1);
    }
    if wrap_unaligned {
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
    let up = if aw_fire { bits::<8>(1) } else { bits::<8>(0) };
    let down = if b_fire && s.outstanding != bits::<8>(0) {
        bits::<8>(1)
    } else {
        bits::<8>(0)
    };
    n.outstanding = s.outstanding + up - down;

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

    let mut d = D::<A, DW, STRB, ID>::dont_care();
    d.state = n;

    if cr.reset.any() {
        d.state = State::<A, DW, STRB, ID>::default();
        o.fault = Fault::None;
        o.faults = bits::<16>(0);
        o.unchecked_beats = bits::<16>(0);
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axi4::check::traffic::{Violation, write_transcript};
    use expect_test::expect;
    use rhdl::prelude::vlog::Pretty;

    // A 32-bit bus: four byte lanes.
    type Mon = Axi4WriteMonitor<32, 32, 4, 4>;
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

    /// `Violation::all` really does list every variant.
    ///
    /// `expected()`'s `match` stops compiling when a variant is added,
    /// but nothing forces the new variant into `all()` -- and a
    /// violation missing from `all()` is a rule the gate silently stops
    /// testing. Pinning the count is the cheap guard.
    #[test]
    fn violation_list_is_complete() {
        expect!["16"].assert_eq(&Violation::all().len().to_string());
    }

    /// Every fault the monitor can report must be reachable from some
    /// transcript.
    ///
    /// This is the permanent form of a one-off mutation run that
    /// disabled each check in turn and asked whether the gate noticed.
    /// It noticed for twelve of fourteen: `BValidDropped` and
    /// `BPayloadChanged` could be deleted outright with every test
    /// still green, because no transcript ever stalled the response
    /// channel. An unexercised check is worse than a missing one --
    /// it reads as coverage.
    ///
    /// A mutation run cannot be a `cargo test`, but its *conclusion*
    /// can be: if a fault is in no `Violation`'s image, nothing
    /// exercises the code that reports it.
    #[test]
    fn every_fault_is_exercised_by_some_transcript() {
        let reachable: Vec<Fault> = Violation::all()
            .into_iter()
            .map(Violation::expected)
            .collect();
        let orphans: Vec<Fault> = Fault::all()
            .into_iter()
            .filter(|f| !reachable.contains(f))
            .collect();
        assert!(
            orphans.is_empty(),
            "the monitor reports faults no transcript provokes, so these checks are never run: {orphans:?}"
        );
    }

    /// Tier 1: reset clears every latched verdict.
    ///
    /// A monitor that held a fault across reset would report the
    /// previous test's traffic, which is the kind of bug that makes a
    /// whole suite look broken at once.
    #[test]
    fn reset_clears_the_verdict() {
        let s = State::<32, 32, 4, 4> {
            fault: Fault::WLastEarly,
            faults: bits(3),
            unchecked: bits(2),
            ..Default::default()
        };
        let q = Q::<32, 32, 4, 4> { state: s };
        let i = WriteCycle::<32, 32, 4, 4>::default();
        let cr = clock_reset(clock(true), reset(true));
        let (o, d) = write_monitor::<32, 32, 4, 4>(cr, i, q);
        assert_eq!(o.fault, Fault::None);
        assert_eq!(o.faults.raw(), 0);
        assert_eq!(o.unchecked_beats.raw(), 0);
        assert_eq!(d.state, State::<32, 32, 4, 4>::default());
    }

    /// Tier 1: accepting an address latches the burst and zeroes the
    /// beat counter.
    #[test]
    fn accepting_an_address_latches_the_burst() {
        let i = WriteCycle::<32, 32, 4, 4> {
            aw_valid: true,
            aw_ready: true,
            aw: AxiAddress {
                addr: bits(0x40),
                len: bits(3),
                size: bits(2),
                burst: BurstKind::Incr,
                ..Default::default()
            },
            ..Default::default()
        };
        let q = Q::<32, 32, 4, 4> {
            state: State::default(),
        };
        let cr = clock_reset(clock(true), reset(false));
        let (o, d) = write_monitor::<32, 32, 4, 4>(cr, i, q);
        assert_eq!(o.fault, Fault::None);
        assert!(d.state.aw_active);
        assert_eq!(d.state.beat.raw(), 0);
        assert_eq!(d.state.aw.addr.raw(), 0x40);
        assert_eq!(d.state.outstanding.raw(), 1);
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
        let s = State::<32, 32, 4, 4> {
            aw_active: true,
            aw: oversized,
            ..Default::default()
        };
        let q = Q::<32, 32, 4, 4> { state: s };
        let cr = clock_reset(clock(true), reset(false));
        let (o, _d) = write_monitor::<32, 32, 4, 4>(cr, i, q);
        assert_eq!(o.fault, Fault::SizeExceedsBus);
    }

    /// Tier 1: a narrow transfer selects only its own lanes, so a beat
    /// asserting every lane is a strobe fault.
    #[test]
    fn narrow_transfer_rejects_lanes_outside_its_own() {
        let s = State::<32, 32, 4, 4> {
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
        let q = Q::<32, 32, 4, 4> { state: s };
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
        let (o, _) = write_monitor::<32, 32, 4, 4>(cr, ok, q);
        assert_eq!(o.fault, Fault::None);

        let mut bad = ok;
        bad.w.strb = bits(0b1111);
        let (o, _) = write_monitor::<32, 32, 4, 4>(cr, bad, q);
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
        let q = Q::<32, 32, 4, 4> {
            state: State::default(),
        };
        let cr = clock_reset(clock(true), reset(false));
        let (o, _d) = write_monitor::<32, 32, 4, 4>(cr, i, q);
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
        let q = Q::<32, 32, 4, 4> {
            state: State::default(),
        };
        let cr = clock_reset(clock(true), reset(false));
        let (o, d) = write_monitor::<32, 32, 4, 4>(cr, i, q);
        assert_eq!(o.fault, Fault::BrespUnexpected);
        assert_eq!(d.state.outstanding.raw(), 0, "counter must not wrap");
    }

    /// Tier 3: the emitted Verilog's **interface** is the contract.
    ///
    /// The body is 914 lines, five times the largest snapshot anywhere
    /// else in this crate (`core::crc32`, at 187), so pinning it inline
    /// would make this file mostly generated Verilog and make every
    /// read of the diff a skim. What is pinned instead is the part
    /// users actually depend on: the module headers, which carry the
    /// bit-packing of `In` and `Out`. RHDL emits a struct port as one
    /// packed vector, so those widths are the ABI -- add a field to
    /// `WriteCycle` or `Out` and the number here moves.
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
            module top(input wire [1:0] clock_reset, input wire [113:0] i, output wire [35:0] o);
            module top_state(input wire [1:0] clock_reset, input wire [232:0] i, output reg [232:0] o);"#]];
        expect.assert_eq(&headers);
        // A lost or merged module would keep the headers above intact.
        expect!["914"].assert_eq(&hdl.lines().count().to_string());
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
        let expect = expect!["5cdf8d4551a5ce02d3021cc75b94331e332a7f297097480ffabe4d620cdbf547"];
        let digest = vcd.dump_to_file(root.join("monitor.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
