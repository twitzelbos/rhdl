//! A passive AXI4 read-channel protocol monitor — validation Tier A.
//!
//! Observes AR and R without driving anything and latches the first rule
//! the traffic breaks. The write-channel half is
//! [`super::monitor::Axi4WriteMonitor`]; together they cover the five
//! channels `axi4-architecture.md` §7.2 asks for.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
      +-+Axi4ReadMonitor+----------+
      |                            |
 Read |                      fault |  Fault
+---->| cycle                      +------->
      |                     faults |  b16
      |                            +------->
      |            unchecked_beats |  b16
      |                            +------->
      +----------------------------+
")]
//!
//! # Why a second widget rather than one monitor over five channels
//!
//! The read and write paths share their address-phase rules and nothing
//! else — [`super::rules::address_fault`] is the shared part, and it is
//! shared as a kernel rather than duplicated. What differs is all the
//! state: a write burst's data phase carries strobes and terminates
//! with `WLAST` against a latched `AWLEN`, while a read burst's beats
//! carry a per-beat `RRESP` and an `RID` to check. Folding both into one
//! widget would double the registered state every instance pays for and
//! push the `Q`/`D` field count toward the ceiling CLAUDE.md §3.1
//! describes, for no shared logic.
//!
//! # What is deferred, and why it is honest to defer this part
//!
//! The spec allows read beats for **different IDs to interleave**,
//! while beats for one ID may not reorder. Checking that needs the
//! outstanding-transaction table of `axi4-architecture.md` §5, which is
//! Phase 4. Until then this monitor tracks a single outstanding read, so
//! it checks `RLAST` position and `RID` agreement for traffic that does
//! not interleave, and counts beats it cannot place on
//! `unchecked_beats` rather than passing them silently. An interleaved
//! read stream therefore reports a large `unchecked_beats` and no
//! faults — which is a visible statement that the monitor did not check
//! it, not a claim that it was clean.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/axi4_read_monitor.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/axi4_read_monitor.md")]

use super::super::types::{AxiAddress, AxiReadBeat, burst_beats};
use super::rules::{Fault, address_fault, first_fault};
use super::traffic::ReadCycle;
use crate::core::dff;
use rhdl::prelude::*;

/// The monitor's internal registers, bundled.
///
/// One `Digital` struct in one `DFF` rather than a dozen sibling
/// `DFF`s, per CLAUDE.md §3.1.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct ReadState<const A: usize, const DW: usize, const BYTES: usize, const ID: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<BYTES>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    /// Has any cycle been observed? Gates the stability checks, which
    /// compare against a previous cycle that does not exist on the
    /// first one.
    seen: bool,
    /// Last cycle's AR handshake and payload.
    p_ar_valid: bool,
    p_ar_ready: bool,
    p_ar: AxiAddress<A, ID>,
    /// Last cycle's R handshake and payload.
    p_r_valid: bool,
    p_r_ready: bool,
    p_r: AxiReadBeat<DW, ID>,
    /// Is a read burst's address latched and its data phase in flight?
    ar_active: bool,
    /// The latched address phase of the burst in flight.
    ar: AxiAddress<A, ID>,
    /// Beats of the burst in flight already accepted.
    beat: Bits<8>,
    /// Transactions addressed but not yet fully returned.
    outstanding: Bits<8>,
    /// The first fault seen.
    fault: Fault,
    /// How many faults were seen in total.
    faults: Bits<16>,
    /// Read beats the position checks could not be applied to.
    unchecked: Bits<16>,
}

/// What the monitor reports.
#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct ReadOut {
    /// The first rule broken, or [`Fault::None`].
    pub fault: Fault,
    /// How many faults were observed, counting repeats.
    pub faults: Bits<16>,
    /// Read beats seen with no address latched, and so not
    /// position-checked. See the module docs.
    pub unchecked_beats: Bits<16>,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// A passive read-channel protocol monitor.
///
/// `A` is the address width, `DW` the data width, `BYTES = DW / 8` and
/// `ID` the transaction-ID width.
///
/// The data width is `DW` and not `D` because `#[rhdl(dq_no_prefix)]`
/// generates a type literally named `D`, which a const generic of that
/// name shadows -- the resulting error is a confusing `type provided
/// when a constant was expected` pointed at the field list.
///
/// `BYTES` is a separate parameter rather than `DW / 8` because
/// division is not an operator the kernel compiler accepts, which is
/// the same constraint that makes `AxiWriteBeat` carry `STRB`.
pub struct Axi4ReadMonitor<const A: usize, const DW: usize, const BYTES: usize, const ID: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<BYTES>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    state: dff::DFF<ReadState<A, DW, BYTES, ID>>,
}

impl<const A: usize, const DW: usize, const BYTES: usize, const ID: usize> Default
    for Axi4ReadMonitor<A, DW, BYTES, ID>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<BYTES>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    fn default() -> Self {
        Self {
            state: dff::DFF::new(ReadState::dont_care()),
        }
    }
}

impl<const A: usize, const DW: usize, const BYTES: usize, const ID: usize> SynchronousIO
    for Axi4ReadMonitor<A, DW, BYTES, ID>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<BYTES>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    type I = ReadCycle<A, DW, ID>;
    type O = ReadOut;
    type Kernel = read_monitor<A, DW, BYTES, ID>;
}

#[kernel]
/// Kernel for [Axi4ReadMonitor].
pub fn read_monitor<const A: usize, const DW: usize, const BYTES: usize, const ID: usize>(
    cr: ClockReset,
    i: ReadCycle<A, DW, ID>,
    q: Q<A, DW, BYTES, ID>,
) -> (ReadOut, D<A, DW, BYTES, ID>)
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<BYTES>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    let s = q.state;
    let mut n = s;

    // `BYTES` rather than `DW / 8`: division is not a kernel operator,
    // and `Bits<{ DW / 8 }>` does not compile on stable Rust either.
    // This is the same workaround, for the same reason, that makes
    // `AxiWriteBeat` carry `STRB` as its own parameter --
    // `check_widths` is how the two are tied together.
    let bus_bytes = bits::<9>(BYTES as u128);

    let mut fault = s.fault;
    let mut hits = bits::<16>(0);

    // ---- handshake stability: a beat offered and not accepted must
    // still be offered, unchanged, next cycle.
    let ar_held = s.seen && s.p_ar_valid && !s.p_ar_ready;
    let ar_dropped = ar_held && !i.ar_valid;
    let ar_changed = ar_held && i.ar_valid && i.ar != s.p_ar;
    let r_held = s.seen && s.p_r_valid && !s.p_r_ready;
    let r_dropped = r_held && !i.r_valid;
    let r_changed = r_held && i.r_valid && i.r != s.p_r;

    // ---- address phase, by the rules AW and AR share
    let ar_fire = i.ar_valid && i.ar_ready;
    let addr_fault = address_fault::<A, ID>(i.ar, bus_bytes);
    let addr_bad = ar_fire
        && match addr_fault {
            Fault::None => false,
            _ => true,
        };

    // ---- data phase
    let r_fire = i.r_valid && i.r_ready;
    let checked = r_fire && s.ar_active;
    let unchecked = r_fire && !s.ar_active;
    let last_beat = s.beat == burst_beats(s.ar.len).resize::<8>() - bits::<8>(1);
    let last_early = checked && i.r.last && !last_beat;
    let last_missing = checked && !i.r.last && last_beat;
    let id_mismatch = checked && i.r.id != s.ar.id;
    let beat_unexpected = r_fire && s.outstanding == bits::<8>(0);

    fault = first_fault(fault, ar_dropped, Fault::ArValidDropped);
    fault = first_fault(fault, ar_changed, Fault::ArPayloadChanged);
    fault = first_fault(fault, r_dropped, Fault::RValidDropped);
    fault = first_fault(fault, r_changed, Fault::RPayloadChanged);
    fault = first_fault(fault, addr_bad, addr_fault);
    fault = first_fault(fault, last_early, Fault::RLastEarly);
    fault = first_fault(fault, last_missing, Fault::RLastMissing);
    fault = first_fault(fault, id_mismatch, Fault::RIdMismatch);
    fault = first_fault(fault, beat_unexpected, Fault::RBeatUnexpected);

    if ar_dropped {
        hits += bits::<16>(1);
    }
    if ar_changed {
        hits += bits::<16>(1);
    }
    if r_dropped {
        hits += bits::<16>(1);
    }
    if r_changed {
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
    if id_mismatch {
        hits += bits::<16>(1);
    }
    if beat_unexpected {
        hits += bits::<16>(1);
    }

    // ---- next state, every field assigned exactly once and
    // unconditionally. Assigning under an `if` would make every field
    // of `d` depend on that condition.
    n.seen = true;
    n.p_ar_valid = i.ar_valid;
    n.p_ar_ready = i.ar_ready;
    n.p_ar = i.ar;
    n.p_r_valid = i.r_valid;
    n.p_r_ready = i.r_ready;
    n.p_r = i.r;

    let r_closes = checked && i.r.last;
    n.ar = if ar_fire { i.ar } else { s.ar };
    n.ar_active = if ar_fire {
        true
    } else {
        s.ar_active && !r_closes
    };
    // A new address and a closing burst both restart the count.
    n.beat = if ar_fire || r_closes {
        bits::<8>(0)
    } else if checked {
        s.beat + bits::<8>(1)
    } else {
        s.beat
    };

    // Outstanding transactions, not underflowing past zero: a beat with
    // nothing outstanding is reported above, and must not wrap the
    // counter round to swallow the next 255 checks. A read transaction
    // retires on its RLAST, not on its first beat.
    let up = if ar_fire { bits::<8>(1) } else { bits::<8>(0) };
    let down = if r_closes && s.outstanding != bits::<8>(0) {
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

    let mut o = ReadOut::dont_care();
    o.fault = fault;
    o.faults = n.faults;
    o.unchecked_beats = n.unchecked;

    let mut d = D::<A, DW, BYTES, ID>::dont_care();
    d.state = n;

    if cr.reset.any() {
        d.state = ReadState::<A, DW, BYTES, ID>::default();
        o.fault = Fault::None;
        o.faults = bits::<16>(0);
        o.unchecked_beats = bits::<16>(0);
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axi4::check::traffic::{ReadViolation, read_transcript};
    use crate::axi4::types::BurstKind;
    use expect_test::expect;
    use rhdl::prelude::vlog::Pretty;

    // A 32-bit bus.
    type Mon = Axi4ReadMonitor<32, 32, 4, 4>;
    const BUS_BYTES: u128 = 4;

    /// Run a transcript through the monitor and report its verdict.
    fn verdict(v: ReadViolation) -> (Fault, u128, u128) {
        let uut = Mon::default();
        let cycles = read_transcript::<32, 32, 4>(BUS_BYTES, v);
        let input = cycles.into_iter().with_reset(1).clock_pos_edge(100);
        let out = uut
            .run(input)
            .synchronous_sample()
            .map(|x| x.output)
            .collect::<Vec<_>>();
        let last = *out.last().unwrap();
        (last.fault, last.faults.raw(), last.unchecked_beats.raw())
    }

    /// **The gate**, read-channel half: every deliberately broken
    /// transcript draws exactly the fault it was built to break.
    #[test]
    fn every_injected_read_violation_is_caught() {
        let mut wrong = Vec::new();
        for v in ReadViolation::all() {
            let (got, _, _) = verdict(v);
            if got != v.expected() {
                wrong.push(format!("{v:?}: expected {:?}, got {got:?}", v.expected()));
            }
        }
        assert!(
            wrong.is_empty(),
            "read monitor verdicts wrong:\n{}",
            wrong.join("\n")
        );
    }

    /// The conformant control draws no fault and leaves no beat
    /// unchecked.
    ///
    /// The `unchecked_beats == 0` half rules out a monitor that passes
    /// the gate by declining to check anything.
    #[test]
    fn conformant_read_traffic_is_fully_checked_and_clean() {
        let (fault, faults, unchecked) = verdict(ReadViolation::None);
        assert_eq!(fault, Fault::None);
        assert_eq!(faults, 0);
        assert_eq!(unchecked, 0, "a conformant read burst left beats unchecked");
    }

    /// Tier 1: reset clears every latched verdict.
    #[test]
    fn reset_clears_the_read_verdict() {
        let s = ReadState::<32, 32, 4, 4> {
            fault: Fault::RLastEarly,
            faults: bits(3),
            unchecked: bits(2),
            ..Default::default()
        };
        let q = Q::<32, 32, 4, 4> { state: s };
        let i = ReadCycle::<32, 32, 4>::default();
        let cr = clock_reset(clock(true), reset(true));
        let (o, d) = read_monitor::<32, 32, 4, 4>(cr, i, q);
        assert_eq!(o.fault, Fault::None);
        assert_eq!(o.faults.raw(), 0);
        assert_eq!(d.state, ReadState::<32, 32, 4, 4>::default());
    }

    /// Tier 1: a read transaction retires on `RLAST`, not on its first
    /// beat.
    ///
    /// Getting this wrong would make every multi-beat read report
    /// `RBeatUnexpected` from its second beat onward.
    #[test]
    fn a_read_transaction_retires_on_rlast() {
        let s = ReadState::<32, 32, 4, 4> {
            ar_active: true,
            ar: AxiAddress {
                addr: bits(0x40),
                len: bits(3),
                size: bits(2),
                burst: BurstKind::Incr,
                id: bits(1),
                ..Default::default()
            },
            beat: bits(0),
            outstanding: bits(1),
            ..Default::default()
        };
        let q = Q::<32, 32, 4, 4> { state: s };
        let cr = clock_reset(clock(true), reset(false));

        // A non-final beat: still outstanding afterwards.
        let mid = ReadCycle::<32, 32, 4> {
            r_valid: true,
            r_ready: true,
            r: AxiReadBeat {
                id: bits(1),
                ..Default::default()
            },
            ..Default::default()
        };
        let (o, d) = read_monitor::<32, 32, 4, 4>(cr, mid, q);
        assert_eq!(o.fault, Fault::None);
        assert_eq!(d.state.outstanding.raw(), 1, "retired too early");
        assert_eq!(d.state.beat.raw(), 1);
        assert!(d.state.ar_active);
    }

    /// Tier 1: a read beat bearing the wrong `RID` is caught.
    #[test]
    fn a_read_beat_with_the_wrong_id_is_caught() {
        let s = ReadState::<32, 32, 4, 4> {
            ar_active: true,
            ar: AxiAddress {
                len: bits(3),
                size: bits(2),
                burst: BurstKind::Incr,
                id: bits(1),
                ..Default::default()
            },
            outstanding: bits(1),
            ..Default::default()
        };
        let q = Q::<32, 32, 4, 4> { state: s };
        let cr = clock_reset(clock(true), reset(false));
        let i = ReadCycle::<32, 32, 4> {
            r_valid: true,
            r_ready: true,
            r: AxiReadBeat {
                id: bits(5),
                ..Default::default()
            },
            ..Default::default()
        };
        let (o, _d) = read_monitor::<32, 32, 4, 4>(cr, i, q);
        assert_eq!(o.fault, Fault::RIdMismatch);
    }

    /// Tier 1: a read beat with no address latched is counted, not
    /// silently dropped.
    #[test]
    fn an_unplaceable_read_beat_is_counted() {
        let q = Q::<32, 32, 4, 4> {
            state: ReadState::default(),
        };
        let cr = clock_reset(clock(true), reset(false));
        let i = ReadCycle::<32, 32, 4> {
            r_valid: true,
            r_ready: true,
            ..Default::default()
        };
        let (o, _d) = read_monitor::<32, 32, 4, 4>(cr, i, q);
        assert_eq!(o.unchecked_beats.raw(), 1);
    }

    /// Tier 3: the emitted Verilog's interface is the contract.
    ///
    /// The body is pinned by line count rather than inline, for the
    /// reason given in `monitor.rs`: an inline snapshot of this size
    /// makes every diff review a skim.
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
            module top(input wire [1:0] clock_reset, input wire [107:0] i, output wire [36:0] o);
            module top_state(input wire [1:0] clock_reset, input wire [227:0] i, output reg [227:0] o);"#]];
        expect.assert_eq(&headers);
        // A lost or merged module would leave the headers above intact.
        expect!["693"].assert_eq(&hdl.lines().count().to_string());
        Ok(())
    }

    /// Tier 4: the ground truth — emitted Verilog against the Rust
    /// simulation, through both the RTL and the NTL path.
    #[test]
    fn test_widget_hdl_works() -> miette::Result<()> {
        let uut = Mon::default();
        let cycles = read_transcript::<32, 32, 4>(BUS_BYTES, ReadViolation::RLastEarly);
        let input = cycles.into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        let tm = tb.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = tb.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    /// Tier 5: a digest of the waveform.
    #[test]
    fn test_widget_trace() -> miette::Result<()> {
        let uut = Mon::default();
        let cycles = read_transcript::<32, 32, 4>(BUS_BYTES, ReadViolation::RLastEarly);
        let input = cycles.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("axi4_read_monitor");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["3c8e6b523f9c5c0bbfdf9bb448206dbb4b3f26cfcf2193d546e8f4697e47ddf7"];
        let digest = vcd.dump_to_file(root.join("monitor.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
