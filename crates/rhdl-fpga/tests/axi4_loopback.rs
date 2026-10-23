//! **The AXI4 loopback gate**: byte-exact, with both monitors silent.
//!
//! Phase 2 established this for aligned `Incr` and `Fixed`; Phase 3
//! adds `Wrap`, unaligned starts, and the structure-aware randomised
//! suite of `axi4-architecture.md` §7.4.
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
use rhdl_fpga::axi4::types::{AxiAddress, BurstKind};
use rhdl_fpga::doc::DetRng;
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
/// Base for the `Wrap` family, aligned to the largest block the suite
/// uses (16 beats x 4 bytes = 64), so that a start *inside* the block is
/// genuinely a wrap-point rather than some other block's base.
const BASE_BLOCK: u128 = 0x80;

fn descriptor(len: u128, size: u128, burst: BurstKind, addr: u128) -> AxiAddress<A, ID> {
    AxiAddress::<A, ID> {
        id: bits(1),
        addr: bits(addr),
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
fn lanes_of(start: u64, beat_addr: u64, size: u128, first: bool) -> std::ops::Range<u64> {
    let tb = 1u64 << size;
    let bus = STRB as u64;
    let base = (beat_addr & (bus - 1)) & !(tb - 1);
    // An unaligned burst's first beat covers only from the start address
    // to the end of its size-aligned block.
    let lo = if first {
        std::cmp::max(start & (bus - 1), base)
    } else {
        base
    };
    lo..(base + tb)
}

/// A data word whose four bytes are all distinct and beat-dependent.
fn word_for(beat: usize) -> u128 {
    let b = (beat as u128) & 0x3F;
    (b * 4 + 3) << 24 | (b * 4 + 2) << 16 | (b * 4 + 1) << 8 | (b * 4)
}

/// The address of beat `n`, computed from `axi4-architecture.md` §4 in
/// plain Rust.
///
/// Deliberately **not** the `beat_address` kernel the endpoint uses: if
/// both sides called it, a bug in it would cancel out and the loopback
/// would pass regardless. §4 warns about exactly this for narrow
/// transfers; it applies just as much to WRAP's block arithmetic, which
/// is the part of AXI4 addressing most easily got wrong.
fn beat_addr_of(d: AxiAddress<A, ID>, beat: u128) -> u64 {
    let start = d.addr.raw() as u64;
    let size = d.size.raw() as u32;
    let tb = 1u64 << size;
    let beats = (d.len.raw() as u64) + 1;
    let aligned = start & !(tb - 1);
    match d.burst {
        // Every beat is the same address: the address is a port, not a
        // location.
        BurstKind::Fixed => start,
        // Beats after the first sit on multiples of the transfer size;
        // the first beat's own offset shows up in its strobes, not here.
        BurstKind::Incr => aligned + beat as u64 * tb,
        // The burst wraps inside a block of `beats * tb` bytes. Both
        // factors are powers of two, so the modulo is a mask -- but it
        // is written as `%` here precisely because the kernel version
        // is the masked one, and agreeing by two different routes is
        // the point.
        BurstKind::Wrap => {
            let block = beats * tb;
            let block_lo = start & !(block - 1);
            block_lo + (((start - block_lo) + beat as u64 * tb) % block)
        }
    }
}

fn run_loop(len: u128, size: u128, burst: BurstKind, addr: u128) -> Loop {
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
    let desc = descriptor(len, size, burst, addr);

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
            // What the manager drove is the authority on the write side;
            // the strobes above already encode the reduced first beat.
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
            // A read returns whole words; only the lanes the transfer
            // selects are meaningful, and on an unaligned first beat the
            // bytes below the start address are to be discarded.
            for lane in lanes_of(desc.addr.raw() as u64, ba, size, out.beats_read == 0) {
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

/// **The loopback gate**, now covering all three burst kinds and
/// unaligned starts.
///
/// Phase 2 established this for aligned `Incr` and `Fixed`; Phase 3
/// adds `Wrap`'s block arithmetic and the reduced first beat of an
/// unaligned burst. Every geometry is written and read back byte for
/// byte against the independent host model, with both monitors silent
/// and every beat checked.
#[test]
fn byte_exact_loopback_with_both_monitors_silent() {
    let mut wrong: Vec<String> = Vec::new();
    let mut cases = 0usize;

    // (label, len, size, kind, start address)
    let mut geoms: Vec<(String, u128, u128, BurstKind, u128)> = Vec::new();
    for size in 0u128..=2 {
        // Aligned INCR across every length the spec allows.
        for len in 0u128..=255 {
            geoms.push((
                format!("Incr len={len} size={size}"),
                len,
                size,
                BurstKind::Incr,
                BASE,
            ));
        }
        // FIXED is capped at 16 beats by the spec.
        for len in 0u128..=15 {
            geoms.push((
                format!("Fixed len={len} size={size}"),
                len,
                size,
                BurstKind::Fixed,
                BASE,
            ));
        }
        // WRAP: only 2, 4, 8 or 16 beats, and a size-aligned start.
        for len in [1u128, 3, 7, 15] {
            geoms.push((
                format!("Wrap len={len} size={size}"),
                len,
                size,
                BurstKind::Wrap,
                BASE,
            ));
        }
    }
    // Unaligned INCR: every start offset inside a transfer, for the
    // sizes where "unaligned" is even meaningful (a 1-byte transfer is
    // always aligned).
    for size in 1u128..=2 {
        let tb = 1u128 << size;
        for off in 1..tb {
            for len in 0u128..=15 {
                geoms.push((
                    format!("Incr len={len} size={size} off={off}"),
                    len,
                    size,
                    BurstKind::Incr,
                    BASE + off,
                ));
            }
        }
    }

    for (tag, len, size, kind, addr) in geoms {
        cases += 1;
        let beats = (len + 1) as usize;
        let l = run_loop(len, size, kind, addr);

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
            let sample: Vec<String> = l
                .expected
                .iter()
                .filter(|(k, v)| l.got.get(k) != Some(v))
                .take(3)
                .map(|(k, v)| format!("{k:#x}: wrote {v:#04x} read {:?}", l.got.get(k)))
                .collect();
            wrong.push(format!(
                "{tag}: {diffs} of {} bytes differ -- {sample:?}",
                l.expected.len()
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
    assert!(
        wrong.is_empty(),
        "{} of {cases} geometries failed:\n{}",
        wrong.len(),
        wrong
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    // Pinned, not a guessed floor: 3 sizes x (256 Incr + 16 Fixed + 4
    // Wrap) plus the unaligned Incr offsets. A silently shrinking sweep
    // is the failure mode this guards, so the number is exact.
    assert_eq!(cases, 892, "the geometry sweep changed size");
}

// ------------------------------------------------- §7.4 randomised suite

/// Is this descriptor one a conformant manager may drive?
///
/// Mirrors `axi4::manager::drivable`, written out again here in plain
/// Rust on purpose: a randomised suite has to know the *right* answer
/// independently, and asking the widget would only confirm the widget
/// agrees with itself.
fn should_be_drivable(len: u128, size: u128, kind: BurstKind, addr: u128) -> bool {
    let tb = 1u128 << size;
    if tb > STRB as u128 {
        return false;
    }
    match kind {
        BurstKind::Fixed => true,
        BurstKind::Incr => {
            // The 4 KiB rule, on the bytes actually touched.
            let aligned = addr & !(tb - 1);
            let last = aligned + (len + 1) * tb - 1;
            (aligned >> 12) == (last >> 12)
        }
        BurstKind::Wrap => matches!(len, 1 | 3 | 7 | 15) && (addr & (tb - 1)) == 0,
    }
}

/// Does this `Wrap` burst actually wrap inside its block?
///
/// A `Wrap` whose start *is* the block base behaves exactly like
/// `Incr`, exercising none of the block arithmetic. Solving for starts
/// that do wrap is what §7.4 means by "WRAP wrap-points".
fn wrap_actually_wraps(len: u128, size: u128, addr: u128) -> bool {
    let tb = 1u128 << size;
    let block = (len + 1) * tb;
    (addr & (block - 1)) != 0
}

/// One case: loop it if it is drivable, require refusal if it is not.
fn check_case(
    tag: &str,
    len: u128,
    size: u128,
    kind: BurstKind,
    addr: u128,
    wrong: &mut Vec<String>,
) {
    if !should_be_drivable(len, size, kind, addr) {
        // Driving this would make the manager the non-conformant master
        // the monitors exist to catch, so the correct behaviour is to
        // refuse: no beats at all.
        let l = run_loop(len, size, kind, addr);
        if l.beats_written != 0 {
            wrong.push(format!(
                "{tag}: drove {} beats of illegal traffic",
                l.beats_written
            ));
        }
        return;
    }
    let beats = (len + 1) as usize;
    let l = run_loop(len, size, kind, addr);
    if l.beats_written != beats || l.beats_read != beats {
        wrong.push(format!(
            "{tag}: wrote {} read {} beats, expected {beats}",
            l.beats_written, l.beats_read
        ));
        return;
    }
    if l.got != l.expected {
        let diffs = l
            .expected
            .iter()
            .filter(|(k, v)| l.got.get(k) != Some(v))
            .count();
        wrong.push(format!(
            "{tag}: {diffs} of {} bytes differ",
            l.expected.len()
        ));
    }
    let ((wf, wu), (rf, ru)) = verdicts(&l);
    if wf != Fault::None || rf != Fault::None {
        wrong.push(format!("{tag}: monitors reported write={wf:?} read={rf:?}"));
    }
    if wu != 0 || ru != 0 {
        wrong.push(format!("{tag}: {wu}/{ru} beats unchecked"));
    }
}

/// **Phase 3's gate**: the structure-aware randomised suite of §7.4.
///
/// Three things make this more than fuzzing.
///
/// **It is deterministic.** [`DetRng`] with a fixed seed, per CLAUDE.md
/// §12 rule 10. A suite that finds a bug on Tuesday and cannot
/// reproduce it on Wednesday has not found a bug.
///
/// **It solves for the interesting cases rather than hoping to hit
/// them.** §7.4 is explicit that the RoCE corpus showed uniform random
/// fuzzing misses narrow windows, so the generator *constructs*, by
/// arithmetic: `Incr` bursts ending exactly at a 4 KiB boundary and
/// ones crossing it by a single beat; `Wrap` bursts whose start is
/// strictly inside the block, so the address genuinely wraps rather
/// than behaving like `Incr`; and the narrowest transfer on the widest
/// bus. Uniform sampling of a 32-bit address space would essentially
/// never produce a 4 KiB crossing.
///
/// **It checks the illegal cases too.** A good share of the generated
/// descriptors are deliberately not drivable, and the assertion there
/// is that the manager *refuses* them. The tail assertions require both
/// outcomes to have actually occurred, so the suite cannot pass by
/// generating nothing interesting.
#[test]
fn randomised_conformance_suite() {
    let mut rng = DetRng::new(0xA5C4_1234);
    let mut wrong: Vec<String> = Vec::new();
    let mut drivable_seen = 0usize;
    let mut refused_seen = 0usize;
    let mut wrapping_seen = 0usize;

    // --- solved-for family 1: the 4 KiB boundary, from both sides.
    for size in 0u128..=2 {
        let tb = 1u128 << size;
        for beats in [1u128, 2, 4, 16] {
            // Ending exactly at the page boundary: legal, and the
            // tightest legal case that exists.
            let flush = 0x1000 - beats * tb;
            check_case(
                &format!("flush4k beats={beats} size={size}"),
                beats - 1,
                size,
                BurstKind::Incr,
                flush,
                &mut wrong,
            );
            // One beat further on: crosses, and must be refused.
            check_case(
                &format!("cross4k beats={beats} size={size}"),
                beats - 1,
                size,
                BurstKind::Incr,
                flush + tb,
                &mut wrong,
            );
        }
    }

    // --- solved-for family 2: WRAP starts that genuinely wrap.
    for size in 0u128..=2 {
        let tb = 1u128 << size;
        for len in [1u128, 3, 7, 15] {
            let block = (len + 1) * tb;
            let mut off = tb;
            while off < block {
                let addr = BASE_BLOCK + off;
                assert!(
                    wrap_actually_wraps(len, size, addr),
                    "the generator failed to solve for a wrap-point"
                );
                wrapping_seen += 1;
                check_case(
                    &format!("wrap len={len} size={size} off={off}"),
                    len,
                    size,
                    BurstKind::Wrap,
                    addr,
                    &mut wrong,
                );
                off += tb;
            }
        }
    }

    // --- solved-for family 3: narrowest transfer, unaligned start.
    for len in [0u128, 1, 15, 255] {
        check_case(
            &format!("narrowest len={len}"),
            len,
            0,
            BurstKind::Incr,
            BASE + 1,
            &mut wrong,
        );
    }

    // --- the random body, over §7.4's axes.
    for n in 0..400 {
        let kind = match rng.below(3) {
            0 => BurstKind::Fixed,
            1 => BurstKind::Incr,
            _ => BurstKind::Wrap,
        };
        // 3 is deliberately out of range for a 32-bit bus, so some
        // descriptors are illegal by size alone.
        let size = rng.below(4);
        let len = match rng.below(4) {
            0 => rng.below(16),
            1 => [1u128, 3, 7, 15][rng.below(4) as usize],
            2 => rng.below(256),
            _ => 0,
        };
        // Alignment is an axis in its own right.
        let tb = 1u128 << size.min(2);
        let addr = BASE_BLOCK + if rng.chance(50) { 0 } else { rng.below(tb) };
        let tag = format!("rand#{n} {kind:?} len={len} size={size} addr={addr:#x}");
        if should_be_drivable(len, size, kind, addr) {
            drivable_seen += 1;
        } else {
            refused_seen += 1;
        }
        check_case(&tag, len, size, kind, addr, &mut wrong);
    }

    assert!(
        wrong.is_empty(),
        "{} randomised cases failed:\n{}",
        wrong.len(),
        wrong
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    // The suite must have actually exercised both outcomes and the
    // solved-for wrap-points, or it could pass vacuously.
    assert!(
        drivable_seen > 100,
        "only {drivable_seen} drivable cases generated"
    );
    assert!(
        refused_seen > 20,
        "only {refused_seen} illegal cases generated"
    );
    assert!(
        wrapping_seen > 30,
        "only {wrapping_seen} genuine wrap-points solved for"
    );
}
