//! **Phase 2's gate**: a byte-exact loopback, with both monitors silent.
//!
//! `axi4-architecture.md` §8: *"byte-exact loopback across every `len`
//! 1..256 and every `size` up to the bus width, with both monitors
//! silent."* This is that test.
//!
//! Three widgets are stepped in lockstep through
//! [`Synchronous::sim`] — the write manager, the memory endpoint, and
//! the read manager — with the bus transcript recorded as it happens
//! and handed to the monitors afterwards.
//!
//! # Byte-exact against an independent model, not against the endpoint
//!
//! The expected contents are accumulated host-side from what the
//! manager *actually drove*: each accepted beat's address, strobes and
//! data are applied to a plain `BTreeMap<u64, u8>`. The read-back is
//! then compared against that map.
//!
//! This matters for narrow transfers. A narrow beat touches only some
//! lanes of a word, and the endpoint honours that by read-modify-write
//! — so comparing the read-back against *the endpoint's own idea* of
//! memory would be comparing the merge logic with itself. The map is
//! independent of it, which is the whole point: §4 warns that narrow
//! transfers "cannot be tested by a loopback between two RHDL widgets
//! that share an off-by-one".
//!
//! # What this does and does not prove
//!
//! It exercises the Rust behavioural model of all three widgets
//! together. Each widget's emitted Verilog is separately checked
//! against its own model by its Tier 4 `iverilog` round-trip, so the
//! hardware is covered — but *this composition* is not itself run
//! through `iverilog`. A fixture widget wrapping all three would give
//! that, and is worth doing when the interconnect arrives in Phase 5
//! and there is more than one shape to compose.

use rhdl::prelude::*;
use rhdl_fpga::axi4::check::monitor::Axi4WriteMonitor;
use rhdl_fpga::axi4::check::read_monitor::Axi4ReadMonitor;
use rhdl_fpga::axi4::check::rules::Fault;
use rhdl_fpga::axi4::check::traffic::{ReadCycle, WriteCycle};
use rhdl_fpga::axi4::endpoint::mem::{Axi4MemEndpoint, In as EpIn};
use rhdl_fpga::axi4::manager::read_burst::{Axi4ReadBurst, In as RdIn};
use rhdl_fpga::axi4::manager::write_burst::{Axi4WriteBurst, In as WrIn};
use rhdl_fpga::axi4::types::{AxiAddress, BurstKind, beat_address};
use std::collections::BTreeMap;

const A: usize = 32;
const DW: usize = 32;
const STRB: usize = 4;
const SHIFT: usize = 2;
const ID: usize = 4;
const N: usize = 10; // 1024 words = 4 KiB, one page

type Wr = Axi4WriteBurst<A, DW, STRB, ID>;
type Rd = Axi4ReadBurst<A, DW, STRB, ID>;
type Ep = Axi4MemEndpoint<A, DW, STRB, SHIFT, ID, N>;
const _: () = Ep::check_widths();

const BASE: u128 = 0x40;

fn descriptor(len: u128, size: u128, burst: BurstKind) -> AxiAddress<A, ID> {
    AxiAddress::<A, ID> {
        id: bits(1),
        addr: bits(BASE),
        len: bits(len),
        size: bits(size),
        burst,
        ..Default::default()
    }
}

/// One write burst, then one read burst of the same geometry.
struct Loop {
    /// What the manager actually drove, byte by byte.
    expected: BTreeMap<u64, u8>,
    /// Bytes the read brought back, byte by byte.
    got: BTreeMap<u64, u8>,
    write_cycles: Vec<WriteCycle<A, DW, STRB, ID>>,
    read_cycles: Vec<ReadCycle<A, DW, ID>>,
    beats_written: usize,
    beats_read: usize,
}

/// The byte lanes a beat occupies, computed in plain Rust.
///
/// Deliberately *not* the manager's `lane_strobe` kernel: reusing it
/// would make the expectation and the thing under test share any
/// off-by-one, which is exactly what §4 warns about for narrow
/// transfers.
fn lanes_of(beat_addr: u64, size: u128) -> std::ops::Range<u64> {
    let tb = 1u64 << size;
    let bus = STRB as u64;
    let lo = (beat_addr & (bus - 1)) & !(tb - 1);
    lo..(lo + tb)
}

/// A data word whose four bytes are all distinct and beat-dependent.
fn word_for(beat: usize) -> u128 {
    let b = (beat as u128) & 0x3F;
    (b * 4 + 3) << 24 | (b * 4 + 2) << 16 | (b * 4 + 1) << 8 | (b * 4)
}

fn beat_addr_of(d: AxiAddress<A, ID>, beat: u128) -> u64 {
    beat_address::<A>(d.addr, d.size, d.len, d.burst, bits(beat)).raw() as u64
}

fn run_loop(len: u128, size: u128, burst: BurstKind) -> Loop {
    let beats = (len + 1) as usize;
    // A cycle is TWO `sim` calls. `sim` is per clock *phase*, and a
    // `DFF` latches on the low-to-high transition it detects by
    // comparing against its stored clock -- so holding `clock(true)`
    // across calls means no edge ever happens and no register ever
    // updates. Outputs are read from the low phase, which is the
    // pre-edge state, and the high phase commits.
    let lo = clock_reset(clock(false), reset(false));
    let hi = clock_reset(clock(true), reset(false));
    let rlo = clock_reset(clock(false), reset(true));
    let rhi = clock_reset(clock(true), reset(true));
    let bus = STRB as u64;

    let wr = Wr::default();
    let rd = Rd::default();
    let ep = Ep::default();
    let mut wr_s = wr.init();
    let mut rd_s = rd.init();
    let mut ep_s = ep.init();
    for cr in [rlo, rhi] {
        wr.sim(cr, WrIn::default(), &mut wr_s);
        rd.sim(cr, RdIn::default(), &mut rd_s);
        ep.sim(cr, EpIn::default(), &mut ep_s);
    }

    let mut out = Loop {
        expected: BTreeMap::new(),
        got: BTreeMap::new(),
        write_cycles: Vec::new(),
        read_cycles: Vec::new(),
        beats_written: 0,
        beats_read: 0,
    };
    let desc = descriptor(len, size, burst);

    // ---- write phase
    let mut issued = false;
    let mut finished = false;
    let mut idle_after = 0;
    for _ in 0..(beats * 3 + 64) {
        // The endpoint's outputs are state-derived, so probing a clone
        // with a default input yields the same values the real step
        // will. That is what lets two widgets be wired without a
        // combinational loop between them.
        let ep_o = ep.sim(lo, EpIn::default(), &mut ep_s.clone());

        let wr_i = WrIn::<A, DW, ID> {
            cmd: if issued { None } else { Some(desc) },
            // Every lane distinct and beat-dependent. The first
            // version of this put almost all its entropy in byte 0,
            // which made the lane-1..3 comparisons vacuous and hid a
            // real merge bug in the endpoint.
            data: Some(bits(word_for(out.beats_written))),
            aw_ready: ep_o.aw_ready,
            w_ready: ep_o.w_ready,
            b_valid: ep_o.b_valid,
            b: ep_o.b,
        };
        issued = true;
        let wr_o = wr.sim(lo, wr_i, &mut wr_s);
        wr.sim(hi, wr_i, &mut wr_s);

        // Record the beat the manager drove, byte by byte.
        if wr_o.w_valid && ep_o.w_ready {
            let ba = beat_addr_of(desc, out.beats_written as u128);
            let word_base = ba & !(bus - 1);
            let data = wr_o.w.data.raw();
            let strb = wr_o.w.strb.raw();
            for lane in 0..bus {
                if (strb >> lane) & 1 == 1 {
                    out.expected
                        .insert(word_base + lane, ((data >> (8 * lane)) & 0xFF) as u8);
                }
            }
            out.beats_written += 1;
        }
        if wr_o.done.is_some() {
            finished = true;
        }

        let ep_i = EpIn::<A, DW, STRB, ID> {
            aw_valid: wr_o.aw_valid,
            aw: wr_o.aw,
            w_valid: wr_o.w_valid,
            w: wr_o.w,
            b_ready: wr_o.b_ready,
            ar_valid: false,
            ar: AxiAddress::default(),
            r_ready: false,
        };
        ep.sim(lo, ep_i, &mut ep_s);
        ep.sim(hi, ep_i, &mut ep_s);

        out.write_cycles.push(WriteCycle::<A, DW, STRB, ID> {
            aw_valid: wr_o.aw_valid,
            aw_ready: ep_o.aw_ready,
            aw: wr_o.aw,
            w_valid: wr_o.w_valid,
            w_ready: ep_o.w_ready,
            w: wr_o.w,
            b_valid: ep_o.b_valid,
            b_ready: wr_o.b_ready,
            b: ep_o.b,
        });
        if finished {
            idle_after += 1;
            if idle_after > 2 {
                break;
            }
        }
    }

    // ---- read phase
    let mut issued = false;
    let mut finished = false;
    let mut idle_after = 0;
    for _ in 0..(beats * 4 + 64) {
        let ep_o = ep.sim(lo, EpIn::default(), &mut ep_s.clone());

        let rd_i = RdIn::<A, DW, STRB, ID> {
            cmd: if issued { None } else { Some(desc) },
            ar_ready: ep_o.ar_ready,
            r_valid: ep_o.r_valid,
            r: ep_o.r,
            sink_ready: true,
        };
        issued = true;
        let rd_o = rd.sim(lo, rd_i, &mut rd_s);
        rd.sim(hi, rd_i, &mut rd_s);

        if let Some(word) = rd_o.data {
            let ba = beat_addr_of(desc, out.beats_read as u128);
            let word_base = ba & !(bus - 1);
            let w = word.raw();
            for lane in lanes_of(ba, size) {
                out.got
                    .insert(word_base + lane, ((w >> (8 * lane)) & 0xFF) as u8);
            }
            out.beats_read += 1;
        }
        if rd_o.done.is_some() {
            finished = true;
        }

        let ep_i = EpIn::<A, DW, STRB, ID> {
            aw_valid: false,
            aw: AxiAddress::default(),
            w_valid: false,
            w: Default::default(),
            b_ready: false,
            ar_valid: rd_o.ar_valid,
            ar: rd_o.ar,
            r_ready: rd_o.r_ready,
        };
        ep.sim(lo, ep_i, &mut ep_s);
        ep.sim(hi, ep_i, &mut ep_s);

        out.read_cycles.push(ReadCycle::<A, DW, ID> {
            ar_valid: rd_o.ar_valid,
            ar_ready: ep_o.ar_ready,
            ar: rd_o.ar,
            r_valid: ep_o.r_valid,
            r_ready: rd_o.r_ready,
            r: ep_o.r,
        });
        if finished {
            idle_after += 1;
            if idle_after > 2 {
                break;
            }
        }
    }
    out
}

/// Tier A verdicts for both channels of one loop.
fn verdicts(l: &Loop) -> ((Fault, u128), (Fault, u128)) {
    let wm = Axi4WriteMonitor::<A, DW, STRB, ID>::default();
    let wi = l
        .write_cycles
        .clone()
        .into_iter()
        .with_reset(1)
        .clock_pos_edge(100);
    let wo = wm
        .run(wi)
        .synchronous_sample()
        .map(|s| s.output)
        .last()
        .unwrap();

    let rm = Axi4ReadMonitor::<A, DW, STRB, ID>::default();
    let ri = l
        .read_cycles
        .clone()
        .into_iter()
        .with_reset(1)
        .clock_pos_edge(100);
    let ro = rm
        .run(ri)
        .synchronous_sample()
        .map(|s| s.output)
        .last()
        .unwrap();

    (
        (wo.fault, wo.unchecked_beats.raw()),
        (ro.fault, ro.unchecked_beats.raw()),
    )
}

/// **Phase 2's gate.** Every `len` and every `size` the bus can carry,
/// written and read back byte for byte, with both monitors silent and
/// every beat checked.
#[test]
fn byte_exact_loopback_with_both_monitors_silent() {
    let mut wrong: Vec<String> = Vec::new();
    for size in 0u128..=2 {
        for len in 0u128..=255 {
            for (name, kind) in [("Incr", BurstKind::Incr), ("Fixed", BurstKind::Fixed)] {
                // FIXED is capped at 16 beats by the spec.
                if matches!(kind, BurstKind::Fixed) && len > 15 {
                    continue;
                }
                let beats = (len + 1) as usize;
                let tag = format!("{name} len={len} size={size}");
                let l = run_loop(len, size, kind);

                if l.beats_written != beats {
                    wrong.push(format!(
                        "{tag}: wrote {} beats, expected {beats}",
                        l.beats_written
                    ));
                    continue;
                }
                if l.beats_read != beats {
                    wrong.push(format!(
                        "{tag}: read {} beats, expected {beats}",
                        l.beats_read
                    ));
                    continue;
                }
                if l.got != l.expected {
                    let diffs = l
                        .expected
                        .iter()
                        .filter(|(k, v)| l.got.get(k) != Some(v))
                        .count();
                    wrong.push(format!(
                        "{tag}: {diffs} of {} bytes differ (wrote {} distinct, read {} distinct)",
                        l.expected.len(),
                        l.expected.len(),
                        l.got.len()
                    ));
                }
                let ((wf, wu), (rf, ru)) = verdicts(&l);
                if wf != Fault::None {
                    wrong.push(format!("{tag}: write monitor reported {wf:?}"));
                }
                if rf != Fault::None {
                    wrong.push(format!("{tag}: read monitor reported {rf:?}"));
                }
                if wu != 0 || ru != 0 {
                    wrong.push(format!(
                        "{tag}: {wu} write and {ru} read beats unchecked -- silence \
                         would mean the monitor stopped looking"
                    ));
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} geometries failed:\n{}",
        wrong.len(),
        wrong
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
