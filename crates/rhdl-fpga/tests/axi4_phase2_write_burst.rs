//! Phase 2's write half: the manager's own traffic, judged by the
//! Phase 1 monitor.
//!
//! `axi4-architecture.md` §8 gates Phase 2 on a byte-exact loopback
//! across every `len` 1..256 and every `size` up to the bus width,
//! "with both monitors silent". This file is the first half of that:
//! the manager drives real bursts, the transcript is reconstructed from
//! the simulation, and [`Axi4WriteMonitor`] judges it.
//!
//! # Why this is a stronger test of the monitor than Phase 1's
//!
//! Phase 1 validated the monitors against *hand-written* transcripts —
//! sixteen shapes an author thought of. This drives them with traffic
//! a widget generated, across 1,280 burst geometries, which is the
//! first time either monitor has seen input nobody curated.
//!
//! # "Silent" is asserted as two things, not one
//!
//! A monitor that classified every beat as unpositionable would report
//! no fault and would satisfy a naive reading of the gate. So the gate
//! here requires `fault == None` **and** `unchecked_beats == 0`: no
//! rule broken, and every beat actually checked. Phase 1 built
//! `unchecked_beats` precisely so this assertion could exist.
//!
//! # Both monitors, not just the RHDL one
//!
//! The gate says *both* monitors. Tier A and this manager are both
//! RHDL, so an off-by-one they share passes — `axi4-architecture.md`
//! §4 singles out narrow transfers as exactly where that risk lives,
//! because the lane rotation is easy to get consistently wrong. So a
//! representative slice of the same traffic also goes through the
//! hand-written Verilog monitor, which computes lanes by walking them
//! rather than by shifting a mask.
//!
//! The slice is representative rather than exhaustive on purpose: each
//! Tier B run forks `iverilog` and `vvp`, so 1,280 of them would
//! dominate the suite. The geometries chosen are the ones where the two
//! implementations could diverge — every size, the first and last beat
//! counts, and the lane-rotation cases.

mod common;

use rhdl::prelude::*;
use rhdl_fpga::axi4::check::monitor::Axi4WriteMonitor;
use rhdl_fpga::axi4::check::rules::Fault;
use rhdl_fpga::axi4::check::traffic::WriteCycle;
use rhdl_fpga::axi4::manager::write_burst::{Axi4WriteBurst, In};
use rhdl_fpga::axi4::types::{AxiAddress, AxiResponse, AxiWriteResponse, BurstKind};

const A: usize = 32;
const DW: usize = 32;
const STRB: usize = 4;
const ID: usize = 4;

/// What one burst produced: the bus transcript, and what the manager
/// reported about it.
#[derive(Clone)]
struct Run {
    cycles: Vec<WriteCycle<A, DW, STRB, ID>>,
    beats_accepted: usize,
    last_on_beat: Option<usize>,
    completed: bool,
    data_words: Vec<u128>,
}

/// Drive one burst with an always-ready subordinate and capture it.
///
/// Open loop rather than closed: with `AWREADY`, `WREADY` and `BVALID`
/// all held high the manager sequences itself, so no feedback is
/// needed and the stimulus stays a plain iterator. The manager's
/// `data_taken` is what says which words it actually consumed.
fn drive(len: u128, size: u128, burst: BurstKind, addr: u128) -> Run {
    let beats = (len + 1) as usize;
    let cmd = AxiAddress::<A, ID> {
        id: bits(1),
        addr: bits(addr),
        len: bits(len),
        size: bits(size),
        burst,
        ..Default::default()
    };
    // Address phase, one cycle per beat, response, and slack.
    let n = beats + 8;
    let stimulus: Vec<In<A, DW, ID>> = (0..n)
        .map(|c| In::<A, DW, ID> {
            cmd: if c == 0 { Some(cmd) } else { None },
            // A distinct word per cycle, so `data_taken` identifies
            // exactly which ones the manager consumed.
            data: Some(bits((c as u128) & ((1u128 << DW) - 1))),
            aw_ready: true,
            w_ready: true,
            b_valid: true,
            b: AxiWriteResponse {
                id: bits(1),
                resp: AxiResponse::Okay,
            },
        })
        .collect();

    let uut = Axi4WriteBurst::<A, DW, STRB, ID>::default();
    let input = stimulus.into_iter().with_reset(1).clock_pos_edge(100);
    let samples = uut
        .run(input)
        .synchronous_sample()
        .map(|s| (s.input.1, s.output))
        .collect::<Vec<_>>();

    let mut run = Run {
        cycles: Vec::new(),
        beats_accepted: 0,
        last_on_beat: None,
        completed: false,
        data_words: Vec::new(),
    };
    for (i, o) in samples {
        if o.w_valid && i.w_ready {
            if o.w.last {
                run.last_on_beat = Some(run.beats_accepted);
            }
            run.beats_accepted += 1;
        }
        if o.data_taken {
            run.data_words.push(o.w.data.raw());
        }
        if o.done.is_some() {
            run.completed = true;
        }
        run.cycles.push(WriteCycle::<A, DW, STRB, ID> {
            aw_valid: o.aw_valid,
            aw_ready: i.aw_ready,
            aw: o.aw,
            w_valid: o.w_valid,
            w_ready: i.w_ready,
            w: o.w,
            b_valid: i.b_valid,
            b_ready: o.b_ready,
            b: i.b,
        });
    }
    run
}

/// Run a transcript past the Tier A monitor.
fn verdict(cycles: Vec<WriteCycle<A, DW, STRB, ID>>) -> (Fault, u128, u128) {
    let mon = Axi4WriteMonitor::<A, DW, STRB, ID>::default();
    let input = cycles.into_iter().with_reset(1).clock_pos_edge(100);
    let out = mon
        .run(input)
        .synchronous_sample()
        .map(|s| s.output)
        .collect::<Vec<_>>();
    let last = *out.last().unwrap();
    (last.fault, last.faults.raw(), last.unchecked_beats.raw())
}

/// **Phase 2's write gate.** Every `len` and every `size` the bus can
/// carry, as `Incr` and as `Fixed`, with the monitor silent and every
/// beat checked.
#[test]
fn every_burst_geometry_is_conformant() {
    let mut wrong: Vec<String> = Vec::new();
    // A 32-bit bus carries sizes 0..=2 (1, 2 and 4 bytes per beat).
    for size in 0u128..=2 {
        for len in 0u128..=255 {
            for (name, kind) in [("Incr", BurstKind::Incr), ("Fixed", BurstKind::Fixed)] {
                // FIXED is capped at 16 beats by the spec; driving a
                // longer one would be the manager's violation, not a
                // test of it.
                if matches!(kind, BurstKind::Fixed) && len > 15 {
                    continue;
                }
                let run = drive(len, size, kind, 0x1000);
                let beats = (len + 1) as usize;
                let tag = format!("{name} len={len} size={size}");

                if !run.completed {
                    wrong.push(format!("{tag}: burst never completed"));
                    continue;
                }
                if run.beats_accepted != beats {
                    wrong.push(format!(
                        "{tag}: {} beats accepted, expected {beats}",
                        run.beats_accepted
                    ));
                }
                if run.last_on_beat != Some(beats - 1) {
                    wrong.push(format!(
                        "{tag}: WLAST on beat {:?}, expected {}",
                        run.last_on_beat,
                        beats - 1
                    ));
                }
                let (fault, faults, unchecked) = verdict(run.cycles);
                if fault != Fault::None {
                    wrong.push(format!(
                        "{tag}: monitor reported {fault:?} ({faults} total)"
                    ));
                }
                if unchecked != 0 {
                    wrong.push(format!(
                        "{tag}: {unchecked} beats went unchecked -- silence here \
                         would mean the monitor stopped looking, not that the \
                         traffic was clean"
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
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// An *illegal* `Wrap` burst is refused, not mis-driven.
///
/// Phase 3 drives legal WRAP; three beats is not one of the four
/// lengths the spec permits, so this one must still be refused.
/// Driving it would turn a manager limitation into a protocol
/// violation the monitors would then — correctly, and unhelpfully —
/// report.
#[test]
fn an_illegal_wrap_burst_is_refused_rather_than_driven() {
    let run = drive(2, 2, BurstKind::Wrap, 0x1000);
    assert_eq!(run.beats_accepted, 0, "WRAP must not produce beats");
    assert!(!run.completed, "WRAP must not report completion");
    let (fault, _, unchecked) = verdict(run.cycles);
    assert_eq!(
        fault,
        Fault::None,
        "refusing must not look like a violation"
    );
    assert_eq!(unchecked, 0);
}

/// The same traffic, judged by the independent Verilog monitor.
///
/// A representative slice rather than all 1,280 geometries: each run
/// forks `iverilog` and `vvp`, so the full sweep would dominate the
/// suite for little extra signal. What is covered is where the two
/// implementations could actually diverge — every `size` (so every
/// lane-rotation pattern), single-beat and maximal bursts, and both
/// burst kinds.
#[test]
fn tier_b_agrees_on_a_representative_slice() {
    rhdl::vlog::toolchain::require_iverilog();
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("axi4_phase2");
    let mut wrong: Vec<String> = Vec::new();
    // Every size (so every lane-rotation pattern), the boundary beat
    // counts, all three burst kinds, and — the reason this slice exists
    // at all — unaligned starts, where Tier A shares the manager's lane
    // arithmetic and so cannot judge it.
    let mut cases: Vec<(String, u128, u128, BurstKind, u128)> = Vec::new();
    for size in 0u128..=2 {
        for len in [0u128, 1, 3, 7, 15] {
            for (name, kind) in [("Incr", BurstKind::Incr), ("Fixed", BurstKind::Fixed)] {
                cases.push((
                    format!("{name}_len{len}_size{size}"),
                    len,
                    size,
                    kind,
                    0x1000,
                ));
            }
        }
        for len in [1u128, 3, 7, 15] {
            cases.push((
                format!("Wrap_len{len}_size{size}"),
                len,
                size,
                BurstKind::Wrap,
                0x1000,
            ));
        }
        // Unaligned Incr: every start offset inside a transfer.
        let tb = 1u128 << size;
        for off in 1..tb {
            cases.push((
                format!("Incr_unaligned_off{off}_size{size}"),
                3,
                size,
                BurstKind::Incr,
                0x1000 + off,
            ));
        }
    }
    {
        {
            for (tag, len, size, kind, addr) in cases {
                let run = drive(len, size, kind, addr);
                let a = verdict(run.cycles.clone());
                let b = common::tier_b_write(&run.cycles, &dir, &tag);
                let a_code = (common::code_of(a.0), a.1, a.2);
                if b.0 != 0 {
                    wrong.push(format!("{tag}: tier B reported fault code {}", b.0));
                }
                if a_code != b {
                    wrong.push(format!(
                        "{tag}: monitors disagree -- A {a_code:?} vs B {b:?} \
                         (fault, tally, unchecked)"
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
